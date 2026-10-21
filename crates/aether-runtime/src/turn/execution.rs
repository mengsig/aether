use super::*;

pub(super) fn latest_invocation_id(
    runtime: &Runtime,
    agent: AgentId,
) -> Result<Option<InvocationId>> {
    let recent = runtime.store.latest_events(
        1,
        &EventFilter {
            types: vec!["invocation.completed".into(), "invocation.failed".into()],
            ..EventFilter::own_events(agent)
        },
    )?;
    Ok(recent.first().and_then(|e| match &e.kind {
        EventKind::InvocationCompleted { invocation, .. }
        | EventKind::InvocationFailed { invocation, .. } => Some(*invocation),
        _ => None,
    }))
}

/// One attempt at one link of the model chain.
pub(super) enum LinkOutcome {
    /// The turn reached a conclusion on this link.
    Done(Next),
    /// The link failed in a way the next link may survive.
    FellBack,
}

pub(super) async fn execute_inner(
    runtime: &Arc<Runtime>,
    ctx: &mut TurnCtx,
    cancel: CancellationToken,
) -> Result<Next> {
    // ---- 2. settle tool calls whose approvals were resolved -----------------
    crate::build_cache::claim(runtime, &ctx.agent)?;
    let tool_ctx = Arc::new(tool_context(runtime, ctx));
    settle_resolved_approvals(runtime, ctx, &tool_ctx, &cancel).await?;
    if cancel.is_cancelled() {
        return Ok(Next::Cancelled(Some("cancelled".into())));
    }

    // A stop may land after tool outcomes but before their transcript message.
    compose_tool_results(runtime, ctx)?;

    if let Some(next) = manual_compaction(runtime, ctx, &cancel).await? {
        return Ok(next);
    }

    // ---- 2b. walk the model chain (VISION §4.5) ------------------------------
    let chain = ctx.agent.chain();
    let links: Vec<aether_core::model::ModelRef> = chain.links().cloned().collect();
    for (position, model) in links.iter().enumerate() {
        let rest = &links[position + 1..];
        // A link the scheduler would not have admitted - backed off, or at its
        // provider's cap - would only fail or overrun the limit. Claiming it
        // is the same check, under the lock that also records the move.
        if let Err(reason) = crate::scheduler::claim_link(runtime, ctx.agent.id, &model.provider) {
            match rest.first() {
                Some(next) => {
                    announce_fallback(runtime, ctx, model, next, reason, None)?;
                    continue;
                }
                // Nowhere left to go: hand the turn back unspent rather than
                // run past the cap. The scheduler re-admits the agent once a
                // slot or the backoff frees, as a single-link agent already is.
                None => {
                    tracing::info!(%model, %reason, "the last link cannot carry the turn yet");
                    crate::scheduler::hold_after_entry_refusal(runtime, ctx.agent.id);
                    return unanswered_turn(runtime, ctx, &reason);
                }
            }
        }
        match run_link(runtime, ctx, &tool_ctx, model, rest, cancel.clone()).await? {
            LinkOutcome::Done(next) => return Ok(next),
            LinkOutcome::FellBack => continue,
        }
    }
    // A link only falls back while a next link exists, so the last one always
    // concludes; reaching here would mean the chain lost its verdict.
    Err(RuntimeError::Internal(format!(
        "model chain `{chain}` ended without a verdict"
    )))
}

async fn manual_compaction(
    runtime: &Arc<Runtime>,
    ctx: &mut TurnCtx,
    cancel: &CancellationToken,
) -> Result<Option<Next>> {
    let inbox = runtime.store.inbox(ctx.agent.id, ctx.agent.inbox_cursor)?;
    let is_request = |item: &aether_core::inbox::InboxItem| matches!(&item.kind, InboxKind::Wake { reason } if reason == crate::commands::COMPACTION_WAKE);
    if !inbox.iter().any(is_request) {
        return Ok(None);
    }
    // Only a leading run is read: the cursor cannot skip an item the restored wait still defers.
    let requests = inbox.iter().take_while(|item| is_request(item)).count();
    let needs_reply = match &ctx.agent.resume_wait {
        // The request alone woke this wait; anything else reaches it as the scheduler decides.
        Some(wait) => {
            !wait.involves_approval()
                && crate::scheduler::urgent_inbox(runtime, &ctx.agent)?.is_some()
        }
        None if requests < inbox.len() => true,
        None => crate::scheduler::has_transcript_work(runtime, &ctx.agent)?,
    };
    if needs_reply {
        if requests == inbox.len() {
            settle_requests(runtime, ctx, &inbox)?;
        }
        return Ok(None);
    }
    if let Some(request) = compaction_pending_for_turn(runtime, ctx)? {
        let checkpoint = runtime.store.latest_checkpoint(ctx.agent.id)?;
        let tasks = runtime
            .store
            .tasks(&TaskFilter {
                run: Some(ctx.agent.run),
                ..Default::default()
            })?
            .into_iter()
            .filter(|task| task.creator == Actor::agent(ctx.agent.id))
            .collect::<Vec<_>>();
        compact(runtime, ctx, request, checkpoint.as_ref(), &tasks, cancel).await?;
    }
    if cancel.is_cancelled() {
        // A daemon stop leaves the request unread for the next start; an operator stop ends it.
        if !ctx.hard_cancel.is_cancelled() {
            settle_requests(runtime, ctx, &inbox[..requests])?;
        }
        return Ok(Some(Next::Cancelled(Some("cancelled".into()))));
    }
    settle_requests(runtime, ctx, &inbox[..requests])?;
    Ok(Some(Next::Waiting(
        ctx.agent
            .resume_wait
            .clone()
            .unwrap_or(WaitCondition::Inbox),
    )))
}

/// Count the maintenance turn and read `items`, the requests it served.
fn settle_requests(
    runtime: &Runtime,
    ctx: &mut TurnCtx,
    items: &[aether_core::inbox::InboxItem],
) -> Result<()> {
    let hold = runtime.hold();
    ctx.agent.stats.turns = ctx.agent.stats.turns.max(ctx.turn);
    ctx.agent.updated_at = now();
    let mut commit = Commit::new();
    if let Some(last) = items.last() {
        ctx.agent.inbox_cursor = last.seq;
        commit.push_event(ctx.draft(EventKind::InboxConsumed {
            up_to: last.seq,
            items: items.iter().map(|item| item.id).collect(),
        }));
    }
    commit.push_mutation(Mutation::UpsertAgent(agent_row_with_session(
        runtime, ctx, &hold,
    )?));
    runtime.apply_held(hold, commit)?;
    Ok(())
}

/// A link that could never have carried this turn: move to the next one, and
/// fail only when there is no next one. `cause` is Aether's own account of
/// why the link is unusable - a registry or credential fact, never provider
/// body text.
pub(super) fn unusable_link(
    runtime: &Runtime,
    ctx: &TurnCtx,
    model: &aether_core::model::ModelRef,
    rest: &[aether_core::model::ModelRef],
    cause: String,
) -> Result<LinkOutcome> {
    // A credential helper's own output can reach this string.
    let cause = aether_model::http::safe_provider_text(&cause, 500);
    let Some(next) = rest.first() else {
        return Ok(LinkOutcome::Done(Next::Failed(format!(
            "model unavailable: {cause}"
        ))));
    };
    announce_fallback(
        runtime,
        ctx,
        model,
        next,
        format!("unusable: {cause}"),
        None,
    )?;
    Ok(LinkOutcome::FellBack)
}

/// Record that a turn is moving to the next link, so the move is visible in
/// the log, the inspector and any client following events.
pub(super) fn announce_fallback(
    runtime: &Runtime,
    ctx: &TurnCtx,
    from: &aether_core::model::ModelRef,
    to: &aether_core::model::ModelRef,
    reason: String,
    class: Option<ModelErrorClass>,
) -> Result<()> {
    tracing::info!(%from, %to, %reason, "falling back to the next model in the chain");
    crate::scheduler::record_fallback(runtime, &from.provider, to, &reason, class);
    runtime.apply(Commit::new().event(ctx.draft(EventKind::ProviderFallback {
        from: from.clone(),
        to: to.clone(),
        reason,
        class,
    })))?;
    Ok(())
}

/// A native session belongs to the engine that created it, not to every
/// configured fallback. Rebuild across that boundary from retained records;
/// never ask the exhausted provider to compact before trying a healthy link.
pub(super) fn prepare_fallback_session(
    runtime: &Runtime,
    ctx: &mut TurnCtx,
    model: &ModelRef,
) -> Result<()> {
    let Some(session) = ctx.agent.engine_session.clone() else {
        return Ok(());
    };
    let recent = runtime.store.latest_events(
        1,
        &EventFilter {
            types: vec!["snapshot.created".into()],
            ..EventFilter::own_events(ctx.agent.id)
        },
    )?;
    let Some(EventKind::SnapshotCreated { model: from, .. }) =
        recent.first().map(|event| &event.kind)
    else {
        return Ok(());
    };
    if runtime.engine_of(from) == runtime.engine_of(model) {
        return Ok(());
    }
    ctx.agent.engine_session = None;
    ctx.agent.engine_sent = None;
    let lost = format!(
        "Left native session {} for another engine; the turn rebuilds from the retained \
         Aether transcript/checkpoint. Native-only history is not transferred or assumed \
         resumable across engines.",
        aether_model::http::safe_provider_text(&session, 200)
    );
    let handoff = ctx.draft(EventKind::ContextHandoff {
        from: from.clone(),
        to: model.clone(),
        carried: 0,
        checkpoint: ctx.agent.checkpoint,
        lost: vec![lost],
    });
    let hold = runtime.hold();
    let row = agent_row_with_session(runtime, ctx, &hold)?;
    runtime.apply_held(
        hold,
        Commit::new()
            .mutate(Mutation::UpsertAgent(row))
            .event(handoff),
    )?;
    Ok(())
}

/// One attempt at one model. `rest` is what remains of the chain after it.
pub(super) async fn run_link(
    runtime: &Arc<Runtime>,
    ctx: &mut TurnCtx,
    tool_ctx: &Arc<ToolContext>,
    model: &aether_core::model::ModelRef,
    rest: &[aether_core::model::ModelRef],
    cancel: CancellationToken,
) -> Result<LinkOutcome> {
    // ---- 3. assemble context and freeze the snapshot -------------------------
    // A link that cannot even be built was never usable: an unknown or
    // disabled provider, a model the registry no longer lists. That is a
    // reason to try the next link, not to spend the turn's verdict here.
    let resolved = match runtime.models.resolve(model) {
        Ok(r) => r,
        Err(e) => return unusable_link(runtime, ctx, model, rest, e.to_string()),
    };
    if runtime.client_override.is_none() {
        prepare_fallback_session(runtime, ctx, model)?;
    }
    if runtime.client_override.is_none() && resolved.provider.protocol.is_engine() {
        use crate::turn_engine::EngineLink;
        return match crate::turn_engine::execute(runtime, ctx, &resolved, model, cancel).await? {
            EngineLink::Exhausted { class, error } => match rest.first() {
                Some(fallback) => {
                    announce_fallback(runtime, ctx, model, fallback, error, Some(class))?;
                    Ok(LinkOutcome::FellBack)
                }
                // Native engines report no retry-after, so the probe schedule
                // starts at the floor; reset prose is never parsed.
                None => Ok(LinkOutcome::Done(match ExhaustionClass::of(class) {
                    Some(class) => Next::Suspended(Box::new(crate::exhaustion::Exhausted {
                        model: model.clone(),
                        class,
                        detail: error,
                        retry_after: None,
                    })),
                    None => Next::Failed(error),
                })),
            },
            EngineLink::Done(next) => Ok(LinkOutcome::Done(next)),
        };
    }
    let client = match &runtime.client_override {
        Some(c) => c.clone(),
        // Same for a link with no usable credential: the most ordinary reason
        // to have configured a chain at all.
        None => match runtime.models.client(model).await {
            Ok(c) => c,
            Err(e) => return unusable_link(runtime, ctx, model, rest, e.to_string()),
        },
    };
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
    // Messages whose images would not fit one model request stay queued for the next turn.
    let pending_inbox = aether_context::render::deliverable_prefix(&queued_inbox);
    ctx.user_reply_pending |= has_direct_user_message(pending_inbox);
    let pending_approvals: Vec<ApprovalRequest> = runtime
        .store
        .pending_approvals()?
        .into_iter()
        .filter(|a| a.agent_id == ctx.agent.id)
        .collect();
    let mut checkpoint = runtime.store.latest_checkpoint(ctx.agent.id)?;
    if let Some(trigger) = compaction_pending_for_turn(runtime, ctx)?
        && let Some(fresh) = compact(
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
    let transcript_after = checkpoint
        .as_ref()
        .map(|c| c.provenance.to_seq)
        .unwrap_or(0);
    let transcript = runtime.store.transcript(ctx.agent.id, transcript_after)?;
    let memories = retrieve_memories(runtime, ctx, pending_inbox)?;
    let memory_records: Vec<_> = memories.iter().map(|m| m.record.clone()).collect();
    let notes = crate::notes::digest(runtime, ctx.agent.id)?;
    let mut tools = runtime.agent_tool_specs(&ctx.agent);
    // Task-closing tools make no sense without a task; hiding them avoids
    // pointless failed calls from a chat agent.
    if ctx.task.as_ref().is_none_or(|t| t.status.is_terminal()) {
        tools.retain(|t| t.name != "complete_task" && t.name != "fail_task");
    }
    // The assembler reads the model and effort off the agent; on a fallback link
    // they are this attempt's, not a change to the agent's config.
    let mut agent_for_link = ctx.agent.clone();
    model.clone_into(&mut agent_for_link.model);
    agent_for_link.config.effort = resolved.model.effort_for(ctx.agent.config.effort);
    let profile = runtime.model_profile(&resolved);
    let assembled = runtime.context.assemble(ContextInputs {
        agent: &agent_for_link,
        task: ctx.task.as_ref(),
        children: &children,
        child_tasks: &child_tasks,
        pending_inbox,
        pending_approvals: &pending_approvals,
        checkpoint: checkpoint.as_ref(),
        transcript: &transcript,
        memories: &memory_records,
        notes: &notes.notes,
        notes_omitted: notes.omitted,
        tools,
        model_profile: profile.as_deref(),
        engine_policy: None,
        capabilities: &resolved.model.capabilities,
        adapter: client.adapter(),
        attachment_dir: runtime.attachment_dir(),
        turn: ctx.turn,
        now: now(),
    });

    let snapshot = assembled.snapshot;
    let input_budget = resolved
        .model
        .capabilities
        .context_window
        .saturating_sub(snapshot.params.max_output_tokens.unwrap_or(0));
    if snapshot.token_estimate > input_budget {
        // Freeze rejected context for exhaustion diagnostics without consuming
        // input that a larger fallback must still receive.
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
        let error = ModelError::new(
            ModelErrorClass::ContextLimit,
            format!(
                "context budget exceeded for {model}: estimated {} input tokens, \
                 available {input_budget} after reserving output; \
                 the retained message and fixed context cannot fit",
                snapshot.token_estimate,
            ),
        );
        return record_failure(
            runtime,
            ctx,
            InvocationId::new(),
            &snapshot,
            now(),
            model,
            rest,
            error,
            ModelResponse::default(),
            &cancel,
        );
    }
    // Scoped: the hold is not `Send`, and this turn awaits the model below.
    {
        // `touch` and `agent_row` below both read rows this commit writes back.
        let hold = runtime.hold();
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
            commit.push_mutation(Mutation::UpsertAgent(agent_row(runtime, ctx, &hold)?));
        }
        // A new-model handoff already folds this turn's transcript on the
        // next pass. A later threshold request would hide it in the ledger.
        let handoff_pending = matches!(
            compaction_pending(runtime, ctx.agent.id)?,
            Some(CompactionTrigger::Handoff { .. })
        );
        if assembled.needs_compaction && !handoff_pending {
            commit.push_event(ctx.draft(EventKind::CompactionRequested {
                trigger: CompactionTrigger::TokenThreshold {
                    tokens: snapshot.token_estimate,
                    threshold: assembled.threshold_tokens,
                },
            }));
        }
        if !memories.is_empty() {
            let touched = runtime.memory.touch(&hold, &snapshot.memory_ids)?;
            commit.mutations.extend(touched.mutations);
        }
        runtime.apply_held(hold, commit)?;
    }

    if cancel.is_cancelled() {
        return Ok(LinkOutcome::Done(Next::Cancelled(Some("cancelled".into()))));
    }

    // ---- 4. invoke the model -------------------------------------------------
    let invocation_id = InvocationId::new();
    let started_at = now();
    runtime.bus.publish_delta(
        ctx.agent.id,
        invocation_id,
        DeltaKind::TurnStarted { turn: ctx.turn },
    );
    // Aliases (`sol`) are Aether's; the provider knows only canonical ids. The
    // snapshot keeps the configured link, as the engine path does.
    let mut request = assembled.request;
    request.model.model.clone_from(&resolved.model.id);
    // The snapshot keeps references; only the request sent carries the bytes.
    runtime.inline_images(&mut request.messages, resolved.model.capabilities.vision);
    let stream = match client.stream(request, cancel.child_token()).await {
        Ok(s) => s,
        Err(e) => {
            runtime
                .bus
                .publish_delta(ctx.agent.id, invocation_id, DeltaKind::TurnFinished);
            return record_failure(
                runtime,
                ctx,
                invocation_id,
                &snapshot,
                started_at,
                model,
                rest,
                e,
                ModelResponse::default(),
                &cancel,
            );
        }
    };
    let bus = runtime.bus.clone();
    let agent_id = ctx.agent.id;
    let mut tool_names: BTreeMap<usize, String> = BTreeMap::new();
    let mut receipt_seen = false;
    let outcome = collect_partial(stream, |event| match event {
        StreamEvent::MessageStop => receipt_seen = true,
        StreamEvent::TextDelta { text, .. } => bus.publish_delta(
            agent_id,
            invocation_id,
            DeltaKind::Text { text: text.clone() },
        ),
        StreamEvent::ThinkingDelta { text, .. } => bus.publish_delta(
            agent_id,
            invocation_id,
            DeltaKind::Thinking { text: text.clone() },
        ),
        StreamEvent::BlockStart {
            index,
            block: aether_core::model::BlockStart::ToolUse { name, .. },
        } => {
            tool_names.insert(*index, name.clone());
            bus.publish_delta(
                agent_id,
                invocation_id,
                DeltaKind::ToolInput {
                    name: name.clone(),
                    partial_json: String::new(),
                },
            );
        }
        StreamEvent::ToolInputDelta {
            index,
            partial_json,
        } => {
            let name = tool_names.get(index).cloned().unwrap_or_default();
            bus.publish_delta(
                agent_id,
                invocation_id,
                DeltaKind::ToolInput {
                    name,
                    partial_json: partial_json.clone(),
                },
            );
        }
        _ => {}
    })
    .await;
    runtime
        .bus
        .publish_delta(ctx.agent.id, invocation_id, DeltaKind::TurnFinished);

    let response = match outcome {
        Ok(r) => r,
        Err(mut failure) => {
            account_failed_stream(
                &mut failure.partial.usage,
                resolved.model.capabilities.cost,
                receipt_seen,
                cancel.is_cancelled(),
            );
            return record_failure(
                runtime,
                ctx,
                invocation_id,
                &snapshot,
                started_at,
                model,
                rest,
                failure.error,
                failure.partial,
                &cancel,
            );
        }
    };

    let mut tool_ids = BTreeSet::new();
    let duplicate = response.content.iter().find_map(|block| match block {
        ContentBlock::ToolUse { id, .. } if !tool_ids.insert(id.as_str()) => Some(id.clone()),
        _ => None,
    });
    if let Some(id) = duplicate {
        let mut partial = response;
        partial
            .usage
            .price_from(resolved.model.capabilities.cost, None);
        return record_failure(
            runtime,
            ctx,
            invocation_id,
            &snapshot,
            started_at,
            model,
            rest,
            ModelError::new(
                ModelErrorClass::InvalidRequest,
                format!(
                    "provider returned duplicate tool-use id: {}",
                    aether_model::http::safe_provider_text(&id, 200)
                ),
            ),
            partial,
            &cancel,
        );
    }

    // ---- 5. record the response --------------------------------------------
    let finished_at = now();
    let mut usage = response.usage;
    // A model with no price table leaves the invocation unpriced, which every
    // total above it then carries as a floor rather than as a zero.
    usage.price_from(resolved.model.capabilities.cost, None);
    let invocation = Invocation {
        id: invocation_id,
        snapshot_id: snapshot.id,
        agent_id: ctx.agent.id,
        turn: ctx.turn,
        model: model.clone(),
        started_at,
        finished_at: Some(finished_at),
        content: response.content.clone(),
        usage,
        stop_reason: response.stop_reason.clone(),
        error: None,
        provider_message_id: response.provider_message_id.clone(),
    };
    ctx.add_usage(usage);
    ctx.agent.stats.context_tokens = Some(usage.context_tokens());
    count_turn(ctx);
    ctx.agent.stats.last_turn_at = Some(finished_at);
    ctx.agent.updated_at = finished_at;
    let mut commit = Commit::new()
        .event(ctx.draft(EventKind::InvocationCompleted {
            invocation: invocation_id,
            snapshot: snapshot.id,
            model: model.clone(),
            usage,
            stop_reason: response.stop_reason.clone(),
            duration_ms: (finished_at - started_at).num_milliseconds().max(0) as u64,
        }))
        .mutate(Mutation::PutInvocation(invocation));
    if !response.content.is_empty() {
        commit.push_event(ctx.draft(EventKind::AgentMessage {
            role: MessageRole::Assistant,
            content: response.content.clone(),
            invocation: Some(invocation_id),
        }));
    }
    // Scoped: the hold is not `Send`, and this turn awaits its tools below.
    {
        let hold = runtime.hold();
        commit.push_mutation(Mutation::UpsertAgent(agent_row(runtime, ctx, &hold)?));
        close_exhaustion_episode(runtime, ctx, &hold, &mut commit, model)?;
        runtime.apply_held(hold, commit)?;
    }

    if let Some(StopReason::Refusal { category }) = &response.stop_reason {
        return Ok(LinkOutcome::Done(Next::Failed(format!(
            "model refused the request{}",
            category
                .as_deref()
                .map(|c| format!(" ({c})"))
                .unwrap_or_default()
        ))));
    }

    // ---- 6. tools and actions ------------------------------------------------
    if response
        .content
        .iter()
        .any(|block| matches!(block, ContentBlock::Text { text } if !text.trim().is_empty()))
    {
        ctx.user_reply_pending = false;
    }
    let tool_uses: Vec<(String, String, serde_json::Value)> = response
        .tool_uses()
        .map(|(id, name, input)| (id.to_owned(), name.to_owned(), input.clone()))
        .collect();
    if tool_uses.is_empty() {
        return prose_only_next(
            runtime,
            ctx,
            &response,
            false,
            CompletionSource::ApiResponse,
        )
        .map(LinkOutcome::Done);
    }
    let effects = run_tool_calls(runtime, ctx, tool_ctx, invocation_id, tool_uses, &cancel).await?;
    // A task outcome the batch already committed outranks the cancel, as on the engine path.
    if cancel.is_cancelled() && !effects.terminal() {
        return Ok(LinkOutcome::Done(Next::Cancelled(Some("cancelled".into()))));
    }
    if !effects.pending_approvals.is_empty() {
        let conds: Vec<WaitCondition> = effects
            .pending_approvals
            .iter()
            .map(|id| WaitCondition::Approval { id: *id })
            .collect();
        return Ok(LinkOutcome::Done(Next::Waiting(WaitCondition::All {
            of: conds,
        })));
    }
    compose_tool_results(runtime, ctx)?;
    Ok(LinkOutcome::Done(effects.next.unwrap_or(Next::Ready)))
}

fn account_failed_stream(
    usage: &mut aether_core::model::Usage,
    cost: Option<aether_core::model::ModelCost>,
    receipt_seen: bool,
    cancelled: bool,
) {
    debug_assert!(usage.cost_usd.is_finite() && usage.cost_usd >= 0.0);
    debug_assert!(cost.is_none_or(|cost| cost.rates().all(|rate| rate.is_finite() && rate >= 0.0)));
    usage.price_from(cost, None);
    if cancelled || !receipt_seen {
        usage.mark_cost_gap(aether_core::model::CostGap::Interrupted);
    }
}
