//! Bound on a native tool call that never returns. The engine's own tool
//! timeout should end it; when it does not, the turn would hold the agent's
//! inbox forever, so the runtime stops the engine and wakes the agent.
use std::time::Duration;

use aether_core::inbox::InboxKind;

use super::events::PendingCall;
use super::{Actor, Commit, EngineKind, EngineOutcome, EventKind, Result, Runtime, TurnCtx, json};

/// Recorded when the bound stops a turn, so clients can show or subscribe to it.
pub(super) const OVERDUE_EVENT: &str = "agent.tool_call_overdue";

/// Opens every wake this bound sends, so a turn that ran on one is told apart.
const WAKE_OPENING: &str = "Aether stopped your turn: tool call";

/// Stop the turn over its oldest outstanding call, which has had no sign of
/// life for `silent`: record why, and wake the agent for a fresh turn unless
/// the turn it stops ran on such a wake alone. Returns the run's outcome.
pub(super) fn stop(
    runtime: &Runtime,
    ctx: &TurnCtx,
    engine: EngineKind,
    calls: &[&PendingCall],
    silent: Duration,
) -> Result<EngineOutcome> {
    let bound = runtime.config.tool_call_bound;
    assert!(silent >= bound, "a call is stopped only past the bound");
    let oldest = calls
        .iter()
        .min_by_key(|call| call.started)
        .ok_or_else(|| crate::RuntimeError::Invalid("no outstanding tool call to stop".into()))?;
    let outstanding = oldest.started.elapsed();
    let mut reason = format!(
        "the tool-call bound: {} had no result after {}s and the engine showed no sign of \
         life for {}s (bound {}s)",
        oldest.tool,
        outstanding.as_secs(),
        silent.as_secs(),
        bound.as_secs()
    );
    let resumed = !ran_on_own_wake(runtime, ctx)?;
    if !resumed {
        reason.push_str("; the turn before ended the same way, so the agent waits for a message");
    }
    let mut commit = Commit::new().event(ctx.draft(EventKind::Custom {
        name: OVERDUE_EVENT.into(),
        payload: json!({
            "agent": ctx.agent.id,
            "engine": engine.as_str(),
            "tool": oldest.tool,
            "call": oldest.id,
            "outstanding_secs": outstanding.as_secs(),
            "silent_secs": silent.as_secs(),
            "bound_secs": bound.as_secs(),
            "resumed": resumed,
        }),
    }));
    let hold = runtime.hold();
    if resumed {
        let wake = format!(
            "{WAKE_OPENING} {} returned nothing for {}s. Its result is lost and it may still have \
             had effects: check them before running it again, and do not repeat it unchanged.",
            oldest.tool,
            outstanding.as_secs()
        );
        runtime.push_inbox(
            &hold,
            &mut commit,
            &ctx.agent,
            InboxKind::Wake { reason: wake },
            Actor::System,
        )?;
    }
    runtime.apply_held(hold, commit)?;
    Ok(EngineOutcome {
        is_error: true,
        interrupted: true,
        error: Some(reason),
        ..Default::default()
    })
}

/// Whether the newest item in the agent's inbox is this bound's own wake:
/// nothing arrived since an earlier turn was stopped, so this turn ran on
/// that wake alone and stopping it again must not wake it a third time.
fn ran_on_own_wake(runtime: &Runtime, ctx: &TurnCtx) -> Result<bool> {
    let last = runtime.store.inbox_last_seq(ctx.agent.id)?;
    if last == 0 {
        return Ok(false);
    }
    let newest = runtime.store.inbox(ctx.agent.id, last - 1)?;
    debug_assert!(newest.len() <= 1, "one item has the last sequence");
    Ok(newest.last().is_some_and(
        |item| matches!(&item.kind, InboxKind::Wake { reason } if reason.starts_with(WAKE_OPENING)),
    ))
}
