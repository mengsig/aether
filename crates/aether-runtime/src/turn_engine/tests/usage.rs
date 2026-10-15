//! Native invocation records, usage and session pricing regressions.
use super::*;

/// A snapshot `handle_event`'s `Assistant` arm actually reads (`id`,
/// `model`) - the rest is inert filler, not a real context assembly.
pub(super) fn bare_snapshot(agent_id: AgentId) -> InvocationSnapshot {
    InvocationSnapshot {
        id: SnapshotId::new(),
        agent_id,
        task_id: None,
        turn: 0,
        model: ModelRef::new("claude-code", "haiku"),
        adapter: None,
        capabilities: None,
        memory_ids: Vec::new(),
        layers: Vec::new(),
        messages: Vec::new(),
        tools: Vec::new(),
        params: InvocationParams::default(),
        token_estimate: 0,
        created_at: now(),
    }
}

fn assistant_saying(text: &str) -> EngineEvent {
    EngineEvent::Assistant {
        content: vec![ContentBlock::text(text)],
        usage: None,
        model: None,
    }
}

/// The model a turn was configured with, as `drive` resolves it.
fn configured(runtime: &Arc<Runtime>, model: &str) -> ResolvedModel {
    let model = ModelRef::parse(model).expect("a model reference");
    runtime
        .models
        .registry()
        .resolve(&model)
        .unwrap_or_else(|e| panic!("resolve {model}: {e}"))
}

/// `InvocationCompleted.duration_ms` was once a literal
/// `0` regardless of how long the engine actually took, and each
/// invocation's own `started_at`/`finished_at` were the same instant.
///
/// The clock is paused and advanced by exact amounts (`tokio::time`,
/// `test-util`) rather than backdating `state.invocation_since` and
/// racing a real sleep: `handle_event` reads `tokio::time::Instant::now()`
/// itself, so an unpaused clock leaves real wall time between the
/// backdate and that read, which a loaded host can stretch past a fixed
/// tolerance. A paused clock makes the observed interval exactly the
/// advanced amount, so the assertions below can be exact equalities.
#[tokio::test(start_paused = true)]
async fn engine_invocation_duration_is_the_observed_interval_and_resets_per_message() {
    let (runtime, _dir) = test_runtime(|_| {}).await;
    let mut ctx = started_agent_ctx(&runtime).await;
    let agent_id = ctx.agent.id;
    let snapshot = bare_snapshot(agent_id);
    // Marks `invocation_since` at the paused clock's current instant,
    // exactly as `StreamState::new` does in `drive` before any advance.
    let mut state = StreamState::new(
        TurnPricing::default(),
        configured(&runtime, "claude-code/default"),
    );

    // First message: the engine took exactly 120ms since the turn started.
    tokio::time::advance(std::time::Duration::from_millis(120)).await;
    handle_event(
        &runtime,
        &mut ctx,
        &snapshot,
        assistant_saying("first"),
        &mut state,
    )
    .expect("first assistant event");

    // Second message: exactly 250ms since the *first* completed. A bug
    // that never resets `invocation_since` would accumulate from the
    // first mark and read 370ms (120+250) here instead of 250ms.
    tokio::time::advance(std::time::Duration::from_millis(250)).await;
    handle_event(
        &runtime,
        &mut ctx,
        &snapshot,
        assistant_saying("second"),
        &mut state,
    )
    .expect("second assistant event");

    let events = runtime
        .store
        .events(0, 1000, &EventFilter::for_agent(agent_id))
        .expect("events");
    let completed: Vec<(InvocationId, u64)> = events
        .iter()
        .filter_map(|e| match &e.kind {
            EventKind::InvocationCompleted {
                invocation,
                duration_ms,
                ..
            } => Some((*invocation, *duration_ms)),
            _ => None,
        })
        .collect();
    assert_eq!(completed.len(), 2, "one InvocationCompleted per message");
    let (first_id, first_ms) = completed[0];
    let (second_id, second_ms) = completed[1];

    assert_eq!(
        first_ms, 120,
        "first invocation must show the observed interval, not the hardcoded 0: got {first_ms}ms"
    );
    assert_eq!(
        second_ms, 250,
        "second invocation must reset from the first's completion \
         instead of accumulating turn-start time: got {second_ms}ms"
    );

    // The persisted `Invocation` record must agree with the event's own
    // figure - both are derived from the same measured interval.
    for (id, expected_ms) in [(first_id, first_ms), (second_id, second_ms)] {
        let invocation = runtime
            .store
            .invocation(id)
            .expect("read invocation")
            .expect("invocation persisted");
        assert_eq!(
            invocation.duration_ms(),
            Some(expected_ms),
            "persisted invocation span must match the event's duration_ms"
        );
    }

    runtime.shutdown().await;
}

/// Clients retire a message's live text when a message under the same
/// invocation commits. One id for the whole engine turn left every streamed
/// message on screen, duplicated below the turn's later tool calls.
#[tokio::test]
async fn each_engine_message_streams_under_the_invocation_it_commits_as() {
    let (runtime, _dir) = test_runtime(|_| {}).await;
    let mut ctx = started_agent_ctx(&runtime).await;
    let agent_id = ctx.agent.id;
    let snapshot = bare_snapshot(agent_id);
    let mut state = StreamState::new(
        TurnPricing::default(),
        configured(&runtime, "claude-code/default"),
    );
    let mut bus = runtime.bus.subscribe();
    for text in ["first", "second"] {
        let delta = EngineEvent::Delta(StreamEvent::TextDelta {
            index: 0,
            text: text.into(),
        });
        handle_event(&runtime, &mut ctx, &snapshot, delta, &mut state).expect("delta");
        handle_event(
            &runtime,
            &mut ctx,
            &snapshot,
            assistant_saying(text),
            &mut state,
        )
        .expect("assistant event");
    }

    let mut streamed = Vec::new();
    while let Ok(message) = bus.try_recv() {
        if let BusMessage::Delta {
            invocation,
            delta: DeltaKind::Text { text },
            ..
        } = message
        {
            streamed.push((text, invocation));
        }
    }
    let committed: Vec<(String, InvocationId)> = runtime
        .store
        .events(0, 1000, &EventFilter::for_agent(agent_id))
        .expect("events")
        .iter()
        .filter_map(|e| match &e.kind {
            EventKind::AgentMessage {
                role: MessageRole::Assistant,
                content,
                invocation: Some(invocation),
            } => match &content[..] {
                [ContentBlock::Text { text }] => Some((text.clone(), *invocation)),
                _ => None,
            },
            _ => None,
        })
        .collect();
    assert_eq!(streamed, committed);
    assert_ne!(committed[0].1, committed[1].1, "one invocation per message");

    runtime.shutdown().await;
}

/// The assistant messages an engine turn committed, in order.
fn committed_assistant(runtime: &Arc<Runtime>, agent_id: AgentId) -> Vec<Vec<ContentBlock>> {
    runtime
        .store
        .events(0, 1000, &EventFilter::for_agent(agent_id))
        .expect("events")
        .into_iter()
        .filter_map(|e| match e.kind {
            EventKind::AgentMessage {
                role: MessageRole::Assistant,
                content,
                ..
            } => Some(content),
            _ => None,
        })
        .collect()
}

fn thinking_delta(text: &str) -> EngineEvent {
    EngineEvent::Delta(StreamEvent::ThinkingDelta {
        index: 0,
        text: text.into(),
    })
}

fn unsigned_thinking(text: &str) -> ContentBlock {
    ContentBlock::Thinking {
        text: text.into(),
        signature: None,
        redacted: false,
    }
}

/// Codex streams reasoning only as deltas and commits its text and tool items
/// without it. Once a message's live text retires on commit, reasoning the
/// commit does not carry is lost to the transcript, live and on replay.
#[tokio::test]
async fn codex_reasoning_is_committed_with_the_message_it_preceded() {
    let (runtime, _dir) = test_runtime(|_| {}).await;
    let mut ctx = started_agent_ctx(&runtime).await;
    let agent_id = ctx.agent.id;
    let snapshot = bare_snapshot(agent_id);
    let mut state = StreamState::new(
        TurnPricing::default(),
        configured(&runtime, "codex/default"),
    );
    let tool_use = ContentBlock::ToolUse {
        id: "call_1".into(),
        name: "bash".into(),
        input: json!({"command": "ls"}),
    };
    let events = [
        thinking_delta("Listing the "),
        thinking_delta("files first."),
        EngineEvent::Assistant {
            content: vec![tool_use.clone()],
            usage: None,
            model: None,
        },
        thinking_delta("One file; summarise."),
        assistant_saying("The workspace holds one file."),
        assistant_saying("Nothing else to add."),
    ];
    for event in events {
        handle_event(&runtime, &mut ctx, &snapshot, event, &mut state).expect("event");
    }

    assert_eq!(
        committed_assistant(&runtime, agent_id),
        vec![
            vec![unsigned_thinking("Listing the files first."), tool_use],
            vec![
                unsigned_thinking("One file; summarise."),
                ContentBlock::text("The workspace holds one file."),
            ],
            vec![ContentBlock::text("Nothing else to add.")],
        ]
    );

    runtime.shutdown().await;
}

/// Claude Code commits the thinking it streamed, signed; folding the stream in
/// as well would draw the same reasoning twice.
#[tokio::test]
async fn committed_thinking_is_not_folded_in_twice() {
    let (runtime, _dir) = test_runtime(|_| {}).await;
    let mut ctx = started_agent_ctx(&runtime).await;
    let agent_id = ctx.agent.id;
    let snapshot = bare_snapshot(agent_id);
    let mut state = StreamState::new(
        TurnPricing::default(),
        configured(&runtime, "claude-code/default"),
    );
    let signed = ContentBlock::Thinking {
        text: "Check the logs.".into(),
        signature: Some("sig".into()),
        redacted: false,
    };
    let events = [
        thinking_delta("Check the logs."),
        EngineEvent::Assistant {
            content: vec![signed.clone()],
            usage: None,
            model: None,
        },
        assistant_saying("Checking the logs."),
    ];
    for event in events {
        handle_event(&runtime, &mut ctx, &snapshot, event, &mut state).expect("event");
    }

    assert_eq!(
        committed_assistant(&runtime, agent_id),
        vec![vec![signed], vec![ContentBlock::text("Checking the logs.")]]
    );

    runtime.shutdown().await;
}

/// An engine model with no price of its own is billed nothing,
/// so its dollars must arrive as a labelled estimate beside a cost that
/// stays unreported - never as spend a budget could fire on.
#[tokio::test]
async fn an_unpriced_engine_turn_reports_an_estimate_and_no_spend() {
    let (runtime, _dir) = test_runtime(|_| {}).await;
    let mut ctx = started_agent_ctx(&runtime).await;
    let agent_id = ctx.agent.id;
    // `codex/gpt-6-astra` as an operator's `providers.toml` entry without a
    // price would resolve: only its API twin, `openai/gpt-6-astra`, has one.
    let mut resolved = configured(&runtime, "codex/gpt-6-astra");
    resolved.model.capabilities.cost = None;
    let pricing = TurnPricing::of(&runtime, &resolved);
    assert!(pricing.billed.is_none(), "the model itself prices nothing");
    let rate = pricing.estimate.expect("openai/gpt-6-astra's list price");
    let mut state = StreamState::new(pricing, resolved.clone());

    let usage = Usage {
        input_tokens: 39_500,
        output_tokens: 61_000,
        cache_read_tokens: 3_210_000,
        ..Default::default()
    };
    handle_event(
        &runtime,
        &mut ctx,
        &bare_snapshot(agent_id),
        EngineEvent::UsageDelta { usage },
        &mut state,
    )
    .expect("usage delta");

    let total = ctx.agent.stats.usage;
    assert_eq!(total.cost_usd, 0.0, "nobody billed this turn");
    assert!(!total.cost_complete, "and the total says so");
    let expected = rate.cost_of(&usage);
    assert!(expected > 0.0, "the twin's price is a real number");
    assert_eq!(total.estimated_cost_usd, Some(expected));
    assert_eq!(total.cost_basis, aether_core::model::CostBasis::ListPrice);
    assert_eq!(total.api_equivalent_usd(), Some(expected));
    assert!(
        total
            .estimate_line()
            .expect("a labelled estimate")
            .contains(Usage::COST_ESTIMATE),
        "{total:?}"
    );
    assert_eq!(total.cache_read_tokens, 3_210_000, "the split is kept");

    runtime.shutdown().await;
}

/// The engine opens its session, reports the model it picked, then bills
/// tokens against it - the order `drive` sees them in.
fn session_then_usage(
    runtime: &Arc<Runtime>,
    ctx: &mut TurnCtx,
    state: &mut StreamState,
    reported: &str,
    usage: Usage,
) {
    let snapshot = bare_snapshot(ctx.agent.id);
    handle_event(
        runtime,
        ctx,
        &snapshot,
        EngineEvent::Session {
            id: "s-236".into(),
            model: Some(reported.into()),
            tools: Vec::new(),
        },
        state,
    )
    .expect("session event");
    handle_event(
        runtime,
        ctx,
        &snapshot,
        EngineEvent::UsageDelta { usage },
        state,
    )
    .expect("usage delta");
}

/// `default` is the standard configuration and the registry
/// can price neither engine's placeholder - the CLI picks the model. The
/// session is where it says which, so that report is what the turn costs.
#[tokio::test]
async fn a_session_prices_a_default_turn_the_configured_model_could_not() {
    let (runtime, _dir) = test_runtime(|_| {}).await;
    let usage = Usage {
        input_tokens: 20_000,
        output_tokens: 5_000,
        cache_read_tokens: 400_000,
        ..Default::default()
    };

    // Codex is a subscription with no price of its own, so the session model
    // it reports estimates the turn at the OpenAI API list price.
    let mut ctx = started_agent_ctx(&runtime).await;
    let codex = configured(&runtime, "codex/default");
    let mut state = StreamState::new(TurnPricing::of(&runtime, &codex), codex);
    assert!(
        state.pricing.billed.is_none() && state.pricing.estimate.is_none(),
        "`codex/default` is exactly the case nobody could price"
    );
    session_then_usage(&runtime, &mut ctx, &mut state, "gpt-6-astra", usage);
    let astra = runtime
        .models
        .registry()
        .api_equivalent_price(&configured(&runtime, "codex/gpt-6-astra"))
        .expect("codex/gpt-6-astra has an API twin");
    let total = ctx.agent.stats.usage;
    assert_eq!(total.estimated_cost_usd, Some(astra.cost_of(&usage)));
    assert_eq!(total.cost_usd, 0.0, "nobody billed these dollars");
    assert_eq!(total.cost_basis, aether_core::model::CostBasis::ListPrice);
    assert!(!total.cost_complete, "an estimate is not a billed cost");

    // Claude Code bills the subscription at the list price the registry
    // holds, so the same report produces a cost, not an estimate.
    let mut ctx = started_agent_ctx(&runtime).await;
    let claude = configured(&runtime, "claude-code/default");
    let mut state = StreamState::new(TurnPricing::of(&runtime, &claude), claude);
    session_then_usage(&runtime, &mut ctx, &mut state, "claude-fable-5-1", usage);
    let fable = configured(&runtime, "claude-code/fable")
        .model
        .capabilities
        .cost
        .expect("fable is priced");
    let total = ctx.agent.stats.usage;
    assert_eq!(total.cost_usd, fable.cost_of(&usage));
    assert!(total.cost_complete, "a billed turn is a confident sum");
    assert_eq!(
        total.estimated_cost_usd, None,
        "a priced turn needs no estimate"
    );

    // A model neither table lists is not a licence to invent a price.
    let mut ctx = started_agent_ctx(&runtime).await;
    let claude = configured(&runtime, "claude-code/default");
    let mut state = StreamState::new(TurnPricing::of(&runtime, &claude), claude);
    session_then_usage(&runtime, &mut ctx, &mut state, "claude-nonesuch-9", usage);
    let total = ctx.agent.stats.usage;
    assert_eq!(total.cost_usd, 0.0);
    assert!(!total.cost_complete);
    assert_eq!(total.estimated_cost_usd, None);

    // A configured model with a price of its own stays the authority,
    // whatever the session reports.
    let mut ctx = started_agent_ctx(&runtime).await;
    let haiku = configured(&runtime, "claude-code/haiku");
    let priced = haiku.model.capabilities.cost.expect("haiku is priced");
    let mut state = StreamState::new(TurnPricing::of(&runtime, &haiku), haiku);
    session_then_usage(&runtime, &mut ctx, &mut state, "claude-fable-5-1", usage);
    let total = ctx.agent.stats.usage;
    assert_eq!(total.cost_usd, priced.cost_of(&usage));
    assert!(total.cost_complete);
    assert_eq!(total.estimated_cost_usd, None);

    // A session that names its model only after usage was accounted must
    // not switch the rate mid-turn: the earlier delta went in unpriced,
    // and a mixed sum would lose its billed part at reconciliation.
    let mut ctx = started_agent_ctx(&runtime).await;
    let claude = configured(&runtime, "claude-code/default");
    let mut state = StreamState::new(TurnPricing::of(&runtime, &claude), claude);
    let snapshot = bare_snapshot(ctx.agent.id);
    handle_event(
        &runtime,
        &mut ctx,
        &snapshot,
        EngineEvent::UsageDelta { usage },
        &mut state,
    )
    .expect("early usage delta");
    session_then_usage(&runtime, &mut ctx, &mut state, "claude-fable-5-1", usage);
    assert!(
        state.pricing.billed.is_none() && state.pricing.estimate.is_none(),
        "a late session report leaves the turn's pricing as it was"
    );
    let total = ctx.agent.stats.usage;
    assert_eq!(total.input_tokens, 2 * usage.input_tokens);
    assert_eq!(total.cost_usd, 0.0, "no delta was priced, so no dollars");
    assert!(!total.cost_complete);
    assert_eq!(total.estimated_cost_usd, None);

    runtime.shutdown().await;
}

#[tokio::test]
async fn streaming_reasoning_is_bounded_on_utf8_boundaries_with_one_note() {
    let (runtime, _dir) = test_runtime(|_| {}).await;
    let mut ctx = started_agent_ctx(&runtime).await;
    let snapshot = bare_snapshot(ctx.agent.id);
    let mut state = StreamState::new(
        TurnPricing::default(),
        configured(&runtime, "codex/default"),
    );
    let text = "é".repeat(2 * 1024 * 1024 + 1);
    for _ in 0..3 {
        handle_event(
            &runtime,
            &mut ctx,
            &snapshot,
            thinking_delta(&text),
            &mut state,
        )
        .unwrap();
    }
    assert_eq!(state.thinking.len(), 4 * 1024 * 1024);
    assert!(state.thinking.ends_with('é'));
    let events = runtime
        .store
        .events(0, 100, &EventFilter::own_events(ctx.agent.id))
        .unwrap();
    let notes = events
        .iter()
        .filter(|event| {
            let EventKind::Custom { name, payload } = &event.kind else {
                return false;
            };
            name == "engine.protocol_note"
                && payload["note"].as_str().unwrap().contains("truncated")
        })
        .count();
    assert_eq!(notes, 1);
    handle_event(
        &runtime,
        &mut ctx,
        &snapshot,
        assistant_saying("done"),
        &mut state,
    )
    .unwrap();
    handle_event(
        &runtime,
        &mut ctx,
        &snapshot,
        thinking_delta("next"),
        &mut state,
    )
    .unwrap();
    assert_eq!(state.thinking, "next");
    runtime.shutdown().await;
}

#[tokio::test]
async fn streamed_text_is_kept_bounded_until_its_message_commits() {
    let (runtime, _dir) = test_runtime(|_| {}).await;
    let mut ctx = started_agent_ctx(&runtime).await;
    let snapshot = bare_snapshot(ctx.agent.id);
    let mut state = StreamState::new(
        TurnPricing::default(),
        configured(&runtime, "codex/default"),
    );
    let text_delta = |text: &str| {
        EngineEvent::Delta(StreamEvent::TextDelta {
            index: 0,
            text: text.into(),
        })
    };
    let text = "é".repeat(2 * 1024 * 1024 + 1);
    for _ in 0..3 {
        handle_event(&runtime, &mut ctx, &snapshot, text_delta(&text), &mut state).unwrap();
    }
    assert_eq!(state.text.len(), 4 * 1024 * 1024);
    assert!(state.text.ends_with('é'));
    let notes = runtime
        .store
        .events(0, 100, &EventFilter::own_events(ctx.agent.id))
        .unwrap()
        .into_iter()
        .filter(|event| {
            matches!(&event.kind, EventKind::Custom { name, payload }
                if name == "engine.protocol_note"
                    && payload["note"].as_str().unwrap().contains("streamed text"))
        })
        .count();
    assert_eq!(notes, 1);
    handle_event(
        &runtime,
        &mut ctx,
        &snapshot,
        assistant_saying("done"),
        &mut state,
    )
    .unwrap();
    assert!(state.text.is_empty(), "a committed message is no partial");
    handle_event(
        &runtime,
        &mut ctx,
        &snapshot,
        text_delta("next"),
        &mut state,
    )
    .unwrap();
    handle_event(
        &runtime,
        &mut ctx,
        &snapshot,
        thinking_delta(" "),
        &mut state,
    )
    .unwrap();
    assert_eq!(state.take_partial(), vec![ContentBlock::text("next")]);
    assert!(state.text.is_empty() && state.thinking.is_empty());
    runtime.shutdown().await;
}

#[tokio::test]
async fn a_partial_cut_by_the_cap_is_taken_whole() {
    let (runtime, _dir) = test_runtime(|_| {}).await;
    let mut ctx = started_agent_ctx(&runtime).await;
    let snapshot = bare_snapshot(ctx.agent.id);
    let mut state = StreamState::new(
        TurnPricing::default(),
        configured(&runtime, "codex/default"),
    );
    handle_event(
        &runtime,
        &mut ctx,
        &snapshot,
        EngineEvent::UsageDelta {
            usage: one_reading(),
        },
        &mut state,
    )
    .unwrap();
    let over_cap = EngineEvent::Delta(StreamEvent::TextDelta {
        index: 0,
        text: "a".repeat(4 * 1024 * 1024 + 1),
    });
    handle_event(&runtime, &mut ctx, &snapshot, over_cap, &mut state).unwrap();
    let events = runtime
        .store
        .events(0, usize::MAX, &EventFilter::for_agent(ctx.agent.id))
        .unwrap();
    let usage_seq = events
        .iter()
        .find(|event| matches!(event.kind, EventKind::AgentUsageUpdated { .. }))
        .unwrap()
        .seq;
    let note_seq = events.iter().find(|event| matches!(&event.kind, EventKind::Custom { name, .. } if name == PROTOCOL_NOTE_EVENT)).unwrap().seq;
    assert!(
        usage_seq < note_seq,
        "held usage precedes the truncation note"
    );
    let partial = state.take_partial();
    let [ContentBlock::Text { text }] = partial.as_slice() else {
        panic!("expected one text block, got {partial:?}");
    };
    assert_eq!(text.len(), 4 * 1024 * 1024, "kept up to the cap");
    assert!(state.text.is_empty() && state.thinking.is_empty());
    runtime.shutdown().await;
}

#[tokio::test]
async fn engine_context_occupancy_saturates_large_provider_counters() {
    let (runtime, _dir) = test_runtime(|_| {}).await;
    let mut ctx = started_agent_ctx(&runtime).await;
    let snapshot = bare_snapshot(ctx.agent.id);
    let mut state = StreamState::new(
        TurnPricing::default(),
        configured(&runtime, "codex/default"),
    );
    handle_event(
        &runtime,
        &mut ctx,
        &snapshot,
        EngineEvent::Assistant {
            content: vec![ContentBlock::text("done")],
            model: None,
            usage: Some(Usage {
                input_tokens: u64::MAX,
                cache_read_tokens: 1,
                ..Default::default()
            }),
        },
        &mut state,
    )
    .unwrap();
    assert_eq!(ctx.agent.stats.context_tokens, Some(u64::MAX));
    runtime.shutdown().await;
}

#[tokio::test]
async fn late_engine_failure_and_root_notice_share_one_commit() {
    let (runtime, _dir) = test_runtime(|_| {}).await;
    let (root, _) = runtime
        .create_agent(
            CreateAgentSpec {
                role: "root".into(),
                start: false,
                ..Default::default()
            },
            Actor::user(),
        )
        .unwrap();
    let mut ctx = started_agent_ctx(&runtime).await;
    ctx.agent.parent = Some(root.id);
    runtime
        .apply(Commit::new().mutate(Mutation::UpsertAgent(ctx.agent.clone())))
        .unwrap();
    let snapshot = bare_snapshot(ctx.agent.id);
    let commits = runtime.store.commits();
    crate::turn_engine::outcome::record_engine_incident(
        &runtime,
        &ctx,
        &snapshot,
        EngineKind::CLAUDE_CODE,
        Some("context_length_exceeded"),
    )
    .unwrap();
    assert_eq!(runtime.store.commits() - commits, 1);
    let events = runtime
        .store
        .latest_events(10, &EventFilter::own_events(ctx.agent.id))
        .unwrap();
    assert!(events.iter().any(|event| matches!(
        event.kind,
        EventKind::InvocationFailed {
            class: Some(ModelErrorClass::ContextLimit),
            ..
        }
    )));
    assert!(events.iter().any(|event| matches!(&event.kind,
        EventKind::Custom { name, .. } if name == "exhaustion.reported")));
    let inbox = runtime.store.inbox(root.id, 0).unwrap();
    let notified = inbox.iter().any(|item| {
        let aether_core::inbox::InboxKind::ExternalTrigger {
            source,
            payload,
            operator: false,
        } = &item.kind
        else {
            return false;
        };
        source == "exhaustion"
            && payload["verdict"] == "context limit exceeded; task outcome unchanged"
    });
    assert!(notified);
    runtime.shutdown().await;
}

#[tokio::test]
async fn native_request_pricing_reaches_the_subtree_cost_budget_on_late_output() {
    use aether_engine::{RequestUsage, UsageRequestId};
    let (runtime, _dir) = test_runtime(|_| {}).await;
    let mut parent = started_agent_ctx(&runtime).await;
    parent.agent.config.cost_budget = Some(0.008);
    runtime
        .apply(Commit::new().mutate(Mutation::UpsertAgent(parent.agent.clone())))
        .unwrap();
    let mut child = started_agent_ctx(&runtime).await;
    child.agent.parent = Some(parent.agent.id);
    runtime
        .apply(Commit::new().mutate(Mutation::UpsertAgent(child.agent.clone())))
        .unwrap();
    let resolved = configured(&runtime, "claude-code/haiku");
    let mut state = StreamState::new(TurnPricing::of(&runtime, &resolved), resolved);
    let input = Usage {
        cache_read_tokens: 100_001,
        ..Default::default()
    };
    let total = Usage {
        output_tokens: 2_000,
        ..input
    };
    let snapshot = bare_snapshot(child.agent.id);
    let mut subtree = SubtreeUsage::default();
    for (previous, current) in [(Usage::default(), input), (input, total)] {
        handle_event(
            &runtime,
            &mut child,
            &snapshot,
            EngineEvent::UsageRequest {
                request: RequestUsage {
                    id: UsageRequestId::Engine("long-prompt".into()),
                    previous,
                    total: current,
                },
            },
            &mut state,
        )
        .unwrap();
        let crossed = budget_crossed(&runtime, &parent, &mut subtree).unwrap();
        if current.output_tokens == 0 {
            assert!(crossed.is_none(), "the prompt alone is below the grant");
        } else {
            let crossed = crossed.expect("tier-priced output crosses the ancestor's grant");
            assert!((crossed.usage.cost_usd - 0.01000005).abs() < 1e-12);
            assert_eq!(crossed.usage, child.agent.stats.usage);
        }
    }
    runtime.shutdown().await;
}

#[test]
fn native_request_pricing_reprices_earlier_fragments_when_the_prompt_crosses_the_tier() {
    use aether_core::model::ModelCostTier;
    use aether_engine::{RequestUsage, UsageRequestId};
    let pricing = TurnPricing {
        billed: Some(ModelCost {
            input_per_mtok: 1.0,
            output_per_mtok: 2.0,
            cache_read_per_mtok: 0.1,
            above_threshold: Some(ModelCostTier {
                prompt_tokens: 100_000,
                input_per_mtok: 5.0,
                output_per_mtok: 10.0,
                cache_read_per_mtok: 0.5,
                cache_write_per_mtok: 6.25,
                cache_write_1h_per_mtok: None,
            }),
            ..Default::default()
        }),
        estimate: None,
    };
    let initial = Usage {
        input_tokens: 60_000,
        output_tokens: 2_000,
        ..Default::default()
    };
    let complete = Usage {
        cache_read_tokens: 60_000,
        ..initial
    };
    let mut total = Usage::default();
    for (previous, current) in [(Usage::default(), initial), (initial, complete)] {
        total += pricing.request_delta(&RequestUsage {
            id: UsageRequestId::Engine("late-prompt".into()),
            previous,
            total: current,
        });
    }
    assert!((total.cost_usd - 0.35).abs() < 1e-12);
    assert_eq!(total.total_tokens(), 122_000);
}

fn closing_test_rate() -> ModelCost {
    ModelCost {
        input_per_mtok: 1_000_000.0,
        output_per_mtok: 2_000_000.0,
        cache_read_per_mtok: 3_000_000.0,
        cache_write_per_mtok: 4_000_000.0,
        cache_write_1h_per_mtok: None,
        above_threshold: None,
    }
}

fn closing_test_request() -> Usage {
    Usage {
        input_tokens: 2,
        output_tokens: 4,
        cache_read_tokens: 8,
        cache_write_tokens: 16,
        ..Default::default()
    }
}

fn stream_closing_test_requests(
    runtime: &Arc<Runtime>,
    ctx: &mut TurnCtx,
    pricing: TurnPricing,
) -> StreamState {
    use aether_engine::{RequestUsage, UsageRequestId};
    let snapshot = bare_snapshot(ctx.agent.id);
    let mut state = StreamState::new(pricing, configured(runtime, "claude-code/haiku"));
    for id in ["first", "omitted"] {
        handle_event(
            runtime,
            ctx,
            &snapshot,
            EngineEvent::UsageRequest {
                request: RequestUsage {
                    id: UsageRequestId::Engine(id.into()),
                    previous: Usage::default(),
                    total: closing_test_request(),
                },
            },
            &mut state,
        )
        .unwrap();
    }
    assert!(state.usage_before.is_some());
    assert_eq!(state.stream_usage.total_tokens(), 60);
    state
}

async fn settle_test_receipt(
    runtime: &Arc<Runtime>,
    pricing: TurnPricing,
    closing: Usage,
    cost: Option<f64>,
) -> (Usage, Vec<String>) {
    let mut ctx = started_agent_ctx(runtime).await;
    let before = Usage {
        input_tokens: 7,
        cost_usd: 5.0,
        ..Default::default()
    };
    ctx.agent.stats.usage = before;
    let state = stream_closing_test_requests(runtime, &mut ctx, pricing);
    let run = DriveResult {
        outcome: EngineOutcome {
            usage: closing,
            cost_usd: cost,
            ..Default::default()
        },
        activity_seen: true,
        priced: state.priced,
        usage_seen: state.usage_seen,
        usage_before: state.usage_before,
        stream_usage: state.stream_usage,
        request_usage_seen: state.request_usage_seen,
        pricing: state.pricing,
        ..super::super::execution::test_run()
    };
    super::super::execution::settle_usage(runtime, &mut ctx, &run).unwrap();
    let stored = runtime.store.agent(ctx.agent.id).unwrap().unwrap();
    assert_eq!(stored.stats.usage, ctx.agent.stats.usage);
    let mut expected = before;
    expected += ctx.turn_usage;
    assert_eq!(ctx.agent.stats.usage, expected);
    let notes = runtime
        .store
        .events(0, 1000, &EventFilter::own_events(ctx.agent.id))
        .unwrap()
        .into_iter()
        .filter_map(|event| match event.kind {
            EventKind::Custom { name, payload } if name == PROTOCOL_NOTE_EVENT => {
                Some(payload["note"].as_str().unwrap().to_owned())
            }
            _ => None,
        })
        .collect();
    (ctx.turn_usage, notes)
}

#[tokio::test]
async fn closing_receipt_shortfall_adds_base_price_to_provider_cost() {
    use aether_core::model::CostBasis;
    let (runtime, _dir) = test_runtime(|_| {}).await;
    let pricing = TurnPricing {
        billed: Some(closing_test_rate()),
        estimate: None,
    };
    let (usage, notes) =
        settle_test_receipt(&runtime, pricing, closing_test_request(), Some(10.0)).await;
    assert_eq!(usage.cost_usd, 108.0);
    assert_eq!(usage.cost_basis, CostBasis::Mixed);
    assert!(usage.cost_complete);
    assert_eq!(usage.estimated_cost_usd, None);
    assert_eq!(usage.total_tokens(), 60);
    assert_eq!(notes.len(), 1);
    assert!(notes[0].contains("the missing part is priced from the stream"));
    runtime.shutdown().await;
}

#[tokio::test]
async fn closing_receipt_prices_only_fields_below_the_stream() {
    use aether_core::model::CostBasis;
    let (runtime, _dir) = test_runtime(|_| {}).await;
    let pricing = TurnPricing {
        billed: Some(closing_test_rate()),
        estimate: None,
    };
    let closing = Usage {
        input_tokens: 6,
        output_tokens: 8,
        cache_read_tokens: 8,
        cache_write_tokens: 32,
        ..Default::default()
    };
    let (usage, notes) = settle_test_receipt(&runtime, pricing, closing, Some(10.0)).await;
    assert_eq!(usage.input_tokens, 6);
    assert_eq!(usage.cache_read_tokens, 16);
    assert_eq!(usage.total_tokens(), 62);
    assert_eq!(usage.cost_usd, 34.0);
    assert_eq!(usage.cost_basis, CostBasis::Mixed);
    assert!(usage.cost_complete);
    assert_eq!(usage.estimated_cost_usd, None);
    assert_eq!(notes.len(), 1);
    assert!(notes[0].contains("the missing part is priced from the stream"));
    runtime.shutdown().await;
}

#[tokio::test]
async fn closing_receipt_shortfall_estimates_only_the_unbilled_tokens() {
    use aether_core::model::CostBasis;
    let (runtime, _dir) = test_runtime(|_| {}).await;
    let pricing = TurnPricing {
        billed: None,
        estimate: Some(closing_test_rate()),
    };
    let (usage, notes) =
        settle_test_receipt(&runtime, pricing, closing_test_request(), Some(10.0)).await;
    assert_eq!(usage.cost_usd, 10.0);
    assert!(!usage.cost_complete);
    assert_eq!(usage.cost_basis, CostBasis::Mixed);
    assert_eq!(usage.estimated_cost_usd, Some(98.0));
    assert_eq!(usage.api_equivalent_usd(), Some(108.0));
    assert_eq!(notes.len(), 1);
    assert!(notes[0].contains("the missing part is left out of the billed cost"));
    runtime.shutdown().await;
}

#[tokio::test]
async fn closing_receipt_at_or_above_the_stream_keeps_provider_cost() {
    use aether_core::model::CostBasis;
    let (runtime, _dir) = test_runtime(|_| {}).await;
    let pricing = TurnPricing {
        billed: Some(closing_test_rate()),
        estimate: None,
    };
    let mut closing = closing_test_request();
    closing += closing_test_request();
    for extra in [0, 1] {
        closing.output_tokens += extra;
        let (usage, notes) = settle_test_receipt(&runtime, pricing, closing, Some(10.0)).await;
        assert_eq!(usage.cost_usd, 10.0);
        assert_eq!(usage.cost_basis, CostBasis::ProviderReported);
        assert!(usage.cost_complete);
        assert_eq!(usage.estimated_cost_usd, None);
        assert_eq!(usage.total_tokens(), closing.total_tokens());
        assert!(notes.is_empty());
    }
    runtime.shutdown().await;
}

#[tokio::test]
async fn closing_receipt_without_cost_keeps_stream_pricing_and_estimates() {
    use aether_core::model::CostBasis;
    let (runtime, _dir) = test_runtime(|_| {}).await;
    for billed in [true, false] {
        let pricing = TurnPricing {
            billed: billed.then_some(closing_test_rate()),
            estimate: (!billed).then_some(closing_test_rate()),
        };
        let (usage, notes) =
            settle_test_receipt(&runtime, pricing, closing_test_request(), None).await;
        assert_eq!(usage.cost_usd, if billed { 196.0 } else { 0.0 });
        assert_eq!(usage.cost_complete, billed);
        assert_eq!(usage.cost_basis, CostBasis::ListPrice);
        assert_eq!(usage.estimated_cost_usd, (!billed).then_some(196.0));
        assert_eq!(usage.total_tokens(), 60);
        assert_eq!(notes.len(), 1);
    }
    runtime.shutdown().await;
}

#[tokio::test]
async fn closing_receipt_shortfall_without_rates_is_an_incomplete_floor() {
    use aether_core::model::CostBasis;
    let (runtime, _dir) = test_runtime(|_| {}).await;
    let (usage, notes) = settle_test_receipt(
        &runtime,
        TurnPricing::default(),
        closing_test_request(),
        Some(10.0),
    )
    .await;
    assert_eq!(usage.cost_usd, 10.0);
    assert!(!usage.cost_complete);
    assert_eq!(usage.cost_basis, CostBasis::ProviderReported);
    assert_eq!(usage.estimated_cost_usd, None);
    assert_eq!(usage.total_tokens(), 60);
    assert_eq!(notes.len(), 1);
    assert!(notes[0].contains("the missing part is left out of the billed cost"));
    runtime.shutdown().await;
}

#[tokio::test]
async fn closing_receipt_with_cost_but_no_tokens_keeps_provider_cost() {
    use aether_core::model::CostBasis;
    let (runtime, _dir) = test_runtime(|_| {}).await;
    let pricing = TurnPricing {
        billed: Some(closing_test_rate()),
        estimate: None,
    };
    let (usage, notes) = settle_test_receipt(&runtime, pricing, Usage::default(), Some(10.0)).await;
    assert_eq!(usage.cost_usd, 10.0);
    assert_eq!(usage.cost_basis, CostBasis::ProviderReported);
    assert!(usage.cost_complete);
    assert_eq!(usage.total_tokens(), 60);
    assert!(notes.is_empty());
    runtime.shutdown().await;
}

#[tokio::test]
async fn live_usage_without_counters_or_reason_is_unreported() {
    let (runtime, _dir) = test_runtime(|_| {}).await;
    let mut ctx = started_agent_ctx(&runtime).await;
    let snapshot = bare_snapshot(ctx.agent.id);
    let mut state = StreamState::new(
        TurnPricing::default(),
        configured(&runtime, "claude-code/default"),
    );
    handle_event(
        &runtime,
        &mut ctx,
        &snapshot,
        assistant_saying("answer"),
        &mut state,
    )
    .unwrap();
    let invocations = runtime.store.invocations_for_agent(ctx.agent.id).unwrap();
    assert_eq!(invocations.len(), 1);
    assert_eq!(
        invocations[0].usage.cost_gap,
        Some(aether_core::model::CostGap::Unreported)
    );
    handle_event(
        &runtime,
        &mut ctx,
        &snapshot,
        EngineEvent::UsageDelta {
            usage: Usage {
                input_tokens: 10,
                cost_complete: false,
                ..Default::default()
            },
        },
        &mut state,
    )
    .unwrap();
    assert_eq!(
        state.stream_usage.cost_gap,
        Some(aether_core::model::CostGap::Unreported)
    );
    assert!(!state.stream_usage.cost_complete);
    runtime.shutdown().await;
}

fn stored_usage_events(runtime: &Arc<Runtime>, agent: AgentId) -> Vec<(u64, Usage)> {
    runtime
        .store
        .events(0, usize::MAX, &EventFilter::for_agent(agent))
        .expect("events")
        .into_iter()
        .filter_map(|event| match event.kind {
            EventKind::AgentUsageUpdated { total, .. } => Some((event.seq, total)),
            _ => None,
        })
        .collect()
}

fn one_reading() -> Usage {
    Usage {
        input_tokens: 3,
        output_tokens: 1,
        ..Default::default()
    }
}

/// Readings update memory, where budgets read them, and reach the store as
/// one commit; a crash loses only what came after the last flush, and nothing
/// is counted twice.
#[tokio::test(start_paused = true)]
async fn streamed_readings_wait_in_memory_and_a_crash_loses_only_the_window() {
    let (runtime, _dir) = test_runtime(|_| {}).await;
    let mut ctx = started_agent_ctx(&runtime).await;
    let agent_id = ctx.agent.id;
    let snapshot = bare_snapshot(agent_id);
    let mut state = StreamState::new(
        TurnPricing::default(),
        configured(&runtime, "claude-code/default"),
    );
    let start = tokio::time::Instant::now();
    let commits = runtime.store.commits();
    for n in 0..10 {
        for event in [
            EngineEvent::UsageDelta {
                usage: one_reading(),
            },
            EngineEvent::ContextUsage { tokens: 500 + n },
        ] {
            handle_event(&runtime, &mut ctx, &snapshot, event, &mut state).expect("reading");
        }
    }
    assert_eq!(
        runtime.store.commits(),
        commits,
        "readings only update memory"
    );
    assert_eq!(ctx.agent.stats.usage.output_tokens, 10, "budgets see them");
    assert_eq!(ctx.agent.stats.context_tokens, Some(509));
    assert_eq!(
        state.accounting.deadline(),
        Some(start + super::super::accounting::ACCOUNTING_FLUSH),
        "the window opens at the first held reading"
    );

    flush_accounting(&runtime, &ctx, &mut state.accounting).expect("flush");
    assert_eq!(runtime.store.commits(), commits + 1, "one commit for all");
    assert_eq!(state.accounting.deadline(), None);
    let flushed = ctx.agent.stats.usage;
    let row = runtime.store.agent(agent_id).unwrap().expect("row");
    assert_eq!(row.stats.usage, flushed);
    assert_eq!(row.stats.context_tokens, Some(509));
    let events = stored_usage_events(&runtime, agent_id);
    assert_eq!(events.len(), 1);
    assert_eq!(events[0].1, flushed, "the absolute figure, not a delta");
    let contexts: Vec<_> = runtime
        .store
        .events(0, usize::MAX, &EventFilter::for_agent(agent_id))
        .unwrap()
        .into_iter()
        .filter_map(|e| match e.kind {
            EventKind::Custom { name, payload } if name == "context.usage" => Some(payload),
            _ => None,
        })
        .collect();
    assert_eq!(contexts, vec![json!({"tokens": 509})]);

    // The process dies with readings still held: they never reach the store.
    for _ in 0..5 {
        handle_event(
            &runtime,
            &mut ctx,
            &snapshot,
            EngineEvent::UsageDelta {
                usage: one_reading(),
            },
            &mut state,
        )
        .expect("reading");
    }
    drop(state);
    let row = runtime.store.agent(agent_id).unwrap().expect("row");
    assert_eq!(
        row.stats.usage, flushed,
        "lost the window, counted nothing twice"
    );
    assert_eq!(stored_usage_events(&runtime, agent_id).len(), 1);
    runtime.shutdown().await;
}

/// A held reading lands before an event that followed it; one held through an
/// assistant message rides in that message's commit, after its invocation.
#[tokio::test]
async fn held_readings_keep_their_place_in_the_log() {
    let (runtime, _dir) = test_runtime(|_| {}).await;
    let mut ctx = started_agent_ctx(&runtime).await;
    let agent_id = ctx.agent.id;
    let snapshot = bare_snapshot(agent_id);
    let mut state = StreamState::new(
        TurnPricing::default(),
        configured(&runtime, "claude-code/default"),
    );
    let reading = EngineEvent::UsageDelta {
        usage: one_reading(),
    };
    handle_event(&runtime, &mut ctx, &snapshot, reading.clone(), &mut state).expect("reading");
    handle_event(
        &runtime,
        &mut ctx,
        &snapshot,
        EngineEvent::ProtocolNote("a counter went backwards".into()),
        &mut state,
    )
    .expect("note");
    handle_event(&runtime, &mut ctx, &snapshot, reading, &mut state).expect("reading");
    let commits = runtime.store.commits();
    handle_event(
        &runtime,
        &mut ctx,
        &snapshot,
        assistant_saying("done"),
        &mut state,
    )
    .expect("assistant");
    assert_eq!(
        runtime.store.commits(),
        commits + 1,
        "folded, not flushed apart"
    );
    assert_eq!(state.accounting.deadline(), None);

    let events = runtime
        .store
        .events(0, usize::MAX, &EventFilter::for_agent(agent_id))
        .unwrap();
    let seq_of = |pred: &dyn Fn(&EventKind) -> bool| {
        events
            .iter()
            .find(|e| pred(&e.kind))
            .map(|e| e.seq)
            .expect("event present")
    };
    let note =
        seq_of(&|k| matches!(k, EventKind::Custom { name, .. } if name == PROTOCOL_NOTE_EVENT));
    let invocation = seq_of(&|k| matches!(k, EventKind::InvocationCompleted { .. }));
    let usage = stored_usage_events(&runtime, agent_id);
    assert_eq!(usage.len(), 2);
    assert!(usage[0].0 < note, "the first reading precedes the note");
    assert!(
        usage[1].0 > invocation,
        "the second rides after the invocation"
    );
    assert_eq!(usage[1].1, ctx.agent.stats.usage);
    assert_eq!(usage[1].1.output_tokens, 2);
    runtime.shutdown().await;
}

#[tokio::test]
async fn a_new_budget_in_the_lineage_makes_usage_immediately_durable() {
    for depth in [0, 2] {
        let (runtime, _dir) = test_runtime(|_| {}).await;
        let mut ctx = started_agent_ctx(&runtime).await;
        let mut root = ctx.agent.clone();
        for _ in 0..depth {
            let mut child = started_agent_ctx(&runtime).await;
            child.agent.parent = Some(ctx.agent.id);
            runtime
                .apply(Commit::new().mutate(Mutation::UpsertAgent(child.agent.clone())))
                .unwrap();
            ctx = child;
        }
        let snapshot = bare_snapshot(ctx.agent.id);
        let mut state = StreamState::new(
            TurnPricing::default(),
            configured(&runtime, "claude-code/default"),
        );
        let reading = EngineEvent::UsageDelta {
            usage: one_reading(),
        };
        handle_event(&runtime, &mut ctx, &snapshot, reading.clone(), &mut state).unwrap();
        assert!(state.accounting.deadline().is_some());
        root.config.token_budget = Some(100);
        runtime
            .apply(Commit::new().mutate(Mutation::UpsertAgent(root)))
            .unwrap();
        handle_event(&runtime, &mut ctx, &snapshot, reading, &mut state).unwrap();
        assert!(state.accounting.deadline().is_none());
        let stored = runtime.store.agent(ctx.agent.id).unwrap().unwrap();
        assert_eq!(stored.stats.usage.output_tokens, 2);
        assert_eq!(stored.stats.usage, ctx.agent.stats.usage);
        runtime.shutdown().await;
    }
}
