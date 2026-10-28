//! A person's request to compact an engine agent goes to the engine's own
//! compaction (Claude Code's `/compact`): it folds the live session in place,
//! keeps the session, and needs no second model to read the transcript.
use super::compaction::{RequestRef, preserve_newer_request, record_compaction_usage};
use super::*;
use crate::compaction_client::{Accounting, refuse_checkpoint_input};
use aether_core::model::{ModelError, Usage};
use aether_engine::{Engine, EngineEvent, EngineTurn};

/// Longest a native fold may run before it is reported as failed.
const NATIVE_TIMEOUT: Duration = Duration::from_secs(300);

/// What the engine reported for one native compaction run.
#[derive(Default)]
struct NativeRun {
    before_tokens: Option<u64>,
    after_tokens: Option<u64>,
    duration_ms: Option<u64>,
    folded: bool,
    error: Option<String>,
    session_usage: Option<aether_core::agent::EngineSessionUsage>,
}

/// Fold the agent's engine session with the engine's own compaction. `false`
/// means the engine cannot (no session, no native support) and Aether's
/// compactor should fold instead; `true` means the request was answered,
/// with `compaction.native`, `compaction.failed`, or left pending on cancel.
pub(super) async fn try_engine_compaction(
    runtime: &Arc<Runtime>,
    ctx: &mut TurnCtx,
    request: RequestRef,
    cancel: &CancellationToken,
) -> Result<bool> {
    let Some(session) = ctx.agent.engine_session.clone() else {
        return Ok(false);
    };
    if ctx.agent.scope.backend != aether_core::agent::ExecutionBackend::Host {
        return Ok(false);
    }
    let resolved = runtime.models.resolve(&ctx.agent.model)?;
    let Some(kind) = crate::turn_engine::engine_kind(resolved.provider.protocol) else {
        return Ok(false);
    };
    let Some(engine) = runtime
        .engines
        .recheck(kind)
        .await
        .get(&kind)
        .filter(|engine| engine.compacts_natively())
    else {
        return Ok(false);
    };
    let turn = compact_turn(runtime, ctx, &resolved, session)?;
    debug_assert!(turn.compact_session && turn.session.is_some() && turn.mcp.is_none());
    let mut accounting =
        Accounting::priced(crate::turn_engine::TurnPricing::of(runtime, &resolved));
    let started = std::time::Instant::now();
    let child = cancel.child_token();
    let run = tokio::time::timeout(
        NATIVE_TIMEOUT,
        collect(
            engine.as_ref(),
            turn,
            ctx.launch_context(),
            child.clone(),
            &mut accounting,
        ),
    )
    .await;
    child.cancel();
    accounting.finish(cancel.is_cancelled());
    let mut run = run.unwrap_or_else(|_| NativeRun {
        error: Some(format!(
            "it did not finish within {} minutes",
            NATIVE_TIMEOUT.as_secs() / 60
        )),
        ..NativeRun::default()
    });
    run.duration_ms = run
        .duration_ms
        .or(Some(started.elapsed().as_millis() as u64));
    record_native_usage(runtime, ctx, &mut run, accounting.usage)?;
    // Stopped on purpose: the request stays pending, as after Aether's own fold.
    if cancel.is_cancelled() && !run.folded {
        return Ok(true);
    }
    settle(runtime, ctx, request, &resolved.provider.id, run)?;
    Ok(true)
}

/// A turn that resumes `session` only to run the engine's compaction.
fn compact_turn(
    runtime: &Arc<Runtime>,
    ctx: &TurnCtx,
    resolved: &aether_model::ResolvedModel,
    session: String,
) -> Result<EngineTurn> {
    let session_model_usage = ctx
        .agent
        .engine_usage
        .as_ref()
        .filter(|usage| usage.session == session)
        .and_then(|usage| usage.models.clone());
    Ok(EngineTurn {
        redactor: runtime.tool_runtime.redactor().clone(),
        prompt: String::new(),
        images: Vec::new(),
        system_prompt: String::new(),
        session: Some(session),
        session_usage: ctx.agent.engine_session_usage(),
        session_model_usage,
        new_session_id: aether_engine::new_session_id(),
        session_recorder: None,
        model: Some(resolved.model.id.clone()),
        effort: None,
        cwd: tool_context(runtime, ctx).cwd,
        extra_dirs: Vec::new(),
        summary_only: false,
        compact_session: true,
        auto_compact_tokens: None,
        lean_host_context: false,
        network_access: false,
        bypass_permissions: false,
        hidden_env: runtime.tools.engine_hidden_env().to_vec(),
        tool_hidden_env: runtime.tools.hidden_env().to_vec(),
        temp_dir: crate::turn::agent_temp_dir(runtime, ctx.agent.id)?,
        build_env: Vec::new(),
        mcp: None,
    })
}

async fn collect(
    engine: &dyn Engine,
    turn: EngineTurn,
    context: aether_engine::EngineLaunchContext,
    cancel: CancellationToken,
    accounting: &mut Accounting,
) -> NativeRun {
    debug_assert!(accounting.usage.is_none());
    let mut out = NativeRun::default();
    let mut run = match engine.run_with_context(turn, context, cancel).await {
        Ok(run) => run,
        Err(error) => {
            accounting.unsubmitted = error.proves_no_submission();
            out.error = Some(error.to_string());
            return out;
        }
    };
    while let Some(event) = run.events.recv().await {
        match event {
            EngineEvent::Compacted {
                before_tokens,
                after_tokens,
                duration_ms,
                ..
            } => {
                out.folded = true;
                out.before_tokens = before_tokens;
                out.after_tokens = after_tokens;
                out.duration_ms = duration_ms;
            }
            EngineEvent::CompactionFailed { error } => out.error = Some(error),
            EngineEvent::SessionUsage {
                session,
                total,
                models,
            } => {
                out.session_usage = Some(aether_core::agent::EngineSessionUsage {
                    session,
                    total,
                    models,
                });
            }
            EngineEvent::Assistant {
                usage: Some(usage), ..
            }
            | EngineEvent::UsageDelta { usage } => accounting.reported(usage),
            EngineEvent::UsageRequest { request } => accounting.reported(request.delta()),
            EngineEvent::Permission { request_id, .. }
            | EngineEvent::Question { request_id, .. } => {
                if let Err(error) = refuse_checkpoint_input(&run, request_id).await {
                    out.error = Some(error.message);
                    return out;
                }
            }
            EngineEvent::Done(outcome) => {
                accounting.close(&outcome);
                if outcome.is_error && !out.folded {
                    out.error = out.error.or(outcome.error);
                } else if !out.folded && out.error.is_none() {
                    out.error = outcome.text.filter(|text| !text.trim().is_empty());
                }
                return out;
            }
            _ => {}
        }
    }
    out.error
        .get_or_insert_with(|| "it ended without a result".into());
    out
}

/// Bill the run and carry the session's new counters, so the next turn on
/// the session bills only what it adds.
fn record_native_usage(
    runtime: &Runtime,
    ctx: &mut TurnCtx,
    run: &mut NativeRun,
    usage: Option<Usage>,
) -> Result<()> {
    if let Some(session_usage) = run.session_usage.take() {
        debug_assert_eq!(
            Some(&session_usage.session),
            ctx.agent.engine_session.as_ref()
        );
        ctx.agent.engine_usage = Some(session_usage);
    }
    let error =
        run.error.as_ref().filter(|_| !run.folded).map(|message| {
            ModelError::new(crate::exhaustion::engine_error_class(message), message)
        });
    record_compaction_usage(
        runtime,
        ctx,
        &aether_context::CompactionAttempt {
            model: ctx.agent.model.clone(),
            usage,
            error,
        },
    )
}

/// `compaction.native` with the engine's counts, or `compaction.failed` in
/// words a person can act on. The session is kept either way.
fn settle(
    runtime: &Runtime,
    ctx: &mut TurnCtx,
    request: RequestRef,
    provider: &str,
    run: NativeRun,
) -> Result<()> {
    debug_assert!(request.seq > 0, "a fold answers a stored request");
    let kind = if run.folded {
        if let Some(tokens) = run.after_tokens {
            ctx.agent.stats.context_tokens = Some(tokens);
        }
        EventKind::CompactionNative {
            provider: provider.to_owned(),
            automatic: false,
            before_tokens: run.before_tokens,
            after_tokens: run.after_tokens,
            duration_ms: run.duration_ms,
        }
    } else {
        let reason = run.error.unwrap_or_else(|| "it gave no reason".into());
        EventKind::CompactionFailed {
            error: format!(
                "{provider} could not compact the conversation: {}",
                reason.trim()
            ),
            models: vec![ctx.agent.model.clone()],
        }
    };
    tracing::info!(agent = %ctx.agent.id, folded = run.folded, duration_ms = run.duration_ms, "native compaction settled");
    ctx.agent.updated_at = now();
    let hold = runtime.hold();
    let mut commit = Commit::new()
        .event(ctx.draft(kind).caused_by(request.id))
        .mutate(Mutation::UpsertAgent(agent_row_with_session(
            runtime, ctx, &hold,
        )?));
    preserve_newer_request(runtime, ctx, request.seq, &mut commit, &hold)?;
    runtime.apply_held(hold, commit)?;
    Ok(())
}
