use super::PendingReap;
use crate::{CommitHold, Result, Runtime, RuntimeError};
use aether_core::agent::{Agent, AgentStatus};
use aether_core::event::{Actor, EventDraft, EventKind};
use aether_core::id::AgentId;
use aether_core::inbox::InboxKind;
use aether_core::task::{TaskResult, TaskStatus};
use aether_core::time::now;
use aether_store::{AgentFilter, Commit, Mutation};

/// The custom event naming what an ended agent's engines left running.
pub(crate) const PROCESSES_REAPED_EVENT: &str = "agent.processes_reaped";

impl Runtime {
    /// Stop the running turn but keep the agent: it records what it has so
    /// far and waits for the next message. False when no turn was running.
    pub fn interrupt_agent(&self, id: AgentId, by: Actor) -> Result<bool> {
        self.interrupt_agent_keyed(id, by, None)
    }

    pub fn interrupt_agent_keyed(
        &self,
        id: AgentId,
        by: Actor,
        submission: Option<aether_core::idempotency::Submission>,
    ) -> Result<bool> {
        let hold = self.hold();
        if let Some(receipt) = self.replay_receipt(&hold, submission.as_ref())? {
            return Ok(receipt);
        }
        let agent = self
            .store
            .agent(id)?
            .ok_or_else(|| RuntimeError::NotFound(id.to_string()))?;
        if agent.status.is_terminal() {
            return Err(RuntimeError::Invalid(format!(
                "agent {id} is {}",
                agent.status.kind()
            )));
        }
        let token = self
            .sched
            .lock()
            .running
            .get(&id)
            .map(|r| r.interrupt.clone());
        let was_running = token.is_some();
        self.commit_receipt(hold, Commit::new(), submission.as_ref(), &was_running)?;
        if let Some(token) = token {
            tracing::info!(agent = %id, by = %by.label(), "turn interrupted");
            token.cancel();
        }
        Ok(was_running)
    }

    /// Cancel an agent and everything beneath it.
    pub fn cancel_agent(&self, id: AgentId, reason: Option<String>, by: Actor) -> Result<()> {
        crate::cancellation::request(self, id, reason, by)?;
        crate::cancellation::process(self, id)
    }

    pub fn cancel_agent_keyed(
        &self,
        id: AgentId,
        reason: Option<String>,
        by: Actor,
        submission: Option<aether_core::idempotency::Submission>,
    ) -> Result<()> {
        let hold = self.hold();
        if let Some(receipt) = self.replay_receipt(&hold, submission.as_ref())? {
            return Ok(receipt);
        }
        let commit = crate::cancellation::prepare(self, &hold, id, reason, by)?;
        self.commit_receipt(hold, commit, submission.as_ref(), &())?;
        crate::cancellation::process(self, id)
    }

    pub(crate) fn cancel_agent_now(
        &self,
        id: AgentId,
        reason: Option<String>,
        by: Actor,
    ) -> Result<()> {
        let hold = self.hold();
        let agent = self
            .store
            .agent(id)?
            .ok_or_else(|| RuntimeError::NotFound(id.to_string()))?;
        if agent.status.is_terminal() {
            drop(hold);
            return self.cancel_live_children(id, by);
        }
        // A running turn is cancelled cooperatively; the turn records the outcome.
        let running = self.sched.lock().running.get_mut(&id).map(|running| {
            running
                .cancel_request
                .get_or_insert_with(|| crate::CancelRequest {
                    by: by.clone(),
                    reason: reason.clone(),
                });
            running.cancel.clone()
        });
        if let Some(token) = running {
            drop(hold);
            token.cancel();
        } else {
            let mut commit = Commit::new();
            let reap = self.terminate_agent(
                &hold,
                &mut commit,
                agent,
                AgentStatus::Cancelled {
                    reason: reason.clone(),
                },
                by.clone(),
            )?;
            self.apply_held(hold, commit)?;
            self.reap_ended_agent(reap);
        }
        self.cancel_live_children(id, by)
    }

    /// Cancel every live descendant of `id`, at any depth. Cooperative
    /// turns may settle after the first walk, so turn completion repeats it.
    /// The durable request remains pending until the entire subtree is terminal.
    /// This also works after the root itself has already ended.
    ///
    /// Fetches ALL children, not just live ones: a child that already ended
    /// (completed, failed) is not itself cancelled, but the walk passes
    /// through it transparently to reach a live grandchild beneath it -
    /// cancelling a subtree ends every live agent in it, regardless of a
    /// finished intermediate. Terminates
    /// because parentage is a tree: each recursive call strictly descends.
    pub(crate) fn cancel_live_children(&self, id: AgentId, by: Actor) -> Result<()> {
        for child in self.store.agents(&AgentFilter {
            parent: Some(id),
            ..Default::default()
        })? {
            if child.status.is_terminal() {
                self.cancel_live_children(child.id, by.clone())?;
            } else {
                self.cancel_agent_now(
                    child.id,
                    Some(format!("parent {id} cancelled")),
                    by.clone(),
                )?;
            }
        }
        Ok(())
    }

    /// Let go of what an ended agent was holding, once its terminal
    /// transition is committed: its quarantine hold, whose release the commit
    /// just recorded, the background shell processes it started and whatever
    /// its engines left running. Every terminal transition goes through
    /// `terminate_agent`, so nothing outlives its agent whether it completed,
    /// failed or was cancelled.
    pub(crate) fn reap_ended_agent(&self, pending: PendingReap) {
        let agent = pending.0;
        self.resources.release_quarantine(agent);
        let Ok(handle) = tokio::runtime::Handle::try_current() else {
            tracing::warn!(%agent, "no runtime to stop the agent's processes on");
            return;
        };
        self.reap_engine_processes(&handle, agent);
        let Some(processes) = self.tools.background().cloned() else {
            return;
        };
        handle.spawn(async move {
            let stopped = processes.stop_agent(agent).await;
            if stopped > 0 {
                tracing::info!(%agent, stopped, "stopped background processes with the agent");
            }
        });
    }

    /// Stop what the ended agent's engines left running - a server, a
    /// detached job - and record it as `agent.processes_reaped`.
    fn reap_engine_processes(&self, handle: &tokio::runtime::Handle, agent: AgentId) {
        let grace = self.models.config().agents.reap_grace();
        let me = self.me.clone();
        handle.spawn(async move {
            let ended = || me.upgrade().is_some_and(|runtime| runtime.has_ended(agent));
            let report = aether_engine::reap::reap_agent(agent, grace, ended).await;
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
    ) -> Result<()> {
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

    /// Move an agent to a terminal state, settle its task, notify its parent,
    /// and release its lease. Appends to `commit`; the caller applies it and
    /// then reaps the agent with the returned handle.
    pub(crate) fn terminate_agent(
        &self,
        hold: &CommitHold<'_>,
        commit: &mut Commit,
        mut agent: Agent,
        status: AgentStatus,
        by: Actor,
    ) -> Result<PendingReap> {
        // Callers can carry a stale in-flight row. The hold makes this check
        // and every resulting delivery one terminal-transition dedup fence.
        if self
            .store
            .agent(agent.id)?
            .is_some_and(|row| row.status.is_terminal())
        {
            return Ok(PendingReap(agent.id));
        }
        let mut failure_notified = false;
        let status = match status {
            AgentStatus::Failed { error } => {
                let error = if let Some(diagnostic) =
                    crate::exhaustion::prepare(self, hold, commit, &agent, &error, &by)?
                {
                    failure_notified = true;
                    diagnostic
                } else {
                    failure_notified = crate::exhaustion::prepare_native_failure(
                        self, hold, commit, &agent, &error, &by,
                    )?;
                    error
                };
                AgentStatus::Failed { error }
            }
            other => other,
        };
        let at = now();
        let kind = match &status {
            AgentStatus::Completed { summary } => EventKind::AgentCompleted {
                summary: summary.clone(),
            },
            AgentStatus::Failed { error } => EventKind::AgentFailed {
                error: error.clone(),
            },
            AgentStatus::Cancelled { reason } => EventKind::AgentCancelled {
                reason: reason.clone(),
            },
            // Terminal, but not an ending: an agent becomes `imported` by
            // arriving in an export, never by finishing here. Naming it keeps
            // the `other` arm's "is not terminal" true.
            AgentStatus::Imported { .. } => {
                return Err(RuntimeError::Invalid(format!(
                    "agent {} cannot be ended as `imported`; that status is what an export \
                     arrives as",
                    agent.id
                )));
            }
            other => {
                return Err(RuntimeError::Invalid(format!(
                    "{} is not terminal",
                    other.kind()
                )));
            }
        };
        agent.transition(status.clone(), at)?;
        commit.push_event(
            EventDraft::new(by.clone(), kind)
                .agent(agent.id)
                .parent(agent.parent)
                .run(agent.run),
        );
        // An ended agent runs nothing more, so nothing it asked about may
        // stay answerable.
        self.expire_agent_approvals(hold, commit, &agent, &format!("agent {}", status.kind()))?;
        commit.push_mutation(Mutation::ClearLease(agent.id));
        commit.push_event(
            EventDraft::new(by.clone(), EventKind::LeaseReleased)
                .agent(agent.id)
                .run(agent.run),
        );
        // An agent that has ended fences nothing off any more (§15). The
        // hold comes off after the commit that records the release: the log
        // must never be behind what the fleet can already take.
        let quarantined = self.resources.quarantined_keys(agent.id);
        if !quarantined.is_empty() {
            commit.push_event(
                EventDraft::new(
                    by.clone(),
                    EventKind::ResourceReleased { keys: quarantined },
                )
                .agent(agent.id)
                .run(agent.run),
            );
        }

        if let Some(task_id) = agent.current_task
            && let Some(mut task) = self.store.task(task_id)?
            && !task.status.is_terminal()
        {
            let (task_status, task_kind, inbox_kind) = match &status {
                AgentStatus::Failed { error } => (
                    TaskStatus::Failed {
                        error: error.clone(),
                        structured: None,
                    },
                    EventKind::TaskFailed {
                        error: error.clone(),
                        structured: None,
                    },
                    InboxKind::TaskFailed {
                        task: task_id,
                        error: error.clone(),
                        structured: None,
                    },
                ),
                AgentStatus::Cancelled { reason } => (
                    TaskStatus::Cancelled {
                        reason: reason.clone(),
                    },
                    EventKind::TaskCancelled {
                        reason: reason.clone(),
                    },
                    InboxKind::TaskCancelled {
                        task: task_id,
                        reason: reason.clone(),
                    },
                ),
                AgentStatus::Completed { summary } => {
                    let result =
                        TaskResult::summary(summary.clone().unwrap_or_else(|| "completed".into()));
                    (
                        TaskStatus::Completed {
                            result: result.clone(),
                        },
                        EventKind::TaskCompleted {
                            result: result.clone(),
                        },
                        InboxKind::TaskCompleted {
                            task: task_id,
                            result,
                        },
                    )
                }
                _ => unreachable!("checked above"),
            };
            task.transition(task_status, at)?;
            commit.push_event(
                EventDraft::new(by.clone(), task_kind)
                    .agent(agent.id)
                    .task(task_id)
                    .run(agent.run),
            );
            commit.push_mutation(Mutation::UpsertTask(task.clone()));
            self.notify_task_creator(hold, commit, &task, inbox_kind, by.clone())?;
        }
        if let (AgentStatus::Failed { error }, Some(parent_id)) = (&status, agent.parent)
            && !failure_notified
            && let Some(parent) = self.store.agent(parent_id)?
            && !parent.status.is_terminal()
        {
            self.push_inbox(
                hold,
                commit,
                &parent,
                InboxKind::ChildFailed {
                    agent: agent.id,
                    error: error.clone(),
                },
                by,
            )?;
        }
        let id = agent.id;
        commit.push_mutation(Mutation::UpsertAgent(agent));
        Ok(PendingReap(id))
    }
}

#[cfg(test)]
mod tests;
