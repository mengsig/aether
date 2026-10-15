//! Context assembly: turning authoritative state into the prompt a model
//! sees on one turn, frozen as an immutable snapshot.
//!
//! The prompt is a projection of what the model needs now, built from
//! ordered layers with a budget per layer. When the recent tail would not
//! fit, the assembler drops from the oldest end and asks for compaction; it
//! never truncates silently.

pub mod budget;
pub mod compaction;
pub mod estimate;
pub mod render;
pub mod transcript;

use aether_core::agent::Agent;
use aether_core::checkpoint::Checkpoint;
use aether_core::id::{InboxItemId, SnapshotId};
use aether_core::inbox::InboxItem;
use aether_core::memory::MemoryRecord;
use aether_core::model::{
    CapabilityProfile, Message, ModelRequest, SystemBlock, ThinkingMode, ThinkingSupport,
    ToolChoice, ToolSpec,
};
use aether_core::snapshot::{
    ContextLayer, InvocationParams, InvocationSnapshot, LayerKind, LayerPlacement,
};
use aether_core::task::Task;
use aether_core::time::Timestamp;
use aether_core::tool::ApprovalRequest;
use aether_store::TranscriptEntry;

pub use budget::ContextBudget;
pub use compaction::{
    Compacted, CompactionAttempt, CompactionError, CompactionInputs, CompactionReport, Compactor,
    fold_range,
};
pub use estimate::TokenEstimator;

/// Everything the assembler reads. All of it comes from authoritative state;
/// nothing here is remembered by the model.
#[derive(Debug, Clone)]
pub struct ContextInputs<'a> {
    pub agent: &'a Agent,
    pub task: Option<&'a Task>,
    pub children: &'a [Agent],
    /// Tasks owned by children or created by this agent.
    pub child_tasks: &'a [Task],
    /// Unconsumed inbox items, ascending by seq.
    pub pending_inbox: &'a [InboxItem],
    pub pending_approvals: &'a [ApprovalRequest],
    pub checkpoint: Option<&'a Checkpoint>,
    /// Transcript entries after the checkpoint (or all, when none).
    pub transcript: &'a [TranscriptEntry],
    pub memories: &'a [MemoryRecord],
    pub tools: Vec<ToolSpec>,
    /// Model-family instructions from the registry, if any.
    pub model_profile: Option<&'a str>,
    /// Extra policy for the engine driving this turn, if any. Appended to the
    /// shared policy so it stays in the static, cacheable head of the system
    /// prompt rather than becoming another layer.
    pub engine_policy: Option<&'a str>,
    pub capabilities: &'a CapabilityProfile,
    pub adapter: &'a str,
    /// Where the daemon stores message images, so the user turn can name
    /// each file. `None` when this daemon keeps none.
    pub attachment_dir: Option<&'a std::path::Path>,
    pub turn: u32,
    pub now: Timestamp,
}

/// The result of assembly.
#[derive(Debug, Clone)]
pub struct Assembled {
    pub snapshot: InvocationSnapshot,
    pub request: ModelRequest,
    /// The user message this turn introduces (inbox items rendered), which
    /// the runtime must persist as a transcript entry before invoking.
    pub new_user_message: Option<Message>,
    /// The assembler generated this message from the task because no input
    /// existed. Native engines can omit its duplicate text when hoisting the
    /// role/task layer; API requests and durable transcripts still retain it.
    pub synthesized_task_message: bool,
    /// Inbox items rendered into `new_user_message`.
    pub consumed_inbox: Vec<InboxItemId>,
    pub consumed_up_to: Option<u64>,
    /// The tail exceeded its budget or the context is near the threshold.
    pub needs_compaction: bool,
    pub dropped_messages: usize,
}

/// Builds snapshots. Stateless apart from the policy text and estimator.
#[derive(Debug, Clone)]
pub struct ContextManager {
    policy: String,
    estimator: TokenEstimator,
}

impl Default for ContextManager {
    fn default() -> Self {
        Self::new()
    }
}

impl ContextManager {
    pub fn new() -> Self {
        Self {
            policy: String::new(),
            estimator: TokenEstimator,
        }
    }

    /// Replace the harness policy text (tests, experiments).
    pub fn with_policy(mut self, policy: impl Into<String>) -> Self {
        self.policy = policy.into();
        self
    }

    fn policy_text(&self, engine_policy: Option<&str>) -> Option<String> {
        let parts: Vec<_> = [Some(self.policy.as_str()), engine_policy]
            .into_iter()
            .flatten()
            .map(str::trim)
            .filter(|text| !text.is_empty())
            .collect();
        (!parts.is_empty()).then(|| parts.join("\n\n"))
    }

    pub fn assemble(&self, inputs: ContextInputs<'_>) -> Assembled {
        let budget =
            ContextBudget::for_profile(inputs.capabilities, inputs.agent.config.max_output_tokens);
        let est = &self.estimator;

        let mut layers: Vec<ContextLayer> = Vec::with_capacity(LayerKind::ALL.len());
        let mut push_text = |kind: LayerKind, text: Option<String>, items: u32| {
            let tokens = text.as_deref().map(|t| est.text(t)).unwrap_or(0);
            layers.push(ContextLayer {
                kind,
                text,
                tokens,
                items,
                placement: LayerPlacement::System,
                memories: Vec::new(),
            });
        };

        let policy = self.policy_text(inputs.engine_policy);
        let policy_items = u32::from(policy.is_some());
        push_text(LayerKind::HarnessPolicy, policy, policy_items);
        push_text(
            LayerKind::ModelProfile,
            inputs
                .model_profile
                .filter(|p| !p.trim().is_empty())
                .map(str::to_owned),
            u32::from(inputs.model_profile.is_some()),
        );
        push_text(
            LayerKind::RoleTask,
            Some(render::role_task(inputs.agent, inputs.task)),
            1,
        );
        push_text(
            LayerKind::RuntimeState,
            Some(render::runtime_state(&inputs)),
            (inputs.children.len() + inputs.child_tasks.len()) as u32,
        );
        push_text(
            LayerKind::Checkpoint,
            inputs.checkpoint.map(render::checkpoint),
            u32::from(inputs.checkpoint.is_some()),
        );

        let (memory_text, memory_ids) = memory_layer(inputs.memories, est, budget.memory_max);
        push_text(
            LayerKind::RetrievedMemory,
            memory_text,
            memory_ids.len() as u32,
        );
        if let Some(layer) = layers.last_mut() {
            debug_assert_eq!(layer.kind, LayerKind::RetrievedMemory);
            layer.memories.clone_from(&memory_ids);
        }

        // Recent tail: transcript plus the new user turn from the inbox.
        let mut messages: Vec<Message> = inputs
            .transcript
            .iter()
            .map(|e| e.message.clone())
            .collect();
        let (new_user_message, consumed_inbox, consumed_up_to) =
            render::inbox_message(inputs.pending_inbox, inputs.attachment_dir);
        if let Some(m) = &new_user_message {
            messages.push(m.clone());
        }
        let mut messages = transcript::sanitize(messages);
        let synthesized_task_message = new_user_message.is_none()
            && messages.is_empty()
            && inputs
                .task
                .is_some_and(|task| !task.objective.trim().is_empty());
        let nudged = transcript::ensure_ends_with_user(
            &mut messages,
            inputs.task.map(|t| t.objective.as_str()),
        );

        let tool_tokens = est.tools(&inputs.tools);
        let fixed: u32 = layers.iter().map(|l| l.tokens).sum::<u32>() + tool_tokens;
        let tail_budget = budget.tail_budget(fixed);
        let mut dropped = 0usize;
        let mut tail_tokens = est.messages(&messages);
        while tail_tokens > tail_budget && messages.len() > 1 {
            // Drop whole exchanges: the oldest message plus any leading
            // assistant turns, so the tail always starts at a user turn and
            // sanitizing cannot re-grow it.
            dropped += transcript::drop_oldest_exchange(&mut messages);
            let before = messages.len();
            messages = transcript::sanitize(messages);
            debug_assert!(
                messages.len() <= before,
                "sanitize must not grow a user-led tail"
            );
            tail_tokens = est.messages(&messages);
        }
        let used = fixed + tail_tokens;
        let needs_compaction = dropped > 0 || budget.over_threshold(used);

        layers.push(ContextLayer {
            kind: LayerKind::RecentTail,
            text: None,
            tokens: tail_tokens,
            items: messages.len() as u32,
            placement: LayerPlacement::UserTurn,
            memories: Vec::new(),
        });
        layers.push(ContextLayer {
            kind: LayerKind::ToolSchemas,
            text: None,
            tokens: tool_tokens,
            items: inputs.tools.len() as u32,
            placement: LayerPlacement::System,
            memories: Vec::new(),
        });

        let params = self.params(&inputs, &budget);
        let snapshot = InvocationSnapshot {
            id: SnapshotId::new(),
            agent_id: inputs.agent.id,
            task_id: inputs.task.map(|t| t.id),
            turn: inputs.turn,
            model: inputs.agent.model.clone(),
            adapter: Some(inputs.adapter.to_owned()),
            capabilities: Some(inputs.capabilities.clone()),
            memory_ids,
            layers,
            messages,
            tools: inputs.tools,
            params,
            token_estimate: used,
            created_at: inputs.now,
        };
        let request = build_request(&snapshot);
        let new_user_message = if nudged && new_user_message.is_none() {
            snapshot.messages.last().cloned()
        } else {
            new_user_message
        };
        Assembled {
            snapshot,
            request,
            new_user_message,
            synthesized_task_message,
            consumed_inbox,
            consumed_up_to,
            needs_compaction,
            dropped_messages: dropped,
        }
    }

    fn params(&self, inputs: &ContextInputs<'_>, budget: &ContextBudget) -> InvocationParams {
        let caps = inputs.capabilities;
        let effort = if caps.effort {
            inputs.agent.config.effort
        } else {
            None
        };
        let thinking = match caps.thinking {
            ThinkingSupport::None => None,
            ThinkingSupport::Adaptive => Some(ThinkingMode::Adaptive),
            ThinkingSupport::Budget => Some(ThinkingMode::Budget {
                tokens: budget.thinking_budget(),
            }),
        };
        let mut extensions = std::collections::BTreeMap::new();
        if matches!(thinking, Some(ThinkingMode::Adaptive)) {
            // Ask for readable thinking so the TUI's thinking window has text;
            // providers that do not know the key ignore it.
            extensions.insert(
                "anthropic.thinking_display".to_owned(),
                serde_json::Value::String("summarized".to_owned()),
            );
        }
        InvocationParams {
            effort,
            thinking,
            max_output_tokens: Some(budget.reserve_output),
            temperature: None,
            extensions,
        }
    }
}

/// Build the model request from a snapshot. Stable layers come first and
/// carry the cache breakpoint; per-turn layers follow.
pub fn build_request(snapshot: &InvocationSnapshot) -> ModelRequest {
    let mut system = Vec::new();
    for layer in &snapshot.layers {
        if let Some(text) = &layer.text {
            let cache = layer.kind == LayerKind::RoleTask;
            system.push(SystemBlock {
                text: text.clone(),
                cache,
            });
        }
    }
    let mut request = ModelRequest::new(snapshot.model.clone(), snapshot.messages.clone());
    request.system = system;
    request.tools = snapshot.tools.clone();
    request.tool_choice = ToolChoice::Auto;
    request.max_output_tokens = snapshot.params.max_output_tokens;
    request.effort = snapshot.params.effort;
    request.thinking = snapshot.params.thinking;
    request.temperature = snapshot.params.temperature;
    request.extensions = snapshot.params.extensions.clone();
    request
}

/// The memory layer's text and the ids it renders, in render order: own
/// memories first, then repository notes, as the text lists them.
fn memory_layer(
    records: &[MemoryRecord],
    estimator: &TokenEstimator,
    maximum: u32,
) -> (Option<String>, Vec<aether_core::id::MemoryId>) {
    let (mut own, mut from_files) = (Vec::new(), Vec::new());
    let (mut own_ids, mut file_ids) = (Vec::new(), Vec::new());
    let mut text = None;
    for record in records.iter().filter(|record| record.is_current()) {
        let line = render::memory_line(record);
        let from_file = record.provenance.source.is_some();
        if from_file {
            from_files.push(line);
        } else {
            own.push(line);
        }
        let candidate = render::memory_layer(&own, &from_files);
        if estimator.text(&candidate) > maximum {
            break;
        }
        if from_file {
            file_ids.push(record.id);
        } else {
            own_ids.push(record.id);
        }
        text = Some(candidate);
    }
    debug_assert!(
        text.as_ref()
            .is_none_or(|text| estimator.text(text) <= maximum)
    );
    own_ids.append(&mut file_ids);
    debug_assert_eq!(text.is_some(), !own_ids.is_empty());
    (text, own_ids)
}

#[cfg(test)]
mod tests;
