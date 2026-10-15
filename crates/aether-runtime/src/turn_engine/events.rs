//! Native engine events.
use super::{
    Arc, BTreeMap, Commit, ContentBlock, DeltaKind, EngineEvent, EngineKind, EventKind,
    IdempotencyKey, Instant, Invocation, InvocationId, InvocationSnapshot, MAX_ENGINE_ERROR,
    MessageRole, Mutation, ResolvedModel, Result, Runtime, StreamEvent, ToolCall, ToolCallId,
    ToolOutcome, TurnCtx, TurnPricing, Usage, clamp, engine_call_class, json, now,
    record_context_usage,
};

/// The native spawn tool an engine's own `Session` init advertises despite
/// the kill-switch flag we pass it (`args()` in `claude_code.rs` /
/// `codex_app.rs`), or `None` when the advertised list looks clean.
///
/// Codex does not advertise tools in its session response. Its adapter
/// disables `agents.enabled` and rejects native collaboration notifications
/// directly, before they can be mistaken for tracked Aether delegation.
pub(super) fn disallowed_native_spawn_tool(
    kind: EngineKind,
    tools: &[String],
) -> Option<&'static str> {
    if kind != EngineKind::CLAUDE_CODE {
        return None;
    }
    ["Task", "Agent"]
        .into_iter()
        .find(|name| tools.iter().any(|tool| tool == name))
}

pub(super) struct StreamState {
    /// Provider tool-use id → call id and observed start, for matching and timing results.
    pub(super) calls: BTreeMap<String, PendingCall>,
    /// Content-block index → tool name for input deltas.
    pub(super) tool_names: BTreeMap<usize, String>,
    /// The engine reported per-message usage at least once this turn.
    pub(super) usage_seen: bool,
    /// An assistant message arrived, so an invocation was recorded.
    pub(super) invoked: bool,
    /// The model called at least one tool this turn.
    pub(super) ran_tools: bool,
    /// Present only for incremental billing; previous runs must survive reconciliation.
    pub(super) usage_before: Option<(Usage, Usage)>,
    pub(super) stream_usage: Usage,
    /// Request prices cannot be reconstructed from cumulative run counters.
    pub(super) request_usage_seen: bool,
    /// At least one invocation this turn was priced from the registry.
    pub(super) priced: bool,
    /// How this turn is priced: the registry's price for the model exactly as
    /// on the API path, so an engine invocation carries a cost a report, a
    /// replay or a budget can bind to - or, for an unpriced subscription
    /// model, the rate its API equivalent would have charged, as an estimate.
    pub(super) pricing: TurnPricing,
    /// The model this turn was configured to run, which for a `default` id is
    /// not the model that answers: the session reports that one, and pricing
    /// is re-derived from it within this provider.
    pub(super) configured: ResolvedModel,
    /// Protocol notes committed this turn, so a flapping engine cannot drive
    /// the ledger.
    pub(super) protocol_notes: u32,
    /// Monotonic mark of when the engine started producing the assistant
    /// message still in flight: the turn's start for the first one, the
    /// previous message's completion for every one after. Tokio's
    /// clock, not `std::time::Instant`, so a paused-time test can advance it
    /// deterministically instead of sleeping for a real interval.
    pub(super) invocation_since: tokio::time::Instant,
    /// Id the in-flight assistant message is recorded under; its deltas stream
    /// under it too, so a client can retire the live text once it commits.
    pub(super) message_invocation: InvocationId,
    /// Thinking streamed for the in-flight message. Codex streams reasoning
    /// only as deltas, so its commit carries none unless folded in from here.
    pub(super) thinking: String,
    thinking_truncated: bool,
}

impl StreamState {
    pub(super) fn is_bridge_reply(&self, id: &str, name: &str, input: &serde_json::Value) -> bool {
        self.calls
            .get(id)
            .is_some_and(|call| call.tool == format!("mcp__aether__{name}") && &call.input == input)
    }

    pub(super) fn new(pricing: TurnPricing, configured: ResolvedModel) -> Self {
        Self {
            calls: BTreeMap::new(),
            tool_names: BTreeMap::new(),
            usage_seen: false,
            invoked: false,
            ran_tools: false,
            usage_before: None,
            stream_usage: Usage::default(),
            request_usage_seen: false,
            priced: false,
            pricing,
            configured,
            protocol_notes: 0,
            invocation_since: tokio::time::Instant::now(),
            message_invocation: InvocationId::new(),
            thinking: String::new(),
            thinking_truncated: false,
        }
    }
}

/// Protocol notes one turn may put on the record, and how much of one is
/// kept. Both bounds are on the engine's own text, which is untrusted.
const MAX_PROTOCOL_NOTES: u32 = 8;
const MAX_THINKING_BYTES: usize = 4 * 1024 * 1024;

const PROTOCOL_NOTE_CHARS: usize = 300;

pub(super) const PROTOCOL_NOTE_EVENT: &str = "engine.protocol_note";

pub(super) const SHRUNK_CLOSING_USAGE: &str = "the engine's closing receipt omitted streamed usage";

/// Put a departure from the protocol billing depends on - a counter the engine
/// restarted, a field it stopped sending, a turn it never accounted - on the
/// record. A warn log is not an audit trail: an invoice nobody can check
/// against the ledger is what this exists to prevent.
fn protocol_note(
    runtime: &Arc<Runtime>,
    ctx: &TurnCtx,
    state: &mut StreamState,
    text: &str,
) -> Result<()> {
    tracing::warn!(agent = %ctx.agent.id, "{text}");
    state.protocol_notes = state.protocol_notes.saturating_add(1);
    if state.protocol_notes > MAX_PROTOCOL_NOTES {
        return Ok(());
    }
    let note: String = text.chars().take(PROTOCOL_NOTE_CHARS).collect();
    runtime.apply(Commit::new().event(ctx.draft(EventKind::Custom {
        name: PROTOCOL_NOTE_EVENT.into(),
        payload: json!({ "note": note }),
    })))?;
    Ok(())
}

/// A tool call the engine made and has not reported a result for. The name is
/// kept so a call refused without the engine's echo can still be closed.
pub(super) struct PendingCall {
    pub(super) id: ToolCallId,
    pub(super) tool: String,
    pub(super) input: serde_json::Value,
    pub(super) started: Instant,
}

pub(super) fn handle_event(
    runtime: &Arc<Runtime>,
    ctx: &mut TurnCtx,
    snapshot: &InvocationSnapshot,
    event: EngineEvent,
    state: &mut StreamState,
) -> Result<()> {
    match event {
        EngineEvent::Session { id, model, tools } => {
            // For a `default` model the CLI picks what answers, so this report
            // is the first and only moment the turn can be priced. It
            // reprices nothing a price or an estimate already covers, and an
            // id this engine does not serve leaves the turn unpriced. Outside
            // the session-id check below: a resumed session is not re-announced
            // as new, but its turn still has to be priced. Only before any
            // usage was accounted: a rate that changes mid-turn would price
            // later deltas but not earlier ones, and reconciliation would then
            // drop the billed part of that mixed sum.
            if let Some(reported) = model.as_deref()
                && let Some(repriced) =
                    TurnPricing::of_session(runtime, &state.configured, reported)
            {
                if state.usage_seen {
                    tracing::warn!(
                        agent = %ctx.agent.id,
                        %reported,
                        "engine session reported its model after usage was accounted; the turn stays unpriced"
                    );
                } else {
                    tracing::debug!(
                        agent = %ctx.agent.id,
                        configured = %state.configured.model.id,
                        %reported,
                        "pricing the turn from the model the engine session reported"
                    );
                    state.pricing = repriced;
                }
            }
            if ctx.agent.engine_session.as_deref() != Some(id.as_str()) {
                ctx.agent.engine_session = Some(id.clone());
                ctx.agent.updated_at = now();
                let hold = runtime.hold();
                let row = crate::turn::agent_row_with_session(runtime, ctx, &hold)?;
                // The row the store keeps decides: a model change mid-turn
                // moved this agent off the engine that owns this session.
                ctx.agent.engine_session.clone_from(&row.engine_session);
                runtime.apply_held(
                    hold,
                    Commit::new()
                        .event(ctx.draft(EventKind::Custom {
                            name: "engine.session".into(),
                            payload: json!({ "session": id, "model": model, "tools": tools }),
                        }))
                        .mutate(Mutation::UpsertAgent(row)),
                )?;
            }
        }
        EngineEvent::Delta(se) => {
            let delta = match se {
                StreamEvent::TextDelta { text, .. } => {
                    if !text.trim().is_empty() {
                        ctx.user_reply_pending = false;
                    }
                    Some(DeltaKind::Text { text })
                }
                StreamEvent::ThinkingDelta { text, .. } => {
                    let remaining = MAX_THINKING_BYTES.saturating_sub(state.thinking.len());
                    let mut end = remaining.min(text.len());
                    while !text.is_char_boundary(end) {
                        end -= 1;
                    }
                    state.thinking.push_str(&text[..end]);
                    if end < text.len() && !state.thinking_truncated {
                        state.thinking_truncated = true;
                        protocol_note(
                            runtime,
                            ctx,
                            state,
                            "streamed reasoning exceeded 4 MiB; its transcript copy was truncated",
                        )?;
                    }
                    Some(DeltaKind::Thinking { text })
                }
                StreamEvent::BlockStart {
                    index,
                    block: aether_core::model::BlockStart::ToolUse { name, .. },
                } => {
                    state.tool_names.insert(index, name.clone());
                    Some(DeltaKind::ToolInput {
                        name,
                        partial_json: String::new(),
                    })
                }
                StreamEvent::ToolInputDelta {
                    index,
                    partial_json,
                } => Some(DeltaKind::ToolInput {
                    name: state.tool_names.get(&index).cloned().unwrap_or_default(),
                    partial_json,
                }),
                _ => None,
            };
            if let Some(delta) = delta {
                runtime
                    .bus
                    .publish_delta(ctx.agent.id, state.message_invocation, delta);
            }
        }
        EngineEvent::Assistant {
            mut content,
            usage,
            model: _,
        } => {
            let thinking = std::mem::take(&mut state.thinking);
            state.thinking_truncated = false;
            if !thinking.trim().is_empty()
                && !content
                    .iter()
                    .any(|block| matches!(block, ContentBlock::Thinking { .. }))
            {
                // Unsigned, so providers drop it on replay; it is kept for the record.
                content.insert(
                    0,
                    ContentBlock::Thinking {
                        text: thinking,
                        signature: None,
                        redacted: false,
                    },
                );
            }
            if content.iter().any(|block| {
                matches!(block,
                    ContentBlock::Text { text } if !text.trim().is_empty()
                )
            }) {
                ctx.user_reply_pending = false;
            }
            state.tool_names.clear();
            state.invoked = true;
            // A message carrying no usage is not a free message: leave its
            // cost unset rather than claim $0, so the turn total can stand in.
            let reported = usage.filter(|u| u.total_tokens() > 0);
            if let Some(usage) = reported {
                record_context_usage(runtime, ctx, usage.context_tokens())?;
            }
            let mut usage = reported.unwrap_or_default();
            if reported.is_some() {
                state.pricing.apply(&mut usage);
                state.priced |= usage.cost_complete;
            } else {
                usage.mark_cost_gap(aether_core::model::CostGap::Unreported);
            }
            // The observed interval since the engine started producing this
            // message: the turn's start for the first one, the previous
            // message's completion for every one after (reset below). This
            // necessarily includes any tool round-trip the engine waited
            // through in between - the engine gives no separate boundary
            // for its own model-request start, so a tool-execution-only
            // figure is not something this can honestly report.
            let observed = tokio::time::Instant::now();
            let duration_ms: u64 = observed
                .saturating_duration_since(state.invocation_since)
                .as_millis()
                .try_into()
                .unwrap_or(u64::MAX);
            state.invocation_since = observed;
            let at = now();
            // Derived from the same `duration_ms` reported on the event
            // below, so the persisted invocation's own span never disagrees
            // with it.
            let started_at =
                at - chrono::Duration::milliseconds(i64::try_from(duration_ms).unwrap_or(i64::MAX));
            let invocation = Invocation {
                id: std::mem::replace(&mut state.message_invocation, InvocationId::new()),
                snapshot_id: snapshot.id,
                agent_id: ctx.agent.id,
                turn: ctx.turn,
                model: snapshot.model.clone(),
                started_at,
                finished_at: Some(at),
                content: content.clone(),
                usage,
                stop_reason: None,
                error: None,
                provider_message_id: None,
            };
            let mut commit = Commit::new()
                .event(ctx.draft(EventKind::InvocationCompleted {
                    invocation: invocation.id,
                    snapshot: snapshot.id,
                    model: snapshot.model.clone(),
                    usage,
                    stop_reason: None,
                    duration_ms,
                }))
                .mutate(Mutation::PutInvocation(invocation.clone()));
            if !content.is_empty() {
                commit.push_event(ctx.draft(EventKind::AgentMessage {
                    role: MessageRole::Assistant,
                    content: content.clone(),
                    invocation: Some(invocation.id),
                }));
            }
            for block in &content {
                if let ContentBlock::ToolUse { id, name, input } = block {
                    let class = engine_call_class(runtime, name);
                    // The engine ran this itself, so Aether has no outcome to
                    // replay; the key still says which calls are one effect.
                    let idempotency_key = class
                        .retry_policy()
                        .record_key
                        .then(|| IdempotencyKey::derive(name, input));
                    let call = ToolCall {
                        id: ToolCallId::new(),
                        provider_call_id: id.clone(),
                        name: name.clone(),
                        input: input.clone(),
                        agent_id: ctx.agent.id,
                        task_id: ctx.task.as_ref().map(|t| t.id),
                        invocation_id: Some(invocation.id),
                        resources: Vec::new(),
                        idempotency_key,
                        requested_at: at,
                    };
                    state.calls.insert(
                        id.clone(),
                        PendingCall {
                            id: call.id,
                            tool: name.clone(),
                            input: input.clone(),
                            started: Instant::now(),
                        },
                    );
                    commit.push_event(ctx.draft(EventKind::ToolRequested {
                        call: call.clone(),
                        side_effect: class,
                    }));
                    commit.push_event(ctx.draft(EventKind::ToolStarted { call: call.id }));
                    ctx.agent.stats.tool_calls += 1;
                    state.ran_tools = true;
                }
            }
            if reported.is_some() {
                state.usage_seen = true;
                ctx.add_usage(usage);
            }
            ctx.agent.updated_at = at;
            let hold = runtime.hold();
            commit.push_mutation(Mutation::UpsertAgent(crate::turn::agent_row(
                runtime, ctx, &hold,
            )?));
            let answered = snapshot.model.clone();
            crate::turn::close_exhaustion_episode(runtime, ctx, &hold, &mut commit, &answered)?;
            runtime.apply_held(hold, commit)?;
        }
        EngineEvent::UsageDelta { mut usage } => {
            // The engine could not read every counter behind these tokens, so
            // any price for them could be off either way: counted, unpriced.
            let coverage = usage;
            state.pricing.apply(&mut usage);
            if !coverage.cost_complete {
                usage.set_cost(None);
                usage.cost_gap = Some(
                    coverage
                        .cost_gap
                        .unwrap_or(aether_core::model::CostGap::Unreported),
                );
            }
            record_usage(runtime, ctx, state, usage)?;
        }
        EngineEvent::UsageRequest { request } => {
            let usage = state.pricing.request_delta(&request);
            state.request_usage_seen = true;
            record_usage(runtime, ctx, state, usage)?;
        }
        EngineEvent::ToolResult {
            tool_use_id,
            content,
            is_error,
        } => {
            let Some(call) = state.calls.remove(&tool_use_id) else {
                tracing::debug!(agent = %ctx.agent.id, %tool_use_id, "tool result for an unknown call");
                return Ok(());
            };
            let (call, started) = (call.id, call.started);
            let outcome = ToolOutcome {
                content: content.clone(),
                is_error,
                duration_ms: started.elapsed().as_millis().try_into().unwrap_or(u64::MAX),
                side_effect_occurred: Some(true),
                ..Default::default()
            };
            // Also a transcript entry, exactly as the API path composes one:
            // without it the tool call the engine made dangles, and a handoff
            // would hand the next model a call that looks like it never ran.
            runtime.apply(
                Commit::new()
                    .event(ctx.draft(EventKind::ToolSucceeded { call, outcome }))
                    .event(ctx.draft(EventKind::AgentMessage {
                        role: MessageRole::User,
                        content: vec![ContentBlock::ToolResult {
                            tool_use_id,
                            content: vec![aether_core::model::ToolResultBlock::Text {
                                text: content,
                            }],
                            is_error,
                        }],
                        invocation: None,
                    })),
            )?;
        }
        EngineEvent::RateLimit { payload } => {
            match aether_core::provider::ProviderUsage::from_engine_payload(
                &payload,
                aether_core::time::now(),
            ) {
                Some(window) => {
                    // The engine that reported it, which on a fallback link is
                    // not the agent's primary provider.
                    crate::scheduler::record_quota(runtime, &snapshot.model.provider, window)?
                }
                // Keep an unrecognized shape verbatim rather than lose it.
                None => {
                    runtime.apply(Commit::new().event(ctx.draft(EventKind::Custom {
                        name: "engine.rate_limit".into(),
                        payload,
                    })))?;
                }
            }
        }
        EngineEvent::SessionUsage { session, total } => {
            // Not committed on its own: the row write that carries this turn's
            // next spend carries it too, so the persisted baseline and what
            // was billed against it can never disagree.
            ctx.agent.engine_usage =
                Some(aether_core::agent::EngineSessionUsage { session, total });
        }
        EngineEvent::ContextUsage { tokens } => {
            record_context_usage(runtime, ctx, tokens)?;
        }
        EngineEvent::Note(text) => {
            tracing::debug!(agent = %ctx.agent.id, "{}", clamp(&text, MAX_ENGINE_ERROR))
        }
        EngineEvent::ProtocolNote(text) => protocol_note(runtime, ctx, state, &text)?,
        // Consumed by the drive loop, which decides when a turn is over.
        EngineEvent::BackgroundTask { .. }
        | EngineEvent::Permission { .. }
        | EngineEvent::Question { .. }
        | EngineEvent::InputAccepted
        | EngineEvent::InputRejected { .. } => {}
        EngineEvent::Done(_) => {}
    }
    Ok(())
}

fn record_usage(
    runtime: &Arc<Runtime>,
    ctx: &mut TurnCtx,
    state: &mut StreamState,
    usage: Usage,
) -> Result<()> {
    state.priced |= usage.cost_complete;
    state.usage_seen = true;
    state
        .usage_before
        .get_or_insert((ctx.agent.stats.usage, ctx.turn_usage));
    state.stream_usage += usage;
    ctx.add_usage(usage);
    ctx.agent.updated_at = now();
    // Billing updates publish absolute totals without creating another invocation.
    let commit = Commit::new().event(ctx.draft(EventKind::AgentUsageUpdated {
        total: ctx.agent.stats.usage,
        turn_usage: ctx.turn_usage,
    }));
    let hold = runtime.hold();
    let row = crate::turn::agent_row(runtime, ctx, &hold)?;
    runtime.apply_held(hold, commit.mutate(Mutation::UpsertAgent(row)))?;
    Ok(())
}
