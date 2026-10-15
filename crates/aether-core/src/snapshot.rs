//! Immutable invocation snapshots: exactly what a model saw on a turn.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::id::{AgentId, InvocationId, MemoryId, SnapshotId, TaskId};
use crate::model::{
    CapabilityProfile, ContentBlock, Effort, Message, ModelRef, StopReason, ThinkingMode, ToolSpec,
    Usage,
};
use crate::time::Timestamp;

/// The ordered context layers. Order is the order they are rendered.
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
pub enum LayerKind {
    HarnessPolicy,
    ModelProfile,
    RoleTask,
    RuntimeState,
    Checkpoint,
    RetrievedMemory,
    RecentTail,
    ToolSchemas,
}

impl LayerKind {
    pub const ALL: [LayerKind; 8] = [
        Self::HarnessPolicy,
        Self::ModelProfile,
        Self::RoleTask,
        Self::RuntimeState,
        Self::Checkpoint,
        Self::RetrievedMemory,
        Self::RecentTail,
        Self::ToolSchemas,
    ];

    pub fn as_str(self) -> &'static str {
        match self {
            Self::HarnessPolicy => "harness_policy",
            Self::ModelProfile => "model_profile",
            Self::RoleTask => "role_task",
            Self::RuntimeState => "runtime_state",
            Self::Checkpoint => "checkpoint",
            Self::RetrievedMemory => "retrieved_memory",
            Self::RecentTail => "recent_tail",
            Self::ToolSchemas => "tool_schemas",
        }
    }
}

/// Where a layer's text reached the model on this turn.
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize, schemars::JsonSchema,
)]
#[serde(rename_all = "snake_case")]
pub enum LayerPlacement {
    /// In the system prompt, where the layer was rendered - also the default
    /// for a hoistable layer that had no text to place this turn, so it
    /// means "not hoisted", not necessarily "sent as system text".
    #[default]
    System,
    /// Hoisted into the last user message, whose text carries it instead.
    UserTurn,
    /// Not sent at all: an engine session already holds this exact text.
    Retained,
}

/// One rendered layer. System layers carry text; the tail carries messages
/// and the tool layer carries schemas, so those keep only counts here.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, schemars::JsonSchema)]
pub struct ContextLayer {
    pub kind: LayerKind,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub text: Option<String>,
    /// Estimated tokens contributed by this layer.
    pub tokens: u32,
    /// Items in the layer (messages, memories, tools).
    #[serde(default)]
    pub items: u32,
    /// Which half of the input carried this layer. A hoisted or
    /// retained layer keeps its kind and count but not its text: the text is
    /// in the last user message, or in the engine's own retained history.
    #[serde(default)]
    pub placement: LayerPlacement,
    /// The stored memories this retrieved-memory layer delivered, in render
    /// order: the snapshot's `memory_ids`. Absent when the layer holds none.
    /// A resumed native session may still hold earlier calls' memories too.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub memories: Vec<MemoryId>,
}

#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize, schemars::JsonSchema)]
pub struct InvocationParams {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub effort: Option<Effort>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub thinking: Option<ThinkingMode>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_output_tokens: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub temperature: Option<f32>,
    #[serde(default)]
    pub extensions: BTreeMap<String, Value>,
}

/// Everything sent to the model on one turn, frozen before the request.
///
/// Remote inference is not deterministic, so this does not promise replay.
/// It promises reconstruction: every input the model received, and the
/// adapter and capability profile that shaped the wire request, are here.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, schemars::JsonSchema)]
pub struct InvocationSnapshot {
    pub id: SnapshotId,
    pub agent_id: AgentId,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub task_id: Option<TaskId>,
    pub turn: u32,
    pub model: ModelRef,
    /// Protocol adapter and its version, e.g. `anthropic-messages/1`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub adapter: Option<String>,
    /// The capability profile in force when the request was built.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub capabilities: Option<CapabilityProfile>,
    /// Memories rendered into the context, by id, in render order.
    #[serde(default)]
    pub memory_ids: Vec<MemoryId>,
    /// System layers in render order; their `text` concatenates to the system prompt.
    pub layers: Vec<ContextLayer>,
    pub messages: Vec<Message>,
    pub tools: Vec<ToolSpec>,
    pub params: InvocationParams,
    pub token_estimate: u32,
    pub created_at: Timestamp,
}

impl InvocationSnapshot {
    /// The system prompt as the concatenation of layer texts.
    pub fn system_text(&self) -> String {
        let mut out = String::new();
        for layer in &self.layers {
            if let Some(text) = &layer.text {
                if !out.is_empty() {
                    out.push_str("\n\n");
                }
                out.push_str(text);
            }
        }
        out
    }
}

/// The memories one snapshot put in front of the model, as a host asks for
/// them through `memories_in_context`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, schemars::JsonSchema)]
pub struct SnapshotMemories {
    pub snapshot: SnapshotId,
    /// The invocations in scope answered from this snapshot, oldest first. A
    /// native engine turn answers several messages from one snapshot.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub invocations: Vec<InvocationId>,
    pub agent: AgentId,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub task: Option<TaskId>,
    pub turn: u32,
    pub at: Timestamp,
    /// The snapshot's `memory_ids`: rendered memories, in render order.
    #[serde(default)]
    pub memories: Vec<MemoryId>,
}

impl SnapshotMemories {
    pub fn of(snapshot: &InvocationSnapshot) -> Self {
        Self {
            snapshot: snapshot.id,
            invocations: Vec::new(),
            agent: snapshot.agent_id,
            task: snapshot.task_id,
            turn: snapshot.turn,
            at: snapshot.created_at,
            memories: snapshot.memory_ids.clone(),
        }
    }
}

/// Which memories each model call of a task, or one invocation, was given,
/// oldest snapshot first. `truncated` says snapshots past the limit were left out.
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize, schemars::JsonSchema)]
pub struct MemoriesInContext {
    pub snapshots: Vec<SnapshotMemories>,
    #[serde(default)]
    pub truncated: bool,
}

/// The result of sending a snapshot to a model.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, schemars::JsonSchema)]
pub struct Invocation {
    pub id: InvocationId,
    pub snapshot_id: SnapshotId,
    pub agent_id: AgentId,
    pub turn: u32,
    pub model: ModelRef,
    pub started_at: Timestamp,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub finished_at: Option<Timestamp>,
    #[serde(default)]
    pub content: Vec<ContentBlock>,
    #[serde(default)]
    pub usage: Usage,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub stop_reason: Option<StopReason>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub provider_message_id: Option<String>,
}

impl Invocation {
    pub fn duration_ms(&self) -> Option<u64> {
        self.finished_at
            .map(|f| (f - self.started_at).num_milliseconds().max(0) as u64)
    }
}
