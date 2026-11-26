//! The Antigravity adapter: model discovery, launch flags and the ACP turn,
//! replayed from a recorded `agy acp` session and against a fake CLI.

use std::path::PathBuf;
use std::time::Duration;

use aether_core::model::{ContentBlock, Effort, StreamEvent, Usage};
use serde_json::{Value, json};
use tokio::sync::mpsc;

use super::acp::{self, AcpState, Shared, SharedState, TurnPlan};
use super::launch::{args, turn_plan};
use super::*;
use crate::{EngineEvent, McpBridge, PermissionDecision};

const LISTING: &str = "Fetching available models...\n\
gemini-4-neon-high\tGemini 4 Neon (High)\n\
gemini-4-neon-xhigh\tGemini 4 Neon (XHigh)\n\
gemini-4-neon-low\tGemini 4 Neon (Low)\n\
gemini-4-carbon-medium\tGemini 4 Carbon (Medium)\n\
gemini-4-carbon-high\tGemini 4 Carbon (High)\n\
gemini-pro-orchestrator-neon\tOrchestrator (Neon)\n\
gemini-3.8-flash-low\tGemini 3.8 Flash (Low)\n\
gemini-3.8-flash-high\tGemini 3.8 Flash (High)\n\
claude-opus-5-5-high\tClaude Opus 5.5 (High)\n\
gpt-oss-120b-medium\tGPT-OSS 120B (Medium)\n";

const FIXTURE: &str = include_str!("../../tests/fixtures/antigravity_acp_turn.ndjson");

fn engine() -> AntigravityEngine {
    AntigravityEngine::new("agy", "1.5.18").with_variants(parse_model_list(LISTING))
}

fn turn() -> EngineTurn {
    EngineTurn {
        redactor: Default::default(),
        images: Vec::new(),
        prompt: "hi".into(),
        system_prompt: String::new(),
        session: None,
        session_usage: Usage::default(),
        session_model_usage: None,
        new_session_id: "unused-by-acp".into(),
        session_recorder: None,
        model: Some("gemini-3.8-flash".into()),
        effort: Some(Effort::Low),
        cwd: PathBuf::from("/tmp"),
        extra_dirs: vec![],
        hidden_env: Vec::new(),
        tool_hidden_env: Vec::new(),
        temp_dir: None,
        build_env: Vec::new(),
        summary_only: false,
        compact_session: false,
        auto_compact_tokens: None,
        lean_host_context: false,
        network_access: true,
        bypass_permissions: false,
        mcp: Some(McpBridge {
            command: PathBuf::from("/usr/bin/aether"),
            args: vec!["mcp-bridge".into()],
        }),
    }
}

fn state(plan: TurnPlan) -> (AcpState, SharedState, mpsc::UnboundedReceiver<String>) {
    let shared: SharedState = std::sync::Arc::new(std::sync::Mutex::new(Shared::default()));
    let (tx, rx) = mpsc::unbounded_channel();
    (AcpState::new(plan, shared.clone(), tx), shared, rx)
}

fn sent(rx: &mut mpsc::UnboundedReceiver<String>) -> Vec<Value> {
    std::iter::from_fn(|| rx.try_recv().ok())
        .map(|line| serde_json::from_str(&line).expect("one JSON message per line"))
        .collect()
}

#[test]
fn listing_hides_orchestrators_and_folds_effort_variants() {
    let variants = parse_model_list(LISTING);
    assert_eq!(
        variants.len(),
        9,
        "the progress line and orchestrator are left out"
    );
    assert!(variants.iter().all(|id| !id.contains("orchestrator")));
    let models = engine().models();
    assert_eq!(
        models,
        [
            "gemini-4-neon",
            "gemini-4-carbon",
            "gemini-3.8-flash",
            "claude-opus-5-5",
            "gpt-oss-120b"
        ]
    );
    let unread = AntigravityEngine::new("agy", "1.5.18").models();
    assert_eq!(unread, FALLBACK_MODELS);
}

#[test]
fn effort_picks_the_highest_listed_variant_at_or_below_it() {
    let engine = engine();
    assert_eq!(
        engine.variant_for("gemini-4-neon", Some(Effort::XHigh)),
        "gemini-4-neon-xhigh"
    );
    assert_eq!(
        engine.variant_for("gemini-4-neon", Some(Effort::Max)),
        "gemini-4-neon-xhigh"
    );
    assert_eq!(
        engine.variant_for("gemini-4-neon", Some(Effort::Medium)),
        "gemini-4-neon-low"
    );
    assert_eq!(
        engine.variant_for("gemini-4-carbon", Some(Effort::XHigh)),
        "gemini-4-carbon-high"
    );
    // Below every listed variant: the lowest one listed.
    assert_eq!(
        engine.variant_for("gemini-4-carbon", Some(Effort::Low)),
        "gemini-4-carbon-medium"
    );
    assert_eq!(
        engine.variant_for("gemini-3.8-flash", None),
        "gemini-3.8-flash-low"
    );
    assert_eq!(
        engine.variant_for("gemini-3.8-flash-high", Some(Effort::Low)),
        "gemini-3.8-flash-low"
    );
    // Unlisted family: agy itself names what it lacks.
    assert_eq!(
        engine.variant_for("gemini-4-argon", Some(Effort::High)),
        "gemini-4-argon-high"
    );
}

#[test]
fn flags_follow_the_approval_policy_and_extra_roots() {
    let mut t = turn();
    assert_eq!(args(&t), ["acp"]);
    t.bypass_permissions = true;
    t.extra_dirs = vec![PathBuf::from("/srv/shared")];
    assert_eq!(
        args(&t),
        [
            "acp",
            "--dangerously-skip-permissions",
            "--add-dir",
            "/srv/shared"
        ]
    );
    t.summary_only = true;
    assert!(!args(&t).contains(&"--dangerously-skip-permissions".to_owned()));
}

#[test]
fn plan_carries_the_bridge_model_variant_and_system_text() {
    let mut t = turn();
    t.system_prompt = "be brief".into();
    let plan = turn_plan(&engine(), &t);
    assert_eq!(plan.model.as_deref(), Some("gemini-3.8-flash-low"));
    assert_eq!(plan.mcp_servers[0]["name"], "aether");
    assert_eq!(plan.mcp_servers[0]["args"], json!(["mcp-bridge"]));
    assert_eq!(plan.prompt[0]["text"], "be brief\n\nhi");
    assert!(!plan.deny_all);
    t.summary_only = true;
    let summary = turn_plan(&engine(), &t);
    assert!(summary.mcp_servers.is_empty() && summary.deny_all);
}

#[test]
fn network_denied_and_compaction_turns_are_refused() {
    assert!(engine().validate_network_policy(true).is_ok());
    assert!(matches!(
        engine().validate_network_policy(false),
        Err(EngineError::Unsupported(_))
    ));
    assert!(!engine().compacts_natively());
    assert_eq!(engine().adapter(), "antigravity/1.5.18");
}

/// The recorded turn: handshake, an MCP tool behind a permission prompt,
/// streamed text, and the usage the prompt result reports.
#[test]
fn recorded_turn_maps_to_engine_events() {
    let (mut state, shared, mut rx) = state(turn_plan(&engine(), &turn()));
    let events: Vec<EngineEvent> = FIXTURE.lines().flat_map(|l| state.parse_line(l)).collect();
    let requests = sent(&mut rx);
    assert_eq!(requests[0]["method"], "session/set_model");
    assert_eq!(requests[0]["params"]["modelId"], "gemini-3.8-flash-low");
    assert_eq!(requests[1]["method"], "session/prompt");
    assert_eq!(requests[1]["params"]["sessionId"], "ses-fixture");
    assert!(matches!(&events[0], EngineEvent::Session { id, .. } if id == "ses-fixture"));
    let tool_use = events.iter().find_map(|e| match e {
        EngineEvent::Assistant { content, .. } => match &content[0] {
            ContentBlock::ToolUse { id, name, .. } => Some((id.clone(), name.clone())),
            _ => None,
        },
        _ => None,
    });
    assert_eq!(
        tool_use,
        Some(("call-fixture-1".into(), "mcp_aetherprobe_ping".into()))
    );
    let permission = events.iter().find_map(|e| match e {
        EngineEvent::Permission {
            request_id, tool, ..
        } => Some((request_id.clone(), tool.clone())),
        _ => None,
    });
    assert_eq!(
        permission,
        Some(("\"1\"".into(), "mcp_aetherprobe_ping".into()))
    );
    assert_eq!(
        lock_open(&shared),
        0,
        "the closing result clears open requests"
    );
    let results = events
        .iter()
        .filter(|e| matches!(e, EngineEvent::ToolResult { .. }));
    assert_eq!(results.count(), 1, "one result per tool call");
    assert!(
        events
            .iter()
            .any(|e| matches!(e, EngineEvent::ContextUsage { tokens: 10589 }))
    );
    let streamed: String = events
        .iter()
        .filter_map(|e| match e {
            EngineEvent::Delta(StreamEvent::TextDelta { text, .. }) => Some(text.as_str()),
            _ => None,
        })
        .collect();
    assert!(streamed.ends_with("pong-7731"));
    let Some(EngineEvent::Done(outcome)) = events.last() else {
        panic!("the turn ends with Done: {events:?}");
    };
    assert!(outcome.closing_accounting && !outcome.is_error);
    assert_eq!(outcome.usage.input_tokens, 12564);
    assert_eq!(outcome.usage.output_tokens, 91);
    assert_eq!(outcome.usage.cache_read_tokens, 8373);
    assert_eq!(outcome.session.as_deref(), Some("ses-fixture"));
    assert!(
        outcome
            .text
            .as_deref()
            .is_some_and(|t| t.ends_with("pong-7731"))
    );
}

fn lock_open(shared: &SharedState) -> usize {
    acp::lock(shared).open.len()
}

fn permission_line(id: &str, name: &str, kind: &str) -> String {
    json!({"jsonrpc": "2.0", "id": id, "method": "session/request_permission", "params": {
        "sessionId": "s", "toolCall": {"toolCallId": "c", "name": name, "kind": kind, "rawInput": {"a": 1}},
        "options": [{"optionId": "yes", "kind": "allow_once"}, {"optionId": "no", "kind": "reject_once"}]
    }})
    .to_string()
}

#[test]
fn decisions_answer_the_open_request_once() {
    let (mut state, shared, _rx) = state(turn_plan(&engine(), &turn()));
    let events = state.parse_line(&permission_line("9", "mcp_aether_complete_task", "other"));
    let EngineEvent::Permission {
        request_id,
        tool,
        side_effect,
        ..
    } = &events[0]
    else {
        panic!("a permission request asks the runtime: {events:?}");
    };
    assert_eq!(
        tool, "mcp__aether__complete_task",
        "harness tools keep the runtime's names"
    );
    assert_eq!(*side_effect, None, "the runtime classes its own tools");
    let decide = |allow| PermissionDecision {
        request_id: request_id.clone(),
        allow,
        message: None,
        answer: None,
    };
    let reply: Value =
        serde_json::from_str(&acp::decision_line(&shared, &decide(false)).unwrap()).unwrap();
    assert_eq!(reply["id"], "9");
    assert_eq!(
        reply["result"]["outcome"],
        json!({"outcome": "selected", "optionId": "no"})
    );
    assert!(
        acp::decision_line(&shared, &decide(true)).is_none(),
        "answered once"
    );
    let events = state.parse_line(&permission_line("10", "run_command", "execute"));
    assert!(matches!(
        &events[0],
        EngineEvent::Permission {
            side_effect: Some(aether_core::tool::SideEffectClass::External),
            ..
        }
    ));
    let allow = PermissionDecision {
        request_id: "\"10\"".into(),
        ..decide(true)
    };
    let reply: Value = serde_json::from_str(&acp::decision_line(&shared, &allow).unwrap()).unwrap();
    assert_eq!(reply["result"]["outcome"]["optionId"], "yes");
}

#[test]
fn summary_turns_refuse_every_permission_without_asking() {
    let mut t = turn();
    t.summary_only = true;
    let (mut state, shared, mut rx) = state(turn_plan(&engine(), &t));
    let events = state.parse_line(&permission_line("3", "write_to_file", "edit"));
    assert!(
        events
            .iter()
            .all(|e| !matches!(e, EngineEvent::Permission { .. }))
    );
    assert_eq!(lock_open(&shared), 0);
    let reply = &sent(&mut rx)[0];
    assert_eq!(reply["result"]["outcome"]["optionId"], "no");
}

#[test]
fn a_missing_session_is_reported_lost_before_the_prompt() {
    let mut t = turn();
    t.session = Some("gone".into());
    let plan = turn_plan(&engine(), &t);
    assert!(acp::opening_lines(&plan).contains("session/load"));
    let (mut state, _shared, mut rx) = state(plan);
    let events = state.parse_line(
        r#"{"jsonrpc":"2.0","id":2,"error":{"code":-32002,"message":"Resource not found: gone"}}"#,
    );
    let Some(EngineEvent::Done(outcome)) = events.last() else {
        panic!("a lost session ends the turn: {events:?}");
    };
    assert!(outcome.session_lost && outcome.prompt_not_submitted && outcome.is_error);
    assert!(sent(&mut rx).is_empty(), "no prompt follows a failed load");
    assert!(
        state
            .parse_line(r#"{"jsonrpc":"2.0","id":4,"result":{}}"#)
            .is_empty()
    );
}

#[test]
fn a_resumed_history_replay_is_not_this_turns_output() {
    let mut t = turn();
    t.session = Some("s0".into());
    let (mut state, _shared, mut rx) = state(turn_plan(&engine(), &t));
    let replay = r#"{"jsonrpc":"2.0","method":"session/update","params":{"sessionId":"s0","update":{"sessionUpdate":"agent_message_chunk","content":{"type":"text","text":"old"}}}}"#;
    assert!(
        state.parse_line(replay).is_empty(),
        "history replays before the load answers"
    );
    let events = state.parse_line(r#"{"jsonrpc":"2.0","id":2,"result":{}}"#);
    assert!(matches!(&events[0], EngineEvent::Session { id, .. } if id == "s0"));
    assert_eq!(sent(&mut rx)[0]["method"], "session/set_model");
    let fresh = replay.replace("old", "new");
    assert_eq!(
        state.parse_line(&fresh).len(),
        1,
        "updates after the load are live"
    );
    let events = state.parse_line(r#"{"jsonrpc":"2.0","id":4,"result":{"stopReason":"end_turn","usage":{"inputTokens":1,"outputTokens":1}}}"#);
    let Some(EngineEvent::Done(outcome)) = events.last() else {
        panic!("{events:?}");
    };
    assert_eq!(outcome.text.as_deref(), Some("new"));
}

#[test]
fn cancel_and_refusal_end_the_turn_as_errors() {
    let (mut state, shared, _rx) = state(turn_plan(&engine(), &turn()));
    assert!(
        acp::cancel_line(&shared).is_none(),
        "nothing to cancel before a session"
    );
    state.parse_line(r#"{"jsonrpc":"2.0","id":2,"result":{"sessionId":"s1"}}"#);
    let cancel: Value = serde_json::from_str(&acp::cancel_line(&shared).unwrap()).unwrap();
    assert_eq!(cancel["method"], "session/cancel");
    assert_eq!(cancel["params"]["sessionId"], "s1");
    assert!(
        cancel.get("id").is_none(),
        "session/cancel is a notification"
    );
    let events =
        state.parse_line(r#"{"jsonrpc":"2.0","id":4,"result":{"stopReason":"cancelled"}}"#);
    let Some(EngineEvent::Done(outcome)) = events.last() else {
        panic!("{events:?}");
    };
    assert!(outcome.interrupted && outcome.is_error && !outcome.closing_accounting);
    assert!(
        events
            .iter()
            .any(|e| matches!(e, EngineEvent::ProtocolNote(_)))
    );
}

#[test]
fn unsupported_client_requests_are_refused() {
    let (mut state, _shared, mut rx) = state(turn_plan(&engine(), &turn()));
    let events =
        state.parse_line(r#"{"jsonrpc":"2.0","id":5,"method":"fs/read_text_file","params":{}}"#);
    assert!(events.is_empty());
    let reply = &sent(&mut rx)[0];
    assert_eq!(reply["id"], 5);
    assert_eq!(reply["error"]["code"], -32601);
}

/// A fake `agy acp` answering each request as the real server does, logging
/// what it was sent; it exits when its stdin closes.
const FAKE_ACP: &str = r#"#!/bin/sh
case "$1" in
  --version) echo 1.5.18; exit 0;;
  models) printf 'Fetching available models...\ngemini-3.8-flash-low\tGemini 3.8 Flash (Low)\n'; exit 0;;
esac
while IFS= read -r line; do
  printf '%s\n' "$line" >> requests.log
  case "$line" in
    *'"method":"initialize"'*) echo '{"jsonrpc":"2.0","id":1,"result":{"protocolVersion":1}}';;
    *'"method":"session/new"'*) echo '{"jsonrpc":"2.0","id":2,"result":{"sessionId":"ses-fake"}}';;
    *'"method":"session/set_model"'*) echo '{"jsonrpc":"2.0","id":3,"result":{}}';;
    *'"method":"session/prompt"'*)
      echo '{"jsonrpc":"2.0","method":"session/update","params":{"sessionId":"ses-fake","update":{"sessionUpdate":"agent_message_chunk","content":{"type":"text","text":"ok"}}}}'
      echo '{"jsonrpc":"2.0","id":"7","method":"session/request_permission","params":{"sessionId":"ses-fake","toolCall":{"toolCallId":"c1","name":"run_command","kind":"execute"},"options":[{"optionId":"allow","kind":"allow_once"},{"optionId":"deny","kind":"reject_once"}]}}';;
    *'"id":"7"'*) echo '{"jsonrpc":"2.0","id":4,"result":{"stopReason":"end_turn","usage":{"inputTokens":5,"outputTokens":2,"cachedReadTokens":1}}}';;
  esac
done
"#;

#[tokio::test]
async fn launched_turn_answers_permissions_and_exits_when_done() {
    let dir = tempfile::tempdir().unwrap();
    let program = dir.path().join("agy");
    crate::test_script::write(&program, FAKE_ACP);
    let engine = AntigravityEngine::new(program.to_string_lossy(), "1.5.18")
        .with_variants(vec!["gemini-3.8-flash-low".into()]);
    let recorder = std::sync::Arc::new(crate::ownership_tests::Recorder::default());
    let mut t = turn();
    t.cwd = dir.path().into();
    t.session_recorder = Some(recorder.clone());
    let mut run = engine.run(t, CancellationToken::new()).await.unwrap();
    let decisions = run.decisions.take().expect("ACP asks through decisions");
    let outcome = tokio::time::timeout(Duration::from_secs(10), async {
        while let Some(event) = run.events.recv().await {
            match event {
                EngineEvent::Permission { request_id, .. } => {
                    let allow = PermissionDecision {
                        request_id,
                        allow: true,
                        message: None,
                        answer: None,
                    };
                    decisions.send(allow).await.unwrap();
                }
                EngineEvent::Done(outcome) => return outcome,
                _ => {}
            }
        }
        panic!("the turn ended without Done");
    })
    .await
    .expect("the turn finishes in seconds");
    assert!(!outcome.is_error, "{outcome:?}");
    assert_eq!(outcome.usage.input_tokens, 5);
    assert_eq!(outcome.text.as_deref(), Some("ok"));
    let closed = tokio::time::timeout(Duration::from_secs(10), run.events.recv()).await;
    assert!(
        matches!(closed, Ok(None)),
        "agy acp exits once the turn is over"
    );
    let records = recorder.records.lock().unwrap();
    assert_eq!(records.len(), 1);
    assert_eq!(
        (records[0].engine, records[0].id.as_str()),
        (EngineKind::ANTIGRAVITY, "ses-fake")
    );
    let log = std::fs::read_to_string(dir.path().join("requests.log")).unwrap();
    assert!(log.contains(r#""modelId":"gemini-3.8-flash-low""#));
    assert!(log.contains(r#""optionId":"allow""#));
}

#[tokio::test]
async fn detection_reads_version_and_signed_in_listing() {
    let dir = tempfile::tempdir().unwrap();
    crate::test_script::write(&dir.path().join("agy"), FAKE_ACP);
    let engine = AntigravityEngine::detect_verbose_on(Some(dir.path()))
        .await
        .unwrap();
    assert_eq!(engine.adapter(), "antigravity/1.5.18");
    assert_eq!(engine.models(), ["gemini-3.8-flash"]);
    let signed_out = tempfile::tempdir().unwrap();
    crate::test_script::write(
        &signed_out.path().join("agy"),
        "#!/bin/sh\n[ \"$1\" = --version ] && { echo 1.5.18; exit 0; }\necho 'Please sign in' >&2\nexit 1\n",
    );
    let error = AntigravityEngine::detect_verbose_on(Some(signed_out.path()))
        .await
        .unwrap_err();
    assert!(
        matches!(error, DetectError::MissingCapability { .. }),
        "{error}"
    );
    assert!(error.to_string().contains("signed-in"));
}

/// One real turn on the cheapest model. Opt in with
/// `cargo test -p aether-engine antigravity_live -- --ignored`.
#[tokio::test]
#[ignore = "runs a real agy turn on the operator's subscription"]
async fn antigravity_live_tiny_turn() {
    // Test builds bound probes at two seconds, too short for `agy models`.
    let engine =
        AntigravityEngine::new("agy", "1.5.18").with_variants(vec!["gemini-3.8-flash-low".into()]);
    let dir = tempfile::tempdir().unwrap();
    let mut t = turn();
    t.cwd = dir.path().into();
    t.mcp = None;
    t.prompt = "Reply with just the word: pong".into();
    let mut run = engine.run(t, CancellationToken::new()).await.unwrap();
    let outcome = tokio::time::timeout(Duration::from_secs(120), async {
        while let Some(event) = run.events.recv().await {
            if let EngineEvent::Done(outcome) = event {
                return outcome;
            }
        }
        panic!("no Done");
    })
    .await
    .expect("a tiny turn finishes");
    assert!(!outcome.is_error, "{outcome:?}");
    assert!(outcome.closing_accounting && outcome.usage.input_tokens > 0);
    assert!(
        outcome
            .text
            .unwrap_or_default()
            .to_lowercase()
            .contains("pong")
    );
}
