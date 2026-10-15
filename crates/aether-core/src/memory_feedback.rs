//! Operator evidence about memories a task actually saw. Retrieval is unchanged.

use serde::{Deserialize, Serialize};

use crate::event::Actor;
use crate::id::{InvocationId, MemoryFeedbackId, MemoryId, TaskId};
use crate::time::Timestamp;

pub const MAX_FEEDBACK_REASON_CHARS: usize = 4096;
pub const MEMORY_FEEDBACK_LIMIT: usize = 100;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum MemoryVerdict {
    Helped,
    Hurt,
    Neutral,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum MemoryEvidence {
    Operator,
    Review,
    Outcome,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, schemars::JsonSchema)]
pub struct MemoryFeedbackSpec {
    pub memory: MemoryId,
    pub verdict: MemoryVerdict,
    pub evidence: MemoryEvidence,
    pub task: TaskId,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub invocation: Option<InvocationId>,
    pub reason: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, schemars::JsonSchema)]
pub struct MemoryFeedback {
    pub id: MemoryFeedbackId,
    #[serde(flatten)]
    pub spec: MemoryFeedbackSpec,
    pub actor: Actor,
    pub created_at: Timestamp,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub retracted_at: Option<Timestamp>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub retraction_reason: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, schemars::JsonSchema)]
pub struct MemoryFeedbackList {
    pub records: Vec<MemoryFeedback>,
    pub truncated: bool,
}
