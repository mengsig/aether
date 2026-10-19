use crate::Runtime;
use aether_engine::{Engine, EngineEvent, EngineTurn, PermissionDecision};
use aether_model::{UsageProbe, registry::ProviderDef};
use serde_json::Value;
use tokio_util::sync::CancellationToken;

pub(super) async fn read(
    runtime: &Runtime,
    provider: &ProviderDef,
    cancel: CancellationToken,
) -> Result<Option<Value>, String> {
    let Some(kind) = crate::turn_engine::engine_kind(provider.protocol) else {
        return if provider.usage_probe == UsageProbe::Turn {
            Err("usage probe turns require an engine-backed provider".into())
        } else {
            Ok(None)
        };
    };
    let engine = runtime
        .engines
        .get(&kind)
        .ok_or_else(|| format!("{kind} engine is unavailable"))?;
    if cancel.is_cancelled() {
        return Err("usage refresh cancelled".into());
    }
    let payload = engine
        .read_usage(&runtime.tools.engine_hidden_env(), cancel.clone())
        .await
        .map_err(|error| error.to_string())?;
    if payload.is_some() || provider.usage_probe != UsageProbe::Turn {
        return Ok(payload);
    }
    if cancel.is_cancelled() {
        return Err("usage refresh cancelled".into());
    }
    let directory = tempfile::tempdir().map_err(|e| format!("usage probe directory: {e}"))?;
    let turn = probe_turn(runtime, directory.path());
    collect_turn(engine.as_ref(), turn, cancel).await.map(Some)
}

fn probe_turn(runtime: &Runtime, cwd: &std::path::Path) -> EngineTurn {
    debug_assert!(cwd.is_absolute());
    debug_assert!(cwd.is_dir());
    EngineTurn {
        redactor: runtime.tool_runtime.redactor().clone(),
        prompt: "Reply with OK only. Do not use tools.".into(),
        images: Vec::new(),
        system_prompt: "This is a quota availability probe. Reply with OK only; do not use tools."
            .into(),
        session: None,
        session_usage: Default::default(),
        session_model_usage: None,
        new_session_id: aether_engine::new_session_id(),
        session_recorder: None,
        model: None,
        effort: None,
        cwd: cwd.to_path_buf(),
        extra_dirs: Vec::new(),
        summary_only: true,
        lean_host_context: true,
        network_access: true,
        bypass_permissions: false,
        hidden_env: runtime.tools.engine_hidden_env().to_vec(),
        tool_hidden_env: runtime.tools.hidden_env().to_vec(),
        temp_dir: Some(cwd.to_path_buf()),
        build_env: Vec::new(),
        mcp: None,
    }
}

async fn collect_turn(
    engine: &dyn Engine,
    turn: EngineTurn,
    cancel: CancellationToken,
) -> Result<Value, String> {
    debug_assert!(turn.summary_only && turn.mcp.is_none());
    debug_assert!(turn.session.is_none() && turn.extra_dirs.is_empty());
    let mut run = engine
        .run(turn, cancel.clone())
        .await
        .map_err(|e| e.to_string())?;
    let mut payload = None;
    let mut failure = None;
    let mut completed = false;
    while let Some(event) = run.events.recv().await {
        match event {
            EngineEvent::RateLimit { payload: value } => payload = Some(value),
            EngineEvent::Done(outcome) => {
                completed = true;
                run.input.take();
                run.stop.take();
                if outcome.is_error || outcome.interrupted {
                    failure = Some(outcome.error.unwrap_or_else(|| "usage probe failed".into()));
                }
            }
            EngineEvent::Permission { request_id, .. }
            | EngineEvent::Question { request_id, .. } => {
                if let Err(error) = deny_permission(&run, request_id) {
                    failure = Some(error);
                    cancel.cancel();
                }
            }
            _ => {}
        }
    }
    if let Some(error) = failure {
        return Err(error);
    }
    if !completed {
        return Err("usage probe ended without an outcome".into());
    }
    payload.ok_or_else(|| "usage probe returned no quota observation".into())
}

fn deny_permission(run: &aether_engine::EngineRun, request_id: String) -> Result<(), String> {
    let decisions = run
        .decisions
        .as_ref()
        .ok_or_else(|| "usage probe cannot refuse engine permission".to_owned())?;
    decisions
        .try_send(PermissionDecision {
            request_id,
            allow: false,
            message: Some("usage probes cannot use tools".into()),
            answer: None,
        })
        .map_err(|error| format!("usage probe permission refusal: {error}"))
}
