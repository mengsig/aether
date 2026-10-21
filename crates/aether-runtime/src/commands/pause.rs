use super::PauseOutcome;
use crate::{Result, Runtime, RuntimeError};
use aether_core::agent::{Agent, AgentStatus, BlockReason};
use aether_core::event::{Actor, EventDraft, EventKind};
use aether_core::id::AgentId;
use aether_core::time::now;
use aether_store::{AgentFilter, Commit, Mutation};
use std::collections::BTreeSet;

impl Runtime {
    /// VISION §3.1/§3.8: pause is a durable flag, not a status. It applies
    /// whatever the agent is doing - gated, waiting for a key, mid-turn - and
    /// the scheduler stops dispatching it from the next tick. With `subtree`
    /// it reaches every live descendant too, in one commit: a supervisor that
    /// is held while its workers keep running is not paused in any useful
    /// sense. Only a subtree pause is inherited by children created under it
    /// (`paused_via` records its root); a plain pause holds the one agent.
    pub fn pause_agent(&self, root: AgentId, subtree: bool, by: Actor) -> Result<PauseOutcome> {
        self.pause_agent_keyed(root, subtree, by, None)
    }

    pub fn pause_agent_keyed(
        &self,
        root: AgentId,
        subtree: bool,
        by: Actor,
        submission: Option<aether_core::idempotency::Submission>,
    ) -> Result<PauseOutcome> {
        // Read and written under one hold: the rows are written back whole, so
        // a change committed in between would be reverted.
        let hold = self.hold();
        if let Some(receipt) = self.replay_receipt(&hold, submission.as_ref())? {
            return Ok(receipt);
        }
        let targets = self.live_targets(root, subtree, "pause target")?;
        let ids: Vec<AgentId> = targets.iter().map(|a| a.id).collect();
        let at = now();
        let mut commit = Commit::new();
        let mut changed = Vec::new();
        for mut agent in targets {
            // A subtree pause also stamps an agent already held on its own:
            // the stamp is what reaches the children it delegates next, and
            // what the subtree resume lifts. A plain pause never clears it.
            let held_via = if subtree {
                Some(root)
            } else {
                agent.paused_via
            };
            if agent.paused && agent.paused_via == held_via {
                continue;
            }
            let via = (agent.id != root).then_some(root);
            agent.paused = true;
            agent.paused_via = held_via;
            agent.updated_at = at;
            commit.push_event(
                EventDraft::new(by.clone(), EventKind::AgentPaused { via })
                    .agent(agent.id)
                    .run(agent.run),
            );
            commit.push_mutation(Mutation::UpsertAgent(agent.clone()));
            changed.push(agent);
        }
        let running = self.sched.lock();
        let mid_turn = ids
            .into_iter()
            .filter(|id| running.running.contains_key(id))
            .collect();
        drop(running);
        let outcome = PauseOutcome { changed, mid_turn };
        self.commit_receipt(hold, commit, submission.as_ref(), &outcome)?;
        Ok(outcome)
    }

    /// Lift a pause, clear a wait or an inspection the operator owns, or probe
    /// an exhausted provider now. With `subtree` it lifts the pause and the
    /// suspension on the root and every live descendant carrying one - and only
    /// those: clearing a wait or an inspection is a per-agent judgement a person
    /// makes with the agent in front of them (§3.9), never a silent sweep.
    pub fn resume_agent(&self, root: AgentId, subtree: bool, by: Actor) -> Result<Vec<Agent>> {
        self.resume_agent_keyed(root, subtree, by, None)
    }

    pub fn resume_agent_keyed(
        &self,
        root: AgentId,
        subtree: bool,
        by: Actor,
        submission: Option<aether_core::idempotency::Submission>,
    ) -> Result<Vec<Agent>> {
        if subtree {
            return self.resume_subtree(root, by, submission);
        }
        // Read and written under one hold, as `pause_agent` does.
        let hold = self.hold();
        if let Some(receipt) = self.replay_receipt(&hold, submission.as_ref())? {
            return Ok(receipt);
        }
        let mut agent = self.live_agent(root, "resume target")?;
        let at = now();
        let run = agent.run;
        let mut commit = Commit::new();
        let resumed = |reason: &str| {
            EventDraft::new(
                by.clone(),
                EventKind::AgentResumed {
                    reason: reason.to_owned(),
                    via: None,
                },
            )
            .agent(root)
            .run(run)
        };
        if agent.paused {
            // Unpause only: the agent keeps the status it had, so a wait it
            // chose and a gate that still holds both survive.
            agent.paused = false;
            agent.paused_via = None;
            agent.updated_at = at;
            commit.push_event(resumed("unpaused by operator"));
            commit.push_mutation(Mutation::UpsertAgent(agent.clone()));
            self.commit_receipt(hold, commit, submission.as_ref(), &vec![agent.clone()])?;
            return Ok(vec![agent]);
        }
        // A turn blocked on a resource lease resumes itself when the keys
        // come free; forcing it Ready would run a second turn beside it.
        if self.sched.lock().running.contains_key(&root) {
            return Err(RuntimeError::Invalid(
                "this agent has a turn in flight; it resumes on its own".into(),
            ));
        }
        let mut clearing_inspection = false;
        let mut probing = None;
        match &agent.status {
            AgentStatus::Waiting { .. } => {}
            // Only a person clears an inspection (§3.9).
            AgentStatus::Blocked {
                reason: BlockReason::Inspection { .. },
            } => clearing_inspection = true,
            // The person has decided to try the provider now: this turn is the
            // probe, and the bench that was holding everyone off goes with it.
            AgentStatus::Blocked {
                reason: BlockReason::Exhausted { model, .. },
            } => probing = Some(model.provider.clone()),
            AgentStatus::Blocked { reason } => {
                return Err(RuntimeError::Invalid(format!(
                    "agent {root} is {}; it runs again when that clears",
                    reason.describe()
                )));
            }
            other => {
                return Err(RuntimeError::Invalid(format!(
                    "agent {root} is {}, not paused, waiting or under inspection",
                    other.kind()
                )));
            }
        }
        // The person has looked: whatever the interrupted call was doing to
        // these keys is now their call, and the fleet may take them again.
        // Recorded first, released after the commit lands - a key the fleet
        // can take while the log still shows it held is a key nobody owns.
        if clearing_inspection {
            let keys = self.resources.quarantined_keys(root);
            if !keys.is_empty() {
                commit.push_event(
                    EventDraft::new(by.clone(), EventKind::ResourceReleased { keys })
                        .agent(root)
                        .run(run),
                );
            }
        }
        agent.transition(AgentStatus::Ready, at)?;
        if clearing_inspection {
            self.push_inbox(
                &hold,
                &mut commit,
                &agent,
                aether_core::inbox::InboxKind::Wake {
                    reason: "inspection cleared by operator; continue the interrupted work".into(),
                },
                by.clone(),
            )?;
        }
        commit.push_event(resumed("resumed by operator"));
        commit.push_mutation(Mutation::UpsertAgent(agent.clone()));
        self.commit_receipt(hold, commit, submission.as_ref(), &vec![agent.clone()])?;
        if clearing_inspection {
            self.resources.release_quarantine(root);
            self.clear_problem(&format!("recovery.{root}"));
        }
        if let Some(provider) = probing {
            crate::scheduler::clear_bench(self, &provider)?;
            crate::scheduler::clear_probe_claim(self, &provider);
        }
        Ok(vec![agent])
    }

    fn resume_subtree(
        &self,
        root: AgentId,
        by: Actor,
        submission: Option<aether_core::idempotency::Submission>,
    ) -> Result<Vec<Agent>> {
        let hold = self.hold();
        if let Some(receipt) = self.replay_receipt(&hold, submission.as_ref())? {
            return Ok(receipt);
        }
        let targets = self.resume_targets(root)?;
        let at = now();
        let mut commit = Commit::new();
        let mut changed = Vec::new();
        let mut probing = BTreeSet::new();
        for mut agent in targets {
            // Waiting for a provider's allowance is not a judgement a person
            // has to make per agent, unlike a wait or an inspection: the sweep
            // makes every suspended descendant probe now.
            let suspended = matches!(
                &agent.status,
                AgentStatus::Blocked {
                    reason: BlockReason::Exhausted { .. }
                }
            );
            if !agent.paused && !suspended {
                continue;
            }
            let via = (agent.id != root).then_some(root);
            // A paused agent is only unpaused, as before: it keeps the status
            // it had, suspension included, and probes once it may run again.
            let reason = if agent.paused {
                agent.paused = false;
                agent.paused_via = None;
                agent.updated_at = at;
                "unpaused by operator"
            } else {
                if let AgentStatus::Blocked {
                    reason: BlockReason::Exhausted { model, .. },
                } = &agent.status
                {
                    probing.insert(model.provider.clone());
                }
                agent.transition(AgentStatus::Ready, at)?;
                "resumed by operator"
            };
            commit.push_event(
                EventDraft::new(
                    by.clone(),
                    EventKind::AgentResumed {
                        reason: reason.into(),
                        via,
                    },
                )
                .agent(agent.id)
                .run(agent.run),
            );
            commit.push_mutation(Mutation::UpsertAgent(agent.clone()));
            changed.push(agent);
        }
        if changed.is_empty() {
            return Err(RuntimeError::Invalid(format!(
                "nothing is paused or suspended in agent {root}'s subtree"
            )));
        }
        self.commit_receipt(hold, commit, submission.as_ref(), &changed)?;
        for provider in probing {
            crate::scheduler::clear_bench(self, &provider)?;
            crate::scheduler::clear_probe_claim(self, &provider);
        }
        Ok(changed)
    }

    /// The root and its live descendants. The root may have ended since its
    /// subtree was paused: the pause it stamped below it is still its to lift.
    fn resume_targets(&self, root: AgentId) -> Result<Vec<Agent>> {
        let agents = self.store.agents(&AgentFilter::default())?;
        let order = aether_core::agent::subtree_of(&agents, root);
        if order.is_empty() {
            return Err(RuntimeError::NotFound(root.to_string()));
        }
        Ok(order
            .into_iter()
            .filter_map(|id| {
                agents
                    .iter()
                    .find(|a| a.id == id && (id == root || !a.status.is_terminal()))
                    .cloned()
            })
            .collect())
    }
}
