//! Drives the third-party engine from `examples/line_engine.rs` through the
//! `Engine` trait, so the documented example is held to the contract it
//! documents rather than merely compiling.

// The example's `main` is its demo entry point; the test drives the engine.
#[allow(dead_code)]
#[path = "../examples/line_engine.rs"]
mod example;

use std::path::PathBuf;

use aether_engine::{Engine, EngineEvent, EngineOutcome, EngineTurn, PermissionDecision};
use example::{KIND, LineEngine};
use tokio_util::sync::CancellationToken;

fn fake_cli() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("examples/fake_line_cli.sh")
}

fn turn(cwd: PathBuf) -> EngineTurn {
    EngineTurn {
        prompt: "list the workspace".into(),
        images: Vec::new(),
        system_prompt: "You are inside Aether.".into(),
        session: None,
        session_usage: aether_core::model::Usage::default(),
        new_session_id: "sess-1".into(),
        session_recorder: None,
        model: Some("demo".into()),
        effort: None,
        cwd,
        extra_dirs: Vec::new(),
        summary_only: false,
        lean_host_context: false,
        network_access: true,
        bypass_permissions: false,
        hidden_env: vec!["ANTHROPIC_API_KEY".into()],
        tool_hidden_env: vec!["ANTHROPIC_API_KEY".into()],
        mcp: None,
    }
}

type Failure = Box<dyn std::error::Error>;

/// Collect every event, answering permission prompts with `allow`.
async fn drive(allow: bool) -> Result<(Vec<EngineEvent>, EngineOutcome), Failure> {
    let workspace = tempfile::tempdir()?;
    let engine = LineEngine::new(fake_cli(), "0.1.0");
    let mut run = engine
        .run(
            turn(workspace.path().to_path_buf()),
            CancellationToken::new(),
        )
        .await?;
    let decisions = run
        .decisions
        .take()
        .ok_or("this engine answers permission prompts")?;
    let mut events = Vec::new();
    let mut outcome = None;
    while let Some(event) = run.events.recv().await {
        if let EngineEvent::Permission { request_id, .. } = &event {
            decisions
                .send(PermissionDecision {
                    request_id: request_id.clone(),
                    allow,
                    message: None,
                    answer: None,
                })
                .await?;
        }
        if let EngineEvent::Done(done) = &event {
            outcome = Some(done.clone());
        }
        events.push(event);
    }
    let outcome = outcome.ok_or("Done is always the last event")?;
    Ok((events, outcome))
}

#[tokio::test]
async fn a_third_party_engine_satisfies_the_contract() {
    let (events, outcome) = drive(true).await.expect("drive the engine");

    assert!(
        matches!(&events[0], EngineEvent::Session { id, model, .. }
            if id == "sess-1" && model.as_deref() == Some("demo")),
        "the session and what it loaded come first: {:?}",
        events[0]
    );
    assert!(
        events.iter().any(|e| matches!(e, EngineEvent::Delta(_))),
        "text streams live, not only at the end"
    );
    assert!(events.iter().any(|e| matches!(
        e,
        EngineEvent::Assistant { content, .. } if !content.is_empty()
    )));
    assert!(events.iter().any(|e| matches!(
        e,
        EngineEvent::Permission { tool, .. } if tool == "bash"
    )));
    let result = events
        .iter()
        .find_map(|e| match e {
            EngineEvent::ToolResult {
                content, is_error, ..
            } => Some((content.clone(), *is_error)),
            _ => None,
        })
        .expect("the engine reports what its own tool did");
    assert_eq!(result, ("README.md".to_owned(), false));
    assert!(
        matches!(events.last(), Some(EngineEvent::Done(_))),
        "Done is always the last event"
    );
    assert_eq!(outcome.usage.input_tokens, 11);
    assert_eq!(outcome.usage.output_tokens, 4);
    assert_eq!(outcome.session.as_deref(), Some("sess-1"));
    assert!(!outcome.is_error);

    assert_eq!(KIND.as_str(), "line-cli", "an engine names itself");
    let engine = LineEngine::new(fake_cli(), "0.1.0");
    assert_eq!(engine.adapter(), "line-cli/0.1.0");
    assert!(engine.gates_permissions());
}

/// A refused call must reach the engine as a refusal, not as a silent allow.
#[tokio::test]
async fn a_denied_permission_reaches_the_engine() {
    let (events, outcome) = drive(false).await.expect("drive the engine");
    assert!(
        !outcome.is_error,
        "a refusal ends the turn, it does not break it"
    );
    let result = events
        .iter()
        .find_map(|e| match e {
            EngineEvent::ToolResult {
                content, is_error, ..
            } => Some((content.clone(), *is_error)),
            _ => None,
        })
        .expect("tool result");
    assert_eq!(result, ("refused".to_owned(), true));
}

/// A cancelled turn still ends with `Done`, so the runtime is never left
/// waiting on a stream that stopped.
#[tokio::test]
async fn cancellation_still_produces_a_final_outcome() {
    let workspace = tempfile::tempdir().expect("tempdir");
    let engine = LineEngine::new(fake_cli(), "0.1.0");
    let cancel = CancellationToken::new();
    let mut run = engine
        .run(turn(workspace.path().to_path_buf()), cancel.clone())
        .await
        .expect("start");
    cancel.cancel();
    let mut last = None;
    while let Some(event) = run.events.recv().await {
        last = Some(event);
    }
    assert!(
        matches!(last, Some(EngineEvent::Done(_))),
        "the stream ends with a verdict even when the turn was cut short"
    );
}
