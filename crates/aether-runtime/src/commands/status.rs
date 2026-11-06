use super::RuntimeStatus;
use crate::{Result, Runtime, RuntimeError};
use aether_core::agent::{Agent, AgentStatus, AgentStatusKind, BlockReason};
use aether_core::budget::{RATE_WINDOW_MINUTES, RunSummary, WindowBilling};
use aether_core::event::EventKind;
use aether_core::id::{AgentId, RunId, TaskId};
use aether_core::model::Usage;
use aether_core::provider::ProviderHealth;
use aether_core::task::TaskStatusKind;
use aether_core::time::{Timestamp, now};
use aether_protocol::{UnavailableSection, status_sections as section};
use aether_store::{AgentFilter, EventFilter};
use std::collections::{BTreeMap, BTreeSet};
use std::sync::Arc;
use std::sync::mpsc::{self, RecvTimeoutError};
use std::time::{Duration, Instant};

/// The live agents' tallies, the only part of them `status` reports.
#[derive(Debug, Default, PartialEq)]
struct LiveTally {
    live: u32,
    running: u32,
    waiting: u32,
    blocked: u32,
    suspended: u32,
    paused: u32,
}

fn tally_live(live: &[Agent]) -> LiveTally {
    let mut tally = LiveTally::default();
    for agent in live {
        debug_assert!(agent.is_live(), "only live agents are tallied");
        tally.live += 1;
        tally.paused += u32::from(agent.paused);
        match agent.status.kind() {
            AgentStatusKind::Running => tally.running += 1,
            AgentStatusKind::Waiting => tally.waiting += 1,
            AgentStatusKind::Blocked => {
                tally.blocked += 1;
                if matches!(
                    &agent.status,
                    AgentStatus::Blocked {
                        reason: BlockReason::Exhausted { .. }
                    }
                ) {
                    tally.suspended += 1;
                }
            }
            _ => {}
        }
    }
    assert!(tally.suspended <= tally.blocked && tally.paused <= tally.live);
    tally
}

/// A status section this slow is logged at warn: it names what a slow `status` waited on.
const SLOW_SECTION: std::time::Duration = std::time::Duration::from_secs(2);

/// Runs one status section and logs how long it took, so a slow section on a
/// real daemon shows up under `RUST_LOG=aether_runtime=debug`, and at warn
/// once it takes [`SLOW_SECTION`].
fn timed<T>(section: &'static str, f: impl FnOnce() -> T) -> T {
    let started = std::time::Instant::now();
    let out = f();
    let elapsed = started.elapsed();
    let elapsed_ms = elapsed.as_millis() as u64;
    if elapsed >= SLOW_SECTION {
        tracing::warn!(section, elapsed_ms, "slow status section");
    } else {
        tracing::debug!(section, elapsed_ms, "status section");
    }
    out
}

/// How long the daemon's `status` waits on its sections; under its patience,
/// so a stalled section costs only its own figures. Healthy ones take milliseconds.
pub const STATUS_DEADLINE: Duration = Duration::from_millis(750);

/// One store-backed `status` section, read on a thread of its own.
enum Figure {
    Problems(Vec<String>),
    Providers(Vec<ProviderHealth>),
    Live(LiveTally),
    Runs(Vec<RunSummary>),
    Usage(Usage),
    TasksActive(u32),
    PendingApprovals(u32),
    LowDisk(Option<aether_core::disk::LowDisk>),
    Engines(Vec<String>, u32),
    ResourceLeases(Vec<aether_core::lease::ResourceHolding>),
    Mcp(Vec<aether_core::mcp::McpServerHealth>),
    Writes(aether_core::write_stats::WriteStats),
}

type Answer = (&'static str, Result<Figure>);

/// A reader thread and the sections it answers, in the order it answers them.
struct Reader {
    sections: &'static [&'static str],
    read: fn(&Runtime, &mut Answers),
}

const READERS: [Reader; 8] = [
    Reader {
        sections: &[section::PROBLEMS],
        read: |rt, to| {
            answer(to, section::PROBLEMS, || {
                rt.problems().map(Figure::Problems)
            })
        },
    },
    Reader {
        sections: &[section::PROVIDERS],
        read: |rt, to| {
            answer(to, section::PROVIDERS, || {
                rt.provider_health().map(Figure::Providers)
            })
        },
    },
    Reader {
        sections: &[section::LIVE_AGENTS, section::RUNS],
        read: read_live_and_runs,
    },
    Reader {
        sections: &[section::USAGE_TOTAL],
        read: |rt, to| {
            answer(to, section::USAGE_TOTAL, || {
                Ok(Figure::Usage(rt.store.fleet_usage()?))
            })
        },
    },
    Reader {
        sections: &[section::TASKS_ACTIVE],
        read: |rt, to| answer(to, section::TASKS_ACTIVE, || rt.count_active_tasks()),
    },
    Reader {
        sections: &[section::PENDING_APPROVALS],
        read: |rt, to| {
            answer(to, section::PENDING_APPROVALS, || {
                let pending = rt.store.count_pending_approvals()?;
                Ok(Figure::PendingApprovals(
                    u32::try_from(pending).unwrap_or(u32::MAX),
                ))
            })
        },
    },
    Reader {
        sections: &[section::LOW_DISK],
        read: |rt, to| answer(to, section::LOW_DISK, || Ok(Figure::LowDisk(rt.low_disk()))),
    },
    Reader {
        sections: &[
            section::ENGINES,
            section::RESOURCE_LEASES,
            section::MCP,
            section::WRITES,
        ],
        read: read_held_in_memory,
    },
];

/// Every section a reader answers.
const SECTIONS: [&str; 12] = [
    section::PROBLEMS,
    section::PROVIDERS,
    section::LIVE_AGENTS,
    section::RUNS,
    section::USAGE_TOTAL,
    section::TASKS_ACTIVE,
    section::PENDING_APPROVALS,
    section::LOW_DISK,
    section::ENGINES,
    section::RESOURCE_LEASES,
    section::MCP,
    section::WRITES,
];

/// Where a reader sends its answers, and its in-flight mark, held until its
/// last section is answered or its thread ends, however it ends.
struct Answers {
    send: mpsc::Sender<Answer>,
    last: &'static str,
    in_flight: Option<InFlight>,
}

/// Marks a reader in flight until it is dropped.
struct InFlight {
    runtime: Arc<Runtime>,
    key: &'static str,
}

impl Drop for InFlight {
    fn drop(&mut self) {
        let removed = self.runtime.status_in_flight.lock().remove(self.key);
        debug_assert!(removed, "a reader is marked in flight once");
    }
}

/// Times `read` and hands it to the waiting `status`, or logs a failure
/// that came too late for it.
fn answer(to: &mut Answers, section: &'static str, read: impl FnOnce() -> Result<Figure>) {
    debug_assert!(
        SECTIONS.contains(&section),
        "an answer names a known section"
    );
    let figure = timed(section, read);
    if section == to.last {
        // Unmarked before the answer is sent: a `status` that follows it reads it again.
        assert!(to.in_flight.take().is_some(), "{section} is answered once");
    }
    if let Err(mpsc::SendError((section, Err(error)))) = to.send.send((section, figure)) {
        tracing::warn!(section, %error, "status section failed after its status gave up on it");
    }
}

/// The live agents, then the live runs' spend, which is read from them.
fn read_live_and_runs(rt: &Runtime, to: &mut Answers) {
    let mut live = None;
    answer(to, section::LIVE_AGENTS, || {
        let agents = rt.store.agents(&AgentFilter {
            live_only: true,
            ..Default::default()
        })?;
        let tally = tally_live(&agents);
        live = Some(agents);
        Ok(Figure::Live(tally))
    });
    let Some(live) = live else {
        return;
    };
    answer(to, section::RUNS, || {
        rt.run_summaries(&live).map(Figure::Runs)
    });
}

/// What is held in memory behind locks, and the store files' sizes: each
/// waits on a lock or the filesystem, so each is read under the deadline too.
fn read_held_in_memory(rt: &Runtime, to: &mut Answers) {
    answer(to, section::ENGINES, || {
        let (engines, running) = rt.engines_status();
        Ok(Figure::Engines(engines, running))
    });
    answer(to, section::RESOURCE_LEASES, || {
        Ok(Figure::ResourceLeases(rt.resource_holdings()))
    });
    answer(to, section::MCP, || Ok(Figure::Mcp(rt.mcp_health())));
    answer(to, section::WRITES, || {
        Ok(Figure::Writes(
            rt.store.write_stats().top(WRITE_KINDS_SHOWN),
        ))
    });
}

/// Folds each answer into `status` until every section answered or the
/// deadline passed; returns the sections that did not answer, sorted.
fn collect_answers(
    answers: &mpsc::Receiver<Answer>,
    busy: &[&'static str],
    deadline: Option<Instant>,
    status: &mut RuntimeStatus,
) -> Result<BTreeSet<&'static str>> {
    let mut pending: BTreeSet<&'static str> = SECTIONS
        .into_iter()
        .filter(|section| !busy.contains(section))
        .collect();
    while !pending.is_empty() {
        let received = match deadline {
            Some(deadline) => {
                answers.recv_timeout(deadline.saturating_duration_since(Instant::now()))
            }
            None => answers.recv().map_err(|_| RecvTimeoutError::Disconnected),
        };
        let (section, figure) = match received {
            Ok(answer) => answer,
            Err(RecvTimeoutError::Timeout) => break,
            Err(RecvTimeoutError::Disconnected) => {
                return Err(RuntimeError::Internal(format!(
                    "status sections {pending:?} ended without an answer"
                )));
            }
        };
        assert!(pending.remove(section), "{section} answered twice");
        apply(status, figure?);
    }
    pending.extend(busy);
    Ok(pending)
}

fn apply(status: &mut RuntimeStatus, figure: Figure) {
    match figure {
        Figure::Problems(problems) => status.problems.extend(problems),
        Figure::Providers(providers) => status.providers = providers,
        Figure::Live(tally) => {
            status.agents_live = tally.live;
            status.agents_running = tally.running;
            status.agents_waiting = tally.waiting;
            status.agents_blocked = tally.blocked;
            status.agents_suspended = tally.suspended;
            status.agents_paused = tally.paused;
        }
        Figure::Runs(runs) => status.runs = runs,
        Figure::Usage(usage) => status.usage_total = usage,
        Figure::TasksActive(count) => status.tasks_active = count,
        Figure::PendingApprovals(count) => status.pending_approvals = count,
        Figure::LowDisk(low) => status.low_disk = low,
        Figure::Engines(engines, running) => {
            status.engines = engines;
            status.engine_turns_running = running;
        }
        Figure::ResourceLeases(leases) => status.resource_leases = leases,
        Figure::Mcp(mcp) => status.mcp = mcp,
        Figure::Writes(writes) => status.writes = writes,
    }
}

/// Heaviest record kinds `status` names; the rest are folded into `other`.
const WRITE_KINDS_SHOWN: usize = 8;

/// Receipts read per page while walking a run's recent billing back in time.
const RECEIPT_PAGE: usize = 256;

/// Pages of receipts one run's rate reads at most: thousands of receipts in
/// fifteen minutes is a run billing too often to rate exactly in a `status`.
const RECEIPT_PAGES: usize = 8;

/// Billers whose billing before the window one run's rate looks up, each
/// by an index; more is a run too wide to rate exactly in a `status`.
const BASELINE_LOOKUPS: usize = 64;

/// One page of a run's receipts, newest first.
struct ReceiptPage {
    events: Vec<aether_core::event::Event>,
    /// Older receipts may remain past it.
    more: bool,
    /// Newer receipts between it and the page before it may have gone unread.
    partial: bool,
}

/// What a run's members had billed when the rate window opened.
struct WindowStart {
    at_window_start: BTreeMap<AgentId, f64>,
    /// The walk stopped at [`RECEIPT_PAGES`] before reaching the window's
    /// start, or [`BASELINE_LOOKUPS`] billers were looked up and more remain.
    approximate: bool,
}

impl Runtime {
    /// What `status` reports from configuration alone, with every count at
    /// zero: it reads no lock, store or disk, so it is there when they stall.
    pub fn status_settings(&self) -> RuntimeStatus {
        RuntimeStatus {
            yolo: self.config.yolo,
            approval_default: self.config.approval_default,
            network_default: self.config.network_default,
            max_concurrent_turns: self.config.max_concurrent_turns,
            max_concurrent_engine_turns: self.config.max_concurrent_engine_turns,
            run_budget_usd: self.config.run_budget_usd,
            run_hard_stop_usd: self.config.run_hard_stop.usd,
            run_hard_stop_tokens: self.config.run_hard_stop.tokens,
            ..Default::default()
        }
    }

    /// [`Self::status_settings`] with every section named unavailable: what
    /// `status` reports before any read of its sections has finished.
    pub fn status_unread(&self) -> RuntimeStatus {
        let unavailable: Vec<UnavailableSection> = SECTIONS
            .into_iter()
            .map(|section| UnavailableSection {
                section: section.to_owned(),
                reason: section::TIMED_OUT.to_owned(),
            })
            .collect();
        assert_eq!(unavailable.len(), SECTIONS.len(), "every section is named");
        let status = RuntimeStatus {
            unavailable,
            ..self.status_settings()
        };
        assert!(
            status.runs.is_empty() && status.agents_live == 0,
            "nothing was read"
        );
        status
    }

    /// Every section of `status`, however long the store takes to answer.
    pub fn status(&self) -> Result<RuntimeStatus> {
        self.read_status(None)
    }

    /// `status` as far as the store answered within `patience`: each section
    /// not read by then is named in `unavailable` and left at its default.
    pub fn status_within(&self, patience: Duration) -> Result<RuntimeStatus> {
        assert!(!patience.is_zero(), "a status read needs time to read");
        self.read_status(Some(Instant::now() + patience))
    }

    fn read_status(&self, deadline: Option<Instant>) -> Result<RuntimeStatus> {
        let started = Instant::now();
        let (answers, busy) = self.start_status_readers()?;
        let mut status = self.status_settings();
        let unavailable = collect_answers(&answers, &busy, deadline, &mut status)?;
        status.unavailable = unavailable
            .into_iter()
            .map(|section| UnavailableSection {
                section: section.to_owned(),
                reason: section::TIMED_OUT.to_owned(),
            })
            .collect();
        tracing::debug!(
            elapsed_ms = started.elapsed().as_millis() as u64,
            unavailable = status.unavailable.len(),
            "status: total"
        );
        Ok(status)
    }

    /// The engines found, sorted, and how many turns run on one: the
    /// scheduler's lock is held for the count alone.
    fn engines_status(&self) -> (Vec<String>, u32) {
        let mut engines: Vec<String> = match self.engines.now() {
            Ok(detected) => detected.usable.values().map(|e| e.adapter()).collect(),
            Err(_) => vec![aether_protocol::ENGINES_STARTING.to_owned()],
        };
        engines.sort();
        let running = self.sched.lock().on_engine() as u32;
        (engines, running)
    }

    /// One thread per reader not still running for an earlier `status`, and
    /// the sections of those that are: a stalled read is never doubled.
    fn start_status_readers(&self) -> Result<(mpsc::Receiver<Answer>, Vec<&'static str>)> {
        let me = self.me.upgrade().ok_or_else(|| {
            RuntimeError::Internal("status read on a runtime being dropped".to_owned())
        })?;
        let (sender, answers) = mpsc::channel();
        let mut busy = Vec::new();
        for reader in &READERS {
            let key = reader.sections[0];
            if !self.status_in_flight.lock().insert(key) {
                busy.extend(reader.sections);
                continue;
            }
            let mut answers = Answers {
                send: sender.clone(),
                last: reader.sections[reader.sections.len() - 1],
                in_flight: Some(InFlight {
                    runtime: me.clone(),
                    key,
                }),
            };
            let (runtime, read) = (me.clone(), reader.read);
            std::thread::Builder::new()
                .name(format!("status-{key}"))
                .spawn(move || read(&runtime, &mut answers))
                .map_err(|error| {
                    RuntimeError::Internal(format!("cannot start the {key} status read: {error}"))
                })?;
        }
        assert!(busy.iter().all(|section| SECTIONS.contains(section)));
        Ok((answers, busy))
    }

    fn count_active_tasks(&self) -> Result<Figure> {
        let active = self
            .store
            .count_tasks(&[TaskStatusKind::Active, TaskStatusKind::Assigned])?;
        Ok(Figure::TasksActive(
            u32::try_from(active).unwrap_or(u32::MAX),
        ))
    }

    /// Every live run's spend, rated over the receipts of its recent window.
    /// Each run's totals are the store's kept ones; the only members read are
    /// `live` and those with a receipt in the window, each by an index.
    fn run_summaries(&self, live: &[Agent]) -> Result<Vec<RunSummary>> {
        let at = now();
        let since = at - chrono::Duration::minutes(RATE_WINDOW_MINUTES);
        let mut live_by_run: BTreeMap<RunId, Vec<&Agent>> = BTreeMap::new();
        for agent in live {
            live_by_run.entry(agent.run).or_default().push(agent);
        }
        let mut runs = Vec::with_capacity(live_by_run.len());
        for (run, members) in live_by_run {
            let billing = self.billing_at(run, since)?;
            let window = self.window_billing(&members, &billing.at_window_start)?;
            // Read last, so every agent read live or billing above is counted:
            // none means the run was pruned since, and so is no longer live.
            let Some(totals) = self.store.run_totals(run)? else {
                continue;
            };
            let live_members = (members.len() as u32).min(totals.agents);
            runs.push(aether_core::budget::run_summary(
                run,
                totals,
                live_members,
                &window,
                billing.approximate,
                at,
            ));
        }
        assert!(runs.len() <= live.len(), "a run summary has a live member");
        Ok(runs)
    }

    /// What each member billing in the window has billed by now: a live one
    /// from `live`, any other from the store's usage of it, by id.
    fn window_billing(
        &self,
        live: &[&Agent],
        at_window_start: &BTreeMap<AgentId, f64>,
    ) -> Result<Vec<WindowBilling>> {
        let mut billed_now: BTreeMap<AgentId, f64> = live
            .iter()
            .filter(|agent| at_window_start.contains_key(&agent.id))
            .map(|agent| (agent.id, agent.stats.usage.cost_usd))
            .collect();
        let finished: Vec<AgentId> = at_window_start
            .keys()
            .filter(|id| !billed_now.contains_key(*id))
            .copied()
            .collect();
        for (id, usage) in self.store.agent_usages(&finished)? {
            billed_now.insert(id, usage.cost_usd);
        }
        let window: Vec<WindowBilling> = at_window_start
            .iter()
            .filter_map(|(id, before)| {
                Some(WindowBilling {
                    now: *billed_now.get(id)?,
                    at_window_start: *before,
                })
            })
            .collect();
        assert!(window.len() <= at_window_start.len(), "one per biller");
        Ok(window)
    }

    /// What each agent of `run` with a usage receipt after `since` had billed
    /// then: its last receipt before it, or the start of the turn that billed.
    /// Reads at most [`RECEIPT_PAGES`] pages of the run's newest receipts and
    /// [`BASELINE_LOOKUPS`] billers; past them the billing is `approximate`.
    fn billing_at(&self, run: RunId, since: Timestamp) -> Result<WindowStart> {
        let receipts = EventFilter {
            run: Some(run),
            types: vec!["agent.usage_updated".into()],
            ..Default::default()
        };
        let (first_in_window, mut approximate) = self.first_receipts_in_window(&receipts, since)?;
        let mut at_window_start = BTreeMap::new();
        for (n, (agent, (seq, turn_start))) in first_in_window.into_iter().enumerate() {
            if n == BASELINE_LOOKUPS {
                // A biller left out lowers the rate, so it stays a floor.
                approximate = true;
                break;
            }
            let own = EventFilter {
                agent: Some(agent),
                exclude_children: true,
                ..receipts.clone()
            };
            let before = match self.store.events_before(seq, 1, &own) {
                Ok(before) => before,
                // Retention removed the receipts before it: none is kept.
                Err(aether_store::StoreError::HistoryExpired { .. }) => Vec::new(),
                Err(error) => return Err(error.into()),
            };
            let cost = match before.first().map(|event| &event.kind) {
                Some(EventKind::AgentUsageUpdated { total, .. }) => total.cost_usd,
                _ => turn_start,
            };
            debug_assert!(cost >= 0.0, "a cumulative cost is never negative");
            at_window_start.insert(agent, cost);
        }
        assert!(
            at_window_start.len() <= BASELINE_LOOKUPS,
            "the lookups are bounded"
        );
        Ok(WindowStart {
            at_window_start,
            approximate,
        })
    }

    /// Each agent's first receipt at or after `since`, by seq, with what its
    /// turn had billed before it, and whether some of the window went unread.
    fn first_receipts_in_window(
        &self,
        receipts: &EventFilter,
        since: Timestamp,
    ) -> Result<(BTreeMap<AgentId, (u64, f64)>, bool)> {
        // Newest first, so the last receipt seen per agent is its first in the window.
        let mut first_in_window: BTreeMap<AgentId, (u64, f64)> = BTreeMap::new();
        let mut approximate = false;
        let mut before = None;
        for _ in 0..RECEIPT_PAGES {
            let page = self.receipts_before(before, receipts)?;
            approximate |= page.partial;
            let mut oldest = before.unwrap_or(u64::MAX);
            for event in &page.events {
                assert!(
                    event.seq < oldest,
                    "a page runs newest first, below the last"
                );
                oldest = event.seq;
                if event.at < since {
                    return Ok((first_in_window, approximate));
                }
                if let (Some(agent), EventKind::AgentUsageUpdated { total, turn_usage }) =
                    (event.agent_id, &event.kind)
                {
                    let turn_start = (total.cost_usd - turn_usage.cost_usd).max(0.0);
                    first_in_window.insert(agent, (event.seq, turn_start));
                }
            }
            if !page.more {
                return Ok((first_in_window, approximate));
            }
            before = Some(oldest);
        }
        Ok((first_in_window, true))
    }

    /// A page of `receipts` older than `before` (the newest with none), newest
    /// first. Retention may have removed some: the page then holds those kept after.
    fn receipts_before(&self, before: Option<u64>, receipts: &EventFilter) -> Result<ReceiptPage> {
        let read = match before {
            None => self.store.latest_events(RECEIPT_PAGE, receipts),
            Some(before) => self.store.events_before(before, RECEIPT_PAGE, receipts),
        };
        let expired_until = match read {
            Ok(events) => {
                let more = events.len() == RECEIPT_PAGE;
                return Ok(ReceiptPage {
                    events,
                    more,
                    partial: false,
                });
            }
            Err(aether_store::StoreError::HistoryExpired { last_seq, .. }) => last_seq,
            Err(error) => return Err(error.into()),
        };
        // Nothing older than the span is kept, so the walk ends here.
        let mut events = self.store.events(expired_until, RECEIPT_PAGE, receipts)?;
        let full = events.len() == RECEIPT_PAGE;
        events.retain(|event| before.is_none_or(|before| event.seq < before));
        events.reverse();
        assert!(events.iter().all(|event| event.seq > expired_until));
        // A full read from the span's end may stop short of the newest receipts.
        Ok(ReceiptPage {
            events,
            more: false,
            partial: full,
        })
    }

    /// Tasks that a waiting agent is blocked on, for the live graph.
    pub fn waiting_on(agent: &Agent) -> BTreeSet<TaskId> {
        match &agent.status {
            AgentStatus::Waiting { condition } => condition.task_ids(),
            _ => BTreeSet::new(),
        }
    }
}

#[cfg(test)]
#[path = "status_tests.rs"]
mod tests;

#[cfg(test)]
#[path = "status_scale_tests.rs"]
mod scale_tests;
