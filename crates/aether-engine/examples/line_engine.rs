//! A third-party engine, complete and out of tree.
//!
//! `LineEngine` drives a CLI that speaks newline-delimited JSON on stdout and
//! reads Aether's answers on stdin. Nothing here is special-cased inside
//! Aether: it is exactly what a Gemini CLI or OpenCode adapter would write in
//! its own crate. `docs/engines.md` walks through it.
//!
//! Run it against the scripted fake CLI the tests use:
//!
//! ```sh
//! cargo run -p aether-engine --example line_engine -- \
//!     crates/aether-engine/examples/fake_line_cli.sh
//! ```

use std::path::PathBuf;

use aether_core::model::{ContentBlock, StreamEvent, Usage};
use aether_engine::{
    Engine, EngineError, EngineEvent, EngineKind, EngineOutcome, EngineRun, EngineTurn,
    PermissionDecision,
};
use serde_json::Value;
use tokio::io::{AsyncBufReadExt as _, AsyncWriteExt as _};
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;

/// The name this engine answers to, everywhere Aether records an engine.
pub const KIND: EngineKind = EngineKind::new("line-cli");

#[derive(Debug)]
pub struct LineEngine {
    program: PathBuf,
    version: String,
}

impl LineEngine {
    pub fn new(program: impl Into<PathBuf>, version: impl Into<String>) -> Self {
        Self {
            program: program.into(),
            version: version.into(),
        }
    }
}

#[async_trait::async_trait]
impl Engine for LineEngine {
    fn kind(&self) -> EngineKind {
        KIND
    }

    fn adapter(&self) -> String {
        format!("{}/{}", KIND.as_str(), self.version)
    }

    /// This CLI asks before acting, so Aether's approval policy is honoured.
    /// An engine that cannot gate its own tools returns false and says so
    /// rather than letting the runtime pretend it is gated.
    fn gates_permissions(&self) -> bool {
        true
    }

    async fn run(
        &self,
        turn: EngineTurn,
        cancel: CancellationToken,
    ) -> Result<EngineRun, EngineError> {
        let program = self.program.display().to_string();
        let mut command = tokio::process::Command::new(&self.program);
        command
            .current_dir(&turn.cwd)
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::piped())
            .kill_on_drop(true);
        if let Some(session) = &turn.session {
            command.arg("--resume").arg(session);
        } else {
            command.arg("--session").arg(&turn.new_session_id);
        }
        if let Some(model) = &turn.model {
            command.arg("--model").arg(model);
        }
        // The engine drives its own shell: a credential left in the
        // environment would be one `printenv` away.
        for name in &turn.hidden_env {
            command.env_remove(name);
        }

        let mut child = command.spawn().map_err(|e| match e.kind() {
            std::io::ErrorKind::NotFound => EngineError::NotInstalled {
                program: program.clone(),
            },
            _ => EngineError::Spawn {
                program: program.clone(),
                source: e,
            },
        })?;
        let mut stdin = child
            .stdin
            .take()
            .ok_or_else(|| EngineError::Exited("no stdin".into()))?;
        let stdout = child
            .stdout
            .take()
            .ok_or_else(|| EngineError::Exited("no stdout".into()))?;

        let (events_tx, events_rx) = mpsc::channel(64);
        let (decisions_tx, mut decisions_rx) = mpsc::channel::<PermissionDecision>(8);

        let prompt = serde_json::json!({
            "prompt": turn.prompt,
            "system": turn.system_prompt,
        });
        let prompt_events = events_tx.clone();
        tokio::spawn(async move {
            // A prompt that never reached the child would otherwise surface as
            // the generic "ended without a result"; say what happened instead.
            let sent = async {
                stdin.write_all(format!("{prompt}\n").as_bytes()).await?;
                stdin.flush().await
            }
            .await;
            if let Err(e) = sent {
                let _ = prompt_events
                    .send(EngineEvent::Note(format!("could not send the prompt: {e}")))
                    .await;
                return;
            }
            // Released before the decision loop, which outlives the stream:
            // holding a sender here would keep the event channel open forever.
            drop(prompt_events);
            while let Some(decision) = decisions_rx.recv().await {
                let line = serde_json::json!({
                    "id": decision.request_id,
                    "allow": decision.allow,
                    "answer": decision.answer,
                });
                if stdin
                    .write_all(format!("{line}\n").as_bytes())
                    .await
                    .is_err()
                {
                    break;
                }
                let _ = stdin.flush().await;
            }
        });

        tokio::spawn(async move {
            let mut lines = tokio::io::BufReader::new(stdout).lines();
            let mut outcome = None;
            loop {
                let line = tokio::select! {
                    _ = cancel.cancelled() => {
                        // Cancellation must not strand the child; killing it
                        // ends the stream and the turn reports what it had.
                        let _ = child.start_kill();
                        break;
                    }
                    next = lines.next_line() => match next {
                        Ok(Some(line)) => line,
                        Ok(None) | Err(_) => break,
                    },
                };
                for event in parse(&line) {
                    if let EngineEvent::Done(done) = &event {
                        outcome = Some(done.clone());
                    }
                    if events_tx.send(event).await.is_err() {
                        return;
                    }
                }
            }
            let _ = child.wait().await;
            // `Done` is always the last event, even when the CLI died first.
            let done = outcome.unwrap_or(EngineOutcome {
                is_error: true,
                error: Some("line-cli ended without a result".into()),
                ..Default::default()
            });
            let _ = events_tx.send(EngineEvent::Done(done)).await;
        });

        Ok(EngineRun {
            events: events_rx,
            input: None,
            decisions: Some(decisions_tx),
            stop: None,
        })
    }
}

/// One CLI line to zero or more normalized events. An unknown `type` is a
/// note rather than an error: a newer CLI may say more than this adapter
/// understands, and dropping the turn over it would be worse.
fn parse(line: &str) -> Vec<EngineEvent> {
    let trimmed = line.trim();
    if trimmed.is_empty() {
        return Vec::new();
    }
    let Ok(value) = serde_json::from_str::<Value>(trimmed) else {
        return vec![EngineEvent::Note(format!("unparsed line: {trimmed}"))];
    };
    let text = |key: &str| value.get(key).and_then(Value::as_str).map(str::to_owned);
    let tokens = |key: &str| value.get(key).and_then(Value::as_u64).unwrap_or(0);
    match value.get("type").and_then(Value::as_str) {
        Some("session") => vec![EngineEvent::Session {
            id: text("id").unwrap_or_default(),
            model: text("model"),
            tools: Vec::new(),
        }],
        Some("text") => {
            let text = text("text").unwrap_or_default();
            vec![
                EngineEvent::Delta(StreamEvent::TextDelta {
                    index: 0,
                    text: text.clone(),
                }),
                EngineEvent::Assistant {
                    content: vec![ContentBlock::Text { text }],
                    usage: None,
                    model: None,
                },
            ]
        }
        Some("permission") => vec![EngineEvent::Permission {
            request_id: text("id").unwrap_or_default(),
            tool: text("tool").unwrap_or_default(),
            input: value.get("input").cloned().unwrap_or(Value::Null),
            // The CLI does not classify its tools, so the runtime infers the
            // side-effect class from the tool name.
            side_effect: None,
        }],
        Some("tool_result") => vec![EngineEvent::ToolResult {
            tool_use_id: text("id").unwrap_or_default(),
            content: text("content").unwrap_or_default(),
            is_error: value.get("error").and_then(Value::as_bool).unwrap_or(false),
        }],
        Some("done") => vec![EngineEvent::Done(EngineOutcome {
            usage: Usage {
                input_tokens: tokens("input_tokens"),
                output_tokens: tokens("output_tokens"),
                ..Default::default()
            },
            text: text("text"),
            session: text("session"),
            ..Default::default()
        })],
        _ => vec![EngineEvent::Note(format!("unknown event: {trimmed}"))],
    }
}

/// A one-turn demo against the scripted CLI. The test drives the same engine
/// through the trait; this is what a person runs to watch it work.
#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let Some(program) = std::env::args().nth(1) else {
        return Err("usage: line_engine <path to the line-cli program>".into());
    };
    let engine = LineEngine::new(&program, "0.1.0");
    let turn = EngineTurn {
        prompt: "say hello".into(),
        images: Vec::new(),
        system_prompt: "You are inside Aether.".into(),
        session: None,
        session_usage: Usage::default(),
        session_model_usage: None,
        new_session_id: "sess-1".into(),
        session_recorder: None,
        model: Some("demo".into()),
        effort: None,
        cwd: std::env::current_dir()?,
        extra_dirs: Vec::new(),
        summary_only: false,
        lean_host_context: false,
        network_access: true,
        bypass_permissions: false,
        hidden_env: Vec::new(),
        tool_hidden_env: Vec::new(),
        temp_dir: None,
        mcp: None,
    };
    let mut run = engine.run(turn, CancellationToken::new()).await?;
    let decisions = run
        .decisions
        .take()
        .ok_or("this engine answers permission prompts")?;
    while let Some(event) = run.events.recv().await {
        println!("{event:?}");
        if let EngineEvent::Permission { request_id, .. } = &event {
            decisions
                .send(PermissionDecision {
                    request_id: request_id.clone(),
                    allow: true,
                    message: None,
                    answer: None,
                })
                .await?;
        }
        if matches!(event, EngineEvent::Done(_)) {
            break;
        }
    }
    Ok(())
}
