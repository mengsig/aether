//! Session model resolution and streamed usage accounting.
use super::*;
struct DeltaUsageEngine {
    report_outcome: bool,
}

#[async_trait::async_trait]
impl Engine for DeltaUsageEngine {
    fn kind(&self) -> EngineKind {
        EngineKind::CLAUDE_CODE
    }
    fn adapter(&self) -> String {
        "usage-delta/1".into()
    }
    async fn run(
        &self,
        _turn: EngineTurn,
        _cancel: CancellationToken,
    ) -> Result<EngineRun, EngineError> {
        use aether_core::model::Usage;
        let (tx, rx) = mpsc::channel(8);
        for event in [
            EngineEvent::ContextUsage { tokens: 1002 },
            EngineEvent::UsageDelta {
                usage: Usage {
                    input_tokens: 2,
                    cache_read_tokens: 1000,
                    output_tokens: 1,
                    ..Default::default()
                },
            },
            EngineEvent::Assistant {
                content: vec![ContentBlock::text("done")],
                usage: None,
                model: None,
            },
            EngineEvent::UsageDelta {
                usage: Usage {
                    output_tokens: 99,
                    reasoning_tokens: Some(50),
                    ..Default::default()
                },
            },
            EngineEvent::Done(if self.report_outcome {
                EngineOutcome {
                    closing_accounting: true,
                    text: Some("done".into()),
                    cost_usd: Some(0.123),
                    usage: Usage {
                        input_tokens: 2,
                        cache_read_tokens: 1000,
                        output_tokens: 105,
                        ..Default::default()
                    },
                    ..Default::default()
                }
            } else {
                EngineOutcome {
                    closing_accounting: true,
                    is_error: true,
                    error: Some("stream interrupted".into()),
                    ..Default::default()
                }
            }),
        ] {
            tx.send(event).await.expect("fixture channel capacity");
        }
        Ok(EngineRun {
            events: rx,
            input: None,
            decisions: None,
            stop: None,
        })
    }
}

#[tokio::test]
async fn usage_deltas_preserve_context_and_do_not_double_count_turn_totals() {
    let h = harness_with_engine(
        Arc::new(DeltaUsageEngine {
            report_outcome: true,
        }),
        Arc::new(Mutex::new(Vec::new())),
        Arc::new(Mutex::new(Vec::new())),
    )
    .await;
    let id = chat_agent(&h);
    h.runtime
        .send_message(id, vec![ContentBlock::text("Report usage")], Actor::user())
        .expect("send");
    let agent = wait_for(h.store.as_ref(), id, |a| {
        a.stats.turns == 1 && a.status.kind() == AgentStatusKind::Waiting
    })
    .await;
    assert_eq!(agent.stats.context_tokens, Some(1002));
    assert_eq!(agent.stats.usage.input_tokens, 2);
    assert_eq!(agent.stats.usage.cache_read_tokens, 1000);
    assert_eq!(agent.stats.usage.output_tokens, 105);
    assert_eq!(agent.stats.usage.cost_usd, 0.123);
    assert_eq!(
        agent.stats.usage.cost_basis,
        aether_core::model::CostBasis::ProviderReported
    );
    assert!(agent.stats.usage.cost_complete);
    assert_eq!(agent.stats.usage.reasoning_tokens, Some(50));
    let events = h
        .store
        .events(0, 1000, &EventFilter::for_agent(id))
        .unwrap();
    let totals: Vec<_> = events
        .iter()
        .filter_map(|event| match &event.kind {
            EventKind::AgentUsageUpdated { total, .. } => Some(*total),
            _ => None,
        })
        .collect();
    let recorded = *totals.last().unwrap();
    assert_eq!(recorded, agent.stats.usage);
    assert_eq!(
        events
            .iter()
            .filter(|event| matches!(event.kind, EventKind::InvocationCompleted { .. }))
            .count(),
        1,
        "usage deltas must not create synthetic model calls"
    );
    assert_eq!(
        spoken(h.store.as_ref(), id)
            .iter()
            .filter(|(role, text)| *role == MessageRole::Assistant && text == "done")
            .count(),
        1
    );
    h.runtime
        .send_message(id, vec![ContentBlock::text("Again")], Actor::user())
        .unwrap();
    let resumed = wait_for(h.store.as_ref(), id, |a| {
        a.stats.turns == 2 && a.status.kind() == AgentStatusKind::Waiting
    })
    .await;
    assert_eq!(resumed.stats.usage.output_tokens, 210);
    assert_eq!(resumed.stats.usage.cost_usd, 0.246);
    assert_eq!(resumed.stats.usage.reasoning_tokens, Some(100));
    h.runtime.shutdown().await;
}

#[tokio::test]
async fn closing_receipt_without_reasoning_detail_leaves_cost_unaffected() {
    let h = harness_with_engine(
        Arc::new(DeltaUsageEngine {
            report_outcome: true,
        }),
        Arc::new(Mutex::new(Vec::new())),
        Arc::new(Mutex::new(Vec::new())),
    )
    .await;
    let id = chat_agent(&h);
    h.runtime
        .send_message(id, vec![ContentBlock::text("Report usage")], Actor::user())
        .unwrap();
    let agent = wait_for(h.store.as_ref(), id, |a| {
        a.stats.turns == 1 && a.status.kind() == AgentStatusKind::Waiting
    })
    .await;
    assert_eq!(agent.stats.usage.estimated_cost_usd, None);
    let notes: Vec<_> = h
        .store
        .events(0, 1000, &EventFilter::for_agent(id))
        .unwrap()
        .into_iter()
        .filter_map(|event| match event.kind {
            EventKind::Custom { name, payload } if name == "engine.protocol_note" => {
                Some(payload["note"].as_str().unwrap().to_owned())
            }
            _ => None,
        })
        .collect();
    assert_eq!(notes.len(), 1);
    assert_eq!(
        notes[0],
        "the engine's closing receipt omitted streamed usage; only reasoning detail was missing, the cost is unaffected"
    );
    h.runtime.shutdown().await;
}

#[tokio::test]
async fn interrupted_usage_stream_keeps_consumed_tokens_without_a_final_total() {
    let h = harness_with_engine(
        Arc::new(DeltaUsageEngine {
            report_outcome: false,
        }),
        Arc::new(Mutex::new(Vec::new())),
        Arc::new(Mutex::new(Vec::new())),
    )
    .await;
    let id = chat_agent(&h);
    h.runtime
        .send_message(id, vec![ContentBlock::text("Report")], Actor::user())
        .unwrap();
    let agent = wait_for(h.store.as_ref(), id, |a| {
        a.status.kind() == AgentStatusKind::Failed
    })
    .await;
    assert_eq!(agent.stats.usage.output_tokens, 100);
    assert_eq!(agent.stats.usage.cache_read_tokens, 1000);
    assert_eq!(agent.stats.usage.reasoning_tokens, Some(50));
    assert_eq!(agent.stats.context_tokens, Some(1002));
    h.runtime.shutdown().await;
}

/// A CLI on a `default` model: it opens a session, says which model it really
/// picked, streams that turn's tokens and reports no price of its own.
struct SessionModelEngine {
    kind: EngineKind,
    /// What the session announces, as the CLI spells it.
    model: &'static str,
    usage: aether_core::model::Usage,
}

#[async_trait::async_trait]
impl Engine for SessionModelEngine {
    fn kind(&self) -> EngineKind {
        self.kind
    }

    fn adapter(&self) -> String {
        "session-model/1".into()
    }

    async fn run(
        &self,
        _turn: EngineTurn,
        _cancel: CancellationToken,
    ) -> Result<EngineRun, EngineError> {
        let (events, rx) = mpsc::channel(8);
        let model = self.model;
        let usage = self.usage;
        tokio::spawn(async move {
            for event in [
                EngineEvent::Session {
                    id: "s-236".into(),
                    model: Some(model.into()),
                    tools: vec![],
                },
                EngineEvent::UsageDelta { usage },
                EngineEvent::Assistant {
                    content: vec![ContentBlock::text("done")],
                    usage: None,
                    model: Some(model.into()),
                },
                // The engine's own total for the turn, with no price on it:
                // whatever this turn costs has to come from the registry.
                EngineEvent::Done(EngineOutcome {
                    closing_accounting: true,
                    text: Some("done".into()),
                    cost_usd: None,
                    usage,
                    ..Default::default()
                }),
            ] {
                let _ = events.send(event).await;
            }
        });
        Ok(EngineRun {
            events: rx,
            input: None,
            decisions: None,
            stop: None,
        })
    }
}

/// `codex/default` and `claude-code/default` are the standard
/// configuration, and neither carries a price - the CLI picks the model. The
/// turn's reconciled total must be priced from the model the session named,
/// at the list price of the API model behind it.
#[tokio::test]
async fn a_default_engine_turn_is_priced_by_the_model_its_session_reported() {
    let usage = aether_core::model::Usage {
        input_tokens: 30_000,
        output_tokens: 9_000,
        cache_read_tokens: 500_000,
        ..Default::default()
    };
    // The catalog the harness itself loads: an empty config dir, so the
    // builtin price tables and nothing of the host's.
    let registry = aether_model::Registry::builtin();
    let price_of = |model: &str| {
        registry
            .resolve(&ModelRef::parse(model).expect("model reference"))
            .expect("a builtin model")
            .model
            .capabilities
            .cost
            .expect("a priced builtin model")
            .cost_of(&usage)
    };

    // Codex is a subscription: its session model is estimated at
    // `openai/gpt-6-astra`'s list price, never booked as billed dollars.
    let h = harness_with_engine(
        Arc::new(SessionModelEngine {
            kind: EngineKind::CODEX,
            model: "gpt-6-astra",
            usage,
        }),
        Arc::new(Mutex::new(Vec::new())),
        Arc::new(Mutex::new(Vec::new())),
    )
    .await;
    let total = one_turn_on(&h, ModelRef::new("codex", "default")).await;
    assert_eq!(
        total.estimated_cost_usd,
        Some(price_of("openai/gpt-6-astra")),
        "a `codex/default` turn used to carry no dollars at all"
    );
    assert_eq!(total.cost_usd, 0.0, "nobody billed these dollars");
    assert!(!total.cost_complete, "an estimate is not a billed cost");
    assert_eq!(total.cost_gap, Some(aether_core::model::CostGap::Unpriced));
    assert_eq!(total.cost_basis, aether_core::model::CostBasis::ListPrice);
    h.runtime.shutdown().await;

    // Claude Code bills the subscription at the list price the registry holds,
    // so the same report produces a cost rather than an estimate.
    let h = harness_with_engine(
        Arc::new(SessionModelEngine {
            kind: EngineKind::CLAUDE_CODE,
            model: "claude-fable-5-1",
            usage,
        }),
        Arc::new(Mutex::new(Vec::new())),
        Arc::new(Mutex::new(Vec::new())),
    )
    .await;
    let total = one_turn_on(&h, ModelRef::new("claude-code", "default")).await;
    assert_eq!(total.cost_usd, price_of("claude-code/fable"));
    assert!(total.cost_complete, "billed dollars, not a floor");
    assert_eq!(
        total.estimated_cost_usd, None,
        "a priced turn needs no estimate"
    );
    h.runtime.shutdown().await;
}

/// A Codex turn is estimated at what the OpenAI API would bill for the same
/// tokens. Reasoning is already inside `output_tokens`, as OpenAI and Codex
/// count it, so it counts at the output rate exactly once.
#[tokio::test]
async fn a_codex_turn_is_billed_at_the_api_list_price_with_reasoning_as_output() {
    let usage = aether_core::model::Usage {
        input_tokens: 100_000,
        cache_read_tokens: 400_000,
        output_tokens: 50_000,
        reasoning_tokens: Some(30_000),
        ..Default::default()
    };
    let h = harness_with_engine(
        Arc::new(SessionModelEngine {
            kind: EngineKind::CODEX,
            model: "gpt-6.1-sol",
            usage,
        }),
        Arc::new(Mutex::new(Vec::new())),
        Arc::new(Mutex::new(Vec::new())),
    )
    .await;
    let total = one_turn_on(&h, ModelRef::new("codex", "sol")).await;
    // GPT-6.1 Sol: $2 input, $0.10 cached input, $10 output per MTok.
    let expected = 0.1 * 2.0 + 0.4 * 0.10 + 0.05 * 10.0;
    let estimate = total.estimated_cost_usd.expect("a Codex turn is estimated");
    assert!(
        (estimate - expected).abs() < 1e-9,
        "{estimate} != {expected}"
    );
    assert_eq!(total.cost_usd, 0.0, "nobody billed these dollars");
    assert!(!total.cost_complete, "an estimate is not a billed cost");
    assert_eq!(total.api_equivalent_usd(), Some(estimate));
    assert_eq!(total.reasoning_tokens, Some(30_000));
    h.runtime.shutdown().await;
}

/// One turn of a long-lived agent on `model`, and the usage it ended with.
async fn one_turn_on(h: &Harness, model: ModelRef) -> aether_core::model::Usage {
    let (agent, _) = h
        .runtime
        .create_agent(
            CreateAgentSpec {
                role: "assistant".into(),
                start: true,
                model: Some(model),
                config: Some(AgentConfig {
                    long_lived: true,
                    approval: ApprovalPolicy::none(),
                    ..Default::default()
                }),
                ..Default::default()
            },
            Actor::user(),
        )
        .expect("create");
    h.runtime
        .send_message(agent.id, vec![ContentBlock::text("hello")], Actor::user())
        .expect("send");
    wait_for(h.store.as_ref(), agent.id, |a| {
        a.stats.turns == 1 && a.status.kind() == AgentStatusKind::Waiting
    })
    .await
    .stats
    .usage
}
