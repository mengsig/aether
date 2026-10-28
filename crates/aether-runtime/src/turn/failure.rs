use super::*;

/// An exhausted link's cause with the provider code and retry-after the
/// operator needs to act on; other failures keep their established text.
pub(super) fn exhaustion_detail(error: &ModelError) -> Option<String> {
    if !crate::exhaustion::recoverable_by_fallback(error.class) {
        return None;
    }
    let code = error
        .provider_code
        .as_deref()
        .map(|code| {
            format!(
                "; provider code: {}",
                aether_model::http::safe_provider_text(code, 200)
            )
        })
        .unwrap_or_default();
    let after = error
        .retry_after
        .map(|after| format!("; provider retry-after: {} seconds", after.as_secs()))
        .unwrap_or_default();
    Some(format!("{error}{code}{after}"))
}

/// Write the failed invocation, then decide what the turn does next: fall
/// back to the next link, retry this one after a backoff, or fail.
#[allow(clippy::too_many_arguments)]
pub(super) async fn record_failure(
    runtime: &Runtime,
    ctx: &mut TurnCtx,
    invocation_id: InvocationId,
    snapshot: &aether_core::snapshot::InvocationSnapshot,
    started_at: aether_core::time::Timestamp,
    model: &aether_core::model::ModelRef,
    rest: &[aether_core::model::ModelRef],
    error: ModelError,
    mut partial: ModelResponse,
    cancel: &CancellationToken,
) -> Result<LinkOutcome> {
    let mut error = error;
    error.message = aether_model::http::safe_provider_text(&error.message, 2000);
    let detail = exhaustion_detail(&error).unwrap_or_else(|| error.to_string());
    let finished_at = now();
    let cancelled = cancel.is_cancelled() || error.class == ModelErrorClass::Cancelled;
    if cancelled {
        partial
            .usage
            .mark_cost_gap(aether_core::model::CostGap::Interrupted);
    }
    // A cancellation is ours, not the provider's; everything else is what the
    // provider has been failing with lately.
    if !cancelled {
        crate::scheduler::record_error(runtime, &model.provider, error.class);
    }
    let invocation = Invocation {
        id: invocation_id,
        snapshot_id: snapshot.id,
        agent_id: ctx.agent.id,
        turn: ctx.turn,
        model: model.clone(),
        started_at,
        finished_at: Some(finished_at),
        content: partial.content.clone(),
        usage: partial.usage,
        stop_reason: None,
        error: Some(detail.clone()),
        provider_message_id: partial.provider_message_id.clone(),
    };
    ctx.add_usage(partial.usage);
    count_turn(ctx);
    ctx.agent.updated_at = finished_at;
    let mut commit = Commit::new();
    // Stopped on purpose: what streamed so far stays in the transcript so the
    // next turn continues from it (text only; unfinished tool calls are dropped).
    let kept: Vec<ContentBlock> = partial
        .content
        .iter()
        .filter(|b| matches!(b, ContentBlock::Text { text } if !text.trim().is_empty()))
        .cloned()
        .collect();
    if cancelled && !kept.is_empty() {
        commit.push_event(ctx.draft(EventKind::AgentMessage {
            role: MessageRole::Assistant,
            content: kept,
            invocation: Some(invocation_id),
        }));
    }
    commit.push_event(ctx.draft(EventKind::InvocationFailed {
        class: Some(error.class),
        invocation: invocation_id,
        snapshot: snapshot.id,
        error: detail.clone(),
        retryable: error.is_retryable() && !cancelled,
        cancelled,
        partial: partial.content,
    }));
    commit.push_mutation(Mutation::PutInvocation(invocation));
    // The partial usage is spent either way; a locked database is waited out.
    super::contention::write_through_lock(runtime, ctx.agent.id, "the failed invocation", || {
        let hold = runtime.hold();
        let mut commit = commit.clone();
        commit.push_mutation(Mutation::UpsertAgent(agent_row(runtime, ctx, &hold)?));
        runtime.apply_held(hold, commit)
    })
    .await?;
    if cancelled {
        return Ok(LinkOutcome::Done(Next::Cancelled(Some("cancelled".into()))));
    }
    if crate::exhaustion::recoverable_by_fallback(error.class) {
        if let Some(next) = rest.first() {
            // A spent account answers no other turn either: bench the link the
            // chain is leaving, bounded, as main did when quota was classed a
            // rate limit. The last link is benched by its suspension instead,
            // on the probe schedule, so it is not benched twice here.
            if error.class != ModelErrorClass::ContextLimit {
                let wait = error
                    .retry_after
                    .unwrap_or(crate::scheduler::QUOTA_BACKOFF)
                    .min(Duration::from_secs(300));
                crate::scheduler::backoff(
                    runtime,
                    &model.provider,
                    format!("{} from the provider", error.class.as_str()),
                    wait,
                )?;
            }
            announce_fallback(runtime, ctx, model, next, detail.clone(), Some(error.class))?;
            return Ok(LinkOutcome::FellBack);
        }
        // Spent credits come back on their own; a context limit does not.
        if let Some(class) = ExhaustionClass::of(error.class) {
            return Ok(LinkOutcome::Done(Next::Suspended(Box::new(
                crate::exhaustion::Exhausted {
                    model: model.clone(),
                    class,
                    detail,
                    retry_after: error.retry_after,
                },
            ))));
        }
        return Ok(LinkOutcome::Done(Next::Failed(format!(
            "model error from {}/{}: {detail}",
            model.provider, model.model,
        ))));
    }
    if !error.is_retryable() {
        return Ok(LinkOutcome::Done(Next::Failed(format!(
            "model error: {detail}"
        ))));
    }
    let reason = match error.class {
        ModelErrorClass::RateLimit => "rate limited".to_owned(),
        other => format!("{other:?} from the provider"),
    };
    let wait = error.retry_after.unwrap_or(Duration::from_secs(5));
    crate::scheduler::backoff(
        runtime,
        &model.provider,
        reason.clone(),
        wait.min(Duration::from_secs(300)),
    )?;
    // §4.5: a retryable failure moves the turn to the next link rather than
    // parking the agent behind the backoff it just caused.
    if let Some(next) = rest.first() {
        announce_fallback(runtime, ctx, model, next, reason, Some(error.class))?;
        return Ok(LinkOutcome::FellBack);
    }
    Ok(LinkOutcome::Done(unanswered_turn(runtime, ctx, &detail)?))
}

/// The turn ends with nothing to show: the chain is exhausted, or its last
/// link cannot be entered. Give the agent up only when that has happened once
/// too often in a row; otherwise hand the turn back to the scheduler.
pub(super) fn unanswered_turn(runtime: &Runtime, ctx: &TurnCtx, detail: &str) -> Result<Next> {
    let links = ctx.agent.chain().link_count();
    if consecutive_failed_turns(runtime, ctx.agent.id, links)?
        >= runtime.config.max_consecutive_failures
    {
        let what = if links > 1 {
            "every link of the model chain failed"
        } else {
            "the model failed"
        };
        return Ok(Next::Failed(format!(
            "{what} on {} turns in a row: {detail}",
            runtime.config.max_consecutive_failures
        )));
    }
    Ok(Next::Ready)
}

/// A turn counts once however many links of its chain it walks: the first
/// invocation of the turn advances the counter, the rest do not. Otherwise a
/// chain burns `max_turns` as fast as it has links.
pub(super) fn count_turn(ctx: &mut TurnCtx) {
    ctx.agent.stats.turns = ctx.agent.stats.turns.max(ctx.turn);
}

/// Turns, not links: one turn that walks a whole chain and ends unanswered
/// counts once, and a turn any link answered ends the streak. Otherwise the
/// more fallbacks an agent has, the sooner it is given up on.
///
/// The turn an invocation belongs to lives on the invocation row, so the
/// failures of one chain walk collapse. A failure with no row is a turn the
/// daemon lost mid-flight (`recovery`), which is a whole turn of its own.
///
/// Spent credits are not the agent failing, so a probe counts neither way:
/// counting them would give up on an agent the suspension promised to
/// keep, and resetting on them would forgive a real streak either side.
pub(super) fn consecutive_failed_turns(
    runtime: &Runtime,
    agent: AgentId,
    links: usize,
) -> Result<u32> {
    // Enough events to see past the threshold even when every turn spends a
    // whole chain, so a long chain does not hide the streak.
    let want = (runtime.config.max_consecutive_failures as usize + 1) * links.max(1);
    let recent = runtime.store.latest_events(
        want,
        &EventFilter {
            types: vec!["invocation.completed".into(), "invocation.failed".into()],
            ..EventFilter::own_events(agent)
        },
    )?;
    let mut turns = 0;
    let mut previous: Option<u32> = None;
    for e in recent {
        let EventKind::InvocationFailed {
            invocation, class, ..
        } = e.kind
        else {
            break;
        };
        if class.and_then(ExhaustionClass::of).is_some() {
            continue;
        }
        let turn = runtime.store.invocation(invocation)?.map(|i| i.turn);
        if turn.is_none() || turn != previous {
            turns += 1;
        }
        previous = turn;
    }
    Ok(turns)
}
