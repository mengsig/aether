//! Native engine driver.
use super::{
    Arc, BACKGROUND_GRACE, BTreeSet, BusMessage, CancellationToken, Commit, ContentBlock, Crossed,
    DeltaKind, Engine, EngineEvent, EngineOutcome, EngineRequest, EngineTurn, EventKind,
    GateSubject, InvocationSnapshot, MAX_BACKGROUND_IDS, MAX_ENGINE_NAME, MessageRole, Park,
    PermissionDecision, REFUSAL_GRACE, ReapGrace, ResolvedModel, Result, Runtime, STEER_GRACE,
    Served, Steering, StreamState, SubtreeUsage, TURN_IS_OVER, ToolContext, ToolEffects, TurnCtx,
    TurnPricing, Uncommitted, Usage, Verdict, answer_question, budget_crossed, budget_outcome,
    clamp, disallowed_native_spawn_tool, flush_accounting, gated_call, handle_event,
    handle_request, json, mpsc, names_exhaustion, park_grace, permission_verdict, raise_to,
    record_consumed, record_refused_call, refusal, side_effect_of, start_park, steer,
    stop_for_crossing, stopping, writes_record,
};

/// Legacy per-message streams use turn totals as a fallback. Incremental
/// billing streams retain a baseline so final CLI totals replace only this
/// run's provisional accounting.
pub(super) struct DriveResult {
    pub(super) outcome: EngineOutcome,
    /// Later activity invalidates an earlier closing outcome.
    pub(super) receipt_seen: bool,
    /// The caller's token was cancelled; local harness stops use the engine child.
    pub(super) user_cancelled: bool,
    /// Streamed content, tool/bridge activity or steering prevents replay.
    pub(super) activity_seen: bool,
    /// Present when a harness action ended the turn while the engine was
    /// still running: how the wind-down went.
    pub(super) park: Option<Park>,
    /// An invocation this turn carried a cost from the registry's price.
    pub(super) priced: bool,
    /// Per-message usage arrived, so the turn's tokens are already counted.
    pub(super) usage_seen: bool,
    /// The model answered at least once, so this turn recorded an invocation.
    pub(super) invoked: bool,
    /// The model called at least one tool this turn.
    pub(super) ran_tools: bool,
    /// Present only for incremental billing; previous runs must survive reconciliation.
    pub(super) usage_before: Option<(Usage, Usage)>,
    pub(super) stream_usage: Usage,
    pub(super) request_usage_seen: bool,
    /// The grant this run crossed while it ran. The turn ends as a budget
    /// stop, not as an engine failure.
    pub(super) budget_stop: Option<Crossed>,
    /// This run's configured or session-reported price, retained for non-request
    /// streams that estimate their closing counters rather than each request.
    pub(super) pricing: TurnPricing,
    /// The failed run's process was seen to exit, or never started: its
    /// session is free for another run to resume.
    pub(super) cleaned_up: bool,
    /// Streamed after the engine's last committed message; shown live, so a
    /// failure record keeps it.
    pub(super) uncommitted: Uncommitted,
}

fn event_has_activity(event: &EngineEvent, ctx: &TurnCtx) -> bool {
    match event {
        EngineEvent::Session { .. }
        | EngineEvent::ContextUsage { .. }
        | EngineEvent::RateLimit { .. }
        | EngineEvent::Note(_)
        | EngineEvent::ProtocolNote(_)
        | EngineEvent::Done(_) => false,
        EngineEvent::UsageDelta { usage } if is_cost_gap_only(*usage) => false,
        EngineEvent::SessionUsage { session, total, .. } => {
            let mut baseline = ctx
                .agent
                .engine_usage
                .as_ref()
                .filter(|usage| &usage.session == session)
                .map(|usage| usage.total)
                .unwrap_or_default();
            // A receipt's price-source label alone is not model activity.
            baseline.cost_basis = total.cost_basis;
            *total != baseline
        }
        _ => true,
    }
}

/// An empty unreported-cost delta establishes reconciliation without model activity.
pub(super) fn is_cost_gap_only(usage: Usage) -> bool {
    debug_assert!(usage.cost_usd.is_finite() && usage.cost_usd >= 0.0);
    debug_assert!(
        usage
            .estimated_cost_usd
            .is_none_or(|cost| cost.is_finite() && cost >= 0.0)
    );
    !usage.cost_complete
        && usage.cost_gap == Some(aether_core::model::CostGap::Unreported)
        && usage.total_tokens() == 0
        && usage.reasoning_tokens.is_none_or(|tokens| tokens == 0)
        && usage.cost_usd == 0.0
        && usage.estimated_cost_usd.is_none_or(|cost| cost == 0.0)
}

/// Only evidence of a possible new model request invalidates closing accounting.
fn event_may_start_request(event: &EngineEvent) -> bool {
    matches!(
        event,
        EngineEvent::Assistant { .. }
            | EngineEvent::Delta(
                aether_core::model::StreamEvent::MessageStart { .. }
                    | aether_core::model::StreamEvent::TextDelta { .. }
                    | aether_core::model::StreamEvent::ThinkingDelta { .. }
                    | aether_core::model::StreamEvent::BlockStart { .. }
                    | aether_core::model::StreamEvent::ToolInputDelta { .. }
            )
            | EngineEvent::ToolResult { .. }
            | EngineEvent::Permission { .. }
            | EngineEvent::Question { .. }
            | EngineEvent::BackgroundTask { running: false, .. }
            | EngineEvent::InputAccepted
    )
}

fn failed_launch(
    error: aether_engine::EngineError,
    pricing: TurnPricing,
    user_cancelled: bool,
) -> DriveResult {
    debug_assert!(pricing.billed.is_none() || pricing.estimate.is_none());
    let prompt_not_submitted = error.proves_no_submission();
    debug_assert!(
        pricing
            .billed
            .into_iter()
            .chain(pricing.estimate)
            .all(|cost| cost.rates().all(|rate| rate.is_finite() && rate >= 0.0))
    );
    DriveResult {
        outcome: EngineOutcome {
            is_error: true,
            error: Some(error.to_string()),
            prompt_not_submitted,
            ..Default::default()
        },
        receipt_seen: false,
        user_cancelled,
        activity_seen: false,
        park: None,
        priced: false,
        usage_seen: false,
        invoked: false,
        ran_tools: false,
        usage_before: None,
        stream_usage: Usage::default(),
        request_usage_seen: false,
        budget_stop: None,
        pricing,
        cleaned_up: true,
        uncommitted: Uncommitted::default(),
    }
}

impl DriveResult {
    pub(super) fn unanswered(&self) -> bool {
        !self.activity_seen
            && !self.invoked
            && !self.ran_tools
            && (!self.usage_seen || is_cost_gap_only(self.stream_usage))
            && self
                .outcome
                .text
                .as_deref()
                .is_none_or(|text| text.is_empty())
            && self.outcome.usage.total_tokens() == 0
            && self
                .outcome
                .usage
                .reasoning_tokens
                .is_none_or(|tokens| tokens == 0)
            && self.outcome.cost_usd.is_none_or(|cost| cost == 0.0)
            && self.outcome.usage.cost_usd == 0.0
            && self
                .outcome
                .usage
                .estimated_cost_usd
                .is_none_or(|cost| cost == 0.0)
    }
}

/// Run the engine once and pump its events and bridge requests until it
/// reports an outcome.
#[allow(clippy::too_many_arguments)]
pub(super) async fn drive(
    runtime: &Arc<Runtime>,
    ctx: &mut TurnCtx,
    engine: &dyn Engine,
    turn: EngineTurn,
    snapshot: &InvocationSnapshot,
    tool_ctx: &Arc<ToolContext>,
    req_rx: &mut mpsc::Receiver<EngineRequest>,
    effects: &mut ToolEffects,
    resolved: &ResolvedModel,
    cancel: &CancellationToken,
) -> Result<DriveResult> {
    // The registry's price is the cost authority when it has one, exactly as
    // on the API path. The engine's own turn total is the fallback, so a
    // model the registry cannot price reports no cost rather than a
    // made-up one - and where the registry knows the API model behind the
    // subscription, an API-equivalent estimate rides alongside it. A `default`
    // model the registry cannot price at all is repriced once the session
    // says which model the CLI picked.
    let pricing = TurnPricing::of(runtime, resolved);
    // Subscribed before the engine starts so nothing delivered meanwhile is missed.
    let mut bus = runtime.bus.subscribe();
    // Bound, not thrown away: a detected kill-switch bypass below needs to
    // cancel the process this token was given to, not just close its stdin.
    let engine_cancel = cancel.child_token();
    let mut run = match engine.run(turn, engine_cancel.clone()).await {
        Ok(r) => r,
        Err(e) => {
            return Ok(failed_launch(e, pricing, cancel.is_cancelled()));
        }
    };
    let mut input = run.input.take();
    let decisions = run.decisions.take();
    let stop = run.stop.take();
    let inbox_before = ctx.agent.inbox_cursor;
    let mut activity_seen = false;
    let mut receipt_seen = false;
    let mut receipt_input_cursor = inbox_before;
    let mut steering = Steering::new(inbox_before);
    // Delivered between context assembly and now: hand it over straight away.
    steer(runtime, ctx, &mut steering, &mut input).await?;
    // A question the engine is blocked on; the next message answers it.
    let mut question: Option<String> = None;
    let mut state = StreamState::new(pricing, resolved.clone());
    runtime.bus.publish_delta(
        ctx.agent.id,
        state.message_invocation,
        DeltaKind::TurnStarted { turn: ctx.turn },
    );
    // Tools the engine moved to the background: the turn is not over until
    // they report back and the model has answered again.
    let mut background: BTreeSet<String> = BTreeSet::new();
    // The engine's result while it still owes an answer: to background work,
    // or to a steered message it has not folded in yet.
    let mut settled: Option<EngineOutcome> = None;
    let mut settle_deadline: Option<tokio::time::Instant> = None;
    // A harness action ended the turn; the engine is winding down and may
    // not decide the agent's state again.
    let mut park: Option<Park> = None;
    let mut park_reply: Option<(String, serde_json::Value)> = None;
    let mut native_wait: Option<super::native_wait::NativeWait> = None;
    // The park's stop runs on its own timer (`Park::arm_stop`); this is only
    // how the loop learns that it fired.
    let (stop_fired_tx, mut stop_fired_rx) = mpsc::channel::<()>(1);
    // Set once the process has been stopped: the turn ends when its stream
    // closes, or when this expires.
    let mut reap_grace: Option<ReapGrace> = None;
    // Set exactly where the grant is found crossed; the caller turns it into
    // the same stop the scheduler's sweep performs.
    let mut budget_stop: Option<Crossed> = None;
    let mut subtree = SubtreeUsage::default();
    let mut outcome = loop {
        if native_wait
            .as_ref()
            .is_some_and(|wait| wait.ready(runtime, ctx))
            && let Some(wait) = native_wait.take()
            && let Some((input, reply)) =
                wait.finish("a message arrived or the requested condition was satisfied")
        {
            state.remember_bridge_reply("wait_for".into(), input, reply);
        }
        let native_deadline = native_wait.as_ref().and_then(|wait| wait.deadline());
        let accounting_deadline = state.accounting.deadline();
        tokio::select! {
            ready = async {
                match input.as_ref() {
                    Some(tx) => tx.reserve().await.is_ok(),
                    None => false,
                }
            }, if steering.retry && input.is_some() && park.is_none() && question.is_none() => {
                if ready {
                    flush_accounting(runtime, ctx, &mut state.accounting)?;
                    steer(runtime, ctx, &mut steering, &mut input).await?;
                } else {
                    input = None;
                    steering.retry = false;
                }
            }
            () = tokio::time::sleep_until(native_deadline.unwrap_or_else(tokio::time::Instant::now)), if native_deadline.is_some() => {
                if let Some(wait) = native_wait.take()
                    && let Some((input, reply)) = wait.finish("the requested deadline was reached") {
                    state.remember_bridge_reply("wait_for".into(), input, reply);
                }
            }
            () = tokio::time::sleep_until(accounting_deadline.unwrap_or_else(tokio::time::Instant::now)), if accounting_deadline.is_some() => {
                let commits_before = runtime.store.commits();
                flush_accounting(runtime, ctx, &mut state.accounting)?;
                subtree.note_own_commits(runtime.store.commits().saturating_sub(commits_before));
            }
            ev = run.events.recv() => {
                if ev.as_ref().is_some_and(writes_record) {
                    // Held readings precede what this commits, and are never
                    // left behind a wait on an operator or the engine.
                    let commits_before = runtime.store.commits();
                    flush_accounting(runtime, ctx, &mut state.accounting)?;
                    subtree.note_own_commits(runtime.store.commits().saturating_sub(commits_before));
                }
                let active = ev.as_ref().is_some_and(|event| event_has_activity(event, ctx));
                if let Some(EngineEvent::Done(outcome)) = &ev {
                    let stopped = engine_cancel.is_cancelled() || park.as_ref().is_some_and(|park| park.stop_requested);
                    observe_closing(outcome, stopped, steering.sent_up_to, &mut receipt_seen, &mut receipt_input_cursor);
                } else if ev.as_ref().is_some_and(event_may_start_request) {
                    receipt_seen = false;
                }
                activity_seen |= active;
                state.interruption = interruption_cause(runtime, ctx, cancel, park.as_ref(), budget_stop.as_ref());
                match ev {
                None => break settled.take().unwrap_or_else(|| EngineOutcome {
                    is_error: true,
                    error: Some(match reap_grace {
                        Some(_) => "engine stopped after a harness tool ended the turn".into(),
                        None => "engine stream ended unexpectedly".into(),
                    }),
                    ..Default::default()
                }),
                // Being reaped: keep its last word, but the turn is not over
                // until the stream closes, which is when the process is gone.
                Some(EngineEvent::Done(o)) if reap_grace.is_some() || stopping(&park) || cancel.is_cancelled() => {
                    let exhaustion = settled
                        .as_ref()
                        .and_then(|held| held.error.clone())
                        .filter(|error| names_exhaustion(error));
                    settled = Some(merge_reaped_outcome(settled.take(), o));
                    // A synthetic cancellation during reap is not the cause
                    // that made the provider stop answering.
                    if let (Some(error), Some(outcome)) = (exhaustion, settled.as_mut()) {
                        outcome.error = Some(error);
                        outcome.is_error = true;
                    }
                    reap_grace.get_or_insert_with(ReapGrace::start);
                }
                Some(EngineEvent::Done(o)) => {
                    // No native background/input acknowledgement can make an
                    // exhausted provider answer. Do not strand the turn on it.
                    if o.error.as_deref().is_some_and(names_exhaustion) {
                        match park.as_mut() {
                            Some(park) => park.stop_now(&engine_cancel),
                            None => engine_cancel.cancel(),
                        }
                        drop(input.take());
                        break o;
                    }
                    if effects.terminal() && !background.is_empty() {
                        // A final receipt does not settle background work still
                        // reported live. Keep it while the bounded park drains
                        // or kills the process; its waiter remains fenced out.
                        settled = Some(o);
                        continue;
                    }
                    let owed = !background.is_empty() || !steering.outstanding.is_empty();
                    if !owed || input.is_none() {
                        // Close stdin first so the engine exits on its own.
                        drop(input.take());
                        break o;
                    }
                    settled = Some(o);
                    settle_deadline = None;
                }
                Some(EngineEvent::BackgroundTask { id, running }) => {
                    // Filled from the engine's own stdout, so bounded like
                    // any other input it controls.
                    let id = clamp(&id, MAX_ENGINE_NAME);
                    if !running && background.contains(&id)
                        && let Some(wait) = native_wait.take()
                        && let Some((input, reply)) = wait.finish(&format!("native background task {id} finished"))
                    {
                        state.remember_bridge_reply("wait_for".into(), input, reply);
                    }
                    if running {
                        if background.len() < MAX_BACKGROUND_IDS {
                            background.insert(id);
                        } else {
                            tracing::warn!(agent = %ctx.agent.id, %id, "engine reported more background tasks than the turn tracks; not waiting on this one");
                        }
                    } else if background.remove(&id) && background.is_empty() {
                        // The last result a parked turn was waiting for is
                        // in. Whatever the engine learns from it may no
                        // longer decide anything, so the turn stops paying
                        // for the model loop that would read it.
                        if let Some(park) = park.as_mut()
                            && !park.killed()
                        {
                            park.stop_now(&engine_cancel);
                            reap_grace = Some(ReapGrace::start());
                        }
                    }
                }
                Some(EngineEvent::InputAccepted) => {
                    if let Some(pending) = steering.outstanding.pop_front() {
                        record_consumed(runtime, ctx, pending)?;
                    }
                    // The engine is answering it; its next result decides.
                    settled = None;
                    settle_deadline = None;
                }
                Some(EngineEvent::InputRejected { reason }) => {
                    tracing::warn!(agent = %ctx.agent.id, %reason, "engine refused a steered message; it waits for the next turn");
                    steering.rewind(ctx.agent.inbox_cursor);
                    // Nothing else owed: the result already in hand is final.
                    if background.is_empty()
                        && let Some(o) = settled.take()
                    {
                        drop(input.take());
                        break o;
                    }
                }
                Some(EngineEvent::Permission { request_id, tool, input: tool_input, side_effect }) => {
                    // Once the turn's end is committed nothing more may run in
                    // it, so a prompt is denied outright: no operator is asked,
                    // and the wind-down is never suspended on an answer.
                    let verdict = match park.as_mut() {
                        Some(park) => {
                            park.refused(&tool);
                            Verdict { allow: false, message: Some(TURN_IS_OVER.to_owned()) }
                        }
                        None => {
                            let class = side_effect.unwrap_or_else(|| side_effect_of(&tool));
                            permission_verdict(
                                runtime,
                                ctx,
                                GateSubject::from_prompt(runtime, ctx, &tool, &tool_input, class, tool_input["tool_use_id"].as_str())?,
                                cancel,
                                true,
                            )
                            .await?
                        }
                    };
                    if let Some(tx) = decisions.as_ref() {
                        let _ = tx.send(PermissionDecision {
                            request_id,
                            allow: verdict.allow,
                            message: verdict.message,
                            answer: None,
                        }).await;
                    }
                }
                Some(EngineEvent::Question { request_id, prompt }) => {
                    // Shown as the agent asking; the person's reply is routed back below.
                    runtime.apply(Commit::new().event(ctx.draft(EventKind::AgentMessage {
                        role: MessageRole::Assistant,
                        content: vec![ContentBlock::Text { text: prompt }],
                        invocation: None,
                    })))?;
                    match steering.pending(runtime, ctx)? {
                        // Already answered while the question was being asked.
                        Some(pending) => answer_question(runtime, ctx, decisions.as_ref(), request_id, pending, &mut steering, &engine_cancel).await?,
                        None => question = Some(request_id),
                    }
                }
                Some(EngineEvent::Session { id, model, tools }) => {
                    // Verified at runtime, not just at argv: a renamed or
                    // rejected `--disallowedTools` is a silent bypass otherwise - the runtime would never see a
                    // spawn the CLI ran natively.
                    if let Some(bypassed) = disallowed_native_spawn_tool(engine.kind(), &tools) {
                        runtime.apply(Commit::new().event(ctx.draft(EventKind::Custom {
                            name: "engine.kill_switch_bypassed".into(),
                            payload: json!({ "engine": engine.kind().as_str(), "tool": bypassed }),
                        })))?;
                        // Stop the process itself, the same path a cancel
                        // uses - closing stdin only asks it to exit, and it
                        // may already be running native subagents the
                        // harness cannot see.
                        // Through the park when the turn has one, or its
                        // wind-down would record this runtime kill as the
                        // engine having stopped itself.
                        match park.as_mut() {
                            Some(park) => park.stop_now(&engine_cancel),
                            None => engine_cancel.cancel(),
                        }
                        break EngineOutcome {
                            interrupted: true,
                            is_error: true,
                            error: Some(format!(
                                "the engine still advertises its native spawn tool \
                                 {bypassed:?} despite the kill-switch flag; refusing to run it \
                                 blind to the harness"
                            )),
                            ..Default::default()
                        };
                    }
                    handle_event(
                        runtime,
                        ctx,
                        snapshot,
                        EngineEvent::Session { id, model, tools },
                        &mut state,
                    )?
                }
                Some(ev) => {
                    let reply_received = matches!(&ev, EngineEvent::ToolResult { tool_use_id, .. }
                        if park_reply.as_ref().is_some_and(|(name, input)| state.is_bridge_reply(tool_use_id, name, input)));
                    // Under bypassed approvals the engine's own tools never
                    // reach the bridge, so an accounting is the only moment a
                    // native turn's crossing becomes known. Any
                    // event that moved the figure counts, not one variant: a
                    // third-party engine may hang its usage on its messages.
                    let before = ctx.agent.stats.usage;
                    let commits_before = runtime.store.commits();
                    handle_event(runtime, ctx, snapshot, ev, &mut state)?;
                    if reply_received && let Some(park) = park.as_mut() {
                        park_reply = None;
                        park.ask_to_stop(&mut input, stop.as_ref(), runtime.config.engine_park_grace);
                    }
                    subtree.note_own_commits(
                        runtime.store.commits().saturating_sub(commits_before),
                    );
                    if budget_stop.is_none()
                        && ctx.agent.stats.usage != before
                        && let Some(crossed) = budget_crossed(runtime, ctx, &mut subtree)?
                    {
                        // Ends here, without the wind-down the bridge path
                        // gets: the accounting that crossed the grant is the
                        // figure, so there is nothing left to reap - and
                        // anything the engine said next would be work the
                        // agent was no longer granted.
                        // Through the park when the turn has one (see :738-741) -
                        // else `stopped_by` would blame the engine for this stop.
                        match park.as_mut() {
                            Some(park) => park.stop_now(&engine_cancel),
                            None => engine_cancel.cancel(),
                        }
                        let outcome = budget_outcome(&crossed.reason, settled.take());
                        budget_stop = Some(crossed);
                        break outcome;
                    }
                }
                }
            },
            // The park's own timer stopped the process. From here the turn
            // only reaps what the engine measured before it went.
            Some(()) = stop_fired_rx.recv(), if reap_grace.is_none() => {
                reap_grace = Some(ReapGrace::start());
            }
            // The stopped process did not report in. Its result is a floor,
            // not a sum, and the next turn may resume the session anyway.
            () = async {
                match reap_grace.as_mut() {
                    Some(grace) => grace.spent().await,
                    None => std::future::pending().await,
                }
            } => {
                tracing::warn!(agent = %ctx.agent.id, "stopped engine did not exit within the reap grace");
                break settled.take().unwrap_or_default();
            }
            // What the engine owed was not answered in time.
            () = tokio::time::sleep_until(settle_deadline.unwrap_or_else(tokio::time::Instant::now)), if settle_deadline.is_some() => {
                drop(input.take());
                if let Some(o) = settled.take() {
                    break o;
                }
            }
            req = req_rx.recv() => {
                if let Some(req) = req {
                    receipt_seen = false;
                    activity_seen = true;
                    // What the engine spent before it asked is on the record
                    // before whatever the request commits.
                    flush_accounting(runtime, ctx, &mut state.accounting)?;
                    if native_wait.is_some() && req.name == "wait_for" {
                        let _ = req.reply.send(("A native background wait is already pending in this session.".into(), true));
                        continue;
                    }
                    let name = req.name.clone();
                    // The grant is checked before the engine acts, not after
                    // the turn: discovering the overrun at the end is one
                    // published comment too late.
                    let known = budget_stop.is_some();
                    if !known
                        && let Some(crossed) = budget_crossed(runtime, ctx, &mut subtree)?
                    {
                        budget_stop = Some(crossed);
                    }
                    if let Some(reason) = budget_stop.as_ref().map(|c| c.reason.clone()) {
                        // Read before the reply consumes the request: a
                        // permission prompt is named `permission` but gates
                        // some other tool, whose name is in the payload.
                        let gated = gated_call(&req);
                        let _ = req.reply.send(refusal(&req.name, &reason));
                        record_refused_call(runtime, ctx, &gated, &reason, &mut state)?;
                        // Stop the process too: refusing bridge calls alone
                        // leaves its own tools running past the grant. Through
                        // the park, so the engine is asked in its own verb and
                        // the reap keeps its final usage. The grace is the
                        // window the refusal needs to reach the engine through
                        // the bridge child, which the stop takes with it.
                        if !known
                            && stop_for_crossing(
                                &mut park,
                                ctx.agent.id,
                                REFUSAL_GRACE,
                                &background,
                                &mut input,
                                stop.as_ref(),
                                &engine_cancel,
                                stop_fired_tx.clone(),
                            )
                        {
                            reap_grace = Some(ReapGrace::start());
                        }
                    } else if stopping(&park) {
                        // The process is being killed. Nothing served now can
                        // reach it, and a mutation would commit for a turn
                        // whose engine is already gone.
                        let _ = req.reply.send((TURN_IS_OVER.to_owned(), true));
                        if let Some(park) = park.as_mut() {
                            park.refused(&name);
                        }
                    } else {
                        let reply = (engine.kind() == super::EngineKind::CLAUDE_CODE && stop.is_some())
                            .then(|| (req.name.clone(), req.input.clone()));
                        let (tool, tool_input) = (req.name.clone(), req.input.clone());
                        let served =
                            handle_request(runtime, ctx, tool_ctx, req, effects, cancel, park.is_some(), !background.is_empty())
                                .await?;
                        if let Served::Ran(Some(result)) = &served {
                            state.remember_bridge_reply(tool, tool_input, result.clone());
                        }
                        if let Served::NativeWait(wait) = served {
                            native_wait = Some(wait);
                            continue;
                        }
                        if let Some(park) = park.as_mut() {
                            if matches!(served, Served::Refused) {
                                park.refused(&name);
                            }
                        } else if effects.parked() {
                            // Let Claude acknowledge the MCP reply before interrupting;
                            // otherwise it substitutes a spurious user-rejection result.
                            park_reply = reply.filter(|_| !effects.terminal());
                            let started = start_park(
                                ctx.agent.id,
                                park_grace(
                                    &effects.next,
                                    !background.is_empty(),
                                    runtime.config.engine_park_grace,
                                ),
                                &background,
                                &mut input,
                                // Preserve the native final response and usage
                                // receipt after terminal task tools. The bounded
                                // park timer still stops an unresponsive engine.
                                stop.as_ref().filter(|_| !effects.terminal()),
                                &engine_cancel,
                                stop_fired_tx.clone(),
                                park_reply.is_some(),
                            );
                            if started.killed() {
                                reap_grace = Some(ReapGrace::start());
                            }
                            park = Some(started);
                        }
                    }
                }
            }
            // Nothing a person sends can reach a turn that is over: an answer
            // handed to a process about to be killed would be consumed and
            // lost, so it stays in the inbox for the next turn.
            msg = bus.recv(), if park.is_none() && (input.is_some() || question.is_some()) => {
                let delivered = match msg {
                    Ok(BusMessage::Event(ev)) => {
                        ev.agent_id == Some(ctx.agent.id)
                            && matches!(ev.kind, EventKind::InboxDelivered { .. })
                    }
                    // Fell behind the bus; the store still has what arrived.
                    Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => true,
                    _ => false,
                };
                if delivered {
                    flush_accounting(runtime, ctx, &mut state.accounting)?;
                    match question.take() {
                        Some(request_id) => match steering.pending(runtime, ctx)? {
                            Some(pending) => answer_question(runtime, ctx, decisions.as_ref(), request_id, pending, &mut steering, &engine_cancel).await?,
                            None => question = Some(request_id),
                        },
                        // Steering: what arrived goes to the engine at its next
                        // step instead of waiting for this turn to end.
                        None => steer(runtime, ctx, &mut steering, &mut input).await?,
                    }
                }
            }
        }
        // Background work has no deadline. Once it is done, or while a steered
        // message awaits its acknowledgement, the engine gets bounded time to
        // answer; then the turn ends and unacknowledged messages stay queued.
        if settled.is_some()
            && background.is_empty()
            && settle_deadline.is_none()
            && reap_grace.is_none()
        {
            let grace = if steering.outstanding.is_empty() {
                BACKGROUND_GRACE
            } else {
                STEER_GRACE
            };
            settle_deadline = Some(tokio::time::Instant::now() + grace);
        }
    };
    let class = crate::exhaustion::engine_error_class(outcome.error.as_deref().unwrap_or(""));
    let mut cleaned_up = false;
    if outcome.is_error
        && crate::exhaustion::native_transient(class)
        && park.is_none()
        && budget_stop.is_none()
        && !outcome.interrupted
        && !cancel.is_cancelled()
    {
        // Done precedes native process cleanup; replay must wait for stream closure.
        drop(input.take());
        engine_cancel.cancel();
        let mut cleanup_grace = ReapGrace::start();
        loop {
            let accounting_deadline = state.accounting.deadline();
            tokio::select! {
                () = tokio::time::sleep_until(accounting_deadline.unwrap_or_else(tokio::time::Instant::now)), if accounting_deadline.is_some() => {
                    flush_accounting(runtime, ctx, &mut state.accounting)?;
                }
                () = cancel.cancelled() => {
                    activity_seen = true;
                    break;
                }
                () = cleanup_grace.spent() => {
                    activity_seen = true;
                    tracing::warn!(agent = %ctx.agent.id, "failed engine did not close its stream; not retrying it");
                    break;
                }
                request = req_rx.recv() => if let Some(request) = request {
                    receipt_seen = false;
                    activity_seen = true;
                    let _ = request.reply.send((TURN_IS_OVER.to_owned(), true));
                },
                event = run.events.recv() => match event {
                    None => {
                        cleaned_up = true;
                        break;
                    }
                    Some(EngineEvent::Done(receipt)) => {
                        observe_closing(&receipt, true, steering.sent_up_to, &mut receipt_seen, &mut receipt_input_cursor);
                        activity_seen = true;
                        merge_cleanup_receipt(&mut outcome, receipt);
                    }
                    Some(event) => {
                        state.interruption = interruption_cause(runtime, ctx, cancel, park.as_ref(), budget_stop.as_ref());
                        let active = event_has_activity(&event, ctx);
                        receipt_seen &= !event_may_start_request(&event);
                        activity_seen |= active;
                        let usage_before = ctx.agent.stats.usage;
                        handle_event(runtime, ctx, snapshot, event, &mut state)?;
                        if ctx.agent.stats.usage != usage_before
                            && budget_stop.is_none()
                            && let Some(crossed) = budget_crossed(runtime, ctx, &mut subtree)?
                        {
                            budget_stop = Some(crossed);
                        }
                    }
                }
            }
        }
    }
    // The turn is over: a timer still counting down would stop a process
    // that is no longer this turn's, and would misreport who stopped it.
    if let Some(park) = park.as_mut() {
        park.disarm();
    }
    flush_accounting(runtime, ctx, &mut state.accounting)?;
    // Aether finished these calls even if the engine died before echoing
    // their replies. Settle them before classifying genuinely unfinished work.
    let pending: Vec<_> = state.calls.keys().cloned().collect();
    for tool_use_id in pending {
        let reply = state
            .calls
            .get(&tool_use_id)
            .and_then(|call| state.bridge_reply(call))
            .cloned();
        if let Some((content, is_error)) = reply {
            handle_event(
                runtime,
                ctx,
                snapshot,
                EngineEvent::ToolResult {
                    tool_use_id,
                    content,
                    is_error,
                },
                &mut state,
            )?;
        }
    }
    if let Some(cause) =
        interruption_cause(runtime, ctx, cancel, park.as_ref(), budget_stop.as_ref()).or_else(
            || {
                outcome.interrupted.then(|| {
                    outcome
                        .error
                        .clone()
                        .unwrap_or_else(|| "engine interruption".into())
                })
            },
        )
    {
        let mut commit = Commit::new();
        for call in state.calls.values() {
            if crate::tool_interruption::denial_reason(runtime, ctx.agent.id, call.id)?.is_some() {
                continue;
            }
            crate::tool_interruption::record(
                &mut commit,
                &ctx.agent,
                call.id,
                &call.tool,
                &call.input,
                &cause,
            );
        }
        runtime.apply(commit)?;
    }
    let uncommitted = Uncommitted {
        invocation: Some(state.message_invocation),
        content: state.take_partial(),
    };
    // Under the id of a message left uncommitted, if any: its streamed text
    // stays live until the interrupt or failure that ends the turn keeps it.
    runtime.bus.publish_delta(
        ctx.agent.id,
        state.message_invocation,
        DeltaKind::TurnFinished,
    );
    Ok(DriveResult {
        receipt_seen: receipt_seen
            && !outcome.interrupted
            && steering.sent_up_to <= receipt_input_cursor,
        outcome,
        user_cancelled: cancel.is_cancelled(),
        activity_seen: activity_seen || steering.sent_up_to > inbox_before,
        park,
        priced: state.priced,
        usage_seen: state.usage_seen,
        invoked: state.invoked,
        ran_tools: state.ran_tools,
        usage_before: state.usage_before,
        stream_usage: state.stream_usage,
        request_usage_seen: state.request_usage_seen,
        budget_stop,
        pricing: state.pricing,
        cleaned_up,
        uncommitted,
    })
}

fn observe_closing(
    outcome: &EngineOutcome,
    stopped: bool,
    sent: u64,
    seen: &mut bool,
    cursor: &mut u64,
) {
    debug_assert!(
        outcome
            .cost_usd
            .is_none_or(|cost| cost.is_finite() && cost >= 0.0)
    );
    debug_assert!(!outcome.closing_accounting || !outcome.prompt_not_submitted);
    // Once stopped without coverage, no later receipt can prove all output
    // was reported. A receipt already in hand remains valid until new work.
    if outcome.interrupted || (stopped && !*seen) {
        *seen = false;
    } else if outcome.closing_accounting {
        *seen = true;
        *cursor = sent;
    }
}

fn merge_reaped_outcome(held: Option<EngineOutcome>, mut receipt: EngineOutcome) -> EngineOutcome {
    debug_assert!(
        receipt
            .cost_usd
            .is_none_or(|cost| cost.is_finite() && cost >= 0.0)
    );
    debug_assert!(held.as_ref().is_none_or(|o| {
        o.cost_usd
            .is_none_or(|cost| cost.is_finite() && cost >= 0.0)
    }));
    let Some(mut held) = held else {
        return receipt;
    };
    receipt.is_error |= held.is_error;
    receipt.forced |= held.forced;
    receipt.interrupted |= held.interrupted;
    receipt.error = receipt.error.or(held.error.take());
    if receipt.usage.total_tokens() > 0 || held.usage.total_tokens() == 0 {
        return receipt;
    }
    held.is_error = receipt.is_error;
    held.forced = receipt.forced;
    held.interrupted = receipt.interrupted;
    held.error = receipt.error;
    held
}

fn merge_cleanup_receipt(outcome: &mut EngineOutcome, receipt: EngineOutcome) {
    debug_assert!(outcome.usage.cost_usd.is_finite() && outcome.usage.cost_usd >= 0.0);
    debug_assert!(receipt.usage.cost_usd.is_finite() && receipt.usage.cost_usd >= 0.0);
    raise_to(&mut outcome.usage, receipt.usage);
    outcome.usage.cost_usd = outcome.usage.cost_usd.max(receipt.usage.cost_usd);
    outcome.usage.merge_cost_coverage(receipt.usage);
    outcome.usage.estimated_cost_usd = match (
        outcome.usage.estimated_cost_usd,
        receipt.usage.estimated_cost_usd,
    ) {
        (Some(before), Some(after)) => Some(before.max(after)),
        (before, after) => before.or(after),
    };
    if receipt
        .cost_usd
        .is_some_and(|cost| outcome.cost_usd.is_none_or(|before| cost >= before))
    {
        outcome.cost_usd = receipt.cost_usd;
        outcome.cost_usage = receipt.cost_usage;
    }
    let class = crate::exhaustion::engine_error_class(receipt.error.as_deref().unwrap_or(""));
    if crate::exhaustion::recoverable_by_fallback(class) {
        outcome.error = receipt.error;
    }
    if receipt.text.as_deref().is_some_and(|text| !text.is_empty()) {
        outcome.text = receipt.text;
    }
    outcome.forced |= receipt.forced;
    outcome.interrupted |= receipt.interrupted;
}

fn interruption_cause(
    runtime: &Runtime,
    ctx: &TurnCtx,
    cancel: &CancellationToken,
    park: Option<&Park>,
    budget: Option<&Crossed>,
) -> Option<String> {
    let (stop_reason, request) = runtime
        .sched
        .lock()
        .running
        .get(&ctx.agent.id)
        .map(|running| (running.stop_reason.clone(), running.cancel_request.clone()))
        .unwrap_or_default();
    if let Some(reason) = stop_reason {
        return Some(format!("scheduler stop: {reason}"));
    }
    if let Some(request) = request {
        return Some(format!(
            "agent cancellation: {}",
            request.reason.as_deref().unwrap_or("cancel requested")
        ));
    }
    if runtime.shutdown.is_cancelled() {
        return Some("daemon stop or restart".into());
    }
    if let Some(budget) = budget {
        return Some(format!("budget stop: {}", budget.reason));
    }
    if cancel.is_cancelled() {
        return Some("turn interrupt".into());
    }
    if park.is_some_and(|park| park.stop_requested || park.killed()) {
        return Some("harness ended the turn".into());
    }
    None
}

#[cfg(test)]
mod receipt_tests {
    use super::*;

    #[test]
    fn only_cost_gap_metadata_leaves_a_run_unanswered() {
        let mut gap = Usage::default();
        gap.mark_cost_gap(aether_core::model::CostGap::Unreported);
        let mut run = super::super::execution::test_run();
        run.usage_seen = true;
        run.stream_usage = gap;
        assert!(is_cost_gap_only(gap));
        assert!(run.unanswered());
        for usage in [
            Usage {
                input_tokens: 1,
                ..gap
            },
            Usage {
                reasoning_tokens: Some(1),
                ..gap
            },
            Usage {
                cost_usd: 1.0,
                ..gap
            },
            Usage {
                estimated_cost_usd: Some(1.0),
                ..gap
            },
            Usage {
                cost_gap: Some(aether_core::model::CostGap::Interrupted),
                ..gap
            },
            Usage::default(),
        ] {
            run.stream_usage = usage;
            assert!(!is_cost_gap_only(usage));
            assert!(!run.unanswered());
        }
    }

    #[test]
    fn cleanup_keeps_cost_counters_paired_with_the_selected_dollar_receipt() {
        let mut outcome = EngineOutcome {
            cost_usd: Some(1.0),
            ..EngineOutcome::default()
        };
        let covered = Usage {
            input_tokens: 200,
            ..Usage::default()
        };
        merge_cleanup_receipt(
            &mut outcome,
            EngineOutcome {
                cost_usd: Some(2.0),
                cost_usage: Some(Box::new(covered)),
                ..EngineOutcome::default()
            },
        );
        assert_eq!(outcome.cost_usd, Some(2.0));
        assert_eq!(outcome.cost_usage.as_deref(), Some(&covered));
        merge_cleanup_receipt(
            &mut outcome,
            EngineOutcome {
                cost_usd: Some(1.0),
                cost_usage: Some(Box::default()),
                ..EngineOutcome::default()
            },
        );
        assert_eq!(outcome.cost_usd, Some(2.0));
        assert_eq!(outcome.cost_usage.as_deref(), Some(&covered));
    }

    #[test]
    fn synthetic_done_cannot_establish_or_replace_closing_accounting() {
        let mut seen = false;
        let mut cursor = 0;
        observe_closing(&EngineOutcome::default(), false, 7, &mut seen, &mut cursor);
        assert!(!seen);
        assert_eq!(cursor, 0);
        observe_closing(
            &EngineOutcome {
                closing_accounting: true,
                ..Default::default()
            },
            false,
            7,
            &mut seen,
            &mut cursor,
        );
        observe_closing(&EngineOutcome::default(), false, 9, &mut seen, &mut cursor);
        assert!(seen);
        assert_eq!(cursor, 7, "synthetic outcomes cannot cover newer input");
    }

    #[test]
    fn interrupted_outcome_never_counts_as_closing_accounting() {
        let mut seen = true;
        let mut cursor = 7;
        observe_closing(
            &EngineOutcome {
                closing_accounting: true,
                interrupted: true,
                ..Default::default()
            },
            false,
            9,
            &mut seen,
            &mut cursor,
        );
        assert!(!seen);
        assert_eq!(cursor, 7);
    }

    #[test]
    fn receipts_after_an_uncovered_stop_cannot_restore_coverage() {
        let mut seen = false;
        let mut cursor = 0;
        let receipt = EngineOutcome {
            closing_accounting: true,
            ..Default::default()
        };
        for _ in 0..2 {
            observe_closing(&receipt, true, 7, &mut seen, &mut cursor);
            assert!(!seen);
            assert_eq!(cursor, 0);
        }
    }

    #[test]
    fn a_stop_preserves_a_receipt_already_seen() {
        let mut seen = false;
        let mut cursor = 0;
        let receipt = EngineOutcome {
            closing_accounting: true,
            ..Default::default()
        };
        observe_closing(&receipt, false, 7, &mut seen, &mut cursor);
        observe_closing(&receipt, true, 7, &mut seen, &mut cursor);
        assert!(seen);
        assert_eq!(cursor, 7);
    }
}
