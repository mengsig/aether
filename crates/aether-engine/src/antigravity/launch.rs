//! Start `agy acp` for one turn and wire its stdin to the runtime's channels.

use std::time::Duration;

use serde_json::{Value, json};
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;

use super::AntigravityEngine;
use super::acp::{self, AcpState, Shared, SharedState, TurnPlan};
use crate::{
    Engine, EngineError, EngineEvent, EngineLaunchContext, EngineOutcome, EngineRun, EngineTurn,
    PermissionDecision, run_process_with_env,
};

pub(super) async fn launch(
    engine: &AntigravityEngine,
    turn: EngineTurn,
    context: Option<EngineLaunchContext>,
    cancel: CancellationToken,
) -> Result<EngineRun, EngineError> {
    if turn.compact_session {
        return Err(EngineError::Unsupported(
            "Antigravity has no native compaction Aether can drive".into(),
        ));
    }
    if !turn.summary_only {
        engine.validate_network_policy(turn.network_access)?;
    }
    let plan = turn_plan(engine, &turn);
    let shared: SharedState = std::sync::Arc::new(std::sync::Mutex::new(Shared::default()));
    let (out_tx, out_rx) = mpsc::unbounded_channel::<String>();
    let finished = CancellationToken::new();
    let parse = parser(
        AcpState::new(plan.clone(), shared.clone(), out_tx.clone()),
        turn.clone(),
        finished.clone(),
        cancel.clone(),
    );
    let process = run_process_with_env(
        &engine.program,
        args(&turn),
        turn.cwd.clone(),
        &turn.hidden_env,
        turn.temp_dir.as_deref(),
        &turn.build_env,
        &[],
        context.as_ref(),
        &turn.redactor,
        acp::opening_lines(&plan),
        true,
        None,
        cancel.clone(),
        parse,
    )
    .await?;
    let Some(stdin) = process.stdin else {
        return Err(EngineError::Exited("stdin was not kept open".into()));
    };
    tokio::spawn(forward(out_rx, stdin, finished.clone(), cancel.clone()));
    let decisions = spawn_decisions(shared.clone(), out_tx.clone(), finished.clone());
    let stop = spawn_stop(shared, out_tx, finished);
    Ok(EngineRun {
        events: process.events,
        input: None,
        decisions: Some(decisions),
        stop: Some(stop),
    })
}

/// `agy acp`'s flags: approvals off when the agent runs freely, and the
/// agent's extra roots as workspace directories.
pub(super) fn args(turn: &EngineTurn) -> Vec<String> {
    let mut args = vec!["acp".to_owned()];
    if turn.bypass_permissions && !turn.summary_only {
        args.push("--dangerously-skip-permissions".into());
    }
    for dir in &turn.extra_dirs {
        args.push("--add-dir".into());
        args.push(dir.to_string_lossy().into_owned());
    }
    args
}

pub(super) fn turn_plan(engine: &AntigravityEngine, turn: &EngineTurn) -> TurnPlan {
    let model = turn
        .model
        .as_deref()
        .filter(|m| *m != "default")
        .map(|m| engine.variant_for(m, turn.effort));
    let text = match turn.system_prompt.trim() {
        "" => turn.prompt.clone(),
        system => format!("{system}\n\n{}", turn.prompt),
    };
    let mut prompt = vec![json!({"type": "text", "text": text})];
    prompt.extend(turn.images.iter().filter_map(image_block));
    // A summary turn runs with no harness tools at all.
    let mcp_servers = match (&turn.mcp, turn.summary_only) {
        (Some(mcp), false) => vec![json!({
            "name": "aether",
            "command": mcp.command.to_string_lossy(),
            "args": mcp.args,
            "env": [],
        })],
        _ => Vec::new(),
    };
    TurnPlan {
        cwd: turn.cwd.to_string_lossy().into_owned(),
        resume: turn.session.clone(),
        model,
        prompt,
        mcp_servers,
        deny_all: turn.summary_only,
    }
}

/// An image as an ACP content block. One whose file cannot be read is left
/// out: the message text names the stored file either way.
fn image_block(image: &crate::EngineImage) -> Option<Value> {
    use base64::Engine as _;
    match std::fs::read(&image.path) {
        Ok(bytes) => Some(json!({
            "type": "image",
            "mimeType": image.media_type,
            "data": base64::engine::general_purpose::STANDARD.encode(bytes),
        })),
        Err(error) => {
            tracing::warn!(path = %image.path.display(), %error, "image left out of an Antigravity message");
            None
        }
    }
}

/// The line parser, recording a fresh session's ownership as soon as the
/// server names it; a failed record stops the turn before its prompt runs.
fn parser(
    mut state: AcpState,
    turn: EngineTurn,
    finished: CancellationToken,
    cancel: CancellationToken,
) -> impl FnMut(&str) -> Vec<EngineEvent> + Send + 'static {
    move |line| {
        let events = state.parse_line(line);
        for event in &events {
            match event {
                EngineEvent::Session { id, .. } => {
                    if let Err(error) = turn.record_new_session(turn_kind(), id, Vec::new()) {
                        cancel.cancel();
                        finished.cancel();
                        return vec![EngineEvent::Done(EngineOutcome {
                            is_error: true,
                            error: Some(error.to_string()),
                            prompt_not_submitted: true,
                            ..Default::default()
                        })];
                    }
                }
                EngineEvent::Done(_) => finished.cancel(),
                _ => {}
            }
        }
        events
    }
}

fn turn_kind() -> crate::EngineKind {
    crate::EngineKind::ANTIGRAVITY
}

/// Write queued requests to the server until the turn is over; dropping
/// `stdin` then closes the server's input, which ends `agy acp`.
async fn forward(
    mut lines: mpsc::UnboundedReceiver<String>,
    stdin: mpsc::Sender<String>,
    finished: CancellationToken,
    cancel: CancellationToken,
) {
    loop {
        let line = tokio::select! {
            biased;
            line = lines.recv() => line,
            _ = finished.cancelled() => None,
            _ = cancel.cancelled() => None,
        };
        let Some(line) = line else {
            return;
        };
        if stdin.send(line).await.is_err() {
            return;
        }
    }
}

fn spawn_decisions(
    shared: SharedState,
    out: mpsc::UnboundedSender<String>,
    finished: CancellationToken,
) -> mpsc::Sender<PermissionDecision> {
    let (tx, mut rx) = mpsc::channel::<PermissionDecision>(16);
    tokio::spawn(async move {
        loop {
            let decision = tokio::select! {
                decision = rx.recv() => decision,
                _ = finished.cancelled() => None,
            };
            let Some(decision) = decision else {
                return;
            };
            match acp::decision_line(&shared, &decision) {
                Some(line) => {
                    let _ = out.send(line);
                }
                None => {
                    tracing::debug!(request = %decision.request_id, "decision for a closed Antigravity request")
                }
            }
        }
    });
    tx
}

/// `session/cancel` on the runtime's stop; the server answers the open
/// prompt with `cancelled`, which ends the turn.
fn spawn_stop(
    shared: SharedState,
    out: mpsc::UnboundedSender<String>,
    finished: CancellationToken,
) -> mpsc::Sender<Duration> {
    let (tx, mut rx) = mpsc::channel::<Duration>(1);
    tokio::spawn(async move {
        let stop = tokio::select! {
            stop = rx.recv() => stop,
            _ = finished.cancelled() => None,
        };
        if stop.is_none() {
            return;
        }
        match acp::cancel_line(&shared) {
            Some(line) => {
                let _ = out.send(line);
            }
            None => tracing::debug!("Antigravity stop before its session started"),
        }
    });
    tx
}
