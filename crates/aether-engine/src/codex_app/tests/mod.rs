use std::path::PathBuf;

use aether_core::model::{ContentBlock, Effort, Usage};
use aether_core::tool::SideEffectClass;
use serde_json::{Value, json};
use tokio_util::sync::CancellationToken;

use super::{CodexAppEngine, ServerReply, TurnState, thread_params, turn_params};
use crate::detect::DetectError;
use crate::{Engine, EngineEvent, EngineOutcome, EngineTurn, PermissionDecision};

mod counter_recovery;
mod events;
mod launch;
mod live;
mod permissions;
#[cfg(target_os = "linux")]
mod sandbox;
mod usage;

/// A turn resuming thread `th` with what the previous turn persisted.
fn resumed(session_usage: Usage) -> EngineTurn {
    EngineTurn {
        session_usage,
        ..turn(Some("th"), false)
    }
}

/// The tokens a batch of events bills, in order.
fn billed(events: &[EngineEvent]) -> Vec<Usage> {
    events
        .iter()
        .filter_map(|e| match e {
            EngineEvent::UsageDelta { usage } => Some(*usage),
            _ => None,
        })
        .collect()
}

pub(super) fn turn(session: Option<&str>, bypass: bool) -> EngineTurn {
    EngineTurn {
        images: Vec::new(),
        prompt: "do it".into(),
        system_prompt: "# Aether".into(),
        session: session.map(str::to_owned),
        session_usage: Usage::default(),
        session_model_usage: None,
        new_session_id: "n".into(),
        session_recorder: None,
        model: Some("gpt-5.3-codex-spark".into()),
        effort: Some(Effort::Low),
        cwd: PathBuf::from("/w"),
        extra_dirs: vec![PathBuf::from("/shared")],
        hidden_env: Vec::new(),
        tool_hidden_env: Vec::new(),
        temp_dir: None,
        build_env: Vec::new(),
        summary_only: false,
        lean_host_context: false,
        network_access: true,
        bypass_permissions: bypass,
        mcp: Some(crate::McpBridge {
            command: PathBuf::from("/bin/aether"),
            args: vec!["mcp-bridge".into()],
        }),
    }
}

/// The notes a batch of events raised.
fn notes(events: &[EngineEvent]) -> Vec<String> {
    events
        .iter()
        .filter_map(|e| match e {
            EngineEvent::ProtocolNote(n) => Some(n.clone()),
            _ => None,
        })
        .collect()
}

/// One `thread/tokenUsage/updated` carrying `total`, and what it bills.
fn account(state: &mut TurnState, total: Value) -> (Usage, Vec<String>) {
    let events = state.notification(
        "thread/tokenUsage/updated",
        &json!({
            "tokenUsage": {
                "last": {"inputTokens": 1, "cachedInputTokens": 0, "outputTokens": 1},
                "total": total,
            }
        }),
    );
    let mut usage = Usage::default();
    for one in billed(&events) {
        usage += one;
    }
    (usage, notes(&events))
}
