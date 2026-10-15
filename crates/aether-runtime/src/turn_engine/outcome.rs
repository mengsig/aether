//! Native engine outcome.
use super::{
    Arc, Commit, ContentBlock, Crossed, EngineKind, EventKind, Invocation, InvocationId,
    InvocationSnapshot, ModelErrorClass, Mutation, Next, Park, Result, Runtime, TurnCtx, Usage,
    now,
};

/// The engine ended the turn without running the model, so the prompt went
/// unanswered: record the failed invocation, and resend all of it next turn.
pub(super) fn record_unanswered_prompt(
    runtime: &Arc<Runtime>,
    ctx: &mut TurnCtx,
    engine: EngineKind,
    snapshot: &InvocationSnapshot,
) -> Result<()> {
    debug_assert_eq!(snapshot.agent_id, ctx.agent.id, "another agent's snapshot");
    let error = format!(
        "{} ended the turn without running the model",
        engine.as_str()
    );
    let at = now();
    let invocation = Invocation {
        id: InvocationId::new(),
        snapshot_id: snapshot.id,
        agent_id: ctx.agent.id,
        turn: ctx.turn,
        model: snapshot.model.clone(),
        started_at: at,
        finished_at: Some(at),
        content: Vec::new(),
        usage: Usage::default(),
        stop_reason: None,
        error: Some(error.clone()),
        provider_message_id: None,
    };
    // The session never read this prompt, so it holds none of its layers.
    ctx.agent.engine_sent = None;
    let hold = runtime.hold();
    let row = crate::turn::agent_row(runtime, ctx, &hold)?;
    let commit = Commit::new()
        .event(ctx.draft(EventKind::InvocationFailed {
            class: None,
            invocation: invocation.id,
            snapshot: snapshot.id,
            error,
            retryable: true,
            cancelled: false,
            partial: Vec::new(),
        }))
        .mutate(Mutation::PutInvocation(invocation))
        .mutate(Mutation::UpsertAgent(row));
    runtime.apply_held(hold, commit)?;
    Ok(())
}

/// How a finished engine turn ended, in the order it has to be read.
pub(super) enum EngineVerdict {
    /// The grant is gone. A wind-down a harness action had already started
    /// travels with it: its trace is the only record of what the engine did
    /// after the stop, and the budget stop is what ends the agent.
    Budget(Crossed, Option<Park>),
    /// A harness action decided and committed the turn's end while the engine
    /// was still running; the wind-down that followed is not the outcome.
    Parked(Park, Next),
    Cancelled,
    Ran,
}

/// A budget stop outranks the cancel it caused. `stop_for_budget` cancels this
/// turn's own token, so the cancel flag is always set on the budget path -
/// reading it first would show the operator a bare cancel with no budget in
/// it. A recorded end outranks the cancel for the
/// same reason: the park's own stop cancels the turn, and a completion the
/// task already holds must not come back as `Cancelled`. Total and pure, so
/// the precedence is pinned by a test rather than by the order of statements.
pub(super) fn engine_verdict(
    budget_stop: Option<Crossed>,
    park: Option<Park>,
    next: Option<Next>,
    cancelled: bool,
) -> EngineVerdict {
    match (budget_stop, park, next, cancelled) {
        // A wind-down a harness action started (`stop_for_crossing` included)
        // is recorded whether or not the same turn also produced a `Next` -
        // the budget stop is terminal either way, so nothing else will.
        (Some(crossed), park, _, _) => EngineVerdict::Budget(crossed, park),
        (None, Some(park), Some(next), _) => EngineVerdict::Parked(park, next),
        // A park with neither a `Next` nor a budget stop should be
        // unreachable: `stop_for_crossing` is the only park-starting path
        // with no `Next`, and it always runs alongside a `budget_stop`
        // (`:812-816`). Falling through would drop the park unrecorded.
        (None, Some(_), None, cancelled) => {
            debug_assert!(
                false,
                "a park reached engine_verdict with no Next and no budget stop"
            );
            if cancelled {
                EngineVerdict::Cancelled
            } else {
                EngineVerdict::Ran
            }
        }
        (None, None, _, true) => EngineVerdict::Cancelled,
        (None, None, _, false) => EngineVerdict::Ran,
    }
}

/// What streamed for a message the engine never committed, under the id
/// its deltas carried, so a failure record can keep what was shown live.
#[derive(Debug, Default)]
pub(super) struct Uncommitted {
    pub(super) invocation: Option<InvocationId>,
    pub(super) content: Vec<ContentBlock>,
}

pub(super) fn record_engine_failure(
    runtime: &Runtime,
    ctx: &TurnCtx,
    snapshot: &InvocationSnapshot,
    error: &str,
    class: ModelErrorClass,
    retryable: bool,
    partial: Uncommitted,
) -> Result<()> {
    runtime.apply(engine_failure_commit(
        ctx, snapshot, error, class, retryable, partial,
    ))?;
    Ok(())
}

pub(super) fn record_engine_incident(
    runtime: &Runtime,
    ctx: &TurnCtx,
    snapshot: &InvocationSnapshot,
    engine: EngineKind,
    detail: Option<&str>,
) -> Result<()> {
    let error = engine_failure_text(engine, detail);
    let class = crate::exhaustion::engine_error_class(detail.unwrap_or("unknown error"));
    crate::scheduler::record_error(runtime, &snapshot.model.provider, class);
    let hold = runtime.hold();
    let mut commit =
        engine_failure_commit(ctx, snapshot, &error, class, false, Uncommitted::default());
    crate::exhaustion::record_incident(
        runtime,
        &hold,
        &mut commit,
        &ctx.agent,
        &snapshot.model,
        &aether_core::model::ModelError::new(class, error),
        &ctx.base_actor,
    )?;
    runtime.apply_held(hold, commit)?;
    Ok(())
}

fn engine_failure_commit(
    ctx: &TurnCtx,
    snapshot: &InvocationSnapshot,
    error: &str,
    class: ModelErrorClass,
    retryable: bool,
    partial: Uncommitted,
) -> Commit {
    debug_assert_eq!(snapshot.agent_id, ctx.agent.id, "another agent's snapshot");
    debug_assert!(
        partial.content.is_empty() || partial.invocation.is_some(),
        "a partial without the id its deltas streamed under would show twice"
    );
    let at = now();
    let invocation = Invocation {
        // With no streamed message to retire, `Default` mints a fresh id.
        id: partial.invocation.unwrap_or_default(),
        snapshot_id: snapshot.id,
        agent_id: ctx.agent.id,
        turn: ctx.turn,
        model: snapshot.model.clone(),
        started_at: at,
        finished_at: Some(at),
        content: partial.content.clone(),
        usage: Usage::default(),
        stop_reason: None,
        error: Some(error.into()),
        provider_message_id: None,
    };
    Commit::new()
        .event(ctx.draft(EventKind::InvocationFailed {
            invocation: invocation.id,
            snapshot: snapshot.id,
            error: error.into(),
            class: Some(class),
            retryable,
            cancelled: false,
            partial: partial.content,
        }))
        .mutate(Mutation::PutInvocation(invocation))
}

pub(super) fn engine_failure_text(engine: super::EngineKind, error: Option<&str>) -> String {
    format!(
        "{} failed: {}",
        engine.as_str(),
        aether_model::http::safe_provider_text(error.unwrap_or("unknown error"), 2000)
    )
}
