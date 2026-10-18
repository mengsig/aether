use super::AgentUpdate;
use crate::{Result, Runtime, RuntimeError};
use aether_core::agent::{Agent, AgentStatusKind};
use aether_core::event::{Actor, EventDraft, EventKind};
use aether_core::id::AgentId;
use aether_core::time::now;
use aether_store::{Commit, Mutation};

impl Runtime {
    /// Change a live agent's knobs. Read, decide and write happen under one
    /// hold: a turn in flight writes the agent row back after every model
    /// response, so a model change read outside it is silently reverted - and
    /// the handoff it committed then names a model the agent is not running.
    pub fn update_agent(&self, id: AgentId, update: AgentUpdate, by: Actor) -> Result<Agent> {
        self.update_agent_keyed(id, update, by, None)
    }

    pub fn update_agent_keyed(
        &self,
        id: AgentId,
        update: AgentUpdate,
        by: Actor,
        submission: Option<aether_core::idempotency::Submission>,
    ) -> Result<Agent> {
        let hold = self.hold();
        if let Some(receipt) = self.replay_receipt(&hold, submission.as_ref())? {
            return Ok(receipt);
        }
        let mut agent = self
            .store
            .agent(id)?
            .ok_or_else(|| RuntimeError::NotFound(id.to_string()))?;
        // A Failed agent may be patched: raising the limit that stopped it
        // is how a person prepares `wake_agent`. Not one a restart already
        // carried on from: that row never runs again, its successor does.
        if agent.status.is_terminal() && agent.status.kind() != AgentStatusKind::Failed {
            return Err(RuntimeError::Invalid(format!(
                "agent {id} is {}; only live or failed agents can be updated or renamed",
                agent.status.kind()
            )));
        }
        if let Some(successor) = agent.restarted_as {
            return Err(RuntimeError::Invalid(format!(
                "agent {id} was restarted as {successor}; update that agent instead"
            )));
        }
        if let Some(name) = &update.name {
            let trimmed = name.trim();
            if trimmed.is_empty()
                || trimmed.chars().count() > 256
                || name
                    .chars()
                    .any(|c| c.is_control() || matches!(c, '\u{2028}' | '\u{2029}'))
            {
                return Err(RuntimeError::Invalid(
                    "name must contain 1 to 256 characters without line breaks or control characters".into(),
                ));
            }
            agent.name = trimmed.to_owned();
        }
        // §6.3: moving between engines, or across the engine/API boundary,
        // means the next turn starts from a checkpoint. Codex's native
        // default also needs a fresh session when leaving an explicit model:
        // omitting a model on a retained thread keeps its last override.
        // Other moves within one engine keep the session and its history.
        let mut handoff = None;
        if let Some(model) = &update.model {
            self.models.resolve(model)?;
            let pending_handoff = match crate::turn::compaction_pending(self, id)? {
                Some(aether_core::event::CompactionTrigger::Handoff { from, to, session })
                    if to == agent.model =>
                {
                    Some((from, session))
                }
                _ => None,
            };
            let native_default_reset = agent.model.provider == "codex"
                && agent.model.model != "default"
                && model.provider == "codex"
                && model.model == "default";
            if let Some((from, session)) = pending_handoff {
                // Several selections before the next turn still hand off
                // from the model that actually held the old context.
                handoff = Some(aether_core::event::CompactionTrigger::Handoff {
                    from,
                    to: model.clone(),
                    session,
                });
            } else if self.engine_of(&agent.model) != self.engine_of(model) || native_default_reset
            {
                handoff = Some(aether_core::event::CompactionTrigger::Handoff {
                    from: agent.model.clone(),
                    to: model.clone(),
                    // Carried on the request because the fold happens turns
                    // later, off a row this clear has already emptied.
                    session: agent.engine_session.take(),
                });
            }
            agent.model = model.clone();
        }
        if let Some(links) = &update.fallbacks {
            for link in links {
                self.models.resolve(link)?;
            }
            agent.fallbacks.clone_from(links);
        }
        // Either half of the pair can move in one patch, so check what results.
        if update.model.is_some() || update.fallbacks.is_some() {
            self.check_billing_class(&agent.model, &agent.fallbacks)?;
        }
        if let Some(approval) = &update.approval {
            self.check_approval_against_parent(&agent, approval)?;
            // Tightening this agent alone must not leave an existing child
            // less gated than it (finding: `SetApprovals` already enforces
            // this for a subtree move; a lone `UpdateAgent` did not).
            if let Some(descendant) = self.least_gated_live_descendant(id, approval)? {
                return Err(RuntimeError::Invalid(format!(
                    "agent {descendant} is a live descendant gated less than that policy; \
                     tighten it first, or move the whole subtree with SetApprovals (/yolo)"
                )));
            }
        }
        if let Some(approval) = &update.approval
            && !approval.require_for.is_empty()
        {
            if self.config.yolo {
                return Err(RuntimeError::Invalid(
                    "the daemon runs in yolo mode; approvals are off for every agent".into(),
                ));
            }
            if !self.engine_can_gate(&agent.model) {
                return Err(RuntimeError::Invalid(
                    "this codex build runs unsandboxed and cannot ask for approval in exec mode; \
                     upgrade codex (app-server), or use claude-code or an API model for gated work"
                        .into(),
                ));
            }
        }
        // Scope knobs go through the same narrowing rule as a child's scope:
        // a live agent may be tightened freely, never widened past its parent.
        let scope_changed = update.network.is_some() || update.backend.is_some();
        if scope_changed {
            let mut scope = agent.scope.clone();
            if let Some(network) = update.network {
                scope.network = network;
            }
            if let Some(backend) = update.backend.clone() {
                scope.backend = backend;
            }
            if let Some(parent) = agent.parent.and_then(|p| self.store.agent(p).transpose()) {
                let parent = parent?;
                if self
                    .scope_widened_for_existing(&scope, &agent, &parent)?
                    .is_some()
                {
                    return Err(RuntimeError::Invalid(
                        "scope must stay within the parent's: network and backend can be \
                         tightened, never widened"
                            .into(),
                    ));
                }
            }
            // The containment this knob exists for runs down the subtree: an
            // agent confined to a container cannot have descendants still on
            // the host. Refuse rather than silently confine only the one row.
            if let Some(wider) = self.widest_live_descendant(id, &scope)? {
                return Err(RuntimeError::Invalid(format!(
                    "agent {wider} is a live descendant outside that scope; stop it or tighten \
                     it first"
                )));
            }
            agent.scope = scope;
        }
        // Any of the three can move in one patch, so check what results.
        if scope_changed || update.model.is_some() || update.fallbacks.is_some() {
            self.check_backend_supported(&agent.model, &agent.fallbacks, &agent.scope)?;
        }
        let c = &mut agent.config;
        if update.effort.is_some() {
            c.effort = update.effort;
        }
        if let Some(i) = update.instructions {
            c.instructions = Some(i);
        }
        // `Some(0)` clears, as it does for `concurrency`: without that there
        // is no way to remove a limit, and a stored 0 fails the agent on its
        // next tick.
        if let Some(n) = update.max_turns {
            c.max_turns = (n > 0).then_some(n);
        }
        if let Some(b) = update.token_budget {
            c.token_budget = (b > 0).then_some(b);
        }
        if let Some(b) = update.cost_budget {
            if !b.is_finite() || b < 0.0 {
                return Err(RuntimeError::Invalid(
                    "a cost budget must be a non-negative, finite amount".into(),
                ));
            }
            c.cost_budget = (b > 0.0).then_some(b);
        }
        if let Some(t) = update.tools {
            c.tools = t;
        }
        if let Some(a) = update.approval {
            c.approval = a;
        }
        if let Some(p) = update.priority {
            c.priority = p;
        }
        if let Some(n) = update.concurrency {
            c.concurrency = (n > 0).then_some(n);
        }
        if let Some(deadline) = update.deadline {
            c.deadline = deadline.applied();
        }
        if let Some(l) = update.long_lived {
            c.long_lived = l;
        }
        // Budget and chain can move in one patch, so check what results.
        self.check_cost_budget(&agent.model, &agent.fallbacks, agent.config.cost_budget)?;
        if matches!(
            by,
            Actor::User {
                origin: Some(aether_core::event::OperatorOrigin::Device { .. }),
                ..
            }
        ) && !agent.config.approval.is_yolo()
            && std::iter::once(&agent.model)
                .chain(&agent.fallbacks)
                .any(|model| !self.engine_can_gate(model))
        {
            return Err(RuntimeError::Invalid(
                "remote update cannot select an engine that drops required approvals".into(),
            ));
        }
        agent.updated_at = now();
        let mut commit = Commit::new()
            .event(
                EventDraft::new(
                    by.clone(),
                    EventKind::AgentConfigUpdated {
                        name: update.name.map(|_| agent.name.clone()),
                        model: update.model,
                        config: agent.config.clone(),
                        scope: scope_changed.then(|| agent.scope.clone()),
                    },
                )
                .agent(id)
                .run(agent.run),
            )
            .mutate(Mutation::UpsertAgent(agent.clone()));
        if let Some(trigger) = handoff {
            // The destination turn handles this request when new work arrives.
            // A model selection alone must not wake an idle conversation.
            commit.push_event(
                EventDraft::new(by.clone(), EventKind::CompactionRequested { trigger })
                    .agent(id)
                    .run(agent.run),
            );
        }
        self.commit_receipt(hold, commit, submission.as_ref(), &agent)?;
        Ok(agent)
    }
}
