use super::*;

/// Execute tool calls whose approvals were resolved since the last turn.
pub(super) async fn settle_resolved_approvals(
    runtime: &Arc<Runtime>,
    ctx: &mut TurnCtx,
    tool_ctx: &Arc<ToolContext>,
    cancel: &CancellationToken,
) -> Result<()> {
    let resolved: Vec<(ApprovalId, ApprovalDecision)> = runtime
        .store
        .inbox(ctx.agent.id, ctx.agent.inbox_cursor)?
        .into_iter()
        .filter_map(|i| match i.kind {
            InboxKind::ApprovalResolved {
                approval, decision, ..
            } => Some((approval, decision)),
            _ => None,
        })
        .collect();
    if resolved.is_empty() {
        return Ok(());
    }
    let mut approvals = Vec::new();
    for (id, decision) in resolved {
        if let Some(approval) = runtime.store.approval(id)? {
            approvals.push((approval, decision));
        }
    }
    let call_ids: Vec<_> = approvals
        .iter()
        .map(|(approval, _)| approval.call_id)
        .collect();
    let events = runtime
        .store
        .tool_events(ctx.agent.id, aether_store::ToolEventScope::Calls(&call_ids))?;
    let requested = requested_calls(&events);
    let settled = settled_calls(&events);
    let mut runnable = Vec::new();
    let mut commit = Commit::new();
    for (approval, decision) in approvals {
        if settled.contains_key(&approval.call_id) {
            continue;
        }
        let Some(call) = requested.get(&approval.call_id).cloned() else {
            continue;
        };
        match decision {
            ApprovalDecision::Denied => {
                commit.push_event(ctx.draft(EventKind::ToolDenied {
                    call: call.id,
                    reason: "denied by approver".into(),
                }));
            }
            ApprovalDecision::Approved => match runtime.tools.get(&call.name) {
                Some(tool) => match replayable(runtime, &call)? {
                    // Same guarantee as the main path: an outcome already
                    // committed for this invocation is not produced twice.
                    Some(recorded) => {
                        commit.push_event(ctx.draft(EventKind::ToolReplayed {
                            call: call.id,
                            key: recorded.key.clone(),
                            original: recorded.call_id,
                        }));
                        commit.push_event(ctx.draft(EventKind::ToolSucceeded {
                            call: call.id,
                            outcome: recorded.outcome,
                        }));
                        ctx.agent.stats.tool_calls += 1;
                    }
                    None => runnable.push((call, tool)),
                },
                None => commit.push_event(ctx.draft(EventKind::ToolFailed {
                    call: call.id,
                    error: "tool no longer registered".into(),
                    retryable: false,
                    duration_ms: None,
                })),
            },
        }
    }
    runtime.apply(commit)?;
    if !runnable.is_empty() {
        let guard = match hold_resources(runtime, ctx, &runnable, cancel).await? {
            Held::Keys(guard) => Some(guard),
            Held::NoKeys => None,
            Held::Stopped => {
                let mut settled = Commit::new();
                for (call, _) in &runnable {
                    settled.push_event(ctx.draft(EventKind::ToolCancelled { call: call.id }));
                }
                runtime.apply(settled)?;
                return Ok(());
            }
        };
        let mut started = Commit::new();
        for (call, _) in &runnable {
            started.push_event(ctx.draft(EventKind::ToolStarted { call: call.id }));
        }
        runtime.apply(started)?;
        let results = runtime
            .tool_runtime
            .execute_batch(
                runnable,
                tool_ctx.clone(),
                cancel.clone(),
                ctx.hard_cancel.clone(),
                &runtime.config.execution,
            )
            .await;
        release_resources(runtime, ctx, guard)?;
        let mut effects = ToolEffects::default();
        for result in results {
            let duration = result.duration_ms();
            let ran = result.ran;
            let mut record = None;
            let kind = match result.result {
                Ok(ToolExecution::Outcome(mut o)) => {
                    o.duration_ms = duration;
                    record = recording_for(&result.call, &o);
                    EventKind::ToolSucceeded {
                        call: result.call.id,
                        outcome: o,
                    }
                }
                Ok(ToolExecution::Action(action)) => {
                    let outcome = apply_action(runtime, ctx, action, &mut effects).await?;
                    EventKind::ToolSucceeded {
                        call: result.call.id,
                        outcome: outcome.with_duration(duration),
                    }
                }
                Err(ToolError::Cancelled) => EventKind::ToolCancelled {
                    call: result.call.id,
                },
                Err(e) => EventKind::ToolFailed {
                    call: result.call.id,
                    error: runtime
                        .tool_runtime
                        .redactor()
                        .redact(&e.to_string())
                        .into_owned(),
                    retryable: e.is_retryable(),
                    duration_ms: ran.then_some(duration),
                },
            };
            ctx.agent.stats.tool_calls += 1;
            let mut commit = Commit::new().event(ctx.draft(kind));
            if let Some(recorded) = record {
                commit.push_mutation(Mutation::RecordToolOutcome(recorded));
            }
            runtime.apply(commit)?;
        }
    }
    compose_tool_results(runtime, ctx)?;
    Ok(())
}

/// The outcome committed for this call's key while driving its invocation,
/// masked again: it may predate masking or a secret the runtime knows now.
pub(super) fn replayable(runtime: &Runtime, call: &ToolCall) -> Result<Option<RecordedOutcome>> {
    let (Some(key), Some(invocation)) = (&call.idempotency_key, call.invocation_id) else {
        return Ok(None);
    };
    let recorded = runtime
        .store
        .recorded_outcome(call.agent_id, invocation, call.id, key)?;
    Ok(recorded.map(|mut recorded| {
        debug_assert_eq!(&recorded.key, key, "the store answers for this key");
        aether_tools::redact_outcome(runtime.tool_runtime.redactor(), &mut recorded.outcome);
        recorded
    }))
}

/// What to record so a re-drive of this call replays instead of running.
/// Only a success is worth replaying; a failure is meant to be tried again.
pub(super) fn recording_for(call: &ToolCall, outcome: &ToolOutcome) -> Option<RecordedOutcome> {
    let (key, invocation) = (call.idempotency_key.clone()?, call.invocation_id?);
    (!outcome.is_error).then(|| RecordedOutcome {
        key,
        agent_id: call.agent_id,
        invocation_id: invocation,
        call_id: call.id,
        tool: call.name.clone(),
        at: now(),
        outcome: outcome.clone(),
    })
}

fn requested_calls(events: &[aether_core::event::Event]) -> BTreeMap<ToolCallId, ToolCall> {
    events
        .iter()
        .filter_map(|event| match &event.kind {
            EventKind::ToolRequested { call, .. } => Some((call.id, call.clone())),
            _ => None,
        })
        .collect()
}

/// Outcomes for settled calls: (model-facing text, is_error).
fn settled_calls(events: &[aether_core::event::Event]) -> BTreeMap<ToolCallId, (String, bool)> {
    let mut out = BTreeMap::new();
    for e in events {
        let (call, text, err) = match &e.kind {
            EventKind::ToolSucceeded { call, outcome } => {
                (*call, outcome.content.clone(), outcome.is_error)
            }
            EventKind::ToolFailed { call, error, .. } => {
                (*call, format!("Tool failed: {error}"), true)
            }
            EventKind::ToolCancelled { call } => (*call, "Tool call was cancelled.".into(), true),
            EventKind::ToolDenied { call, reason } => {
                (*call, format!("Tool call denied: {reason}"), true)
            }
            _ => continue,
        };
        out.entry(call).or_insert((text, err));
    }
    out
}

/// If every tool call of the last assistant message has an outcome and no
/// tool-result message follows it yet, append that message.
pub(super) fn compose_tool_results(runtime: &Runtime, ctx: &TurnCtx) -> Result<()> {
    let recent = runtime.store.latest_events(
        2,
        &EventFilter {
            types: vec!["agent.message".into()],
            ..EventFilter::own_events(ctx.agent.id)
        },
    )?;
    let Some(last) = recent.first() else {
        return Ok(());
    };
    let EventKind::AgentMessage {
        role: MessageRole::Assistant,
        content,
        invocation,
    } = &last.kind
    else {
        return Ok(());
    };
    let ids: Vec<String> = content
        .iter()
        .filter_map(|b| match b {
            ContentBlock::ToolUse { id, .. } => Some(id.clone()),
            _ => None,
        })
        .collect();
    if ids.is_empty() {
        return Ok(());
    }
    // Every producer that ever puts tool uses in an assistant message stamps
    // its own invocation on it; without one there is no invocation to scope
    // the lookup to, so leave the calls unresolved rather than guess.
    let Some(invocation) = invocation else {
        return Ok(());
    };
    let events = runtime.store.tool_events(
        ctx.agent.id,
        aether_store::ToolEventScope::Invocation(*invocation),
    )?;
    let requested = requested_calls(&events);
    let settled = settled_calls(&events);
    let by_provider_id: BTreeMap<_, _> = requested
        .values()
        .map(|call| (call.provider_call_id.as_str(), call))
        .collect();
    let redactor = runtime.tool_runtime.redactor();
    let mut blocks = Vec::with_capacity(ids.len());
    for provider_id in &ids {
        let Some(call) = by_provider_id.get(provider_id.as_str()) else {
            return Ok(());
        };
        let Some((text, is_error)) = settled.get(&call.id) else {
            return Ok(()); // still pending
        };
        // Masked again: an outcome recorded before masking, or before its
        // secret was known, must not reach the provider as stored.
        let text = redactor.redact(text).into_owned();
        blocks.push(ContentBlock::ToolResult {
            tool_use_id: provider_id.clone(),
            content: vec![ToolResultBlock::Text { text }],
            is_error: *is_error,
        });
    }
    let message = Message::user_blocks(blocks);
    runtime.apply(Commit::new().event(ctx.draft(EventKind::AgentMessage {
        role: MessageRole::User,
        content: message.content,
        invocation: None,
    })))?;
    Ok(())
}
