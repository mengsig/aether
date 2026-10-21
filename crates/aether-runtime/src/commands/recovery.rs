use super::RESTART_TRANSCRIPT_TAIL;
use crate::{CommitHold, Result, Runtime, RuntimeError};
use aether_core::agent::{Agent, AgentConfig, AgentStats, AgentStatus, AgentStatusKind};
use aether_core::checkpoint::{Checkpoint, CheckpointProvenance};
use aether_core::event::{Actor, CompactionTrigger, EventDraft, EventKind};
use aether_core::id::{AgentId, CheckpointId};
use aether_core::inbox::InboxKind;
use aether_core::task::{Task, TaskStatus};
use aether_core::time::now;
use aether_store::{AgentFilter, Commit, Mutation};

pub(crate) const COMPACTION_WAKE: &str = "compaction requested";

impl Runtime {
    /// Fold older context promptly; a manual request alone needs no reply.
    /// Model-requested checkpoints still continue the agent's work.
    pub fn request_compaction(&self, id: AgentId, by: Actor) -> Result<()> {
        let hold = self.hold();
        let mut agent = self
            .store
            .agent(id)?
            .ok_or_else(|| RuntimeError::NotFound(id.to_string()))?;
        if agent.status.is_terminal() {
            return Err(RuntimeError::Invalid(format!(
                "agent {id} is {}",
                agent.status.kind()
            )));
        }
        // A handoff already owes a full checkpoint. A later explicit request
        // must not replace it with an ordinary, tail-retaining compaction.
        let handoff_pending = matches!(
            crate::turn::compaction_pending(self, id)?,
            Some(CompactionTrigger::Handoff { .. })
        );
        if handoff_pending && agent.status.kind() != AgentStatusKind::Waiting {
            return Ok(());
        }
        let mut commit = Commit::new();
        if !handoff_pending {
            commit.push_event(
                EventDraft::new(
                    by.clone(),
                    EventKind::CompactionRequested {
                        trigger: CompactionTrigger::Explicit { by: by.clone() },
                    },
                )
                .agent(id)
                .run(agent.run),
            );
        }
        // Running turns compact at their next context assembly or when they end;
        // a wake would steer the native model.
        self.ready_for_compaction(&hold, &mut commit, &mut agent, by)?;
        self.apply_held(hold, commit)?;
        Ok(())
    }

    /// Ready a waiting agent for its owed fold, keeping the wait it returns to.
    pub(crate) fn ready_for_compaction(
        &self,
        hold: &CommitHold<'_>,
        commit: &mut Commit,
        agent: &mut Agent,
        by: Actor,
    ) -> Result<()> {
        debug_assert!(!agent.status.is_terminal());
        if let AgentStatus::Waiting { condition } = &agent.status {
            agent.resume_wait = Some(condition.clone());
            agent.transition(AgentStatus::Ready, now())?;
            commit.push_mutation(Mutation::UpsertAgent(agent.clone()));
        }
        if agent.status.kind() != AgentStatusKind::Ready {
            return Ok(());
        }
        let wake = self.push_inbox(
            hold,
            commit,
            agent,
            InboxKind::Wake {
                reason: COMPACTION_WAKE.into(),
            },
            by,
        )?;
        debug_assert_eq!(wake.agent_id, agent.id);
        Ok(())
    }

    /// VISION §3.8: recreate a Failed or Cancelled agent from its last
    /// checkpoint and the tail of its transcript, as a new agent linked to
    /// the old one in both directions. The old agent is left as it ended;
    /// its spend stays on its own record, so nothing is counted twice.
    pub fn restart_agent(&self, id: AgentId, by: Actor) -> Result<Agent> {
        self.restart_agent_keyed(id, by, None)
    }

    pub fn restart_agent_keyed(
        &self,
        id: AgentId,
        by: Actor,
        submission: Option<aether_core::idempotency::Submission>,
    ) -> Result<Agent> {
        // The successor link is read here and written by the commit below, so
        // the read happens under the hold: two restarts racing on one agent
        // would otherwise both see no successor and both create one.
        let hold = self.hold();
        if let Some(receipt) = self.replay_receipt(&hold, submission.as_ref())? {
            return Ok(receipt);
        }
        let mut old = self
            .store
            .agent(id)?
            .ok_or_else(|| RuntimeError::NotFound(id.to_string()))?;
        match old.status.kind() {
            AgentStatusKind::Failed | AgentStatusKind::Cancelled => {}
            other => {
                return Err(RuntimeError::Invalid(format!(
                    "agent {id} is {other}; only a failed or cancelled agent is restarted"
                )));
            }
        }
        if let Some(successor) = old.restarted_as {
            return Err(RuntimeError::Invalid(format!(
                "agent {id} was already restarted as {successor}"
            )));
        }
        self.models.resolve(&old.model)?;
        // §16: the successor is a new agent under the same parent, and the
        // parent may have been confined while this one sat Failed - a state
        // `widest_live_descendant` cannot see. Restarting with the old scope
        // would hand back what the tightening took away.
        if let Some(parent_id) = old.parent {
            let parent = self
                .store
                .agent(parent_id)?
                .ok_or_else(|| RuntimeError::NotFound(parent_id.to_string()))?;
            if parent.status.is_terminal() {
                return Err(RuntimeError::Invalid(format!(
                    "agent {id}'s parent {parent_id} is {}; nothing is left to contain the \
                     restart",
                    parent.status.kind()
                )));
            }
            if let Some(field) = self.scope_widened_for_existing(&old.scope, &old, &parent)? {
                return Err(RuntimeError::Invalid(format!(
                    "agent {id}'s scope is wider than parent {parent_id}'s on `{field}`; the \
                     parent was tightened after this agent ended, so restarting it would widen \
                     the subtree again"
                )));
            }
        }

        let at = now();
        // Everything the new agent inherits was committed before this point,
        // so this is where its own transcript begins.
        let boundary = self.store.last_seq()?;
        let source = self.store.latest_checkpoint(id)?;
        let carried = self.store.transcript(
            id,
            source.as_ref().map(|c| c.provenance.to_seq).unwrap_or(0),
        )?;
        let tail = &carried[carried.len().saturating_sub(RESTART_TRANSCRIPT_TAIL)..];

        let new_id = AgentId::new();
        let checkpoint = source.map(|c| Checkpoint {
            id: CheckpointId::new(),
            agent_id: new_id,
            version: c.version,
            draft: c.draft,
            provenance: CheckpointProvenance {
                from_seq: c.provenance.from_seq,
                // The copy stands for everything up to the restart: the new
                // agent has no history of its own before that.
                to_seq: boundary,
                source_snapshot: c.provenance.source_snapshot,
                compactor_model: c.provenance.compactor_model,
                previous: Some(c.id),
                trigger: c.provenance.trigger,
            },
            created_at: at,
        });
        let mut agent = Agent {
            id: new_id,
            parent: old.parent,
            role: old.role.clone(),
            name: old.name.clone(),
            scope: old.scope.clone(),
            model: old.model.clone(),
            fallbacks: old.fallbacks.clone(),
            status: AgentStatus::Created,
            // A restart is the person asking for work to resume; like any new
            // agent it starts unpaused.
            paused: false,
            paused_via: None,
            current_task: None,
            archived: false,
            inbox_cursor: 0,
            checkpoint: checkpoint.as_ref().map(|c| c.id),
            // An engine session belongs to the run that failed; the new agent
            // starts a fresh one from the checkpoint.
            engine_session: None,
            engine_usage: None,
            engine_sent: None,
            status_since: Some(at),
            resume_wait: None,
            restarted_from: Some(old.id),
            restarted_as: None,
            config: AgentConfig {
                // The usual reason to restart is the hard stop that fired.
                // Carrying a deadline that has already passed would fail the
                // successor on the very next tick.
                deadline: old.config.deadline.filter(|d| at < *d),
                ..old.config.clone()
            },
            stats: AgentStats::default(),
            run: old.run,
            created_at: at,
            updated_at: at,
        };

        let mut commit = Commit::new();
        let (parent_id, run) = (agent.parent, agent.run);
        let base = |kind: EventKind| {
            EventDraft::new(by.clone(), kind)
                .agent(new_id)
                .parent(parent_id)
                .run(run)
        };
        commit.push_event(base(EventKind::AgentCreated {
            role: agent.role.clone(),
            name: agent.name.clone(),
            parent: agent.parent,
            model: agent.model.clone(),
            fallbacks: agent.fallbacks.clone(),
            scope: agent.scope.clone(),
        }));
        // §17 line 7: a person's message is never lost. Anything the old
        // agent was given and never read reaches its successor.
        let carried_inbox = self.store.inbox(old.id, old.inbox_cursor)?;
        let restarted = EventKind::AgentRestarted {
            from: old.id,
            to: new_id,
            checkpoint: agent.checkpoint,
            transcript_tail: tail.len(),
            transcript_dropped: carried.len() - tail.len(),
            inbox_carried: carried_inbox.len(),
            dropped_deadline: old
                .config
                .deadline
                .filter(|d| agent.config.deadline.as_ref() != Some(d)),
        };
        commit.push_event(base(restarted.clone()));
        // The same fact on the old agent, so its stream shows where it went.
        commit.push_event(
            EventDraft::new(by.clone(), restarted)
                .agent(old.id)
                .parent(old.parent)
                .run(old.run),
        );
        for entry in tail {
            commit.push_event(base(EventKind::AgentMessage {
                role: entry.message.role,
                content: entry.message.content.clone(),
                // The invocation belongs to the old agent; this agent never
                // made that call.
                invocation: None,
            }));
        }
        if let Some(checkpoint) = checkpoint {
            commit.push_mutation(Mutation::PutCheckpoint(checkpoint));
        }

        // A restart is only useful if the work is picked up again: the old
        // task ended with the old agent, so the new one gets its own.
        if let Some(old_task) = old
            .current_task
            .and_then(|t| self.store.task(t).transpose())
        {
            let old_task = old_task?;
            let mut task = Task::new(old_task.objective.clone(), by.clone(), run, at);
            task.owner = Some(new_id);
            task.parent_task = old_task.parent_task;
            task.priority = old_task.priority;
            task.dependencies = old_task.dependencies.clone();
            task.transition(TaskStatus::Assigned, at)?;
            agent.current_task = Some(task.id);
            commit.push_event(
                base(EventKind::TaskCreated {
                    objective: task.objective.clone(),
                    parent_task: task.parent_task,
                    dependencies: task.dependencies.clone(),
                    priority: task.priority,
                })
                .task(task.id),
            );
            commit.push_event(base(EventKind::TaskAssigned { owner: new_id }).task(task.id));
            commit.push_mutation(Mutation::UpsertTask(task));
        }

        for item in carried_inbox {
            self.push_inbox(&hold, &mut commit, &agent, item.kind, by.clone())?;
        }
        agent.transition(AgentStatus::Ready, at)?;
        commit.push_event(base(EventKind::AgentReady));
        commit.push_mutation(Mutation::UpsertAgent(agent.clone()));
        old.restarted_as = Some(new_id);
        old.updated_at = at;
        commit.push_mutation(Mutation::UpsertAgent(old));
        self.commit_receipt(hold, commit, submission.as_ref(), &agent)?;
        Ok(agent)
    }

    /// Bring a Failed agent back in place: same id, transcript, checkpoint
    /// and engine session, so the same context. The failure stays in its
    /// history (`agent.failed`, and the task that ended with it), and only a
    /// person makes this call: nothing wakes a failed agent on its own.
    ///
    /// Unlike `restart_agent` no successor is created: the point is to talk
    /// to the agent that failed, not to a copy of its last few messages.
    pub fn wake_agent(&self, id: AgentId, by: Actor) -> Result<Agent> {
        // Under the hold: a turn winding down, a restart or a second wake
        // racing on this row must either see this write or precede it.
        let hold = self.hold();
        let mut agent = self
            .store
            .agent(id)?
            .ok_or_else(|| RuntimeError::NotFound(id.to_string()))?;
        let AgentStatus::Failed { error } = agent.status.clone() else {
            return Err(RuntimeError::Invalid(format!(
                "agent {id} is {}; only a failed agent is woken",
                agent.status.kind()
            )));
        };
        self.refuse_unwakeable(&hold, &agent)?;

        let at = now();
        let (parent, run) = (agent.parent, agent.run);
        let base = |kind: EventKind| {
            EventDraft::new(by.clone(), kind)
                .agent(id)
                .parent(parent)
                .run(run)
        };
        let mut commit = Commit::new();
        // The task ended with the failure and stays ended: it is the record
        // of what went wrong. Clearing the slot is what lets the agent take
        // a new one.
        if let Some(task_id) = agent.current_task
            && self
                .store
                .task(task_id)?
                .is_some_and(|t| t.status.is_terminal())
        {
            agent.current_task = None;
        }
        // A live agent is never archived (`archive_agents` refuses one).
        if agent.archived {
            agent.archived = false;
            commit.push_event(base(EventKind::AgentArchived { archived: false }));
        }
        // Like a restart: the person is asking for work to resume.
        agent.paused = false;
        agent.paused_via = None;
        agent.resume_wait = None;
        agent.transition(AgentStatus::Ready, at)?;
        commit.push_event(base(EventKind::AgentResumed {
            reason: format!("woken by {} after failing: {error}", by.label()),
            via: None,
        }));
        // Ready alone runs nothing when the last word was the agent's own
        // (`has_work`); the wake item is what gives it a turn, and says why.
        self.push_inbox(
            &hold,
            &mut commit,
            &agent,
            InboxKind::Wake {
                reason: format!(
                    "You failed with: {error}. The operator woke you. Any task you had ended \
                     with that failure; continue from your history and any newer messages."
                ),
            },
            by.clone(),
        )?;
        commit.push_mutation(Mutation::UpsertAgent(agent.clone()));
        self.apply_held(hold, commit)?;
        Ok(agent)
    }

    /// Why a Failed agent cannot be woken right now, if it cannot. Each
    /// refusal names what to do instead; none of them is a wake that would
    /// only be undone by the next scheduler tick.
    fn refuse_unwakeable(&self, hold: &CommitHold<'_>, agent: &Agent) -> Result<()> {
        let id = agent.id;
        if self.sched.lock().running.contains_key(&id) {
            return Err(RuntimeError::Invalid(format!(
                "agent {id}'s last turn is still winding down; wake it again in a moment"
            )));
        }
        if let Some(successor) = agent.restarted_as {
            return Err(RuntimeError::Invalid(format!(
                "agent {id} was restarted as {successor}; that agent carries on for it"
            )));
        }
        // Its own pending cancellation, or an ancestor's, would cancel it
        // again as soon as the subtree walk repeats.
        crate::cancellation::check_parent(self, hold, id)?;
        self.models.resolve(&agent.model)?;
        // §16: the parent may have been confined while this agent sat Failed,
        // a state `widest_live_descendant` cannot see (as for a restart).
        if let Some(parent_id) = agent.parent {
            let parent = self
                .store
                .agent(parent_id)?
                .ok_or_else(|| RuntimeError::NotFound(parent_id.to_string()))?;
            if parent.status.is_terminal() {
                return Err(RuntimeError::Invalid(format!(
                    "agent {id}'s parent {parent_id} is {}; wake the parent first",
                    parent.status.kind()
                )));
            }
            if let Some(field) = self.scope_widened_for_existing(&agent.scope, agent, &parent)? {
                return Err(RuntimeError::Invalid(format!(
                    "agent {id}'s scope is wider than parent {parent_id}'s on `{field}`; the \
                     parent was tightened after this agent failed"
                )));
            }
        }
        // A limit that already stopped it would stop it again on the next
        // tick: the person raises it first, then wakes. Same three checks
        // as `start_turn`, since that is where the next turn would die.
        if let Some(reason) = agent.deadline_passed(now()) {
            return Err(RuntimeError::Invalid(format!(
                "agent {id} is past its {reason}; extend it before waking"
            )));
        }
        if let Some(max) = agent.config.max_turns
            && agent.stats.turns >= max
        {
            return Err(RuntimeError::Invalid(format!(
                "agent {id} is at its turn limit of {max}; raise it before waking"
            )));
        }
        if agent.config.token_budget.is_some() || agent.config.cost_budget.is_some() {
            let all = self.store.agents(&AgentFilter::default())?;
            let usage = aether_core::budget::subtree_usage(&all)
                .get(&id)
                .copied()
                .unwrap_or_default();
            if let Some(reason) = agent.budget_exceeded(&usage) {
                return Err(RuntimeError::Invalid(format!(
                    "agent {id} is over its {reason}; raise it before waking"
                )));
            }
        }
        Ok(())
    }
}
