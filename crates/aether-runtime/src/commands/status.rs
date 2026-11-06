use super::RuntimeStatus;
use crate::{Result, Runtime, RuntimeError};
use aether_core::agent::{Agent, AgentStatus, AgentStatusKind, BlockReason};
use aether_core::budget::{RATE_WINDOW_MINUTES, RunSummary};
use aether_core::event::EventKind;
use aether_core::id::{AgentId, RunId, TaskId};
use aether_core::model::Usage;
use aether_core::provider::ProviderHealth;
use aether_core::task::TaskStatusKind;
use aether_core::time::{Timestamp, now};
use aether_protocol::{UnavailableSection, status_sections as section};
use aether_store::{AgentFilter, EventFilter, TaskFilter};
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

/// How long the daemon's `status` waits on the store; under its patience,
/// so a stalled section costs only its own figures.
pub const STATUS_DEADLINE: Duration = Duration::from_secs(2);

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
}

type Answer = (&'static str, Result<Figure>);

/// A reader thread and the sections it answers, in the order it answers them.
struct Reader {
    sections: &'static [&'static str],
    read: fn(&Runtime, &mpsc::Sender<Answer>),
}

const READERS: [Reader; 7] = [
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
                Ok(Figure::PendingApprovals(
                    rt.store.pending_approvals()?.len() as u32,
                ))
            })
        },
    },
    Reader {
        sections: &[section::LOW_DISK],
        read: |rt, to| answer(to, section::LOW_DISK, || Ok(Figure::LowDisk(rt.low_disk()))),
    },
];

/// Every section a reader answers.
const SECTIONS: [&str; 8] = [
    section::PROBLEMS,
    section::PROVIDERS,
    section::LIVE_AGENTS,
    section::RUNS,
    section::USAGE_TOTAL,
    section::TASKS_ACTIVE,
    section::PENDING_APPROVALS,
    section::LOW_DISK,
];

/// Marks a reader in flight until its thread ends, however it ends.
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
fn answer(to: &mpsc::Sender<Answer>, section: &'static str, read: impl FnOnce() -> Result<Figure>) {
    debug_assert!(
        SECTIONS.contains(&section),
        "an answer names a known section"
    );
    let figure = timed(section, read);
    if let Err(mpsc::SendError((section, Err(error)))) = to.send((section, figure)) {
        tracing::warn!(section, %error, "status section failed after its status gave up on it");
    }
}

/// The live agents, then the live runs' spend, which is read from them.
fn read_live_and_runs(rt: &Runtime, to: &mpsc::Sender<Answer>) {
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
    }
}

/// Heaviest record kinds `status` names; the rest are folded into `other`.
const WRITE_KINDS_SHOWN: usize = 8;

/// Receipts read per page while walking a run's recent billing back in time.
const RECEIPT_PAGE: usize = 256;

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
        let mut status = self.status_in_memory();
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

    /// The sections held in memory: no store read, so they are never late.
    fn status_in_memory(&self) -> RuntimeStatus {
        let mut engines: Vec<String> = match self.engines.now() {
            Ok(detected) => detected.usable.values().map(|e| e.adapter()).collect(),
            Err(_) => vec![aether_protocol::ENGINES_STARTING.to_owned()],
        };
        engines.sort();
        // Read before the literal: the guard of a field expression lives to its end.
        let engine_turns_running = self.sched.lock().on_engine() as u32;
        RuntimeStatus {
            engine_turns_running,
            engines,
            resource_leases: timed("resource_leases", || self.resource_holdings()),
            mcp: timed("mcp", || self.mcp_health()),
            writes: self.store.write_stats().top(WRITE_KINDS_SHOWN),
            ..self.status_settings()
        }
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
            let in_flight = InFlight {
                runtime: me.clone(),
                key,
            };
            let sender = sender.clone();
            let read = reader.read;
            std::thread::Builder::new()
                .name(format!("status-{key}"))
                .spawn(move || read(&in_flight.runtime, &sender))
                .map_err(|error| {
                    RuntimeError::Internal(format!("cannot start the {key} status read: {error}"))
                })?;
        }
        assert!(busy.iter().all(|section| SECTIONS.contains(section)));
        Ok((answers, busy))
    }

    fn count_active_tasks(&self) -> Result<Figure> {
        let active = self.store.tasks(&TaskFilter {
            statuses: vec![TaskStatusKind::Active, TaskStatusKind::Assigned],
            ..Default::default()
        })?;
        Ok(Figure::TasksActive(active.len() as u32))
    }

    /// Every live run's spend, rated over the receipts of its recent window.
    /// `live` is the live agents; each live run's members are read by run, so
    /// a fleet of finished runs is never decoded.
    fn run_summaries(&self, live: &[Agent]) -> Result<Vec<RunSummary>> {
        let at = now();
        let since = at - chrono::Duration::minutes(RATE_WINDOW_MINUTES);
        let live_runs: BTreeSet<RunId> = live.iter().map(|a| a.run).collect();
        let mut members = Vec::new();
        for run in &live_runs {
            members.extend(self.store.agents(&AgentFilter {
                run: Some(*run),
                ..Default::default()
            })?);
        }
        debug_assert!(live.iter().all(|l| members.iter().any(|m| m.id == l.id)));
        let mut window_start = BTreeMap::new();
        for run in live_runs {
            window_start.extend(self.billing_at(run, since)?);
        }
        Ok(aether_core::budget::active_runs(
            &members,
            &window_start,
            at,
        ))
    }

    /// What each agent of `run` with a usage receipt after `since` had billed
    /// then: its last receipt before it, or the start of the turn that billed.
    /// One run at a time, so an expired run's history is never in range.
    fn billing_at(&self, run: RunId, since: Timestamp) -> Result<BTreeMap<AgentId, f64>> {
        let receipts = EventFilter {
            run: Some(run),
            types: vec!["agent.usage_updated".into()],
            ..Default::default()
        };
        // Newest first, so the last receipt seen per agent is its first in the window.
        let mut first_in_window: BTreeMap<AgentId, (u64, f64)> = BTreeMap::new();
        let mut page = self.store.latest_events(RECEIPT_PAGE, &receipts)?;
        'pages: while !page.is_empty() {
            let full = page.len() == RECEIPT_PAGE;
            let mut oldest = u64::MAX;
            for event in &page {
                debug_assert!(event.seq < oldest, "a page runs newest first");
                oldest = event.seq;
                if event.at < since {
                    break 'pages;
                }
                if let (Some(agent), EventKind::AgentUsageUpdated { total, turn_usage }) =
                    (event.agent_id, &event.kind)
                {
                    let turn_start = (total.cost_usd - turn_usage.cost_usd).max(0.0);
                    first_in_window.insert(agent, (event.seq, turn_start));
                }
            }
            if !full {
                break;
            }
            page = self.store.events_before(oldest, RECEIPT_PAGE, &receipts)?;
        }
        let mut billing = BTreeMap::new();
        for (agent, (seq, turn_start)) in first_in_window {
            let own = EventFilter {
                agent: Some(agent),
                exclude_children: true,
                ..receipts.clone()
            };
            let before = self.store.events_before(seq, 1, &own)?;
            let cost = match before.first().map(|event| &event.kind) {
                Some(EventKind::AgentUsageUpdated { total, .. }) => total.cost_usd,
                _ => turn_start,
            };
            debug_assert!(cost >= 0.0, "a cumulative cost is never negative");
            billing.insert(agent, cost);
        }
        Ok(billing)
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
