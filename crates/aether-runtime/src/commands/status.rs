use super::RuntimeStatus;
use crate::{Result, Runtime};
use aether_core::agent::{Agent, AgentStatus, AgentStatusKind, BlockReason};
use aether_core::budget::{RATE_WINDOW_MINUTES, RunSummary};
use aether_core::event::EventKind;
use aether_core::id::{AgentId, RunId, TaskId};
use aether_core::time::{Timestamp, now};
use aether_store::{AgentFilter, EventFilter, TaskFilter};
use std::collections::{BTreeMap, BTreeSet};

/// Receipts read per page while walking a run's recent billing back in time.
const RECEIPT_PAGE: usize = 256;

impl Runtime {
    pub fn status(&self) -> Result<RuntimeStatus> {
        let mut engines: Vec<String> = self.engines.values().map(|e| e.adapter()).collect();
        engines.sort();
        // Read before the struct literal: `provider_health` takes the same
        // lock, and a guard living inside the literal would still be held.
        let engine_turns_running = self.sched.lock().on_engine() as u32;
        let mut status = RuntimeStatus {
            yolo: self.config.yolo,
            network_default: self.config.network_default,
            max_concurrent_turns: self.config.max_concurrent_turns,
            max_concurrent_engine_turns: self.config.max_concurrent_engine_turns,
            engine_turns_running,
            run_budget_usd: self.config.run_budget_usd,
            engines,
            providers: self.provider_health()?,
            resource_leases: self.resource_holdings(),
            mcp: self.mcp_health(),
            low_disk: self.low_disk(),
            ..Default::default()
        };
        for agent in self.store.agents(&AgentFilter {
            live_only: true,
            ..Default::default()
        })? {
            status.agents_live += 1;
            if agent.paused {
                status.agents_paused += 1;
            }
            match agent.status.kind() {
                AgentStatusKind::Running => status.agents_running += 1,
                AgentStatusKind::Waiting => status.agents_waiting += 1,
                AgentStatusKind::Blocked => {
                    status.agents_blocked += 1;
                    if matches!(
                        &agent.status,
                        AgentStatus::Blocked {
                            reason: BlockReason::Exhausted { .. }
                        }
                    ) {
                        status.agents_suspended += 1;
                    }
                }
                _ => {}
            }
        }
        let all = self.store.agents(&AgentFilter::default())?;
        for agent in &all {
            // An imported run is an archive of what another machine spent.
            // Counting it here would bill this daemon for someone else's
            // work; it is reported under its own run, by `runs report`.
            if agent.status.kind() != AgentStatusKind::Imported {
                status.usage_total += agent.stats.usage;
            }
        }
        status.runs = self.run_summaries(&all)?;
        status.tasks_active = self
            .store
            .tasks(&TaskFilter {
                statuses: vec![
                    aether_core::task::TaskStatusKind::Active,
                    aether_core::task::TaskStatusKind::Assigned,
                ],
                ..Default::default()
            })?
            .len() as u32;
        status.pending_approvals = self.store.pending_approvals()?.len() as u32;
        status.approval_default = self.config.approval_default;
        Ok(status)
    }

    /// Every live run's spend, rated over the receipts of its recent window.
    fn run_summaries(&self, all: &[Agent]) -> Result<Vec<RunSummary>> {
        let at = now();
        let since = at - chrono::Duration::minutes(RATE_WINDOW_MINUTES);
        let live_runs: BTreeSet<RunId> =
            all.iter().filter(|a| a.is_live()).map(|a| a.run).collect();
        let mut window_start = BTreeMap::new();
        for run in live_runs {
            window_start.extend(self.billing_at(run, since)?);
        }
        Ok(aether_core::budget::active_runs(all, &window_start, at))
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
