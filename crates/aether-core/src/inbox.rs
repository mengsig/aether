//! Agent inboxes: how completions, messages and approvals reach an agent.

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::event::Actor;
use crate::id::{AgentId, ApprovalId, InboxItemId, MessageId, TaskId};
use crate::model::ContentBlock;
use crate::task::TaskResult;
use crate::time::Timestamp;
use crate::tool::ApprovalDecision;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum InboxKind {
    TaskCompleted {
        task: TaskId,
        result: TaskResult,
    },
    TaskFailed {
        task: TaskId,
        error: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        structured: Option<Value>,
    },
    TaskCancelled {
        task: TaskId,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        reason: Option<String>,
    },
    Message {
        message_id: MessageId,
        from: Actor,
        content: Vec<ContentBlock>,
    },
    ApprovalResolved {
        approval: ApprovalId,
        decision: ApprovalDecision,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        by: Option<Actor>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        note: Option<String>,
    },
    ChildFailed {
        agent: AgentId,
        error: String,
    },
    /// A descendant parked on provider exhaustion; it probes and resumes by
    /// itself, so the recipient is informed rather than asked to act.
    ChildSuspended {
        agent: AgentId,
        detail: String,
    },
    /// The same descendant's provider answered again and its turn continues;
    /// informational, so it never interrupts a wait.
    ChildResumed {
        agent: AgentId,
    },
    ExternalTrigger {
        source: String,
        payload: Value,
        /// Set only by the daemon's GitHub watcher, for a request it verified
        /// as written by an operator login. Never read from `payload`.
        #[serde(default, skip_serializing_if = "std::ops::Not::not")]
        operator: bool,
    },
    /// A bare wake with a reason (deadline passed, operator nudge).
    Wake {
        reason: String,
    },
}

impl InboxKind {
    /// The agent this item is about, if any.
    pub fn sender_agent(&self) -> Option<AgentId> {
        match self {
            Self::Message {
                from: Actor::Agent { id },
                ..
            } => Some(*id),
            Self::ChildFailed { agent, .. }
            | Self::ChildSuspended { agent, .. }
            | Self::ChildResumed { agent } => Some(*agent),
            _ => None,
        }
    }

    pub fn task(&self) -> Option<TaskId> {
        match self {
            Self::TaskCompleted { task, .. }
            | Self::TaskFailed { task, .. }
            | Self::TaskCancelled { task, .. } => Some(*task),
            _ => None,
        }
    }
}

/// One delivered item. `seq` is per-agent and strictly increasing.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, schemars::JsonSchema)]
pub struct InboxItem {
    pub id: InboxItemId,
    pub agent_id: AgentId,
    pub seq: u64,
    pub kind: InboxKind,
    pub delivered_at: Timestamp,
}

/// One agent's copy of a message: who received it and under which id
/// (VISION §3.5). A single delivery names one agent too, so a submission
/// record and a fan-out reply speak the same shape.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
pub struct MessageDelivery {
    pub agent: AgentId,
    pub message: MessageId,
}
