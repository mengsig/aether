//! Agents: durable, long- or short-lived actors that own tasks and turns.

use std::collections::{BTreeMap, BTreeSet};
use std::fmt;

use serde::{Deserialize, Serialize};

use crate::error::TransitionError;
use crate::id::{AgentId, CheckpointId, RunId, TaskId};
use crate::model::{ModelChain, ModelRef, Usage};
use crate::snapshot::LayerKind;
use crate::time::Timestamp;
mod config;
mod status;
mod wait;

pub use config::{
    AgentConfig, ApprovalDefault, ApprovalPolicy, DeadlineUpdate, ExecutionBackend, HostContext,
    NetworkPolicy, Scope,
};
pub use status::{AgentStatus, AgentStatusKind, BlockReason, ExhaustionClass};
pub use wait::{WaitCondition, WaitOracle};

/// Opaque role label; an orchestration layer gives it meaning.
/// The harness reads only its dots, to pick a repository's notes for a role
/// family (`memory::role_families`).
#[derive(
    Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize, schemars::JsonSchema,
)]
#[serde(transparent)]
pub struct Role(pub String);

impl Role {
    /// Neutral label for agents started without an orchestration role.
    pub const ASSISTANT: &'static str = "assistant";

    /// Legacy role label retained for existing clients and stored agents.
    /// The runtime reads `Agent::parent`, never the role string.
    pub const COORDINATOR: &'static str = "coordinator";

    pub fn new(role: impl Into<String>) -> Self {
        Self(role.into())
    }

    pub fn coordinator() -> Self {
        Self::new(Self::COORDINATOR)
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for Role {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl From<&str> for Role {
    fn from(value: &str) -> Self {
        Self::new(value)
    }
}

/// What an archive command marks: one finished agent, every finished agent of
/// a run, or every finished agent there is.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum ArchiveTarget {
    Agent(AgentId),
    Run(RunId),
    AllFinished,
}

/// Lifetime token counters and activity stats.
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize, schemars::JsonSchema)]
pub struct AgentStats {
    /// Latest reported context input, separate from cumulative billing usage.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub context_tokens: Option<u64>,
    #[serde(default)]
    pub usage: Usage,
    #[serde(default)]
    pub turns: u32,
    #[serde(default)]
    pub tool_calls: u32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_turn_at: Option<Timestamp>,
}

/// A durable agent record. This is authoritative state; the model is never
/// asked to remember any of it.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, schemars::JsonSchema)]
pub struct Agent {
    pub id: AgentId,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub parent: Option<AgentId>,
    pub role: Role,
    pub name: String,
    pub scope: Scope,
    pub model: ModelRef,
    /// Models to try, in order, when `model` fails with a retryable error or
    /// its provider is backed off (VISION §4.5). Empty for most agents.
    #[serde(default)]
    pub fallbacks: Vec<ModelRef>,
    pub status: AgentStatus,
    /// Start of the current continuous status kind; absent for unknown historical state.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub status_since: Option<Timestamp>,
    /// VISION §3.1: an operator pause, durable across restarts and orthogonal
    /// to the status. A paused agent is never dispatched; its inbox fills.
    #[serde(default)]
    pub paused: bool,
    /// VISION §3.8: set exactly when `paused` and the hold came from a
    /// `--subtree` pause, naming the agent that pause is rooted at (possibly
    /// this one). A subtree hold reaches children created under it and is
    /// lifted by the matching subtree resume; a plain pause holds this agent
    /// alone and leaves this `None`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub paused_via: Option<AgentId>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub current_task: Option<TaskId>,
    /// Cleared out of the live view. A durable mark on a
    /// finished agent: it stays in the store, in reports and in replay, and
    /// only the default view filter hides it.
    #[serde(default)]
    pub archived: bool,
    /// Inbox items with seq <= cursor have been rendered into a turn.
    #[serde(default)]
    pub inbox_cursor: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub checkpoint: Option<CheckpointId>,
    /// Session id of an external engine (Claude Code, Codex) that carries this
    /// agent's conversation between turns. `None` for API-driven agents.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub engine_session: Option<String>,
    /// What `engine_session` had counted for itself when this agent last
    /// billed it. Read through [`Agent::engine_session_usage`], which
    /// discards a record belonging to any other session.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub engine_usage: Option<EngineSessionUsage>,
    /// Per-agent layers `engine_session` has already been sent, hashed. Read
    /// through [`Agent::engine_sent_layers`].
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub engine_sent: Option<EngineSentLayers>,
    /// A wait the scheduler cut short (a person spoke, a child failed).
    /// Restored after the turn unless the agent decides otherwise.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub resume_wait: Option<WaitCondition>,
    /// The failed or cancelled agent this one was restarted from (§3.8).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub restarted_from: Option<AgentId>,
    /// The agent that took over from this one after it failed or was
    /// cancelled. Set once: a second restart would fork the history.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub restarted_as: Option<AgentId>,
    pub config: AgentConfig,
    #[serde(default)]
    pub stats: AgentStats,
    pub run: RunId,
    pub created_at: Timestamp,
    pub updated_at: Timestamp,
}

/// An engine session's own cumulative token counters, as it last reported
/// them, tagged with the session they belong to.
///
/// Codex counts a *thread*, not a turn: the counters it reports on a resumed
/// thread include every earlier turn. Persisting the last reading is what
/// lets the next turn bill `total - this` instead of the thread's history.
/// The session id is part of the record so a cleared or replaced session
/// cannot be billed against another session's counters.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, schemars::JsonSchema)]
pub struct EngineSessionUsage {
    pub session: String,
    pub total: Usage,
    /// Cumulative counters by model, when the engine reports their scope.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub models: Option<std::collections::BTreeMap<String, Usage>>,
}

/// The per-agent context layers an engine session has already been sent, as
/// digests of their rendered text.
///
/// An engine session ordinarily retains every user turn it was sent, so a
/// layer whose text has not changed since it was last sent is normally
/// already in front of the model; sending it again would duplicate it in a
/// conversation that keeps both copies. That guarantee ends at the
/// engine's own context compaction - Claude Code's recorded system prompt is
/// reused only "until the conversation is compacted" - which can replace a
/// retained turn with a summary the model never sees again as the original
/// text. Aether requests its own compaction first in the common case, and
/// `turn_engine` clears this record on a sharp context drop as a cheap bound
/// against the other order, but neither proves the engine's compaction never
/// wins the race.
///
/// The session id is part of the record: a cleared or replaced session
/// retains nothing, so its digests must not gate the next turn.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, schemars::JsonSchema)]
pub struct EngineSentLayers {
    pub session: String,
    /// Layer kind -> digest of the exact text sent for it.
    pub layers: BTreeMap<LayerKind, String>,
}

impl EngineSentLayers {
    /// Digest of one layer's rendered text. Only ever compared with another
    /// digest of the same function, so its width is a storage choice.
    pub fn digest(text: &str) -> String {
        let mut hasher = <sha2::Sha256 as sha2::Digest>::new();
        sha2::Digest::update(&mut hasher, text.as_bytes());
        hex::encode(sha2::Digest::finalize(hasher))[..32].to_owned()
    }
}

impl Agent {
    /// The layers the current engine session has already been sent, or `None`
    /// when there is no session or the record belongs to another one - both
    /// cases mean the engine retains nothing and everything must be resent.
    pub fn engine_sent_layers(&self) -> Option<&BTreeMap<LayerKind, String>> {
        match (&self.engine_sent, &self.engine_session) {
            (Some(sent), Some(session)) if &sent.session == session => Some(&sent.layers),
            _ => None,
        }
    }

    /// What the current engine session has already counted for itself. Zero
    /// when there is no session. A resumed session without matching counters
    /// has an unknown cost baseline, not a known zero-dollar history.
    pub fn engine_session_usage(&self) -> Usage {
        match (&self.engine_usage, &self.engine_session) {
            (Some(seen), Some(session)) if &seen.session == session => seen.total,
            (_, Some(_)) => Usage {
                cost_complete: false,
                cost_gap: Some(crate::model::CostGap::Unknown),
                ..Usage::default()
            },
            _ => Usage::default(),
        }
    }

    /// The models this agent's turn may use, in order.
    pub fn chain(&self) -> ModelChain {
        ModelChain::new(self.model.clone(), self.fallbacks.clone())
    }

    /// Apply a status transition, refusing anything the machine forbids.
    pub fn transition(&mut self, next: AgentStatus, at: Timestamp) -> Result<(), TransitionError> {
        let from = self.status.kind();
        let to = next.kind();
        if !from.can_transition_to(to) {
            return Err(TransitionError {
                entity: "agent",
                id: self.id.to_string(),
                from: from.to_string(),
                to: to.to_string(),
            });
        }
        if from != to {
            self.status_since = Some(at);
        }
        self.status = next;
        self.updated_at = at;
        Ok(())
    }

    pub fn is_live(&self) -> bool {
        !self.status.is_terminal()
    }

    /// Remaining lifetime token budget, if one is configured.
    pub fn remaining_budget(&self) -> Option<u64> {
        self.config
            .token_budget
            .map(|b| b.saturating_sub(self.stats.usage.total_tokens()))
    }

    /// Why the agent must stop, given what it and its descendants have spent.
    pub fn budget_exceeded(&self, subtree: &Usage) -> Option<String> {
        self.budget_reason(subtree, 1.0)
    }

    /// The budget it is close to, for a warning before the stop.
    pub fn budget_warning(&self, subtree: &Usage) -> Option<String> {
        if self.budget_exceeded(subtree).is_some() {
            return None;
        }
        self.budget_reason(subtree, BUDGET_WARN_FRACTION)
    }

    /// Why the agent must stop now that the clock has run out (§3.1).
    pub fn deadline_passed(&self, at: Timestamp) -> Option<String> {
        let deadline = self.config.deadline?;
        (at >= deadline).then(|| format!("deadline of {}", deadline.to_rfc3339()))
    }

    fn budget_reason(&self, subtree: &Usage, fraction: f64) -> Option<String> {
        if let Some(limit) = self.config.token_budget {
            let used = subtree.total_tokens();
            if used as f64 >= limit as f64 * fraction {
                return Some(format!("token budget of {limit}: {used} used"));
            }
        }
        if let Some(limit) = self.config.cost_budget {
            // The known floor. An invocation nobody priced cannot make what
            // was already spent smaller, so enforcing on the floor is safe -
            // and it is the only reading that cannot silently stop firing.
            if subtree.cost_usd >= limit * fraction {
                // Not `:.2`: a budget of a fraction of a cent must read back.
                return Some(format!(
                    "cost budget of ${limit}: {} spent",
                    subtree.cost_line()
                ));
            }
        }
        None
    }

    /// Said once when a cost budget is being enforced on a floor: the agent
    /// can outspend the limit by whatever the unpriced invocations cost.
    pub fn cost_floor_note(&self, subtree: &Usage) -> Option<String> {
        let limit = self.config.cost_budget?;
        if subtree.cost_complete {
            return None;
        }
        Some(format!(
            "{COST_FLOOR_NOTE}: cost budget of ${limit} is enforced on {} in the subtree",
            subtree.cost_line()
        ))
    }
}

/// How [`Agent::cost_floor_note`] opens, so the scheduler can tell that
/// notice apart from a soft-limit warning and say each one once.
pub const COST_FLOOR_NOTE: &str = "cost is a lower bound";

/// How much of a budget may be spent before the person is warned (§3.1).
pub const BUDGET_WARN_FRACTION: f64 = 0.8;

/// `root` and every descendant of it within `agents`, breadth-first, each
/// exactly once; empty when `root` is not in the set. One walk shared by
/// everything that acts on a subtree, so a fan-out can never reach the same
/// agent twice and can never miss a branch.
pub fn subtree_of(agents: &[Agent], root: AgentId) -> Vec<AgentId> {
    if !agents.iter().any(|a| a.id == root) {
        return Vec::new();
    }
    let mut children: BTreeMap<AgentId, Vec<AgentId>> = BTreeMap::new();
    for agent in agents {
        if let Some(parent) = agent.parent {
            children.entry(parent).or_default().push(agent.id);
        }
    }
    let mut out = Vec::new();
    let mut seen = BTreeSet::from([root]);
    let mut queue = std::collections::VecDeque::from([root]);
    while let Some(id) = queue.pop_front() {
        out.push(id);
        for child in children.get(&id).into_iter().flatten() {
            if seen.insert(*child) {
                queue.push_back(*child);
            }
        }
    }
    out
}

#[cfg(test)]
mod tests;
