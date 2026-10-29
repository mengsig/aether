//! What an ended agent leaves running. A child that completed under a live
//! ancestor hands it to the nearest one; any other end stops it, together
//! with what ended descendants handed to it. A daemon stop stops it all.

use crate::commands::PendingReap;
use crate::{CommitHold, Runtime};
use aether_core::agent::{Agent, AgentStatus};
use aether_core::event::{Actor, EventDraft, EventKind};
use aether_core::id::AgentId;
use aether_core::model::ContentBlock;
use aether_engine::reap::AgentProcess;
use aether_store::Commit;
use aether_tools::background::{BackgroundProcesses, ProcessStatus};
use std::collections::HashMap;

/// The custom event naming what an ended agent's engines left running.
pub(crate) const PROCESSES_REAPED_EVENT: &str = "agent.processes_reaped";
/// The custom event naming what a completed child handed to its new owner.
pub(crate) const PROCESSES_HANDED_UP_EVENT: &str = "agent.processes_handed_up";

/// The longest a daemon stop waits between SIGTERM and SIGKILL to its agents'
/// processes, whatever `reap_grace_secs` says: a stop must not hang for minutes.
const SHUTDOWN_REAP_GRACE: std::time::Duration = std::time::Duration::from_secs(10);
/// The most processes a hand-up message names; its event records them all.
const HANDED_UP_LISTED: usize = 50;
/// The deepest ancestry walked for an owner; a parent cycle must not spin.
const MAX_ANCESTRY: usize = 10_000;

impl Runtime {
    /// Let go of what an ended agent was holding, once its terminal
    /// transition is committed: its quarantine hold, whose release the commit
    /// just recorded, the background shell processes it started and whatever
    /// its engines and shell commands left running. Every terminal transition
    /// goes through `terminate_agent`, so this runs however the agent ended.
    pub(crate) fn reap_ended_agent(&self, pending: PendingReap) {
        let agent = pending.0;
        self.resources.release_quarantine(agent);
        // A turn cancelled as its agent ended may be starting a process now.
        // Past these fences it has started or never will, so the sweep sees it.
        self.engine_spawns.fence();
        if let Some(processes) = self.tools.background() {
            processes.fence_spawns();
        }
        // No wake or other reap commits under the hold, so what the agent holds
        // now is what its end leaves, and a job its next turn starts is never among it.
        let hold = self.hold();
        self.awaiting_reap.lock().remove(&agent);
        let Ok(handle) = tokio::runtime::Handle::try_current() else {
            tracing::warn!(%agent, "no runtime to stop the agent's processes on");
            return;
        };
        if !self.has_ended(agent) {
            tracing::info!(%agent, "restarted before its reap; its processes stay its own");
            return;
        }
        match self.handed_up_to(&hold, agent) {
            Some(owner) => self.hand_up_processes(&hold, &handle, agent, owner),
            None => self.stop_owned_processes(&hold, &handle, agent),
        }
    }

    /// Whether `row` still holds what is handed to it: it is live, or it
    /// ended and its own reap, which passes that on or stops it, is to come.
    fn holds_processes(&self, _hold: &CommitHold<'_>, row: &Agent) -> bool {
        !row.status.is_terminal() || self.awaiting_reap.lock().contains(&row.id)
    }

    /// The ancestor a completed agent hands its processes to. None when it
    /// ended otherwise, or no ancestor holds processes: then they are stopped.
    fn handed_up_to(&self, hold: &CommitHold<'_>, agent: AgentId) -> Option<AgentId> {
        let row = match self.store.agent(agent) {
            Ok(Some(row)) => row,
            Ok(None) => return None,
            Err(error) => {
                tracing::warn!(%agent, %error, "cannot read the ended agent; stopping its processes");
                return None;
            }
        };
        // Restarted since: its processes are its next engine's, and no reap signals them.
        if !matches!(row.status, AgentStatus::Completed { .. }) {
            return None;
        }
        let owner = self.nearest_holder(hold, row.parent)?;
        assert_ne!(owner, agent, "an ended agent is not its own owner");
        Some(owner)
    }

    /// The first agent from `next` up the parent chain that holds processes.
    fn nearest_holder(&self, hold: &CommitHold<'_>, mut next: Option<AgentId>) -> Option<AgentId> {
        for _ in 0..MAX_ANCESTRY {
            let id = next?;
            match self.store.agent(id) {
                Ok(Some(row)) if self.holds_processes(hold, &row) => return Some(id),
                Ok(Some(row)) => next = row.parent,
                Ok(None) => return None,
                Err(error) => {
                    tracing::warn!(agent = %id, %error, "cannot read an ancestor; stopping the processes");
                    return None;
                }
            }
        }
        tracing::error!(?next, "the ancestry walk exceeded {MAX_ANCESTRY} agents");
        None
    }

    /// `agent` and each reaped descendant reached through reaped agents alone:
    /// what those completed with was handed up to `agent`. A live descendant,
    /// or one whose reap is to come, keeps its own and what was handed to it.
    fn owned_agents(&self, hold: &CommitHold<'_>, agent: AgentId) -> Vec<AgentId> {
        let subtree = match self.store.subtree_agents(agent) {
            Ok(subtree) => subtree,
            Err(error) => {
                tracing::warn!(%agent, %error, "cannot list the descendants whose processes an end takes");
                return vec![agent];
            }
        };
        let mut children: HashMap<AgentId, Vec<&Agent>> = HashMap::new();
        for row in &subtree {
            if let Some(parent) = row.parent {
                children.entry(parent).or_default().push(row);
            }
        }
        let mut owned = vec![agent];
        let mut next = 0;
        while let Some(parent) = owned.get(next).copied() {
            next += 1;
            // An imported agent ran elsewhere; nothing it started is ours.
            owned.extend(
                children
                    .get(&parent)
                    .into_iter()
                    .flatten()
                    .filter(|child| !self.holds_processes(hold, child))
                    .filter(|child| !matches!(child.status, AgentStatus::Imported { .. }))
                    .map(|child| child.id),
            );
            assert!(
                owned.len() <= subtree.len().max(1),
                "a parent cycle in the subtree"
            );
        }
        debug_assert!(
            owned.iter().skip(1).all(|id| *id != agent),
            "a parent cycle reached the ending agent"
        );
        owned
    }

    /// Stop what `agent` and the ended agents it owns left running, their
    /// background jobs included, and record what the reap took.
    fn stop_owned_processes(
        &self,
        hold: &CommitHold<'_>,
        handle: &tokio::runtime::Handle,
        agent: AgentId,
    ) {
        let owned = self.owned_agents(hold, agent);
        assert_eq!(owned.first(), Some(&agent), "an end stops its own first");
        self.reap_engine_processes(handle, agent, owned.clone());
        let Some(processes) = self.tools.background() else {
            return;
        };
        // Taken now: a job an agent woken meanwhile starts is not stopped with these.
        let stopping = processes.stop_agents(&owned);
        handle.spawn(async move {
            let stopped = stopping.await;
            if stopped > 0 {
                tracing::info!(%agent, stopped, "stopped background processes with the agent");
            }
        });
    }

    /// Stop what `owned`'s engines and shell commands left running - a
    /// server, a detached job - and record it on `agent` as `agent.processes_reaped`.
    fn reap_engine_processes(
        &self,
        handle: &tokio::runtime::Handle,
        agent: AgentId,
        owned: Vec<AgentId>,
    ) {
        let grace = self.models.config().agents.reap_grace();
        let me = self.me.clone();
        let gates = self.spawn_gates();
        handle.spawn(async move {
            // A restarted agent's tag is its new engine's, so the sweep spares it.
            let ended = |id| me.upgrade().is_some_and(|runtime| runtime.has_ended(id));
            let report = aether_engine::reap::reap_agents(&owned, grace, ended, &gates).await;
            if report.is_empty() {
                return;
            }
            let Some(runtime) = me.upgrade() else {
                tracing::warn!(%agent, ?report, "runtime gone before the reaped processes were recorded");
                return;
            };
            if let Err(error) = runtime.record_processes_reaped(agent, &report) {
                tracing::error!(%agent, %error, "could not record the agent's reaped processes");
            }
        });
    }

    /// Give `owner` what completed `agent` and the ended agents it owns left
    /// running, background jobs included, and tell `owner` what that is.
    fn hand_up_processes(
        &self,
        hold: &CommitHold<'_>,
        handle: &tokio::runtime::Handle,
        agent: AgentId,
        owner: AgentId,
    ) {
        assert_ne!(agent, owner, "an agent hands nothing to itself");
        let owned = self.owned_agents(hold, agent);
        assert!(
            !owned.contains(&owner),
            "the owner is an ancestor, so not among what it owns"
        );
        let me = self.me.clone();
        let jobs = self.tools.background().cloned();
        // Moved now: a job an agent woken meanwhile starts stays its own.
        let handed_jobs = match &jobs {
            Some(jobs) => hand_up_jobs(jobs, &owned, owner),
            None => Vec::new(),
        };
        handle.spawn(async move {
            let processes = aether_engine::reap::live_processes(&owned).await;
            let Some(runtime) = me.upgrade() else {
                tracing::warn!(%agent, %owner, "runtime gone before the hand-up was recorded");
                return;
            };
            // With no live owner left, the ends that stopped it stopped all of this.
            let Some(owner) = runtime.settle_owner(jobs.as_deref(), owner).await else {
                return;
            };
            if processes.is_empty() && handed_jobs.is_empty() {
                return;
            }
            runtime.report_hand_up(agent, owner, &processes, &handed_jobs);
        });
    }

    /// The live agent that holds what was handed to `owner`, to be told of it.
    /// An owner whose reap is to come passes it on and reports it itself; one
    /// already reaped passes it on, or stops it, here. `None` when not to tell.
    async fn settle_owner(
        &self,
        jobs: Option<&BackgroundProcesses>,
        mut owner: AgentId,
    ) -> Option<AgentId> {
        for _ in 0..MAX_ANCESTRY {
            let (next, stopping) = {
                // Held so the owner cannot wake between the check and the move.
                let hold = self.hold();
                if !self.has_ended(owner) {
                    return Some(owner);
                }
                if self.awaiting_reap.lock().contains(&owner) {
                    return None;
                }
                let next = self.handed_up_to(&hold, owner);
                assert_ne!(next, Some(owner), "an ended owner hands nothing to itself");
                let stopping = match (next, jobs) {
                    (Some(next), Some(jobs)) => {
                        drop(hand_up_jobs(jobs, &[owner], next));
                        None
                    }
                    (None, Some(jobs)) => Some(jobs.stop_agents(&[owner])),
                    (_, None) => None,
                };
                (next, stopping)
            };
            if let Some(stopping) = stopping {
                let stopped = stopping.await;
                tracing::info!(%owner, stopped, "stopped jobs handed to an owner that had ended");
            }
            owner = next?;
        }
        tracing::error!(%owner, "the hand-up walk exceeded {MAX_ANCESTRY} agents");
        None
    }

    /// Record the hand-up on `agent` and tell `owner` what now runs for it.
    fn report_hand_up(
        &self,
        agent: AgentId,
        owner: AgentId,
        processes: &[AgentProcess],
        jobs: &[ProcessStatus],
    ) {
        assert!(
            !processes.is_empty() || !jobs.is_empty(),
            "nothing handed up is not reported"
        );
        // A command line can carry a credential; the owner sees it masked.
        let redactor = self.tool_runtime.redactor();
        let processes: Vec<AgentProcess> = processes
            .iter()
            .map(|process| AgentProcess {
                pid: process.pid,
                command: redactor.redact(&process.command).into_owned(),
            })
            .collect();
        let jobs: Vec<(String, String)> = jobs
            .iter()
            .map(|job| {
                (
                    job.id.to_string(),
                    redactor.redact(&job.command).into_owned(),
                )
            })
            .collect();
        let (processes, jobs) = (processes.as_slice(), jobs.as_slice());
        if let Err(error) = self.record_hand_up(agent, owner, processes, jobs) {
            tracing::error!(%agent, %owner, %error, "could not record the handed-up processes");
        }
        let text = hand_up_message(agent, processes, jobs);
        if let Err(error) =
            self.send_message(owner, vec![ContentBlock::Text { text }], Actor::System)
        {
            tracing::warn!(%agent, %owner, %error, "could not tell the owner of the handed-up processes");
        }
    }

    fn record_hand_up(
        &self,
        agent: AgentId,
        owner: AgentId,
        processes: &[AgentProcess],
        jobs: &[(String, String)],
    ) -> crate::Result<()> {
        let Some(row) = self.store.agent(agent)? else {
            return Ok(());
        };
        let jobs: Vec<serde_json::Value> = jobs
            .iter()
            .map(|(id, command)| serde_json::json!({ "id": id, "command": command }))
            .collect();
        let payload = serde_json::json!({
            "to": owner,
            "pids": processes.iter().map(|process| process.pid).collect::<Vec<_>>(),
            "processes": processes,
            "jobs": jobs,
        });
        let kind = EventKind::Custom {
            name: PROCESSES_HANDED_UP_EVENT.to_owned(),
            payload,
        };
        self.apply(
            Commit::new().event(
                EventDraft::new(Actor::System, kind)
                    .agent(agent)
                    .run(row.run),
            ),
        )?;
        Ok(())
    }

    /// Stop what this daemon's agents still have running as it stops, ended
    /// or not: once it exits, nothing would stop a detached server or
    /// browser until the agent ends. Returns how many processes were signalled.
    pub(crate) async fn reap_agent_processes_at_shutdown(&self) -> usize {
        let mut found = aether_engine::reap::agents_with_processes().await;
        // A turn that outlived the wait for it may still fork; its agent is swept too.
        found.extend(self.sched.lock().running.keys().copied());
        found.sort_unstable();
        found.dedup();
        let ours: Vec<AgentId> = found
            .into_iter()
            .filter(|agent| self.ran_here(*agent))
            .collect();
        if ours.is_empty() {
            return 0;
        }
        let grace = self
            .models
            .config()
            .agents
            .reap_grace()
            .min(SHUTDOWN_REAP_GRACE);
        let report = aether_engine::reap::reap_agents(&ours, grace, |_| true, &[]).await;
        report.terminated.len()
    }

    /// Whether `agent` is one this store ran. Another daemon's agents carry
    /// tags too, and an imported agent ran elsewhere; neither is ours to stop.
    fn ran_here(&self, agent: AgentId) -> bool {
        match self.store.agent(agent) {
            Ok(Some(row)) => !matches!(row.status, AgentStatus::Imported { .. }),
            Ok(None) => false,
            Err(error) => {
                tracing::warn!(%agent, %error, "cannot tell whether the agent is ours; sparing its processes");
                false
            }
        }
    }

    /// Whether `agent` is still ended; a restart means its processes are its new engine's.
    /// The gates every engine and host command starts through, in the order
    /// a sweep holds them.
    fn spawn_gates(&self) -> Vec<aether_engine::SpawnGate> {
        let mut gates = vec![self.engine_spawns.clone()];
        gates.extend(self.tools.background().map(|jobs| jobs.spawn_gate()));
        gates
    }

    fn has_ended(&self, agent: AgentId) -> bool {
        match self.store.agent(agent) {
            Ok(Some(row)) => row.status.is_terminal(),
            Ok(None) => true,
            Err(error) => {
                tracing::warn!(%agent, %error, "cannot tell whether the agent is still ended; sparing its processes");
                false
            }
        }
    }

    fn record_processes_reaped(
        &self,
        agent: AgentId,
        report: &aether_engine::reap::ReapReport,
    ) -> crate::Result<()> {
        assert!(!report.is_empty(), "nothing reaped is not recorded");
        let Some(row) = self.store.agent(agent)? else {
            return Ok(());
        };
        let payload = serde_json::json!({
            "terminated": report.terminated.len(),
            "killed": report.killed.len(),
            "pids": report.terminated,
            "killed_pids": report.killed,
            "survived": report.survived,
        });
        let kind = EventKind::Custom {
            name: PROCESSES_REAPED_EVENT.to_owned(),
            payload,
        };
        self.apply(
            Commit::new().event(
                EventDraft::new(Actor::System, kind)
                    .agent(agent)
                    .run(row.run),
            ),
        )?;
        Ok(())
    }
}

/// Give `owner` the background jobs of each of `from`; those still running.
fn hand_up_jobs(
    jobs: &BackgroundProcesses,
    from: &[AgentId],
    owner: AgentId,
) -> Vec<ProcessStatus> {
    let mut handed = Vec::new();
    for agent in from {
        match jobs.hand_up(*agent, owner) {
            Ok(running) => handed.extend(running),
            Err(error) => {
                tracing::error!(%agent, %owner, %error, "could not hand background jobs up");
            }
        }
    }
    handed
}

/// What the new owner reads: what runs for it now, and when it stops.
/// `jobs` are each a background job's id and command line.
fn hand_up_message(
    agent: AgentId,
    processes: &[AgentProcess],
    jobs: &[(String, String)],
) -> String {
    use std::fmt::Write as _;

    let mut text = format!(
        "Agent {agent} completed and handed you what it left running. It is stopped \
         when you end with no live ancestor, are cancelled or fail, or the daemon \
         stops; stop it sooner with `kill <pid>`."
    );
    if !jobs.is_empty() {
        text.push_str("\nBackground shell jobs, now yours to read and stop:");
        for (id, command) in jobs {
            let _ = write!(text, "\n- {id}: {command}");
        }
    }
    if !processes.is_empty() {
        text.push_str("\nProcesses (pid: command):");
        for process in processes.iter().take(HANDED_UP_LISTED) {
            let _ = write!(text, "\n- {}: {}", process.pid, process.command);
        }
        if processes.len() > HANDED_UP_LISTED {
            let _ = write!(text, "\n- and {} more", processes.len() - HANDED_UP_LISTED);
        }
    }
    text
}

#[cfg(test)]
mod tests;
