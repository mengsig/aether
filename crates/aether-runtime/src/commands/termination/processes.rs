//! What an ended agent leaves running. A child that completed under a live
//! ancestor hands it to the nearest one; any other end stops it, together
//! with what reaped agents handed up to it. A daemon stop or start stops
//! what no live agent holds.

use crate::commands::PendingReap;
use crate::{CommitHold, Runtime};
use aether_core::agent::{Agent, AgentStatus};
use aether_core::event::{Actor, EventDraft, EventKind};
use aether_core::id::AgentId;
use aether_core::model::ContentBlock;
use aether_core::time::Timestamp;
use aether_engine::reap::AgentProcess;
use aether_store::Commit;
use aether_tools::background::{BackgroundProcesses, ProcessStatus};
use std::collections::{HashMap, HashSet};

/// The custom event naming what an ended agent's engines left running.
pub(crate) const PROCESSES_REAPED_EVENT: &str = "agent.processes_reaped";
/// The custom event naming what a completed child handed to its new owner.
pub(crate) const PROCESSES_HANDED_UP_EVENT: &str = "agent.processes_handed_up";

/// The longest a daemon stop waits between SIGTERM and SIGKILL to its agents'
/// processes, whatever `reap_grace_secs` says: a stop must not hang for minutes.
const SHUTDOWN_REAP_GRACE: std::time::Duration = std::time::Duration::from_secs(10);
/// How often a daemon sweep goes back for agents it spared for a holder that
/// may have ended again since; each pass needs another wake and end to race it.
const SWEEP_PASSES: usize = 3;
/// The most processes a hand-up message names with their command; the rest
/// are named by pid alone, and its event records them all.
const HANDED_UP_LISTED: usize = 50;
/// The deepest ancestry walked for an owner; a parent cycle must not spin.
const MAX_ANCESTRY: usize = 10_000;

/// Who holds what an ended agent left running, as the store says now.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Holder {
    /// The nearest ancestor of a completed agent that holds processes.
    Live(AgentId),
    /// It ended otherwise, or no ancestor holds processes.
    Nobody,
    /// It or one of its ancestors could not be read, or is gone from the store.
    Unknown,
}

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
    fn holds_processes(&self, row: &Agent) -> bool {
        !row.status.is_terminal() || self.awaiting_reap.lock().contains(&row.id)
    }

    /// The ancestor a completed agent hands its processes to. None when it
    /// ended otherwise, or no ancestor holds processes: then they are stopped.
    fn handed_up_to(&self, _hold: &CommitHold<'_>, agent: AgentId) -> Option<AgentId> {
        match self.holder(agent) {
            Holder::Live(owner) => Some(owner),
            // An end with an unreadable or broken ancestry stops what it left, as one with no owner does.
            Holder::Nobody | Holder::Unknown => None,
        }
    }

    /// Who holds what ended `agent` left, read from the store now: the nearest
    /// ancestor that holds processes, if it completed.
    fn holder(&self, agent: AgentId) -> Holder {
        let row = match self.store.agent(agent) {
            Ok(Some(row)) => row,
            // History retention removed it since; its ancestry is not known.
            Ok(None) => return Holder::Unknown,
            Err(error) => {
                tracing::warn!(%agent, %error, "cannot read the ended agent");
                return Holder::Unknown;
            }
        };
        // Restarted since: its processes are its next engine's, and no reap signals them.
        if !matches!(row.status, AgentStatus::Completed { .. }) {
            return Holder::Nobody;
        }
        let holder = self.nearest_holder(row.parent);
        assert_ne!(
            holder,
            Holder::Live(agent),
            "an ended agent is not its own owner"
        );
        holder
    }

    /// The first agent from `next` up the parent chain that holds processes.
    fn nearest_holder(&self, mut next: Option<AgentId>) -> Holder {
        for _ in 0..MAX_ANCESTRY {
            let Some(id) = next else {
                return Holder::Nobody;
            };
            match self.store.agent(id) {
                Ok(Some(row)) if self.holds_processes(&row) => return Holder::Live(id),
                Ok(Some(row)) => next = row.parent,
                // Its own parent, which may be live, is not known.
                Ok(None) => {
                    tracing::warn!(agent = %id, "an ancestor is gone from the store");
                    return Holder::Unknown;
                }
                Err(error) => {
                    tracing::warn!(agent = %id, %error, "cannot read an ancestor");
                    return Holder::Unknown;
                }
            }
        }
        tracing::error!(?next, "the ancestry walk exceeded {MAX_ANCESTRY} agents");
        Holder::Unknown
    }

    /// Whether `agent` still holds what is handed to it: it is live, or it
    /// ended and its own reap, which passes that on or stops it, is to come.
    fn holds(&self, _hold: &CommitHold<'_>, agent: AgentId) -> bool {
        !self.has_ended(agent) || self.awaiting_reap.lock().contains(&agent)
    }

    /// `agent` and each reaped agent whose processes were handed to it, or on
    /// to it through other reaped agents. One woken since holds its own again.
    fn owned_agents(&self, hold: &CommitHold<'_>, agent: AgentId) -> Vec<AgentId> {
        let mut handed_by: HashMap<AgentId, Vec<AgentId>> = HashMap::new();
        let recorded = {
            let handed_to = self.handed_to.lock();
            for (from, to) in handed_to.iter() {
                handed_by.entry(*to).or_default().push(*from);
            }
            handed_to.len()
        };
        let mut owned = vec![agent];
        let mut next = 0;
        while let Some(holder) = owned.get(next).copied() {
            next += 1;
            let reaped = handed_by.remove(&holder).unwrap_or_default();
            owned.extend(reaped.into_iter().filter(|from| !self.holds(hold, *from)));
            assert!(owned.len() <= recorded + 1, "a cycle among the hand-ups");
        }
        debug_assert!(
            owned.iter().skip(1).all(|id| *id != agent),
            "a hand-up cycle reached the ending agent"
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
        {
            let mut handed_to = self.handed_to.lock();
            for id in &owned {
                handed_to.remove(id);
            }
        }
        self.reap_engine_processes(hold, handle, agent, owned.clone());
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
        _hold: &CommitHold<'_>,
        handle: &tokio::runtime::Handle,
        agent: AgentId,
        owned: Vec<AgentId>,
    ) {
        let grace = self.models.config().agents.reap_grace();
        let me = self.me.clone();
        let gates = self.spawn_gates();
        let ends = self.current_ends(&owned);
        handle.spawn(async move {
            // An agent restarted since, even one that has ended again, is spared:
            // its tag is its new engine's, or what that end handed up.
            let ended = |id| {
                me.upgrade()
                    .is_some_and(|runtime| runtime.still_in_end(id, ends.get(&id)))
            };
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
        let previous = self.handed_to.lock().insert(agent, owner);
        debug_assert!(previous != Some(agent), "an agent hands nothing to itself");
        let me = self.me.clone();
        // Moved now: a job an agent woken meanwhile starts stays its own.
        let handed_jobs = match self.tools.background() {
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
            let Some(owner) = runtime.settle_owner(owner) else {
                return;
            };
            if processes.is_empty() && handed_jobs.is_empty() {
                return;
            }
            runtime.report_hand_up(agent, owner, &processes, &handed_jobs);
        });
    }

    /// The live agent that holds what was handed to `owner`, to be told of it:
    /// `owner`'s own reap took it along, so follow its hand-ups. `None` when
    /// a stop took it, or a reap still to come reports it itself.
    fn settle_owner(&self, mut owner: AgentId) -> Option<AgentId> {
        // Held so no reap moves what `owner` holds while it is followed.
        let hold = self.hold();
        for _ in 0..MAX_ANCESTRY {
            if !self.has_ended(owner) {
                return Some(owner);
            }
            if self.holds(&hold, owner) {
                return None;
            }
            let next = self.handed_to.lock().get(&owner).copied()?;
            assert_ne!(next, owner, "an ended owner hands nothing to itself");
            owner = next;
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

    /// The tokens of the engines recorded now, for a daemon stop; none when
    /// the registry cannot be read, which is logged.
    pub(crate) fn recorded_engines() -> Vec<String> {
        match aether_engine::orphans::recorded_tokens() {
            Ok(tokens) => tokens,
            Err(error) => {
                tracing::error!(%error, "cannot read the engine records; a stop may leave interrupted commands running");
                Vec::new()
            }
        }
    }

    /// Stop what the engines with `tokens` left outside their process groups
    /// for the `interrupted` agents, whose turns this stop cut short: their
    /// in-flight commands, such as one Claude Code ran in a session of its
    /// own, which no record names once those engines are gone. Kept processes
    /// and what runs for any other agent are spared.
    pub(crate) async fn stop_interrupted_engine_commands(
        mut tokens: Vec<String>,
        interrupted: HashSet<AgentId>,
    ) {
        tokens.sort_unstable();
        tokens.dedup();
        if tokens.is_empty() {
            return;
        }
        let swept = tokio::task::spawn_blocking(move || {
            aether_engine::orphans::kill_carriers(&tokens, &interrupted)
        })
        .await;
        match swept {
            Ok(Ok(())) => {}
            Ok(Err(error)) => {
                tracing::error!(%error, "could not stop the commands of the engines this stop interrupted");
            }
            Err(error) => {
                tracing::error!(%error, "the sweep of the interrupted engines' commands did not finish");
            }
        }
    }

    /// Stop what this daemon's agents left running that no live agent holds:
    /// as it stops, since nothing would stop it after, and as it starts, after
    /// one that died. A live agent keeps its own, and a completed one's stay
    /// with its nearest live ancestor, so agents a restart resumes find their
    /// servers up. Returns how many processes were signalled.
    pub(crate) async fn reap_unheld_agent_processes(&self) -> usize {
        let mut found = aether_engine::reap::agents_with_processes().await;
        // An ended agent whose turn outlived the wait for it may still fork.
        found.extend(self.sched.lock().running.keys().copied());
        found.sort_unstable();
        found.dedup();
        let mut unheld = self.unheld_agents(&found);
        let mut signalled = 0;
        for _ in 0..SWEEP_PASSES {
            if unheld.is_empty() {
                return signalled;
            }
            let (stopped, spared) = self.sweep_unheld(&unheld).await;
            signalled += stopped;
            // An ancestor woken meanwhile holds what the sweep spared for it,
            // unless it ended again before this records the hand-up.
            unheld = self.unheld_agents(&spared);
        }
        if !unheld.is_empty() {
            tracing::warn!(agents = ?unheld, "agents kept waking through the daemon sweep; leaving their processes");
        }
        signalled
    }

    /// Stop what the ended `unheld` left running, sparing each one that a
    /// live agent holds by the time a round signals it. Returns how many were
    /// signalled, and the sorted agents spared.
    async fn sweep_unheld(&self, unheld: &[AgentId]) -> (usize, Vec<AgentId>) {
        assert!(!unheld.is_empty(), "a sweep needs an agent to stop");
        let grace = self
            .models
            .config()
            .agents
            .reap_grace()
            .min(SHUTDOWN_REAP_GRACE);
        let ends = self.current_ends(unheld);
        // An agent woken meanwhile, or one whose ancestor was, is spared; each
        // round holds the gates a wake commits outside of, so none lands mid-round.
        let spared = parking_lot::Mutex::new(Vec::new());
        let ended = |id| {
            let unheld = self.still_in_end(id, ends.get(&id)) && self.holder(id) == Holder::Nobody;
            if !unheld {
                spared.lock().push(id);
            }
            unheld
        };
        let gates = self.spawn_gates();
        let report = aether_engine::reap::reap_agents(unheld, grace, ended, &gates).await;
        let mut spared = spared.into_inner();
        spared.sort_unstable();
        spared.dedup();
        debug_assert!(
            spared.iter().all(|id| unheld.binary_search(id).is_ok()),
            "only swept agents are spared"
        );
        (report.terminated.len(), spared)
    }

    /// The ended agents among the sorted `found` that this store ran and whose
    /// processes no live agent holds. A completed one with a live ancestor is
    /// recorded as handed up to it, as a start after a crash lost that record;
    /// one whose ancestry cannot be read is spared.
    fn unheld_agents(&self, found: &[AgentId]) -> Vec<AgentId> {
        debug_assert!(found.is_sorted(), "found agents are sorted");
        let _hold = self.hold();
        let mut unheld = Vec::new();
        for agent in found.iter().copied().filter(|id| self.ended_here(*id)) {
            match self.holder(agent) {
                Holder::Nobody => unheld.push(agent),
                Holder::Live(owner) => {
                    let recorded = *self.handed_to.lock().entry(agent).or_insert(owner);
                    tracing::info!(%agent, owner = %recorded, "a completed agent's processes stay with its live ancestor");
                }
                Holder::Unknown => {
                    tracing::warn!(%agent, "cannot tell who holds the agent's processes; sparing them");
                }
            }
        }
        assert!(unheld.len() <= found.len(), "only found agents are swept");
        unheld
    }

    /// Whether `agent` is one this store ran and has ended. Another daemon's
    /// agents carry tags too, and an imported agent ran elsewhere; neither is
    /// ours to stop, and a live agent's processes are its own.
    fn ended_here(&self, agent: AgentId) -> bool {
        match self.store.agent(agent) {
            Ok(Some(row)) => {
                row.status.is_terminal() && !matches!(row.status, AgentStatus::Imported { .. })
            }
            Ok(None) => false,
            Err(error) => {
                tracing::warn!(%agent, %error, "cannot tell whether the agent is ours and ended; sparing its processes");
                false
            }
        }
    }

    /// The gates every engine and host command starts through, in the order
    /// a sweep holds them.
    fn spawn_gates(&self) -> Vec<aether_engine::SpawnGate> {
        let mut gates = vec![self.engine_spawns.clone()];
        gates.extend(self.tools.background().map(|jobs| jobs.spawn_gate()));
        gates
    }

    /// When each of `agents` entered its current status: that names its end,
    /// which a later wake and end would replace. An unreadable one is left out.
    fn current_ends(&self, agents: &[AgentId]) -> HashMap<AgentId, Option<Timestamp>> {
        let mut ends = HashMap::with_capacity(agents.len());
        for agent in agents {
            match self.store.agent(*agent) {
                Ok(row) => {
                    ends.insert(*agent, row.and_then(|row| row.status_since));
                }
                Err(error) => {
                    tracing::warn!(%agent, %error, "cannot read the ended agent; sparing its processes");
                }
            }
        }
        debug_assert!(ends.len() <= agents.len(), "an end per agent at most");
        ends
    }

    /// Whether `agent` is still in the end `since` names; a missing row has
    /// ended for good. `None` for `since` was unreadable, so it is spared.
    fn still_in_end(&self, agent: AgentId, since: Option<&Option<Timestamp>>) -> bool {
        let Some(since) = since else {
            return false;
        };
        match self.store.agent(agent) {
            Ok(Some(row)) => row.status.is_terminal() && row.status_since == *since,
            Ok(None) => true,
            Err(error) => {
                tracing::warn!(%agent, %error, "cannot tell whether the agent is still ended; sparing its processes");
                false
            }
        }
    }

    /// Whether `agent` is still ended; a restart means its processes are its new engine's.
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
         when you end with no live ancestor, are cancelled or fail; a daemon \
         restart leaves it running. Stop it sooner with `kill <pid>`."
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
        let rest = processes.get(HANDED_UP_LISTED..).unwrap_or_default();
        if !rest.is_empty() {
            let _ = write!(text, "\n- and {} more, by pid:", rest.len());
            for process in rest {
                let _ = write!(text, " {}", process.pid);
            }
        }
    }
    text
}

#[cfg(test)]
mod tests;
