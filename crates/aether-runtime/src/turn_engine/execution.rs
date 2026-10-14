//! Native engine execution.
use super::outcome::{engine_failure_text, record_engine_incident};
use super::{
    Actor, AgentFilter, ApprovalRequest, Arc, CancellationToken, Commit, CompletionSource,
    ContentBlock, ContextInputs, DriveResult, EngineLink, EngineTurn, EngineTurnGuard,
    EngineTurnHandle, EngineVerdict, EventKind, MAX_ENGINE_ERROR, McpBridge, MessageRole,
    ModelResponse, Mutation, Next, PROTOCOL_NOTE_EVENT, ResolvedModel, Result, Runtime,
    RuntimeError, SHRUNK_CLOSING_USAGE, SubtreeUsage, Task, TaskFilter, ToolEffects, ToolSpec,
    TurnCtx, Usage, budget_crossed, clamp, contract, drive, engine_kind, engine_verdict, json,
    mpsc, names_exhaustion, now, prose_only_next, raise_to, recompute_snapshot_accounting,
    record_engine_failure, record_park, record_unanswered_prompt, tool_context,
};

pub(crate) async fn execute(
    runtime: &Arc<Runtime>,
    ctx: &mut TurnCtx,
    resolved: &ResolvedModel,
    model: &aether_core::model::ModelRef,
    cancel: CancellationToken,
) -> Result<EngineLink> {
    let Some(kind) = engine_kind(resolved.provider.protocol) else {
        return Err(RuntimeError::Internal("not an engine provider".into()));
    };
    let Some(engine) = runtime.engines.get(&kind).cloned() else {
        // Preflight recorded why, when it ran: too old or missing a
        // capability reads very differently from genuinely not installed,
        // and only the stored reason tells them apart.
        let reason = runtime
            .engine_unavailable
            .get(&kind)
            .cloned()
            .unwrap_or_else(|| {
                format!(
                    "{} is not installed on this machine; install it, sign in to {} from your client, and retry",
                    kind.as_str(),
                    resolved.provider.id
                )
            });
        return Ok(EngineLink::Done(Next::Failed(reason)));
    };
    let Some(bridge) = runtime.config.bridge.clone() else {
        return Ok(EngineLink::Done(Next::Failed(
            "engine bridge is not configured on this daemon".into(),
        )));
    };
    // Long-lived agents keep a settled task as `current_task`, so only a
    // settlement made during this turn may outrank a late provider failure.
    let task_open_at_start = ctx
        .task
        .as_ref()
        .is_some_and(|task| !task.status.is_terminal());

    // ---- context: same inputs as the model path; the engine keeps its own
    // history, so the snapshot records what we sent, not what it saw. ----
    let children = runtime.store.agents(&AgentFilter {
        parent: Some(ctx.agent.id),
        ..Default::default()
    })?;
    let child_tasks: Vec<Task> = runtime
        .store
        .tasks(&TaskFilter {
            run: Some(ctx.agent.run),
            ..Default::default()
        })?
        .into_iter()
        .filter(|t| t.creator == Actor::agent(ctx.agent.id))
        .collect();
    let queued_inbox = runtime.store.inbox(ctx.agent.id, ctx.agent.inbox_cursor)?;
    // Messages whose images would not fit one input line stay queued: they
    // are steered in mid-turn or open the next turn.
    let pending_inbox = aether_context::render::deliverable_prefix(&queued_inbox);
    ctx.user_reply_pending |= crate::turn::has_direct_user_message(pending_inbox);
    let pending_approvals: Vec<ApprovalRequest> = runtime
        .store
        .pending_approvals()?
        .into_iter()
        .filter(|a| a.agent_id == ctx.agent.id)
        .collect();
    // A handoff onto this engine owes a checkpoint: the previous model's
    // context is folded here, before the engine is told anything.
    let mut checkpoint = runtime.store.latest_checkpoint(ctx.agent.id)?;
    if let Some(trigger) = crate::turn::compaction_pending_for_turn(runtime, ctx)?
        && let Some(fresh) = crate::turn::compact(
            runtime,
            ctx,
            trigger,
            checkpoint.as_ref(),
            &child_tasks,
            &cancel,
        )
        .await?
    {
        checkpoint = Some(fresh);
    }
    // Claude Code records the system prompt on a conversation's first request
    // and reuses it verbatim on every resume (`--system-prompt-snapshot`,
    // default on), so a resumed turn cannot update it - the per-agent layers
    // reach the model only through the user turn (ARCHITECTURE.md 8).
    // Carry the checkpoint into every turn's inputs; `prepare_user_prompt`
    // decides whether the session has already been sent this exact text.
    let carried = checkpoint.clone();
    // The engine is sent only the new message and resumes its own history, so
    // the snapshot's tail starts at the checkpoint, as on the API path.
    let transcript_after = carried.as_ref().map(|c| c.provenance.to_seq).unwrap_or(0);
    let transcript = runtime.store.transcript(ctx.agent.id, transcript_after)?;
    // Both engines receive memory in the user turn, digest-gated like the
    // assignment. Rank on the task objective: per-message ranking would
    // reorder and resend the layer almost every turn. Memory relevant to a later message
    // but not to the objective is not surfaced mid-session.
    let memories = crate::turn::retrieve_memories(runtime, ctx, &[])?;
    let memory_records: Vec<_> = memories.into_iter().map(|m| m.record).collect();
    let tools: Vec<ToolSpec> = runtime.engine_tool_specs(&ctx.agent);
    let adapter = engine.adapter();
    // The assembler reads the model and effort off the agent; on a fallback link
    // they are this attempt's, not a change to the agent's config.
    let mut agent_for_link = ctx.agent.clone();
    model.clone_into(&mut agent_for_link.model);
    agent_for_link.config.effort = resolved.model.effort_for(ctx.agent.config.effort);
    let profile = runtime.model_profile(resolved);
    let context_inputs = ContextInputs {
        agent: &agent_for_link,
        task: ctx.task.as_ref(),
        children: &children,
        child_tasks: &child_tasks,
        pending_inbox,
        pending_approvals: &pending_approvals,
        checkpoint: carried.as_ref(),
        transcript: &transcript,
        memories: &memory_records,
        tools,
        model_profile: profile.as_deref(),
        engine_policy: None,
        capabilities: &resolved.model.capabilities,
        adapter: &adapter,
        attachment_dir: runtime.attachment_dir(),
        turn: ctx.turn,
        now: now(),
    };
    let status = aether_context::render::engine_runtime_state(&context_inputs);
    let assembled = runtime.context.assemble(context_inputs);
    let mut snapshot = assembled.snapshot;
    // Explicit (or model-default) effort is independent of registry capability
    // flags. None records that the native harness chooses, not an assumed value.
    snapshot.params.effort = agent_for_link.config.effort;
    if let Some(layer) = snapshot
        .layers
        .iter_mut()
        .find(|layer| layer.kind == aether_core::snapshot::LayerKind::RuntimeState)
    {
        layer.text = Some(status);
    }
    let message = if assembled.synthesized_task_message {
        // The hoisted assignment already carries the complete objective.
        // Use provenance, never text matching, to distinguish real user input.
        String::new()
    } else {
        assembled
            .new_user_message
            .as_ref()
            .map(|m| m.text())
            .filter(|text| !text.trim().is_empty())
            .unwrap_or_else(|| "Continue with your task.".into())
    };
    let images = assembled
        .new_user_message
        .as_ref()
        .map(|message| runtime.engine_images(message))
        .unwrap_or_default();
    // A retained session already holds what it was sent, so an unchanged
    // task or checkpoint is not repeated into it; the nonce marks this
    // turn's harness blocks as unforgeable by earlier-written content.
    let nonce = ulid::Ulid::generate().to_string();
    let prepared = contract::prepare_user_prompt(
        &mut snapshot,
        &message,
        contract::HoistPlan {
            retained: ctx.agent.engine_sent_layers(),
            nonce: &nonce,
        },
    );
    let prompt = prepared.prompt.clone();
    recompute_snapshot_accounting(&mut snapshot);
    let mut commit = Commit::new()
        .event(ctx.draft(EventKind::SnapshotCreated {
            snapshot: snapshot.id,
            turn: ctx.turn,
            model: snapshot.model.clone(),
            token_estimate: snapshot.token_estimate,
            layers: snapshot.layers.clone(),
        }))
        .mutate(Mutation::PutSnapshot(snapshot.clone()));
    if let Some(user) = &assembled.new_user_message {
        commit.push_event(ctx.draft(EventKind::AgentMessage {
            role: MessageRole::User,
            content: user.content.clone(),
            invocation: None,
        }));
    }
    if let Some(up_to) = assembled.consumed_up_to {
        ctx.agent.inbox_cursor = up_to;
        ctx.agent.updated_at = now();
        commit.push_event(ctx.draft(EventKind::InboxConsumed {
            up_to,
            items: assembled.consumed_inbox.clone(),
        }));
    }
    // Scoped: the hold is not `Send`. `agent_row` and `touch` read rows this
    // commit writes back, so both run under it.
    {
        let hold = runtime.hold();
        if assembled.consumed_up_to.is_some() {
            commit.push_mutation(Mutation::UpsertAgent(crate::turn::agent_row(
                runtime, ctx, &hold,
            )?));
        }
        if !snapshot.memory_ids.is_empty() {
            let touched = runtime.memory.touch(&hold, &snapshot.memory_ids)?;
            commit.mutations.extend(touched.mutations);
        }
        runtime.apply_held(hold, commit)?;
    }

    let system_prompt = contract::system_prompt(&snapshot);
    let tool_ctx = Arc::new(tool_context(runtime, ctx));
    let mut turn = EngineTurn {
        prompt,
        images,
        system_prompt,
        session: ctx.agent.engine_session.clone(),
        session_usage: ctx.agent.engine_session_usage(),
        new_session_id: aether_engine::new_session_id(),
        model: Some(resolved.model.id.clone()),
        // Launch the same explicit override (or omission) the ledger recorded.
        effort: snapshot.params.effort,
        cwd: tool_ctx.cwd.clone(),
        extra_dirs: ctx.agent.scope.allowed_paths.clone(),
        hidden_env: runtime.tools.engine_hidden_env().to_vec(),
        tool_hidden_env: runtime.tools.hidden_env().to_vec(),
        // No approval requirements at all means the engine may run freely.
        summary_only: false,
        lean_host_context: ctx.agent.config.host_context.is_lean(),
        network_access: ctx.agent.scope.network == aether_core::agent::NetworkPolicy::Allowed,
        bypass_permissions: ctx.agent.config.approval.require_for.is_empty(),
        mcp: None,
    };

    let mut effects = ToolEffects::default();
    // The last context size the engine reported, from the turn before this
    // one - `record_context_usage` only overwrites it once this turn reports
    // its own, so it is still last turn's figure here.
    let context_before_turn = ctx.agent.stats.context_tokens;
    let mut backoff = super::transient::Backoff::new(
        runtime.config.engine_transient_retry_window,
        runtime.config.engine_transient_retry_floor,
    );
    let mut gave_up = None;
    // The final run was billed before its backoff; billing it again below
    // would count it twice.
    let mut run_settled = false;
    let mut subtree = SubtreeUsage::default();
    // What the next attempt is sent; a recheck wraps it, never itself.
    let mut pending_prompt = turn.prompt.clone();
    // An earlier attempt of this turn reached the model, so its session holds
    // the prompt: it may only be continued, never sent the prompt again.
    let mut delivered = false;
    let mut earlier_tools = false;
    let mut recovered_session = false;
    let mut run = loop {
        if cancel.is_cancelled() {
            return Ok(EngineLink::Done(Next::Cancelled(Some("cancelled".into()))));
        }
        let token = ulid::Ulid::generate().to_string();
        turn.mcp = Some(McpBridge {
            command: bridge.command.clone(),
            args: vec![
                "--socket".into(),
                bridge.socket.to_string_lossy().into_owned(),
                "mcp-bridge".into(),
                "--agent".into(),
                ctx.agent.id.to_string(),
                "--token".into(),
                token.clone(),
            ],
        });
        let (req_tx, mut req_rx) = mpsc::channel(16);
        let registered = EngineTurnGuard::register(
            runtime,
            ctx.agent.id,
            EngineTurnHandle { token, tx: req_tx },
        );
        let mut run = drive(
            runtime,
            ctx,
            engine.as_ref(),
            turn.clone(),
            &snapshot,
            &tool_ctx,
            &mut req_rx,
            &mut effects,
            resolved,
            &cancel,
        )
        .await?;
        let pending_request = !req_rx.is_empty();
        drop(registered);
        drop(req_rx);
        let class = crate::exhaustion::engine_error_class(
            run.outcome.error.as_deref().unwrap_or("unknown error"),
        );
        // A crossed grant is not a lost session: retrying would spend past a
        // budget that is already gone. Nor is a transient failure to reopen
        // it: that session still holds its context and is waited out below.
        if run.park.is_none()
            && run.budget_stop.is_none()
            && !cancel.is_cancelled()
            && !run.outcome.interrupted
            && run.outcome.session_lost
            && !crate::exhaustion::recoverable_by_fallback(class)
            && !crate::exhaustion::native_transient(class)
            && run.unanswered()
            && !pending_request
            && !recovered_session
            && !delivered
            && turn.session.is_some()
        {
            recovered_session = true;
            tracing::warn!(agent = %ctx.agent.id, "engine session could not be resumed; starting a fresh one");
            turn.session = None;
            turn.session_usage = Usage::default();
            ctx.agent.engine_session = None;
            // The refused session retained nothing, so anything left out of
            // this prompt because it already held it must go in after all.
            // The snapshot is rewritten to what is really sent, its tail and
            // token_estimate recomputed over the rewritten messages, and the
            // `SnapshotCreated` re-announced so a store rebuilt from events
            // matches the stored row instead of the trimmed turn's record.
            ctx.agent.engine_sent = None;
            if turn.prompt != prepared.full_prompt {
                turn.prompt = contract::resend_everything(&mut snapshot, &prepared);
                pending_prompt.clone_from(&turn.prompt);
                recompute_snapshot_accounting(&mut snapshot);
                runtime.apply(
                    Commit::new()
                        .event(ctx.draft(EventKind::SnapshotCreated {
                            snapshot: snapshot.id,
                            turn: ctx.turn,
                            model: snapshot.model.clone(),
                            token_estimate: snapshot.token_estimate,
                            layers: snapshot.layers.clone(),
                        }))
                        .mutate(Mutation::PutSnapshot(snapshot.clone())),
                )?;
            }
            let hold = runtime.hold();
            let row = crate::turn::agent_row_with_session(runtime, ctx, &hold)?;
            runtime.apply_held(hold, Commit::new().mutate(Mutation::UpsertAgent(row)))?;
            continue;
        }
        let resume = match super::transient::retry_for(
            &run,
            class,
            pending_request,
            cancel.is_cancelled(),
            ctx.agent.engine_session.is_some(),
        ) {
            super::transient::Retry::No => break run,
            super::transient::Retry::Unconfirmed => {
                gave_up = Some(super::transient::UNCONFIRMED_CLEANUP.to_owned());
                break run;
            }
            super::transient::Retry::Resume(resume) => resume,
        };
        backoff.failed();
        if backoff.spent() {
            gave_up = Some(backoff.gave_up(class));
            break run;
        }
        // Billed before the wait, so a grant its closing receipt crossed ends
        // the turn here instead of authorizing another launch.
        if !run.unanswered() {
            settle_usage(runtime, ctx, &run)?;
            run_settled = true;
        }
        if let Some(crossed) = budget_crossed(runtime, ctx, &mut subtree)? {
            run.budget_stop = Some(crossed);
            break run;
        }
        let error = engine_failure_text(engine.kind(), run.outcome.error.as_deref());
        record_engine_failure(runtime, ctx, &snapshot, &error, class, true)?;
        let failure = super::transient::Failure {
            engine: engine.kind(),
            class,
            error: &error,
            resume,
        };
        match super::transient::wait(runtime, ctx, &failure, &mut backoff, &cancel).await? {
            super::transient::Waited::Ready => {}
            super::transient::Waited::Cancelled => break run,
            // Its retryable record stands; the terminal one below records
            // the decision to stop, which ancestors are notified from.
            super::transient::Waited::WindowSpent => {
                gave_up = Some(backoff.gave_up(class));
                break run;
            }
        }
        // Descendants may have spent while this turn waited.
        if let Some(crossed) = budget_crossed(runtime, ctx, &mut subtree)? {
            run.budget_stop = Some(crossed);
            break run;
        }
        earlier_tools |= run.ran_tools;
        match resume {
            super::transient::Resume::Replay => {}
            super::transient::Resume::Continue => {
                delivered = true;
                pending_prompt = super::transient::continuation_note(class);
                turn.prompt.clone_from(&pending_prompt);
                // The session holds the message's images with its prompt.
                turn.images.clear();
            }
            super::transient::Resume::Recheck => {
                delivered = true;
                turn.prompt = super::transient::recheck_note(class, &pending_prompt);
            }
        }
        run_settled = false;
        turn.session.clone_from(&ctx.agent.engine_session);
        if turn.session.is_none() {
            // Nothing reported a session, so nothing in one can be lost; a
            // fresh id keeps a half-created one from colliding with the retry.
            turn.new_session_id = aether_engine::new_session_id();
        }
        turn.session_usage = ctx.agent.engine_session_usage();
    };
    // A sharp context drop between turns is a cheap signal that the engine's
    // own compaction replaced retained turns with a summary -
    // not a proof, but enough to make the next turn resend everything.
    const CONTEXT_DROP_RATIO: u64 = 2;
    let context_likely_compacted = matches!(
        (context_before_turn, ctx.agent.stats.context_tokens),
        (Some(before), Some(after)) if before > 0 && after < before / CONTEXT_DROP_RATIO
    );
    // The prompt reached the engine, so the session now holds these layers -
    // unless the check above says its own compaction likely discarded them.
    // Tagged with the session: one the engine replaced retains none of them.
    ctx.agent.engine_sent =
        match context_likely_compacted || (run.outcome.is_error && run.unanswered()) {
            true => None,
            false => ctx.agent.engine_session.clone().map(|session| {
                aether_core::agent::EngineSentLayers {
                    session,
                    layers: prepared.sent,
                }
            }),
        };
    if !run_settled {
        settle_usage(runtime, ctx, &run)?;
    }
    // Giving up reports the outage, but a grant the settled receipts or the
    // subtree crossed meanwhile still ends the agent as a budget stop.
    if gave_up.is_some()
        && run.budget_stop.is_none()
        && let Some(crossed) = budget_crossed(runtime, ctx, &mut subtree)?
    {
        run.budget_stop = Some(crossed);
    }
    let DriveResult {
        outcome,
        park,
        usage_seen,
        invoked,
        ran_tools,
        budget_stop,
        ..
    } = run;

    // A grant crossed while the turn ran ends the agent through the same path
    // the scheduler's sweep uses: the `budget.exceeded` event, the live
    // subtree cancelled with it, and a terminal status naming the budget -
    // never an engine failure, and never a bare cancel.
    // One decision, so the precedence cannot be split apart at a rebase.
    // A turn whose end a harness action already recorded reports that end,
    // not the cancel its own stop raised.
    let next = effects.next.take();
    // Every harness action that sets `effects.next` starts a park with it
    // (`ToolEffects::parked()` gates `start_park` in `drive`); a `Next` with
    // no park to carry it would fall through and drop the turn's own decision.
    debug_assert!(
        next.is_none() || park.is_some(),
        "a harness action recorded a Next without a park to carry it"
    );
    let hard_exhaustion = outcome.error.as_deref().is_some_and(names_exhaustion);
    match engine_verdict(budget_stop, park, next, cancel.is_cancelled()) {
        EngineVerdict::Budget(crossed, park) => {
            if hard_exhaustion {
                record_engine_incident(
                    runtime,
                    ctx,
                    &snapshot,
                    engine.kind(),
                    outcome.error.as_deref(),
                )?;
            }
            // The recorded end is gone - the budget stop is terminal - but
            // the wind-down still happened, and an engine that ignored it is
            // exactly what this event is for.
            if let Some(park) = park {
                record_park(runtime, ctx, engine.kind(), &park, &outcome)?;
            }
            crate::scheduler::stop_for_budget(
                runtime,
                ctx.agent.id,
                crossed.usage,
                &crossed.reason,
                // The turn found it, not the sweep; the ledger says which.
                Actor::Agent { id: ctx.agent.id },
            )?;
            return Ok(EngineLink::Done(Next::Failed(crossed.reason)));
        }
        EngineVerdict::Parked(park, next) => {
            record_park(runtime, ctx, engine.kind(), &park, &outcome)?;
            let settled_this_turn = task_open_at_start
                && ctx
                    .task
                    .as_ref()
                    .is_some_and(|task| task.status.is_terminal());
            let decided = matches!(
                next,
                Next::Completed(_) | Next::Failed(_) | Next::Cancelled(_)
            );
            // A late hard exhaustion overrides only an open decision (a wait).
            if !hard_exhaustion || decided || settled_this_turn || cancel.is_cancelled() {
                if hard_exhaustion {
                    record_engine_incident(
                        runtime,
                        ctx,
                        &snapshot,
                        engine.kind(),
                        outcome.error.as_deref(),
                    )?;
                }
                return Ok(EngineLink::Done(next));
            }
        }
        EngineVerdict::Cancelled => {
            if hard_exhaustion {
                record_engine_incident(
                    runtime,
                    ctx,
                    &snapshot,
                    engine.kind(),
                    outcome.error.as_deref(),
                )?;
            }
            return Ok(EngineLink::Done(Next::Cancelled(Some("cancelled".into()))));
        }
        EngineVerdict::Ran => {}
    }
    if outcome.interrupted && !hard_exhaustion {
        runtime.apply(Commit::new().event(ctx.draft(EventKind::Custom {
            name: "engine.interrupted".into(),
            payload: json!({
                "engine": engine.kind().as_str(),
                // The sibling engine.parked event already clamps its
                // reason; an adapter error string must not be let through
                // unbounded here just because this path renders it too.
                "reason": outcome.error.as_deref().map(|e| clamp(e, MAX_ENGINE_ERROR)),
            }),
        })))?;
        return Ok(EngineLink::Done(Next::Interrupted));
    }
    if outcome.is_error || hard_exhaustion {
        let mut error = engine_failure_text(engine.kind(), outcome.error.as_deref());
        if let Some(gave_up) = gave_up {
            error = format!("{error} ({gave_up})");
        }
        let class = crate::exhaustion::engine_error_class(
            outcome.error.as_deref().unwrap_or("unknown error"),
        );
        record_engine_failure(runtime, ctx, &snapshot, &error, class, false)?;
        if crate::exhaustion::recoverable_by_fallback(class) {
            return Ok(EngineLink::Exhausted { class, error });
        }
        return Ok(EngineLink::Done(Next::Failed(error)));
    }
    // Nothing said and nothing billed: the model never saw the prompt. The
    // blank reply below then counts as a silent turn, never as an answer.
    let blank = outcome.text.as_deref().is_none_or(|t| t.trim().is_empty());
    if blank && !delivered && !invoked && !usage_seen && outcome.usage.total_tokens() == 0 {
        record_unanswered_prompt(runtime, ctx, engine.kind(), &snapshot)?;
    }
    // `effects.next` was taken above (`next`) and never reassigned since, so
    // it is always `None` here - handled by the `match` above, not here.
    let response = ModelResponse {
        content: outcome
            .text
            .map(|t| vec![ContentBlock::Text { text: t }])
            .unwrap_or_default(),
        ..Default::default()
    };
    // The CLI runs tools inside the turn, so one that worked is no idle turn
    // even when its final message is blank.
    prose_only_next(
        runtime,
        ctx,
        &response,
        ran_tools || earlier_tools,
        CompletionSource::NativeTurn,
    )
    .map(EngineLink::Done)
}

/// Fold one engine run's closing totals into the agent's usage and commit it.
/// Each run reconciles only its own provisional, streamed accounting.
pub(super) fn settle_usage(
    runtime: &Arc<Runtime>,
    ctx: &mut TurnCtx,
    run: &DriveResult,
) -> Result<()> {
    ctx.agent.stats.turns = ctx.agent.stats.turns.max(ctx.turn);
    ctx.agent.stats.last_turn_at = Some(now());
    let reconciled;
    if let Some((before, turn_before)) = run.usage_before {
        let final_usage = closing_usage(runtime, ctx, run)?;
        reconciled = true;
        ctx.agent.stats.usage = before;
        ctx.turn_usage = turn_before;
        ctx.add_usage(final_usage);
        debug_assert!(ctx.agent.stats.usage.total_tokens() >= before.total_tokens());
        debug_assert!(ctx.agent.stats.usage.cost_usd >= before.cost_usd);
    } else {
        let fallback = fallback_usage(run);
        reconciled = fallback != Usage::default();
        ctx.add_usage(fallback);
    }
    ctx.agent.updated_at = now();
    let hold = runtime.hold();
    let row = crate::turn::agent_row(runtime, ctx, &hold)?;
    let mut commit = Commit::new().mutate(Mutation::UpsertAgent(row));
    if reconciled {
        commit.push_event(ctx.draft(EventKind::AgentUsageUpdated {
            total: ctx.agent.stats.usage,
            turn_usage: ctx.turn_usage,
        }));
    }
    runtime.apply_held(hold, commit)?;

    Ok(())
}

/// Closing receipts cover failures, but caller cancellation can abort unreported output.
fn interrupted_accounting(run: &DriveResult) -> bool {
    (!run.receipt_seen || run.user_cancelled)
        && !(run.outcome.prompt_not_submitted && run.unanswered())
}

fn fallback_usage(run: &DriveResult) -> Usage {
    debug_assert!(
        run.usage_before.is_none(),
        "fallback has no streamed baseline"
    );
    debug_assert!(run.outcome.cost_usd.is_none_or(|cost| cost >= 0.0));
    let unsubmitted = run.outcome.prompt_not_submitted && run.unanswered();
    let mut usage = Usage::default();
    if !run.usage_seen && run.outcome.usage.total_tokens() > 0 {
        usage = run.outcome.usage;
        let coverage = usage;
        run.pricing.apply(&mut usage);
        if !coverage.cost_complete {
            usage.set_cost(None);
            usage.cost_gap = coverage
                .cost_gap
                .or(Some(aether_core::model::CostGap::Unreported));
        }
        if let Some(cost) = run.outcome.cost_usd {
            usage.estimated_cost_usd = None;
            usage.set_cost(Some(cost));
        }
    } else if !run.priced && !unsubmitted {
        if run.outcome.cost_usd.is_some() {
            usage += Usage::cost_only(run.outcome.cost_usd);
        } else if !run.usage_seen && fallback_has_work(run) {
            usage.mark_cost_gap(aether_core::model::CostGap::Unreported);
        }
    }
    if interrupted_accounting(run) {
        usage.mark_cost_gap(aether_core::model::CostGap::Interrupted);
    }
    usage
}

fn fallback_has_work(run: &DriveResult) -> bool {
    run.invoked
        || !run.outcome.usage.cost_complete
        || run.ran_tools
        || run
            .outcome
            .text
            .as_deref()
            .is_some_and(|text| !text.trim().is_empty())
        || run
            .outcome
            .usage
            .reasoning_tokens
            .is_some_and(|tokens| tokens > 0)
}

/// Reconcile closing counters and cost against this run's streamed accounting.
fn closing_usage(runtime: &Arc<Runtime>, ctx: &TurnCtx, run: &DriveResult) -> Result<Usage> {
    debug_assert!(
        run.usage_before.is_some(),
        "reconciliation needs a run baseline"
    );
    debug_assert!(run.usage_seen, "the baseline belongs to streamed usage");
    let mut usage = if run.outcome.usage.total_tokens() > 0 {
        run.outcome.usage
    } else {
        run.stream_usage
    };
    let closing_tokens = usage.total_tokens();
    // The engine is untrusted: a closing receipt below the stream must not
    // shrink a turn back under a grant it already crossed.
    let raised = raise_to(&mut usage, run.stream_usage);
    let billed = run.outcome.cost_usd.or_else(|| {
        run.stream_usage
            .cost_complete
            .then_some(run.stream_usage.cost_usd)
    });
    usage.set_cost(billed);
    if billed.is_none() {
        usage.cost_usd = run.stream_usage.cost_usd;
        usage.cost_gap = run
            .stream_usage
            .cost_gap
            .or(Some(aether_core::model::CostGap::Unreported));
    }
    // Preserve request estimates; merged counters cannot select prompt tiers.
    usage.estimated_cost_usd = match billed {
        Some(_) => None,
        None if run.request_usage_seen => {
            run.pricing.request_estimate_of(&usage, &run.stream_usage)
        }
        None => run.pricing.estimate_of(&usage),
    };
    if run.outcome.cost_usd.is_none() {
        usage.cost_basis = if usage.estimated_cost_usd.is_some() {
            aether_core::model::CostBasis::ListPrice
        } else {
            run.stream_usage.cost_basis
        };
    } else if raised {
        run.pricing
            .price_closing_shortfall(&mut usage, &run.outcome.usage);
    }
    if interrupted_accounting(run) {
        usage.mark_cost_gap(aether_core::model::CostGap::Interrupted);
    }
    if raised {
        record_closing_shortfall(runtime, ctx, usage, closing_tokens)?;
    }
    Ok(usage)
}

fn record_closing_shortfall(
    runtime: &Arc<Runtime>,
    ctx: &TurnCtx,
    usage: Usage,
    closing_tokens: u64,
) -> Result<()> {
    debug_assert!(usage.total_tokens() >= closing_tokens);
    debug_assert!(usage.cost_complete || usage.cost_gap.is_some());
    let missing = if usage.total_tokens() == closing_tokens {
        "only reasoning detail was missing, the cost is unaffected"
    } else if usage.cost_complete {
        "the missing part is priced from the stream"
    } else {
        "the missing part is left out of the billed cost"
    };
    let note = format!("{SHRUNK_CLOSING_USAGE}; {missing}");
    tracing::warn!(agent = %ctx.agent.id, "{note}");
    runtime.apply(Commit::new().event(ctx.draft(EventKind::Custom {
        name: PROTOCOL_NOTE_EVENT.into(),
        payload: json!({ "note": note }),
    })))?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_previous_receipt_keeps_its_dollars_without_covering_later_activity() {
        let mut run = DriveResult {
            outcome: aether_engine::EngineOutcome {
                cost_usd: Some(2.0),
                usage: Usage {
                    input_tokens: 17,
                    ..Usage::default()
                },
                ..Default::default()
            },
            receipt_seen: false,
            activity_seen: true,
            ..test_run()
        };
        let partial = fallback_usage(&run);
        assert_eq!(partial.cost_usd, 2.0);
        assert_eq!(partial.total_tokens(), 17);
        assert_eq!(
            partial.cost_gap,
            Some(aether_core::model::CostGap::Interrupted)
        );
        assert!(!partial.cost_complete);
        run.receipt_seen = true;
        let complete = fallback_usage(&run);
        assert_eq!(complete.cost_usd, partial.cost_usd);
        assert_eq!(complete.total_tokens(), partial.total_tokens());
        assert!(complete.cost_complete);
        assert_eq!(complete.cost_gap, None);
    }

    #[tokio::test]
    async fn an_interrupted_mixed_stream_keeps_its_known_dollars() {
        let (runtime, _dir) = crate::test_runtime(|_| {}).await;
        let mut ctx = super::super::tests::started_agent_ctx(&runtime).await;
        let before = (ctx.agent.stats.usage, ctx.turn_usage);
        let mut stream = Usage {
            input_tokens: 17,
            cost_usd: 2.0,
            ..Usage::default()
        };
        stream.mark_cost_gap(aether_core::model::CostGap::Unpriced);
        ctx.add_usage(stream);
        let run = DriveResult {
            receipt_seen: false,
            activity_seen: true,
            priced: true,
            usage_seen: true,
            usage_before: Some(before),
            stream_usage: stream,
            ..test_run()
        };
        settle_usage(&runtime, &mut ctx, &run).unwrap();
        assert_eq!(ctx.turn_usage.cost_usd, 2.0);
        assert_eq!(ctx.turn_usage.total_tokens(), 17);
        assert_eq!(
            ctx.turn_usage.cost_gap,
            Some(aether_core::model::CostGap::Mixed)
        );
        assert!(ctx.agent.stats.usage.cost_usd >= before.0.cost_usd + 2.0);
        assert!(ctx.agent.stats.usage.total_tokens() >= before.0.total_tokens() + 17);
        runtime.shutdown().await;
    }
}

#[cfg(test)]
mod fallback_regressions {
    use super::*;

    #[test]
    fn never_invoked_empty_run_stays_complete() {
        let usage = fallback_usage(&test_run());
        assert!(usage.cost_complete);
        assert_eq!(usage, Usage::default());
    }

    #[test]
    fn done_only_tokens_use_the_billed_rate() {
        let mut run = test_run();
        run.outcome.usage.input_tokens = 100;
        run.pricing.billed = Some(aether_core::model::ModelCost {
            input_per_mtok: 2.0,
            output_per_mtok: 4.0,
            cache_read_per_mtok: 0.0,
            cache_write_per_mtok: 0.0,
            above_threshold: None,
        });
        let usage = fallback_usage(&run);
        assert!(usage.cost_complete);
        assert_eq!(usage.cost_usd, 0.0002);
        assert_eq!(usage.estimated_cost_usd, None);
    }

    #[test]
    fn done_only_text_without_usage_is_unreported() {
        let mut run = test_run();
        run.outcome.text = Some("answer".into());
        let usage = fallback_usage(&run);
        assert_eq!(
            usage.cost_gap,
            Some(aether_core::model::CostGap::Unreported)
        );
        assert!(!usage.cost_complete);
        assert_eq!(usage.total_tokens(), 0);
    }

    #[test]
    fn inexact_done_only_counters_stay_unreported() {
        let mut run = test_run();
        run.outcome.usage.input_tokens = 100;
        run.outcome.usage.cost_complete = false;
        run.pricing.billed = Some(aether_core::model::ModelCost {
            input_per_mtok: 2.0,
            ..Default::default()
        });
        let usage = fallback_usage(&run);
        assert_eq!(
            usage.cost_gap,
            Some(aether_core::model::CostGap::Unreported)
        );
        assert!(!usage.cost_complete);
        assert_eq!(usage.cost_usd, 0.0);
    }

    #[test]
    fn interrupted_unpriced_fallback_combines_both_gaps() {
        let mut run = test_run();
        run.receipt_seen = false;
        run.invoked = true;
        run.outcome.usage.input_tokens = 100;
        let usage = fallback_usage(&run);
        assert_eq!(usage.cost_gap, Some(aether_core::model::CostGap::Mixed));
        assert_eq!(usage.total_tokens(), 100);
    }
}

#[cfg(test)]
pub(super) fn test_run() -> DriveResult {
    DriveResult {
        outcome: aether_engine::EngineOutcome::default(),
        receipt_seen: true,
        user_cancelled: false,
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
        pricing: super::TurnPricing::default(),
        cleaned_up: true,
    }
}
