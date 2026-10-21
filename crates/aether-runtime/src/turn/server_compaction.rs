//! On request, a direct Anthropic conversation is folded by the API's own
//! summarize compaction; the signed block it returns replaces the history.
use super::compaction::{
    RequestRef, preserve_newer_request, record_compaction_usage, unanswered_tool_calls,
};
use super::*;
use aether_core::model::ModelRequest;
use aether_model::{ModelClient, Protocol, ResolvedModel};

pub(super) async fn try_server_compaction(
    runtime: &Arc<Runtime>,
    ctx: &mut TurnCtx,
    answering: RequestRef,
    trigger: &CompactionTrigger,
    cancel: &CancellationToken,
) -> Result<bool> {
    let resolved = runtime.models.resolve(&ctx.agent.model)?;
    if resolved.provider.protocol != Protocol::AnthropicMessages
        || !resolved.model.capabilities.compacts_on_server()
        || matches!(trigger, CompactionTrigger::Handoff { .. })
    {
        return Ok(false);
    }
    // Stopped on purpose: the request stays pending, as after Aether's own fold.
    if cancel.is_cancelled() {
        return Ok(true);
    }
    let client = match &runtime.client_override {
        Some(client) => client.clone(),
        None => match runtime.models.client(&ctx.agent.model).await {
            Ok(client) => client,
            Err(error) => {
                tracing::warn!(%error, "server compaction unavailable; trying Aether's compactor");
                return Ok(false);
            }
        },
    };
    let Some(request) = request(runtime, ctx, &resolved, client.as_ref())? else {
        return Ok(false);
    };
    let signed = request
        .messages
        .iter()
        .any(|m| m.content.iter().any(ContentBlock::is_signed_compaction));
    if !signed && !matches!(trigger, CompactionTrigger::Explicit { .. }) {
        return Ok(false);
    }
    debug_assert_eq!(
        request.extensions["anthropic.compaction"]["type"],
        "summarize"
    );
    debug_assert_eq!(request.model.model, resolved.model.id);
    let started = std::time::Instant::now();
    let response = summarize(runtime, ctx, client.as_ref(), request, cancel).await?;
    if cancel.is_cancelled() {
        return Ok(true);
    }
    let Some(response) = response else {
        return Ok(false);
    };
    let duration = started.elapsed();
    settle(runtime, ctx, answering, trigger, response, duration)?;
    Ok(true)
}

struct Conversation {
    checkpoint: Option<aether_core::checkpoint::Checkpoint>,
    transcript: Vec<TranscriptEntry>,
    children: Vec<Agent>,
    child_tasks: Vec<Task>,
    approvals: Vec<aether_core::tool::ApprovalRequest>,
    memories: Vec<aether_core::memory::MemoryRecord>,
    notes: crate::notes::NoteDigest,
}

fn conversation(runtime: &Runtime, ctx: &TurnCtx) -> Result<Conversation> {
    let checkpoint = runtime.store.latest_checkpoint(ctx.agent.id)?;
    let after = checkpoint.as_ref().map_or(0, |c| c.provenance.to_seq);
    let transcript = runtime.store.transcript(ctx.agent.id, after)?;
    let children = runtime.store.agents(&AgentFilter {
        parent: Some(ctx.agent.id),
        ..Default::default()
    })?;
    let child_tasks: Vec<_> = runtime
        .store
        .tasks(&TaskFilter {
            run: Some(ctx.agent.run),
            ..Default::default()
        })?
        .into_iter()
        .filter(|task| task.creator == Actor::agent(ctx.agent.id))
        .collect();
    let approvals: Vec<_> = runtime
        .store
        .pending_approvals()?
        .into_iter()
        .filter(|approval| approval.agent_id == ctx.agent.id)
        .collect();
    let inbox = runtime.store.inbox(ctx.agent.id, ctx.agent.inbox_cursor)?;
    let pending = aether_context::render::deliverable_prefix(&inbox);
    let memories = retrieve_memories(runtime, ctx, pending)?
        .into_iter()
        .map(|m| m.record)
        .collect();
    let notes = crate::notes::digest(runtime, ctx.agent.id)?;
    debug_assert!(transcript.iter().all(|entry| entry.seq > after));
    debug_assert!(
        children
            .iter()
            .all(|child| child.parent == Some(ctx.agent.id))
    );
    Ok(Conversation {
        checkpoint,
        transcript,
        children,
        child_tasks,
        approvals,
        memories,
        notes,
    })
}

fn request(
    runtime: &Runtime,
    ctx: &TurnCtx,
    resolved: &ResolvedModel,
    client: &dyn ModelClient,
) -> Result<Option<ModelRequest>> {
    debug_assert_eq!(resolved.provider.protocol, Protocol::AnthropicMessages);
    debug_assert!(resolved.model.capabilities.compacts_on_server());
    let state = conversation(runtime, ctx)?;
    if state.transcript.is_empty() || unanswered_tool_calls(&state.transcript) > 0 {
        return Ok(None);
    }
    let profile = runtime.model_profile(resolved);
    let mut agent = ctx.agent.clone();
    agent.config.effort = resolved.model.effort_for(agent.config.effort);
    let mut tools = runtime.agent_tool_specs(&agent);
    if ctx
        .task
        .as_ref()
        .is_none_or(|task| task.status.is_terminal())
    {
        tools.retain(|tool| tool.name != "complete_task" && tool.name != "fail_task");
    }
    let assembled = runtime.context.assemble(ContextInputs {
        agent: &agent,
        task: ctx.task.as_ref(),
        children: &state.children,
        child_tasks: &state.child_tasks,
        pending_inbox: &[],
        pending_approvals: &state.approvals,
        checkpoint: state.checkpoint.as_ref(),
        transcript: &state.transcript,
        memories: &state.memories,
        notes: &state.notes.notes,
        notes_omitted: state.notes.omitted,
        tools,
        model_profile: profile.as_deref(),
        engine_policy: None,
        capabilities: &resolved.model.capabilities,
        adapter: client.adapter(),
        attachment_dir: runtime.attachment_dir(),
        turn: ctx.turn,
        now: now(),
    });
    Ok(summarize_request(runtime, assembled, resolved))
}

fn summarize_request(
    runtime: &Runtime,
    assembled: aether_context::Assembled,
    resolved: &ResolvedModel,
) -> Option<ModelRequest> {
    if assembled.dropped_messages > 0 {
        return None;
    }
    let mut request = assembled.request;
    // Summarize the durable conversation; unconsumed inbox input remains for the next turn.
    if assembled.new_user_message.is_some() {
        request.messages.pop();
    }
    if request.messages.is_empty() {
        return None;
    }
    debug_assert!(!request.messages.is_empty());
    debug_assert!(request.messages.iter().any(|m| !m.content.is_empty()));
    request.model.model.clone_from(&resolved.model.id);
    request.extensions.insert(
        "anthropic.compaction".into(),
        serde_json::json!({"type": "summarize"}),
    );
    runtime.inline_images(&mut request.messages, resolved.model.capabilities.vision);
    Some(request)
}

async fn summarize(
    runtime: &Runtime,
    ctx: &mut TurnCtx,
    client: &dyn ModelClient,
    request: ModelRequest,
    cancel: &CancellationToken,
) -> Result<Option<ModelResponse>> {
    for attempt in 0..2 {
        if cancel.is_cancelled() {
            return Ok(None);
        }
        let (response, error) = invoke(client, request.clone(), cancel).await;
        record_compaction_usage(
            runtime,
            ctx,
            &aether_context::CompactionAttempt {
                model: ctx.agent.model.clone(),
                usage: (response.usage.total_tokens() > 0 || error.is_none())
                    .then_some(response.usage),
                error: error.clone(),
            },
        )?;
        if cancel.is_cancelled()
            || error
                .as_ref()
                .is_some_and(|e| e.class == ModelErrorClass::Cancelled)
        {
            cancel.cancel();
            return Ok(None);
        }
        if let Some(error) = error {
            if attempt == 0
                && (error.status == Some(529)
                    || error.provider_code.as_deref() == Some("compaction_unavailable"))
            {
                continue;
            }
            tracing::warn!(%error, "server compaction failed; trying Aether's compactor");
            return Ok(None);
        }
        debug_assert!(response.usage.cache_write_1h_tokens <= response.usage.cache_write_tokens);
        let valid = matches!(response.content.as_slice(), [ContentBlock::Compaction { content, signature: Some(signature) }] if !content.trim().is_empty() && !signature.is_empty());
        if valid
            && response.stop_reason
                == Some(StopReason::Other {
                    value: "compaction".into(),
                })
        {
            debug_assert_eq!(response.content.len(), 1);
            return Ok(Some(response));
        }
        tracing::warn!(reason = ?response.stop_reason, "server returned no signed summary; trying Aether's compactor");
        return Ok(None);
    }
    Ok(None)
}

async fn invoke(
    client: &dyn ModelClient,
    request: ModelRequest,
    cancel: &CancellationToken,
) -> (ModelResponse, Option<ModelError>) {
    let mut usage = aether_core::model::Usage::default();
    let work = async {
        let stream = client
            .stream(request, cancel.child_token())
            .await
            .map_err(|error| {
                Box::new(aether_model::PartialFailure {
                    error,
                    partial: ModelResponse::default(),
                })
            })?;
        collect_partial(stream, |event| {
            if let StreamEvent::Usage { usage: reported } = event {
                usage = *reported;
            }
        })
        .await
    };
    let result = tokio::select! {
        result = work => result,
        _ = cancel.cancelled() => return (ModelResponse { usage, ..Default::default() }, Some(ModelError::new(ModelErrorClass::Cancelled, "compaction cancelled"))),
    };
    debug_assert!(usage.cache_write_1h_tokens <= usage.cache_write_tokens);
    debug_assert!(
        result.is_err()
            || result
                .as_ref()
                .is_ok_and(|response| response.usage == usage)
    );
    match result {
        Ok(response) => (response, None),
        Err(failure) => (failure.partial, Some(failure.error)),
    }
}

fn settle(
    runtime: &Runtime,
    ctx: &mut TurnCtx,
    answering: RequestRef,
    trigger: &CompactionTrigger,
    response: ModelResponse,
    duration: Duration,
) -> Result<()> {
    debug_assert_eq!(response.content.len(), 1);
    debug_assert!(response.content[0].is_signed_compaction());
    let after = if response.usage.output_tokens > 0 {
        response.usage.output_tokens
    } else {
        u64::from(
            aether_context::TokenEstimator
                .message(&Message::assistant_blocks(response.content.clone())),
        )
    };
    let before = ctx.agent.stats.context_tokens.or_else(|| {
        (response.usage.context_tokens() > 0).then_some(response.usage.context_tokens())
    });
    ctx.agent.stats.context_tokens = Some(after);
    ctx.agent.updated_at = now();
    let hold = runtime.hold();
    let mut commit = Commit::new()
        .event(ctx.draft(EventKind::AgentMessage {
            role: MessageRole::Assistant,
            content: response.content,
            invocation: None,
        }))
        .event(
            ctx.draft(EventKind::CompactionNative {
                provider: "anthropic".into(),
                automatic: !matches!(trigger, CompactionTrigger::Explicit { .. }),
                before_tokens: before,
                after_tokens: Some(after),
                duration_ms: Some(duration.as_millis() as u64),
            })
            .caused_by(answering.id),
        )
        .mutate(Mutation::UpsertAgent(agent_row_with_session(
            runtime, ctx, &hold,
        )?));
    preserve_newer_request(runtime, ctx, answering.seq, &mut commit, &hold)?;
    runtime.apply_held(hold, commit)?;
    Ok(())
}
