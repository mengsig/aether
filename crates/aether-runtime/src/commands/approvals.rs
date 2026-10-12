use crate::approval::Remember;
use crate::{Result, Runtime, RuntimeError};
use aether_core::agent::{Agent, ApprovalPolicy};
use aether_core::approval::RuleScope;
use aether_core::event::{Actor, EventDraft, EventKind};
use aether_core::id::{AgentId, ApprovalId};
use aether_core::inbox::InboxKind;
use aether_core::model::ModelRef;
use aether_core::time::now;
use aether_core::tool::{ApprovalDecision, ApprovalStatus};
use aether_store::{Commit, Mutation};

impl Runtime {
    /// Turn approvals off or back on for an agent and, with `subtree`, every
    /// live agent beneath it. The subtree form is how `/yolo` reaches the
    /// workers a supervisor already spawned.
    pub fn set_approvals(
        &self,
        root: AgentId,
        policy: ApprovalPolicy,
        subtree: bool,
        by: Actor,
    ) -> Result<Vec<Agent>> {
        self.set_approvals_keyed(root, policy, subtree, by, None)
    }

    pub fn set_approvals_keyed(
        &self,
        root: AgentId,
        policy: ApprovalPolicy,
        subtree: bool,
        by: Actor,
        submission: Option<aether_core::idempotency::Submission>,
    ) -> Result<Vec<Agent>> {
        if !policy.is_yolo() && self.config.yolo {
            return Err(RuntimeError::Invalid(
                "the daemon runs in yolo mode; approvals are off for every agent".into(),
            ));
        }
        // Held from here through the commit: set_approvals reads a whole
        // Agent, mutates one field, and writes the whole row back
        // (UpsertAgent is a blind overwrite). Anything a concurrent turn
        // commits to these agents in between - status, usage, inbox_cursor,
        // engine_session - would otherwise be reverted;
        // the hold rules that window out entirely.
        let hold = self.hold();
        if let Some(receipt) = self.replay_receipt(&hold, submission.as_ref())? {
            return Ok(receipt);
        }
        let targets = self.live_targets(root, subtree, "approvals target")?;
        // The root is the one the operator named, so it is the only one whose
        // parent is outside the change: a descendant's parent moves with it.
        if let Some(agent) = targets.first() {
            self.check_approval_against_parent(agent, &policy)?;
        }
        // Tightening only the root, leaving its existing children behind,
        // must not strand a live child less gated than its now-stricter
        // parent (the inheritance invariant, enforced against existing agents
        // too, not only at creation).
        if !subtree && let Some(descendant) = self.least_gated_live_descendant(root, &policy)? {
            return Err(RuntimeError::Invalid(format!(
                "agent {descendant} is a live descendant gated less than that policy; \
                 tighten it first, or pass subtree to move it too"
            )));
        }
        // A subtree move never loosens a descendant deliberately created
        // more gated than *its own parent* - the one case §17's inheritance
        // rule explicitly permits - even though every agent here narrows
        // the daemon's old default; only "stricter than its immediate
        // parent" marks a deliberate choice, so compare against that parent,
        // not the target policy.
        let current: std::collections::HashMap<AgentId, ApprovalPolicy> = targets
            .iter()
            .map(|a| (a.id, a.config.approval.clone()))
            .collect();
        let at = now();
        let mut commit = Commit::new();
        let mut changed = Vec::new();
        // `targets` is root-first BFS (subtree_of), so a parent is always
        // processed before its children: `skipped` lets a skip propagate
        // down the whole branch instead of just the one deliberately
        // stricter agent, so a descendant of a skipped agent is never left
        // ungated beneath a still-gated one.
        let mut skipped: std::collections::BTreeSet<AgentId> = std::collections::BTreeSet::new();
        for mut agent in targets {
            if agent.config.approval == policy {
                continue;
            }
            let deliberately_stricter = agent.id != root
                && agent
                    .parent
                    .and_then(|p| current.get(&p))
                    .is_some_and(|parent_policy| {
                        agent.config.approval != *parent_policy
                            && agent.config.approval.is_narrowing_of(parent_policy)
                    });
            let under_skipped_parent = agent.parent.is_some_and(|p| skipped.contains(&p));
            if deliberately_stricter || under_skipped_parent {
                skipped.insert(agent.id);
                continue;
            }
            if !policy.is_yolo() && !self.engine_can_gate(&agent.model) {
                return Err(RuntimeError::Invalid(format!(
                    "agent {} runs on {}, which cannot ask before acting; approvals cannot be \
                     turned on for it",
                    agent.id, agent.model
                )));
            }
            agent.config.approval = policy.clone();
            agent.updated_at = at;
            commit.push_event(
                EventDraft::new(
                    by.clone(),
                    EventKind::AgentConfigUpdated {
                        name: None,
                        model: None,
                        config: agent.config.clone(),
                        scope: None,
                    },
                )
                .agent(agent.id)
                .run(agent.run),
            );
            commit.push_mutation(Mutation::UpsertAgent(agent.clone()));
            changed.push(agent);
        }
        self.commit_receipt(hold, commit, submission.as_ref(), &changed)?;
        Ok(changed)
    }

    /// Which engine owns this model's loop, if any. `None` is an API model
    /// Aether drives itself. Two models share a session only when this is the
    /// same engine for both.
    pub(crate) fn engine_of(&self, model: &ModelRef) -> Option<aether_engine::EngineKind> {
        self.models
            .resolve(model)
            .ok()
            .and_then(|r| crate::turn_engine::engine_kind(r.provider.protocol))
    }

    /// Record an approval decision and tell the agent through its inbox.
    /// With `remember`, the same answer becomes a durable rule first, so a
    /// call already in flight and every later one see one consistent policy.
    pub fn resolve_approval(
        &self,
        id: ApprovalId,
        decision: ApprovalDecision,
        by: Actor,
        note: Option<String>,
        remember: Option<Remember>,
    ) -> Result<()> {
        self.resolve_approval_keyed(id, decision, by, note, remember, None)
    }

    pub fn resolve_approval_keyed(
        &self,
        id: ApprovalId,
        decision: ApprovalDecision,
        by: Actor,
        note: Option<String>,
        remember: Option<Remember>,
        submission: Option<aether_core::idempotency::Submission>,
    ) -> Result<()> {
        // Before the pending check: two answers racing past it would both
        // commit, and the call could be denied yet run, or run twice.
        let hold = self.hold();
        if let Some(receipt) = self.replay_receipt(&hold, submission.as_ref())? {
            return Ok(receipt);
        }
        let mut approval = self
            .store
            .approval(id)?
            .ok_or_else(|| RuntimeError::NotFound(id.to_string()))?;
        if matches!(approval.status, ApprovalStatus::Resolved { .. }) {
            return Err(RuntimeError::Invalid(format!(
                "approval {id} already resolved"
            )));
        }
        // Built before anything is written: a rule that cannot be created
        // must not leave the approval half-answered.
        let rule = match &remember {
            Some(remember) => {
                let matcher = crate::approval::rule_from_answer(
                    remember,
                    &approval.tool,
                    approval.argument.as_deref(),
                    &approval.resources,
                )?;
                // The narrowest scope is the agent that was answered.
                let scope = remember.scope.clone().unwrap_or(RuleScope::Agent {
                    id: approval.agent_id,
                });
                Some(self.prepare_approval_rule(
                    decision,
                    matcher,
                    scope,
                    by.clone(),
                    remember.note.clone(),
                )?)
            }
            None => None,
        };
        let at = now();
        approval.status = ApprovalStatus::Resolved {
            decision,
            by: by.clone(),
            at,
            note: note.clone(),
        };
        let agent = self
            .store
            .agent(approval.agent_id)?
            .ok_or_else(|| RuntimeError::NotFound(approval.agent_id.to_string()))?;
        let mut commit = Commit::new()
            .event(
                EventDraft::new(
                    by.clone(),
                    EventKind::ApprovalResolved {
                        approval: id,
                        decision,
                        by: by.clone(),
                        note: note.clone(),
                    },
                )
                .agent(agent.id)
                .run(agent.run),
            )
            .mutate(Mutation::PutApproval(approval));
        // The rule and the answer land together: neither can survive alone.
        if let Some((_, rule_commit)) = rule {
            commit.events.extend(rule_commit.events);
            commit.mutations.extend(rule_commit.mutations);
        }
        if !agent.status.is_terminal() {
            self.push_inbox(
                &hold,
                &mut commit,
                &agent,
                InboxKind::ApprovalResolved {
                    approval: id,
                    decision,
                    by: Some(by.clone()),
                    note,
                },
                by,
            )?;
        }
        self.commit_receipt(hold, commit, submission.as_ref(), &())?;
        Ok(())
    }

    /// Whether an approval policy can be honoured for agents on `model`:
    /// always for API models; for engines, only when the engine asks before
    /// acting (Codex's exec mode does not).
    pub fn engine_can_gate(&self, model: &ModelRef) -> bool {
        let Ok(resolved) = self.models.resolve(model) else {
            return true;
        };
        crate::turn_engine::engine_kind(resolved.provider.protocol)
            .and_then(|kind| self.engines.get(&kind))
            .is_none_or(|engine| engine.gates_permissions())
    }
}
