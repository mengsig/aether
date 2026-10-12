use super::*;
use aether_model::ModelsError;
use aether_model::registry::RegistryError;

pub(crate) struct PendingCompaction {
    pub trigger: CompactionTrigger,
    pub request_seq: u64,
}

/// Pin the handoff owed by the model this turn starts on. A later model
/// selection can replace the latest request before context assembly begins.
pub(crate) fn handoff_at_turn_start(
    runtime: &Runtime,
    agent: &Agent,
) -> Result<Option<PendingCompaction>> {
    Ok(pending_request(runtime, agent.id)?.filter(|pending| {
        matches!(&pending.trigger, CompactionTrigger::Handoff { to, .. } if to == &agent.model)
    }))
}

/// The newest outstanding request, including its event sequence so a newer
/// model selection cannot be hidden by this request's eventual completion.
fn pending_request(runtime: &Runtime, agent: AgentId) -> Result<Option<PendingCompaction>> {
    let recent = runtime.store.latest_events(
        1,
        &EventFilter {
            types: vec![
                "compaction.requested".into(),
                "compaction.completed".into(),
                "compaction.failed".into(),
            ],
            ..EventFilter::own_events(agent)
        },
    )?;
    Ok(match recent.first() {
        Some(event) if let EventKind::CompactionRequested { trigger } = &event.kind => {
            if !matches!(
                trigger,
                CompactionTrigger::Explicit { .. } | CompactionTrigger::Handoff { .. }
            ) && let Some(current) = runtime.store.agent(agent)?
                && runtime
                    .models
                    .resolve(&current.model)?
                    .provider
                    .protocol
                    .is_engine()
            {
                return Ok(None);
            }
            Some(PendingCompaction {
                trigger: trigger.clone(),
                request_seq: event.seq,
            })
        }
        _ => None,
    })
}

/// The trigger of a `compaction.requested` newer than the last completed or
/// failed compaction, or `None` when no compaction is owed. Native engines own
/// automatic compaction, including when old automatic requests remain stored.
pub(crate) fn compaction_pending(
    runtime: &Runtime,
    agent: AgentId,
) -> Result<Option<CompactionTrigger>> {
    Ok(pending_request(runtime, agent)?.map(|request| request.trigger))
}

/// A handoff belongs to the turn on the selected destination model. The
/// operator can change the stored model while the previous turn is still
/// assembling its prompt; that turn must finish before folding its history.
pub(crate) fn compaction_pending_for_turn(
    runtime: &Runtime,
    ctx: &mut TurnCtx,
) -> Result<Option<PendingCompaction>> {
    let _hold = runtime.hold();
    let current = pending_request(runtime, ctx.agent.id)?;
    if let Some(pinned) = ctx.start_handoff.take() {
        return Ok(Some(pinned));
    }
    let Some(pending) = current else {
        return Ok(None);
    };
    if let CompactionTrigger::Handoff { to, .. } = &pending.trigger {
        let started = runtime.store.latest_events(
            1,
            &EventFilter {
                types: vec!["agent.started".into()],
                ..EventFilter::own_events(ctx.agent.id)
            },
        )?;
        // A→B→A during one A turn must still wait: the later A selection
        // did not precede the turn already in flight.
        if to != &ctx.agent.model
            || started
                .first()
                .is_some_and(|event| event.seq < pending.request_seq)
        {
            return Ok(None);
        }
    }
    Ok(Some(pending))
}

/// A model change can land while a compactor awaits its provider. Re-issue
/// the newer request after this one's result so the completion event does not
/// make the latest handoff disappear from `compaction_pending`.
fn preserve_newer_request(
    runtime: &Runtime,
    ctx: &TurnCtx,
    finished_seq: u64,
    commit: &mut Commit,
    _hold: &CommitHold<'_>,
) -> Result<()> {
    if let Some(newer) = pending_request(runtime, ctx.agent.id)?
        && newer.request_seq > finished_seq
    {
        commit.push_event(ctx.draft(EventKind::CompactionRequested {
            trigger: newer.trigger,
        }));
    }
    Ok(())
}

/// Which model folds the context and which one retries when it fails.
///
/// §6.3 wants the cheapest capable model first and one stronger retry. The
/// order is: the configured compactor, then - on a handoff - the model that
/// produced the context and can still read it, then the agent's own.
/// `None` means no candidate can be resolved.
pub(super) fn compaction_models(
    configured: Option<ModelRef>,
    agent_model: &ModelRef,
    trigger: &CompactionTrigger,
    drivable: impl Fn(&ModelRef) -> bool,
) -> Option<(ModelRef, Option<ModelRef>)> {
    let mut candidates = Vec::new();
    if let Some(m) = configured {
        candidates.push(m);
    }
    if let CompactionTrigger::Handoff { from, .. } = trigger {
        candidates.push(from.clone());
    }
    candidates.push(agent_model.clone());
    let primary = candidates.into_iter().find(&drivable)?;
    // The agent's own model is the stronger retry, when it is a different
    // model Aether can drive.
    let fallback = (*agent_model != primary && drivable(agent_model)).then(|| agent_model.clone());
    Some((primary, fallback))
}

pub(super) fn compaction_models_for(
    runtime: &Runtime,
    ctx: &TurnCtx,
    trigger: &CompactionTrigger,
) -> Option<(ModelRef, Option<ModelRef>)> {
    compaction_models(
        Some(runtime.models.compactor_for(&ctx.agent.model)),
        &ctx.agent.model,
        trigger,
        |m| runtime.models.resolve(m).is_ok(),
    )
}

/// An explicit compactor naming a retired model, which must fail with its
/// successor rather than yield silently to the next candidate.
fn retired_compactor(runtime: &Runtime) -> Option<(ModelRef, RegistryError)> {
    let compactor = runtime.models.compactor_model()?;
    match runtime.models.resolve(&compactor) {
        Err(ModelsError::Registry(error)) if error.is_retirement() => Some((compactor, error)),
        _ => None,
    }
}

pub(super) async fn compaction_client(
    runtime: &Arc<Runtime>,
    ctx: &TurnCtx,
    model: &ModelRef,
) -> Result<Arc<dyn aether_model::ModelClient>> {
    let resolved = runtime.models.resolve(model)?;
    let Some(kind) = crate::turn_engine::engine_kind(resolved.provider.protocol) else {
        return Ok(runtime.models.client(model).await?);
    };
    if ctx.agent.scope.backend != aether_core::agent::ExecutionBackend::Host {
        return Err(RuntimeError::Invalid(
            "Engine compaction cannot run inside a container backend; configure an API compactor"
                .into(),
        ));
    }
    let engine = runtime
        .engines
        .get(&kind)
        .cloned()
        .ok_or_else(|| RuntimeError::Invalid(format!("{} is not installed", kind.as_str())))?;
    let tool_ctx = tool_context(runtime, ctx);
    Ok(Arc::new(crate::compaction_client::CompactionClient {
        engine,
        turn: aether_engine::EngineTurn {
            prompt: String::new(),
            images: Vec::new(),
            system_prompt: String::new(),
            session: None,
            session_usage: aether_core::model::Usage::default(),
            new_session_id: aether_engine::new_session_id(),
            model: Some(resolved.model.id),
            effort: None,
            cwd: tool_ctx.cwd.clone(),
            extra_dirs: Vec::new(),
            summary_only: true,
            network_access: false,
            bypass_permissions: false,
            hidden_env: runtime.tools.engine_hidden_env().to_vec(),
            tool_hidden_env: runtime.tools.hidden_env().to_vec(),
            mcp: None,
        },
    }))
}

/// Tool calls in `entries` whose result never reached Aether. On the engine
/// path a call the engine ran and never reported leaves its result outside
/// the transcript, so a handoff must say so rather than imply it carried.
pub(super) fn unanswered_tool_calls(entries: &[TranscriptEntry]) -> usize {
    let mut open: BTreeSet<&str> = BTreeSet::new();
    for entry in entries {
        for block in &entry.message.content {
            match block {
                ContentBlock::ToolUse { id, .. } => {
                    open.insert(id.as_str());
                }
                ContentBlock::ToolResult { tool_use_id, .. } => {
                    open.remove(tool_use_id.as_str());
                }
                _ => {}
            }
        }
    }
    open.len()
}

/// Whether `model` is served by an engine that keeps its own session and its
/// own history. A test's client override does not
/// change who held the history.
pub(super) fn keeps_its_own_history(runtime: &Runtime, model: &ModelRef) -> bool {
    runtime
        .models
        .resolve(model)
        .is_ok_and(|r| r.provider.protocol.is_engine())
}

/// User turns in `entries` that no assistant turn ever answered. A message a
/// person sent, that the previous turn consumed and committed but never got
/// to (a model error after `InboxConsumed`), survives a `keep_tail = 0` fold
/// only as whatever the compactor chose to write about it.
pub(super) fn unanswered_user_turns(entries: &[TranscriptEntry]) -> usize {
    let mut open = 0usize;
    for entry in entries {
        match entry.message.role {
            MessageRole::User => open += 1,
            MessageRole::Assistant => open = 0,
        }
    }
    open
}

/// What a handoff could not carry across, in plain words. Empty means the
/// checkpoint holds everything Aether ever saw.
pub(super) fn handoff_losses(
    runtime: &Runtime,
    from: &ModelRef,
    session: Option<&str>,
    unanswered: usize,
    unanswered_user: usize,
) -> Vec<String> {
    let mut lost = Vec::new();
    // Only an engine keeps history Aether never saw, and only the engine the
    // agent is leaving. On the way back onto an engine `from` is the API
    // model, and nothing of its context is behind a session.
    if let Some(session) = session.filter(|_| keeps_its_own_history(runtime, from)) {
        lost.push(format!(
            "{} session {session} kept its own history; only what it streamed to Aether crossed over",
            from.provider
        ));
    }
    if unanswered > 0 {
        lost.push(format!(
            "{unanswered} tool call(s) whose result Aether never received"
        ));
    }
    if unanswered_user > 0 {
        lost.push(format!(
            "{unanswered_user} message(s) from a person that no turn answered before the fold"
        ));
    }
    lost
}

/// Fold the older transcript into a checkpoint. Returns the new checkpoint,
/// or `None` when there is too little to fold or the compactor failed (the
/// failure is recorded as an event; the turn proceeds uncompacted).
///
/// A handoff always records `context.handoff`, checkpoint or not: the operator
/// must be able to see what crossed the engine/API boundary and what did not.
pub(crate) async fn compact(
    runtime: &Arc<Runtime>,
    ctx: &mut TurnCtx,
    request: PendingCompaction,
    previous: Option<&aether_core::checkpoint::Checkpoint>,
    child_tasks: &[Task],
    cancel: &CancellationToken,
) -> Result<Option<aether_core::checkpoint::Checkpoint>> {
    let PendingCompaction {
        trigger,
        request_seq,
    } = request;
    const KEEP_TAIL: usize = 6;
    // An episode that ended folds whole: a closed task's turns have no further
    // use beyond its result, and a raw tail across a handoff would hand the
    // new model dangling tool calls the old one made. Pressure triggers keep
    // a tail, because the work they interrupt is still in flight.
    let keep_tail = if keeps_its_own_history(runtime, &ctx.agent.model) {
        0
    } else {
        match trigger {
            CompactionTrigger::Handoff { .. } | CompactionTrigger::TaskBoundary { .. } => 0,
            _ => KEEP_TAIL,
        }
    };
    let handoff = match &trigger {
        CompactionTrigger::Handoff { from, to, session } => {
            Some((from.clone(), to.clone(), session.clone()))
        }
        _ => None,
    };
    let after = previous.map(|c| c.provenance.to_seq).unwrap_or(0);
    let entries = runtime.store.transcript(ctx.agent.id, after)?;
    let Some((fold, tail)) = aether_context::fold_range(&entries, keep_tail) else {
        let mut commit = Commit::new().event(ctx.draft(EventKind::CompactionFailed {
            error: "Nothing to compact: no older complete transcript remains outside the retained tail".into(),
            models: Vec::new(),
        }));
        if let Some((from, to, session)) = &handoff {
            commit.push_event(ctx.draft(EventKind::ContextHandoff {
                from: from.clone(),
                to: to.clone(),
                checkpoint: None,
                carried: entries.len() as u32,
                lost: handoff_losses(
                    runtime,
                    from,
                    session.as_deref(),
                    unanswered_tool_calls(&entries),
                    unanswered_user_turns(&entries),
                ),
            }));
        }
        let hold = runtime.hold();
        preserve_newer_request(runtime, ctx, request_seq, &mut commit, &hold)?;
        runtime.apply_held(hold, commit)?;
        return Ok(None);
    };
    let unanswered = unanswered_tool_calls(fold);
    let unanswered_user = unanswered_user_turns(fold);
    // Named in every failure: a transcript that left the box for a provider
    // that then failed leaves no other durable trace of the egress.
    let fail = |runtime: &Arc<Runtime>,
                ctx: &TurnCtx,
                error: String,
                models: Vec<ModelRef>|
     -> Result<()> {
        let mut commit = Commit::new().event(ctx.draft(EventKind::CompactionFailed {
            error: error.clone(),
            models,
        }));
        if let Some((from, to, session)) = &handoff {
            let mut lost = handoff_losses(
                runtime,
                from,
                session.as_deref(),
                unanswered,
                unanswered_user,
            );
            lost.push(format!("no checkpoint was built: {error}"));
            commit.push_event(ctx.draft(EventKind::ContextHandoff {
                from: from.clone(),
                to: to.clone(),
                checkpoint: None,
                carried: 0,
                lost,
            }));
        }
        let hold = runtime.hold();
        preserve_newer_request(runtime, ctx, request_seq, &mut commit, &hold)?;
        runtime.apply_held(hold, commit)?;
        Ok(())
    };
    if let Some((compactor, error)) = retired_compactor(runtime) {
        fail(
            runtime,
            ctx,
            format!("compactor model unavailable: {error}"),
            vec![compactor],
        )?;
        return Ok(None);
    }
    let Some((model, fallback_model)) = compaction_models_for(runtime, ctx, &trigger) else {
        fail(
            runtime,
            ctx,
            "no model that Aether can drive is available to fold this context; \
             configure a compactor model"
                .into(),
            Vec::new(),
        )?;
        return Ok(None);
    };
    let attempted: Vec<ModelRef> = std::iter::once(model.clone())
        .chain(fallback_model.clone())
        .collect();
    let client = match &runtime.client_override {
        Some(c) => c.clone(),
        None => match compaction_client(runtime, ctx, &model).await {
            Ok(c) => c,
            Err(e) => {
                fail(
                    runtime,
                    ctx,
                    format!("compactor model unavailable: {e}"),
                    attempted,
                )?;
                return Ok(None);
            }
        },
    };
    // Aliases are Aether's; the compactor's requests carry canonical ids.
    let primary = runtime.models.canonical(&model);
    let mut compactor = aether_context::Compactor::new(client.clone(), primary).with_effort(
        runtime
            .models
            .config()
            .model
            .compactor_effort
            .unwrap_or(aether_core::model::Effort::Medium),
    );
    if let Some(strong) = fallback_model
        && let Some(strong_client) = match &runtime.client_override {
            Some(c) => Some(c.clone()),
            None => match compaction_client(runtime, ctx, &strong).await {
                Ok(client) => Some(client),
                Err(error) => {
                    tracing::warn!(model = %strong, %error, "compaction fallback unavailable; primary remains usable");
                    None
                }
            },
        }
    {
        compactor = compactor.with_fallback(strong_client, runtime.models.canonical(&strong));
    }
    let compaction_agent = ctx.agent.clone();
    let compaction_task = match &trigger {
        CompactionTrigger::TaskBoundary { task } => Some(
            runtime
                .store
                .task(*task)?
                .ok_or_else(|| RuntimeError::NotFound(task.to_string()))?,
        ),
        _ => ctx.task.clone(),
    };
    let inputs = aether_context::CompactionInputs {
        agent: &compaction_agent,
        task: compaction_task.as_ref(),
        child_tasks,
        previous,
        entries: fold,
        trigger: Some(trigger.clone()),
    };
    let mut accounting_error = None;
    let report = compactor
        .compact_observed(
            inputs,
            cancel.child_token(),
            |attempt| match record_compaction_usage(runtime, ctx, attempt) {
                Ok(()) => std::ops::ControlFlow::Continue(()),
                Err(error) => {
                    accounting_error = Some(error);
                    std::ops::ControlFlow::Break(())
                }
            },
        )
        .await;
    if let Some(error) = accounting_error {
        return Err(error);
    }
    match report.result {
        Ok(compacted) => {
            let checkpoint = compacted.checkpoint;
            ctx.agent.checkpoint = Some(checkpoint.id);
            if runtime
                .models
                .resolve(&ctx.agent.model)?
                .provider
                .protocol
                .is_engine()
            {
                ctx.agent.engine_session = None;
            }
            ctx.agent.updated_at = now();
            // The candidates below mark existing records superseded, so the
            // read that finds them and the write that marks them are one step.
            let hold = runtime.hold();
            let mut commit = Commit::new()
                .event(ctx.draft(EventKind::CheckpointCreated {
                    checkpoint: checkpoint.clone(),
                }))
                .event(ctx.draft(EventKind::CompactionCompleted {
                    checkpoint: checkpoint.id,
                    version: checkpoint.version,
                    from_seq: checkpoint.provenance.from_seq,
                    to_seq: checkpoint.provenance.to_seq,
                    before_tokens: Some(history_tokens(previous, &entries)),
                    after_tokens: Some(history_tokens(Some(&checkpoint), tail)),
                }))
                .mutate(Mutation::PutCheckpoint(checkpoint.clone()))
                .mutate(Mutation::UpsertAgent(agent_row_with_session(
                    runtime, ctx, &hold,
                )?));
            let mut superseded = std::collections::BTreeSet::new();
            for candidate in compacted.memory_candidates {
                let (admission, memory_commit) = runtime.memory.evaluate_batch(
                    &hold,
                    candidate,
                    ctx.base_actor.clone(),
                    Some(ctx.agent.id),
                    ctx.task.as_ref().map(|t| t.id),
                    &superseded,
                )?;
                if let aether_memory::Admission::Superseded { old, .. } = admission {
                    superseded.insert(old);
                }
                commit.events.extend(memory_commit.events);
                commit.mutations.extend(memory_commit.mutations);
            }
            if let Some((from, to, session)) = &handoff {
                commit.push_event(ctx.draft(EventKind::ContextHandoff {
                    from: from.clone(),
                    to: to.clone(),
                    checkpoint: Some(checkpoint.id),
                    carried: fold.len() as u32,
                    lost: handoff_losses(
                        runtime,
                        from,
                        session.as_deref(),
                        unanswered,
                        unanswered_user,
                    ),
                }));
            }
            preserve_newer_request(runtime, ctx, request_seq, &mut commit, &hold)?;
            runtime.apply_held(hold, commit)?;
            tracing::info!(agent = %ctx.agent.id, version = checkpoint.version, attempts = compacted.attempts, trigger = trigger.name(), "compacted context");
            Ok(Some(checkpoint))
        }
        // A stop or interrupt is not a compactor failure: the request stays
        // pending, as after a crash, so a handoff is not consumed without its checkpoint.
        Err(_) if cancel.is_cancelled() => Ok(None),
        Err(e) => {
            fail(runtime, ctx, e.to_string(), attempted)?;
            tracing::warn!(agent = %ctx.agent.id, error = %e, "compaction failed; continuing uncompacted");
            Ok(None)
        }
    }
}

/// The history a turn would be sent: the checkpoint, then the transcript after it.
fn history_tokens(
    checkpoint: Option<&aether_core::checkpoint::Checkpoint>,
    entries: &[TranscriptEntry],
) -> u32 {
    let estimate = aether_context::TokenEstimator;
    let folded = checkpoint.map_or(0, |c| estimate.text(&aether_context::render::checkpoint(c)));
    entries.iter().fold(folded, |sum, e| {
        sum.saturating_add(estimate.message(&e.message))
    })
}

/// Failed drafts and provider errors still spend tokens. Commit each attempt
/// before permitting another so the scheduler and live totals see that spend.
pub(super) fn record_compaction_usage(
    runtime: &Runtime,
    ctx: &mut TurnCtx,
    attempt: &aether_context::CompactionAttempt,
) -> Result<()> {
    let resolved = runtime.models.resolve(&attempt.model)?;
    let mut usage = attempt.usage.unwrap_or_default();
    if attempt.usage.is_none() {
        usage.set_cost(None);
    } else if !resolved.provider.protocol.is_engine() {
        usage.price_from(resolved.model.capabilities.cost, None);
    }
    ctx.add_usage(usage);
    ctx.agent.updated_at = now();
    if let Some(error) = &attempt.error
        && crate::exhaustion::recoverable_by_fallback(error.class)
    {
        crate::scheduler::record_error(runtime, &attempt.model.provider, error.class);
    }
    let hold = runtime.hold();
    let row = agent_row_with_session(runtime, ctx, &hold)?;
    let mut commit = Commit::new()
        .event(ctx.draft(EventKind::AgentUsageUpdated {
            total: ctx.agent.stats.usage,
            turn_usage: ctx.turn_usage,
        }))
        .mutate(Mutation::UpsertAgent(row));
    if let Some(error) = &attempt.error {
        crate::exhaustion::record_incident(
            runtime,
            &hold,
            &mut commit,
            &ctx.agent,
            &attempt.model,
            error,
            &ctx.base_actor,
        )?;
    } else {
        crate::exhaustion::recover_incident(
            runtime,
            &mut commit,
            &ctx.agent,
            &attempt.model,
            &ctx.base_actor,
        )?;
    }
    runtime.apply_held(hold, commit)?;
    Ok(())
}
