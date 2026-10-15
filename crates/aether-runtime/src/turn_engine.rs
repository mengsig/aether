//! Turns driven by an external engine (Claude Code, Codex).
//!
//! The engine owns its model loop and world tools; Aether records what it
//! streams as ordinary events, answers its harness tool calls and permission
//! prompts through the MCP bridge, and decides the agent's next state from
//! the actions it took.
use std::collections::{BTreeMap, BTreeSet, VecDeque};

use std::sync::Arc;

use std::sync::atomic::{AtomicBool, Ordering};

use std::time::Instant;

use aether_context::ContextInputs;

use aether_core::event::{Actor, EventKind};

use aether_core::id::{AgentId, ApprovalId, InvocationId, ToolCallId};

use aether_core::idempotency::IdempotencyKey;

use aether_core::model::{
    ContentBlock, MessageRole, ModelCost, ModelErrorClass, ModelResponse, StreamEvent, ToolSpec,
    Usage,
};

use aether_core::snapshot::{Invocation, InvocationSnapshot};

use aether_core::task::Task;

use aether_core::time::now;

use aether_core::tool::SideEffectClass;

use aether_core::tool::{
    ApprovalDecision, ApprovalRequest, ApprovalStatus, ResourceKey, ToolCall, ToolOutcome,
};

use aether_engine::{
    Engine, EngineEvent, EngineKind, EngineOutcome, EngineTurn, McpBridge, PermissionDecision,
    contract, side_effect_of,
};

use aether_model::{ModelPrice, Protocol, ResolvedModel};

use aether_store::{AgentFilter, Commit, Mutation, TaskFilter};

use aether_tools::{ToolContext, ToolExecution};

use serde_json::{Value, json};

use tokio::sync::{mpsc, oneshot};

use tokio_util::sync::CancellationToken;

use crate::approval::{self, Gate};

use crate::bus::{BusMessage, DeltaKind};

use crate::turn::{
    CompletionSource, Next, ToolEffects, TurnCtx, apply_action, prose_only_next, tool_context,
};

use crate::{Result, Runtime, RuntimeError};

/// A tool call or permission prompt arriving from the engine's bridge.
pub(crate) struct EngineRequest {
    pub name: String,
    pub input: Value,
    pub reply: oneshot::Sender<(String, bool)>,
}

/// Registered while an engine turn runs so the daemon can route bridge calls.
pub(crate) struct EngineTurnHandle {
    pub token: String,
    pub tx: mpsc::Sender<EngineRequest>,
}

/// Owns the registration for as long as the turn runs. A hand-written remove
/// on each exit path misses a panic inside the turn, and the stale token it
/// leaves keeps authenticating on the MCP HTTP endpoint until the agent's
/// next turn overwrites it.
struct EngineTurnGuard {
    runtime: Arc<Runtime>,
    agent: AgentId,
}

impl EngineTurnGuard {
    pub(super) fn register(
        runtime: &Arc<Runtime>,
        agent: AgentId,
        handle: EngineTurnHandle,
    ) -> Self {
        runtime.engine_turns.lock().insert(agent, handle);
        Self {
            runtime: runtime.clone(),
            agent,
        }
    }
}

impl Drop for EngineTurnGuard {
    fn drop(&mut self) {
        self.runtime.engine_turns.lock().remove(&self.agent);
    }
}

pub(crate) fn engine_kind(protocol: Protocol) -> Option<EngineKind> {
    match protocol {
        Protocol::ClaudeCode => Some(EngineKind::CLAUDE_CODE),
        Protocol::Codex => Some(EngineKind::CODEX),
        _ => None,
    }
}

/// How a native link ended, for the model chain walk in `turn::run_link`.
pub(crate) enum EngineLink {
    /// A recorded provider failure a configured fallback may recover from.
    /// Never produced once the turn committed its task outcome or was cancelled.
    Exhausted {
        class: ModelErrorClass,
        error: String,
    },
    /// Any other ending, including ordinary failures: final for this link.
    Done(Next),
}

#[allow(dead_code)]
fn _usage(_: Usage) {}

mod accounting;
pub(crate) use accounting::TurnPricing;
use accounting::{
    SubtreeUsage, budget_crossed, raise_to, recompute_snapshot_accounting, record_context_usage,
};
mod execution;
pub(crate) use execution::execute;
mod outcome;
use outcome::{
    EngineVerdict, Uncommitted, engine_verdict, record_engine_failure, record_unanswered_prompt,
};
mod driver;
mod native_wait;
mod transient;
use driver::{DriveResult, drive};
mod budget_stop;
#[cfg(test)]
use budget_stop::refused_call_id;
use budget_stop::{
    Crossed, REFUSAL_GRACE, budget_outcome, gated_call, record_refused_call, refusal,
    stop_for_crossing,
};
mod parking;
#[cfg(test)]
use parking::stopped_by;
use parking::{
    BACKGROUND_GRACE, MAX_BACKGROUND_IDS, MAX_ENGINE_ERROR, MAX_ENGINE_NAME, Park, ReapGrace,
    STEER_GRACE, TURN_IS_OVER, clamp, names_exhaustion, park_grace, record_park, start_park,
    stopping,
};
mod steering;
use steering::{Steering, answer_question, record_consumed, steer};
mod events;
use events::{
    PROTOCOL_NOTE_EVENT, PendingCall, SHRUNK_CLOSING_USAGE, StreamState,
    disallowed_native_spawn_tool, handle_event,
};
mod bridge;
use bridge::{GateSubject, Served, Verdict, engine_call_class, handle_request, permission_verdict};
#[cfg(test)]
use bridge::{permission_argument, permission_resources};

#[cfg(test)]
mod tests;
