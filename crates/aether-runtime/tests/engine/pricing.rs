//! Engine price profiles and reported-versus-estimated costs.
use super::*;
/// One turn, one assistant message with usage the engine reported, no cost:
/// exactly what Claude Code's stream looks like per inner model call.
pub(super) struct UsageEngine;

#[async_trait::async_trait]
impl Engine for UsageEngine {
    fn kind(&self) -> EngineKind {
        EngineKind::CLAUDE_CODE
    }

    fn adapter(&self) -> String {
        "usage/1".into()
    }

    async fn run(
        &self,
        _turn: EngineTurn,
        _cancel: CancellationToken,
    ) -> Result<EngineRun, EngineError> {
        let (events, rx) = mpsc::channel(8);
        tokio::spawn(async move {
            let _ = events
                .send(EngineEvent::Assistant {
                    content: vec![ContentBlock::text("done")],
                    usage: Some(aether_core::model::Usage {
                        input_tokens: 125_000,
                        output_tokens: 25_000,
                        ..Default::default()
                    }),
                    model: None,
                })
                .await;
            let _ = events
                .send(EngineEvent::Done(EngineOutcome {
                    closing_accounting: true,
                    text: Some("done".into()),
                    ..Default::default()
                }))
                .await;
        });
        Ok(EngineRun {
            events: rx,
            input: None,
            decisions: None,
            stop: None,
        })
    }
}

/// A stream that reports whatever the test asks it to: per-message usage (or
/// none at all), plus its own turn total. Real engines differ on both.
struct AccountingEngine {
    /// Usage on the assistant message, as Claude Code reports per inner call.
    per_message: Option<aether_core::model::Usage>,
    /// The engine's own figure for the whole turn.
    turn_cost: Option<f64>,
    turn_usage: aether_core::model::Usage,
}

#[async_trait::async_trait]
impl Engine for AccountingEngine {
    fn kind(&self) -> EngineKind {
        EngineKind::CLAUDE_CODE
    }

    fn adapter(&self) -> String {
        "accounting/1".into()
    }

    async fn run(
        &self,
        _turn: EngineTurn,
        _cancel: CancellationToken,
    ) -> Result<EngineRun, EngineError> {
        let (events, rx) = mpsc::channel(8);
        let per_message = self.per_message;
        let turn_cost = self.turn_cost;
        let turn_usage = self.turn_usage;
        tokio::spawn(async move {
            let _ = events
                .send(EngineEvent::Assistant {
                    content: vec![ContentBlock::text("done")],
                    usage: per_message,
                    model: None,
                })
                .await;
            let _ = events
                .send(EngineEvent::Done(EngineOutcome {
                    closing_accounting: true,
                    text: Some("done".into()),
                    cost_usd: turn_cost,
                    usage: turn_usage,
                    ..Default::default()
                }))
                .await;
        });
        Ok(EngineRun {
            events: rx,
            input: None,
            decisions: None,
            stop: None,
        })
    }
}

/// A subscription turn still costs money. The engine path prices its
/// invocations from the registry exactly as the API path does, so a report, a
/// replay or a budget on an engine agent has a number to bind to.
#[tokio::test]
async fn an_engine_invocation_carries_a_cost_from_the_price_profile() {
    let h = harness_with_engine(
        Arc::new(UsageEngine),
        Arc::new(Mutex::new(Vec::new())),
        Arc::new(Mutex::new(Vec::new())),
    )
    .await;
    let (agent, _) = h
        .runtime
        .create_agent(
            CreateAgentSpec {
                role: "assistant".into(),
                start: true,
                objective: Some("price the engine turn".into()),
                model: Some(ModelRef::new("claude-code", "sonnet")),
                config: Some(AgentConfig {
                    approval: ApprovalPolicy::none(),
                    ..Default::default()
                }),
                ..Default::default()
            },
            Actor::user(),
        )
        .expect("create");
    wait_for(h.store.as_ref(), agent.id, |a| a.stats.turns >= 1).await;

    let priced: Vec<_> = h
        .store
        .invocations_for_agent(agent.id)
        .expect("invocations")
        .into_iter()
        .filter(|i| i.usage.total_tokens() > 0)
        .collect();
    let invocation = priced
        .first()
        .expect("the assistant message became an invocation");
    // claude-code/sonnet is priced like anthropic/claude-sonnet-5-5: $2/$10 per
    // million. Keep the fixture input below compaction pressure: these tests
    // isolate ordinary invocation pricing; compaction billing is tested separately.
    assert_eq!(invocation.usage.cost_usd, 0.5);
    assert_eq!(
        invocation.usage.cost_basis,
        aether_core::model::CostBasis::ListPrice
    );
    assert!(invocation.usage.cost_complete);

    let events = h
        .store
        .events(0, 1000, &EventFilter::all())
        .expect("events");
    let reported = events
        .iter()
        .find_map(|e| match &e.kind {
            EventKind::InvocationCompleted { usage, .. } if usage.total_tokens() > 0 => {
                Some(usage.cost_usd)
            }
            _ => None,
        })
        .expect("invocation.completed");
    assert_eq!(
        reported, 0.5,
        "the event a replay reads must carry the cost too"
    );

    // The engine never closes its task, so the agent may take several turns;
    // what matters is that each priced invocation is counted exactly once and
    // the engine's own turn total is not added on top.
    let agent = h.store.agent(agent.id).expect("agent").expect("exists");
    assert_eq!(
        agent.stats.usage.cost_usd,
        0.5 * priced.len() as f64,
        "counted once per invocation, {} of them",
        priced.len()
    );
    assert!(agent.stats.usage.cost_complete);
    assert_eq!(
        agent.stats.usage.cost_basis,
        aether_core::model::CostBasis::ListPrice
    );
}

/// Create a started engine agent on `claude-code/sonnet` and wait for a turn.
async fn priced_engine_agent(h: &Harness) -> AgentId {
    let (agent, _) = h
        .runtime
        .create_agent(
            CreateAgentSpec {
                role: "assistant".into(),
                start: true,
                objective: Some("account for the engine turn".into()),
                model: Some(ModelRef::new("claude-code", "sonnet")),
                config: Some(AgentConfig {
                    approval: ApprovalPolicy::none(),
                    ..Default::default()
                }),
                ..Default::default()
            },
            Actor::user(),
        )
        .expect("create");
    wait_for(h.store.as_ref(), agent.id, |a| a.stats.turns >= 1).await;
    agent.id
}

/// An engine that reports no usage per message is not reporting free work.
/// Nothing is priced, so the engine's own turn total is what the agent is
/// charged - never a silent zero from pricing an empty usage.
#[tokio::test]
async fn an_unpriced_engine_turn_falls_back_to_the_engine_s_own_total() {
    let h = harness_with_engine(
        Arc::new(AccountingEngine {
            per_message: None,
            turn_cost: Some(0.25),
            turn_usage: aether_core::model::Usage {
                input_tokens: 700,
                output_tokens: 300,
                ..Default::default()
            },
        }),
        Arc::new(Mutex::new(Vec::new())),
        Arc::new(Mutex::new(Vec::new())),
    )
    .await;
    let id = priced_engine_agent(&h).await;

    for invocation in h.store.invocations_for_agent(id).expect("invocations") {
        assert!(
            !invocation.usage.cost_complete,
            "a message with no usage must not claim it was free"
        );
    }
    let agent = h.store.agent(id).expect("agent").expect("exists");
    let turns = agent.stats.turns as f64;
    assert_eq!(
        agent.stats.usage.cost_usd,
        0.25 * turns,
        "the engine's own total is charged once per turn"
    );
    assert_eq!(
        agent.stats.usage.cost_basis,
        aether_core::model::CostBasis::ProviderReported
    );
    assert!(agent.stats.usage.cost_complete);
    assert_eq!(
        agent.stats.usage.total_tokens(),
        1000 * agent.stats.turns as u64,
        "and its tokens with it, every turn, not only the first"
    );
}

/// When the stream did price invocations, the engine's own total is a second
/// account of the same money and must not be added on top.
#[tokio::test]
async fn a_priced_engine_turn_ignores_the_engine_s_own_total() {
    let h = harness_with_engine(
        Arc::new(AccountingEngine {
            per_message: Some(aether_core::model::Usage {
                input_tokens: 125_000,
                output_tokens: 25_000,
                ..Default::default()
            }),
            // Wildly different on purpose: if it were added, the assertion
            // below could not pass by accident.
            turn_cost: Some(99.0),
            turn_usage: aether_core::model::Usage {
                input_tokens: 5_000,
                output_tokens: 5_000,
                ..Default::default()
            },
        }),
        Arc::new(Mutex::new(Vec::new())),
        Arc::new(Mutex::new(Vec::new())),
    )
    .await;
    let id = priced_engine_agent(&h).await;

    let agent = h.store.agent(id).expect("agent").expect("exists");
    let turns = agent.stats.turns as f64;
    // claude-code/sonnet is priced $2/$10 per million; 125k input and 25k output.
    assert_eq!(
        agent.stats.usage.cost_usd,
        0.5 * turns,
        "priced invocations only; the engine's own total is the fallback"
    );
    assert!(agent.stats.usage.cost_complete);
    assert_eq!(
        agent.stats.usage.total_tokens(),
        150_000 * agent.stats.turns as u64,
        "per-message usage counted once, turn totals not added on top"
    );
}

/// An engine that priced one turn and not the next leaves the agent's total a
/// floor. Summing the priced turns alone and storing that as a confident
/// figure is what every surface above then renders as the whole.
#[tokio::test]
async fn a_turn_the_engine_did_not_price_makes_the_agent_total_a_floor() {
    let h = harness_with_engine_config(
        Arc::new(ScriptedEngine {
            reply: Reply::OneTurn,
            prompts: Arc::default(),
            systems: Arc::default(),
            costs: Arc::new(Mutex::new(vec![Some(0.25), None].into())),
            tool_gate: None,
        }),
        |config| config.default_model = Some(ModelRef::new("claude-code", "default")),
    )
    .await;
    let id = chat_agent(&h);
    wait_for(h.store.as_ref(), id, |a| {
        a.status.kind() == AgentStatusKind::Waiting
    })
    .await;
    h.runtime
        .send_message(id, vec![ContentBlock::text("hello")], Actor::user())
        .expect("send");
    let priced = wait_for(h.store.as_ref(), id, |a| a.stats.turns >= 1).await;
    assert_eq!(priced.stats.usage.cost_usd, 0.25);
    assert!(priced.stats.usage.cost_complete, "that turn had a price");

    h.runtime
        .send_message(id, vec![ContentBlock::text("again")], Actor::user())
        .expect("send");
    let mixed = wait_for(h.store.as_ref(), id, |a| a.stats.turns >= 2).await;
    assert_eq!(
        mixed.stats.usage.cost_usd, 0.25,
        "the priced turn still counts"
    );
    assert!(
        !mixed.stats.usage.cost_complete,
        "and the total is only a floor"
    );
    assert_eq!(mixed.stats.usage.cost_line(), "≥ $0.2500 (some unpriced)");
}
