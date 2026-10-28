//! The approval gate: remembered decisions, and the one place that turns a
//! gated tool call into "run it", "refuse it", or "ask a person".

use aether_core::approval::{
    ApprovalRule, CallSubject, Revocation, RuleContext, RuleError, RuleMatch, RuleScope, decide,
};
use aether_core::event::{Actor, EventDraft, EventKind};
use aether_core::id::{AgentId, ApprovalRuleId};
use aether_core::time::now;
use aether_core::tool::{ApprovalDecision, ResourceKey};
use aether_store::{Commit, Mutation};

use crate::{Runtime, RuntimeError};

/// How deep a subtree scope is followed before the chain is called broken.
/// Delegation nests far less than this; the bound only stops a cycle written
/// by a buggy client from hanging a turn.
const MAX_ANCESTORS: usize = 256;

/// What the gate decided about one call.
#[derive(Debug, Clone, PartialEq)]
pub enum Gate {
    /// The agent's policy does not gate this class at all.
    Ungated,
    /// A remembered decision answered it.
    Remembered {
        rule: ApprovalRuleId,
        decision: ApprovalDecision,
        note: Option<String>,
    },
    /// A person has to answer.
    AskPerson,
}

/// What to remember when a person answers an approval.
///
/// The defaults are the narrowest reading of the answer: this agent, this
/// tool, and what this call actually touched. Anything wider is asked for.
#[derive(Debug, Clone, PartialEq)]
pub struct Remember {
    /// Where it applies. `None` means the agent that was answered, alone.
    pub scope: Option<RuleScope>,
    /// Cover every call of the tool instead of just this call's argument and
    /// resources. Explicit, because it is far wider than what was answered.
    pub tool_wide: bool,
    pub note: Option<String>,
}

impl Remember {
    /// The narrowest remember: this agent, this call.
    pub fn narrow(note: Option<String>) -> Self {
        Self {
            scope: None,
            tool_wide: false,
            note,
        }
    }
}

impl Runtime {
    /// Remembered decisions, oldest first.
    pub fn approval_rules(&self, include_revoked: bool) -> crate::Result<Vec<ApprovalRule>> {
        Ok(self.store.approval_rules(include_revoked)?)
    }

    /// Record a remembered decision. The matcher is validated here so a rule
    /// that could never compile is refused at its boundary.
    pub fn add_approval_rule(
        &self,
        decision: ApprovalDecision,
        matcher: RuleMatch,
        scope: RuleScope,
        by: Actor,
        note: Option<String>,
    ) -> crate::Result<ApprovalRule> {
        self.add_approval_rule_keyed(decision, matcher, scope, by, note, None)
    }

    pub fn add_approval_rule_keyed(
        &self,
        decision: ApprovalDecision,
        matcher: RuleMatch,
        scope: RuleScope,
        by: Actor,
        note: Option<String>,
        submission: Option<aether_core::idempotency::Submission>,
    ) -> crate::Result<ApprovalRule> {
        let hold = self.hold();
        if let Some(receipt) = self.replay_receipt(&hold, submission.as_ref())? {
            return Ok(receipt);
        }
        let (rule, commit) = self.prepare_approval_rule(decision, matcher, scope, by, note)?;
        self.commit_receipt(hold, commit, submission.as_ref(), &rule)?;
        Ok(rule)
    }

    /// Validate a rule and build the commit that records it, without
    /// applying it. Lets a caller land the rule and whatever else it is
    /// doing in one transaction.
    pub(crate) fn prepare_approval_rule(
        &self,
        decision: ApprovalDecision,
        matcher: RuleMatch,
        scope: RuleScope,
        by: Actor,
        note: Option<String>,
    ) -> crate::Result<(ApprovalRule, Commit)> {
        matcher
            .validate()
            .map_err(|e: RuleError| RuntimeError::Invalid(e.to_string()))?;
        self.check_scope_exists(&scope)?;
        let note = note.map(|note| self.tool_runtime.redactor().redact(&note).into_owned());
        let rule = ApprovalRule {
            id: ApprovalRuleId::new(),
            decision,
            matcher,
            scope,
            created_at: now(),
            created_by: by.clone(),
            note,
            revoked: None,
        };
        let commit = Commit::new()
            .event(EventDraft::new(
                by,
                EventKind::ApprovalRuleAdded { rule: rule.clone() },
            ))
            .mutate(Mutation::PutApprovalRule(rule.clone()));
        Ok((rule, commit))
    }

    pub fn revoke_approval_rule(
        &self,
        id: ApprovalRuleId,
        by: Actor,
        reason: Option<String>,
    ) -> crate::Result<ApprovalRule> {
        let mut rule = self
            .store
            .approval_rule(id)?
            .ok_or_else(|| RuntimeError::NotFound(id.to_string()))?;
        if rule.revoked.is_some() {
            return Err(RuntimeError::Invalid(format!("rule {id} already revoked")));
        }
        let reason = reason.map(|reason| self.tool_runtime.redactor().redact(&reason).into_owned());
        let revocation = Revocation {
            at: now(),
            by: by.clone(),
            reason,
        };
        rule.revoked = Some(revocation.clone());
        self.apply(
            Commit::new()
                .event(EventDraft::new(
                    by,
                    EventKind::ApprovalRuleRevoked {
                        rule: id,
                        revocation,
                    },
                ))
                .mutate(Mutation::PutApprovalRule(rule.clone())),
        )?;
        Ok(rule)
    }

    /// A rule about an agent or a run that does not exist is a typo, not a
    /// policy; refuse it while the person is still looking at it.
    fn check_scope_exists(&self, scope: &RuleScope) -> crate::Result<()> {
        let agent = match scope {
            RuleScope::Agent { id } | RuleScope::Subtree { root: id } => *id,
            RuleScope::Run { id } => {
                // A mistyped run id would create a rule that decides nothing.
                if self
                    .store
                    .agents(&aether_store::AgentFilter {
                        run: Some(*id),
                        ..Default::default()
                    })?
                    .is_empty()
                {
                    return Err(RuntimeError::NotFound(id.to_string()));
                }
                return Ok(());
            }
            RuleScope::Everywhere => return Ok(()),
        };
        if self.store.agent(agent)?.is_none() {
            return Err(RuntimeError::NotFound(agent.to_string()));
        }
        Ok(())
    }

    /// Place an agent in the scope hierarchy: itself, its ancestors, its run.
    pub fn rule_context(&self, agent: AgentId) -> crate::Result<RuleContext> {
        let record = self
            .store
            .agent(agent)?
            .ok_or_else(|| RuntimeError::NotFound(agent.to_string()))?;
        let mut ancestors = Vec::new();
        let mut next = record.parent;
        while let Some(id) = next {
            if ancestors.len() >= MAX_ANCESTORS || ancestors.contains(&id) {
                tracing::error!(agent = %agent, at = %id, "parent chain is cyclic or too deep");
                break;
            }
            ancestors.push(id);
            next = self.store.agent(id)?.and_then(|a| a.parent);
        }
        Ok(RuleContext {
            agent,
            ancestors,
            run: Some(record.run),
        })
    }

    /// Decide one gated call. `gated` is the agent's own policy verdict for
    /// the call's side-effect class.
    pub fn gate_call(
        &self,
        gated: bool,
        subject: &CallSubject,
        ctx: &RuleContext,
    ) -> crate::Result<Gate> {
        if !gated {
            return Ok(Gate::Ungated);
        }
        let rules = self.store.approval_rules(false)?;
        Ok(match decide(&rules, subject, ctx) {
            Some(rule) => Gate::Remembered {
                rule: rule.id,
                decision: rule.decision,
                note: rule.note.clone(),
            },
            None => Gate::AskPerson,
        })
    }
}

/// Build the rule a person asked to remember from the call they answered.
///
/// Narrow by default: the call's exact argument when the tool exposes one
/// (`Tool::rule_argument`), and its first declared resource when it does
/// not - which keeps a path-only tool like `write_file` pinned to its path.
/// A tool with neither is refused rather than silently widened.
pub fn rule_from_answer(
    remember: &Remember,
    tool: &str,
    argument: Option<&str>,
    resources: &[ResourceKey],
) -> Result<RuleMatch, RuntimeError> {
    let matcher = RuleMatch::tool(tool);
    if remember.tool_wide {
        return Ok(matcher);
    }
    if let Some(argument) = argument {
        return Ok(matcher.with_exact_argument(argument));
    }
    // Filesystem aliases are additional lease claims, not permission to
    // remember their common parent or widen a one-path answer.
    if let Some(declared) = resources.first() {
        return Ok(matcher.with_resource_prefix(declared.0.clone()));
    }
    Err(RuntimeError::Invalid(format!(
        "{tool} exposes no argument and this call touched nothing nameable, so          remembering it would cover the whole tool; say so explicitly"
    )))
}

/// The subject a rule is matched against, taken from the call as recorded.
/// `known` is false when the resources are real but not the whole truth.
pub fn subject_of(
    tool: &str,
    argument: Option<String>,
    resources: &[ResourceKey],
    known: bool,
) -> CallSubject {
    match known {
        true => CallSubject::new(tool, argument, resources.to_vec()),
        false => CallSubject::partial(tool, argument, resources.to_vec()),
    }
}

/// Record that a rule answered a call: the request and its resolution land
/// together, so the log reads the same whether a person or a rule decided.
pub fn remembered_commit(
    commit: &mut Commit,
    draft: impl Fn(EventKind) -> EventDraft,
    approval: &aether_core::tool::ApprovalRequest,
    rule: ApprovalRuleId,
    decision: ApprovalDecision,
    note: Option<String>,
) {
    let by = Actor::Rule { rule };
    commit.push_event(draft(EventKind::ApprovalRequested {
        approval: approval.id,
        call: approval.call_id,
        tool: approval.tool.clone(),
        description: approval.description.clone(),
        argument: approval.argument.clone(),
        side_effect: approval.side_effect,
    }));
    commit.push_event(draft(EventKind::ApprovalResolved {
        approval: approval.id,
        decision,
        by: by.clone(),
        note: note.clone(),
    }));
    let mut resolved = approval.clone();
    resolved.status = aether_core::tool::ApprovalStatus::Resolved {
        decision,
        by,
        at: now(),
        note,
    };
    commit.push_mutation(Mutation::PutApproval(resolved));
}

/// Deny a pending approval on the system's behalf because the turn that asked
/// is gone. Nobody is woken: there is no call left for the answer to release.
pub(crate) fn expiry_commit(
    commit: &mut Commit,
    mut approval: aether_core::tool::ApprovalRequest,
    agent: &aether_core::agent::Agent,
    why: &str,
) {
    assert_eq!(
        approval.agent_id, agent.id,
        "an approval expires with its own agent"
    );
    assert!(
        matches!(approval.status, aether_core::tool::ApprovalStatus::Pending),
        "only a pending approval expires"
    );
    let note = Some(format!("{TURN_ENDED}: {why}"));
    let mut draft = EventDraft::new(
        Actor::System,
        EventKind::ApprovalResolved {
            approval: approval.id,
            decision: ApprovalDecision::Denied,
            by: Actor::System,
            note: note.clone(),
        },
    )
    .agent(agent.id)
    .parent(agent.parent)
    .run(agent.run);
    draft.task_id = approval.task_id;
    commit.push_event(draft);
    approval.status = aether_core::tool::ApprovalStatus::Resolved {
        decision: ApprovalDecision::Denied,
        by: Actor::System,
        at: now(),
        note,
    };
    commit.push_mutation(Mutation::PutApproval(approval));
}

/// How an expired approval's note begins.
pub const TURN_ENDED: &str = "turn ended";

/// What the model is told when a remembered decision refused its call.
pub(crate) fn describe_denial(rule: ApprovalRuleId, note: Option<&str>) -> String {
    match note {
        Some(n) if !n.trim().is_empty() => format!("denied by remembered decision {rule}: {n}"),
        _ => format!("denied by remembered decision {rule}"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn remembered_path_rules_do_not_widen_when_leases_include_aliases() {
        let agent = AgentId::new();
        let ctx = RuleContext {
            agent,
            ancestors: vec![],
            run: None,
        };
        for (path, target) in [
            ("/workspace/alias", "/workspace/real"),
            ("/workspace/alias", "/allowed/target"),
            ("./workspace/alias", "/absolute/workspace/real"),
        ] {
            let declared = ResourceKey::file(path);
            let resources = vec![declared.clone(), ResourceKey::file(target)];
            let matcher =
                rule_from_answer(&Remember::narrow(None), "write_file", None, &resources).unwrap();
            assert_eq!(
                matcher.resource_prefix.as_deref(),
                Some(declared.0.as_str())
            );
            let rule = ApprovalRule {
                id: ApprovalRuleId::new(),
                decision: ApprovalDecision::Approved,
                matcher,
                scope: RuleScope::Agent { id: agent },
                created_at: now(),
                created_by: Actor::Agent { id: agent },
                note: None,
                revoked: None,
            };
            assert!(rule.matches(&CallSubject::new("write_file", None, resources), &ctx));
            for other in ["/workspace/sibling", "/allowed/sibling", "/other/secret"] {
                assert!(
                    !rule.matches(
                        &CallSubject::new("write_file", None, vec![ResourceKey::file(other)]),
                        &ctx
                    ),
                    "remembering {target} must not allow {other}"
                );
            }
        }
    }

    /// The refusal exists so a remembered answer never quietly covers a whole
    /// tool. Giving MCP calls a rule argument must not make it unreachable.
    #[test]
    fn a_call_with_nothing_to_narrow_by_is_refused_unless_tool_wide_is_asked_for() {
        let narrow = Remember::narrow(None);
        let e = rule_from_answer(&narrow, "opaque", None, &[])
            .expect_err("nothing to narrow by must be refused, not widened");
        assert!(e.to_string().contains("explicitly"), "{e}");

        let wide = Remember {
            tool_wide: true,
            ..Remember::narrow(None)
        };
        let matcher =
            rule_from_answer(&wide, "opaque", None, &[]).expect("asked for explicitly, so allowed");
        assert_eq!(matcher.tool.as_deref(), Some("opaque"));
        assert_eq!(matcher.argument, None);

        // An argument is what pins it; an MCP call now always has one.
        let pinned = rule_from_answer(
            &narrow,
            "mcp__srv__write",
            Some(r#"{"key":"k"}"#),
            &[ResourceKey("mcp:srv".into())],
        )
        .expect("an argument narrows it");
        assert!(pinned.argument.is_some(), "{pinned:?}");
        assert_eq!(
            pinned.resource_prefix, None,
            "the constant server key must not be what a rule remembers"
        );
    }
}
