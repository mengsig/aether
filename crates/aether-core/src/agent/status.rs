use std::fmt;

use serde::{Deserialize, Serialize};

#[expect(
    unused_imports,
    reason = "preserve the schema description and its intra-doc link"
)]
use super::Agent;
use super::WaitCondition;
use crate::{
    id::{AgentId, TaskId},
    model::{ModelErrorClass, ModelRef},
    time::Timestamp,
    tool::ResourceKey,
};

/// Why an agent is Blocked. One variant per cause, each carrying what that
/// cause needs, so a Blocked agent is never ambiguous about what would clear
/// it. An operator pause is not here: it is orthogonal to scheduling state
/// and lives in [`Agent::paused`] (VISION §3.1).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(tag = "block", rename_all = "snake_case")]
pub enum BlockReason {
    /// VISION §3.2: the owner of a task whose dependencies are not terminal.
    /// Cleared only when that dependency finishes.
    Dependency { on: TaskId },
    /// VISION §3.6 limit 5: a live turn waiting for a resource key another
    /// agent holds. The turn itself resumes when the key comes free.
    Resource { holder: AgentId, key: ResourceKey },
    /// VISION §3.9: a non-idempotent tool call started without an outcome.
    /// Only a person clears it.
    Inspection { detail: String },
    /// Provider credits/allowance spent on every link of the chain. Cleared by
    /// the scheduler when `next_probe` comes due, or by a person.
    Exhausted {
        /// The final link that was exhausted.
        model: ModelRef,
        class: ExhaustionClass,
        /// Start of this exhaustion episode, kept across failed probes.
        since: Timestamp,
        /// This suspension + the retry-after the provider reported, bounded
        /// like `next_probe`. Reset text in prose is never parsed.
        reported_reset: Option<Timestamp>,
        /// Always present: a suspended agent never parks without a plan.
        next_probe: Timestamp,
        /// Probe turns that have failed in this episode.
        probes: u32,
        /// Sanitized provider text.
        detail: String,
    },
}

/// The subset of [`ModelErrorClass`] a probe can recover from. `ContextLimit`
/// is deliberately unrepresentable: it is not credits, and waiting cannot fix it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum ExhaustionClass {
    QuotaExhausted,
    UsageLimit,
}

impl ExhaustionClass {
    /// `None` for every class that suspending would not recover.
    pub fn of(class: ModelErrorClass) -> Option<Self> {
        match class {
            ModelErrorClass::QuotaExhausted => Some(Self::QuotaExhausted),
            ModelErrorClass::UsageLimit => Some(Self::UsageLimit),
            _ => None,
        }
    }

    /// The wire name, matching [`ModelErrorClass::as_str`].
    pub fn as_str(self) -> &'static str {
        match self {
            Self::QuotaExhausted => "quota_exhausted",
            Self::UsageLimit => "usage_limit",
        }
    }
}

impl BlockReason {
    /// One line for a status display and the `agent.blocked` event.
    pub fn describe(&self) -> String {
        match self {
            Self::Dependency { on } => format!("waiting for task {on}"),
            Self::Resource { holder, key } => format!("waiting for {} held by {holder}", key.0),
            Self::Inspection { detail } => detail.clone(),
            Self::Exhausted {
                model,
                class,
                next_probe,
                ..
            } => format!(
                "exhausted: {} {}; next probe {}",
                model.provider,
                class.as_str(),
                next_probe.format("%H:%M")
            ),
        }
    }

    /// Whether only the operator can clear this block: the others resolve
    /// when the task or resource they wait on does.
    pub fn needs_operator(&self) -> bool {
        matches!(self, Self::Inspection { .. } | Self::Exhausted { .. })
    }

    /// The dependency this block is waiting on, for the event's field.
    pub fn dependency(&self) -> Option<TaskId> {
        match self {
            Self::Dependency { on } => Some(*on),
            _ => None,
        }
    }
}

/// Rows written before the reason was typed carry a free-text `reason`.
/// Park those for a person rather than guessing: the gate re-derives a real
/// dependency block on the next tick once the agent is resumed.
fn block_reason_or_legacy<'de, D>(d: D) -> Result<BlockReason, D::Error>
where
    D: serde::Deserializer<'de>,
{
    #[derive(Deserialize)]
    #[serde(untagged)]
    enum Repr {
        Typed(BlockReason),
        Legacy(String),
    }
    Ok(match Repr::deserialize(d)? {
        Repr::Typed(reason) => reason,
        Repr::Legacy(detail) => BlockReason::Inspection { detail },
    })
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(tag = "state", rename_all = "snake_case")]
pub enum AgentStatus {
    Created,
    Ready,
    Running,
    Waiting {
        condition: WaitCondition,
    },
    Blocked {
        #[serde(deserialize_with = "block_reason_or_legacy")]
        reason: BlockReason,
    },
    Completed {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        summary: Option<String>,
    },
    Failed {
        error: String,
    },
    Cancelled {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        reason: Option<String>,
    },
    /// Read out of another daemon's export. Terminal by construction: an
    /// imported agent is a record of work that happened elsewhere, and no
    /// local command or scheduler pass may give it a turn.
    Imported {
        /// What it was when the export was written. For the record only.
        was: AgentStatusKind,
    },
}

/// The variant of an [`AgentStatus`] without its payload.
#[derive(
    Debug,
    Clone,
    Copy,
    PartialEq,
    Eq,
    Hash,
    PartialOrd,
    Ord,
    Serialize,
    Deserialize,
    schemars::JsonSchema,
)]
#[serde(rename_all = "snake_case")]
pub enum AgentStatusKind {
    Created,
    Ready,
    Running,
    Waiting,
    Blocked,
    Completed,
    Failed,
    Cancelled,
    Imported,
}

impl AgentStatusKind {
    /// Every variant. Anything deriving a set of statuses filters this, so a
    /// new variant cannot be missed by a list written out by hand.
    pub const ALL: [AgentStatusKind; 9] = [
        Self::Created,
        Self::Ready,
        Self::Running,
        Self::Waiting,
        Self::Blocked,
        Self::Completed,
        Self::Failed,
        Self::Cancelled,
        Self::Imported,
    ];

    pub fn as_str(self) -> &'static str {
        match self {
            Self::Created => "created",
            Self::Ready => "ready",
            Self::Running => "running",
            Self::Waiting => "waiting",
            Self::Blocked => "blocked",
            Self::Completed => "completed",
            Self::Failed => "failed",
            Self::Cancelled => "cancelled",
            Self::Imported => "imported",
        }
    }

    pub fn is_terminal(self) -> bool {
        matches!(
            self,
            Self::Completed | Self::Failed | Self::Cancelled | Self::Imported
        )
    }

    /// Whether the machine allows moving from `self` to `to`.
    pub fn can_transition_to(self, to: AgentStatusKind) -> bool {
        use AgentStatusKind::*;
        match self {
            Created => matches!(to, Ready | Cancelled | Failed),
            // Ready -> Waiting: nothing actionable yet (idle chat agent, no inbox).
            Ready => matches!(to, Running | Waiting | Blocked | Cancelled | Failed),
            Running => matches!(
                to,
                Ready | Waiting | Blocked | Completed | Failed | Cancelled
            ),
            Waiting => matches!(to, Ready | Blocked | Cancelled | Failed),
            // Blocked -> Running: a turn held up by a resource lease resumes
            // where it stopped. Blocked -> Blocked: recovery re-classifies a
            // lease wait whose table died with the daemon.
            Blocked => matches!(to, Ready | Running | Blocked | Cancelled | Failed),
            // Failed -> Ready: an operator wakes the same agent in place. The
            // failure stays in its history; nothing else leaves Failed.
            Failed => matches!(to, Ready),
            // Imported never leaves: the run is an archive, not a fleet.
            Completed | Cancelled | Imported => false,
        }
    }
}

impl fmt::Display for AgentStatusKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

impl AgentStatus {
    pub fn kind(&self) -> AgentStatusKind {
        match self {
            Self::Created => AgentStatusKind::Created,
            Self::Ready => AgentStatusKind::Ready,
            Self::Running => AgentStatusKind::Running,
            Self::Waiting { .. } => AgentStatusKind::Waiting,
            Self::Blocked { .. } => AgentStatusKind::Blocked,
            Self::Completed { .. } => AgentStatusKind::Completed,
            Self::Failed { .. } => AgentStatusKind::Failed,
            Self::Cancelled { .. } => AgentStatusKind::Cancelled,
            Self::Imported { .. } => AgentStatusKind::Imported,
        }
    }

    pub fn is_terminal(&self) -> bool {
        self.kind().is_terminal()
    }
}
