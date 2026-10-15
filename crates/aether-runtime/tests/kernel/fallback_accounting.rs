//! fallback accounting scenarios and fixtures.
use super::*;

/// A model that answers part-way and then fails, so the turn has real burned
/// tokens to account for. `ScriptedClient` fails before any event, which is
/// exactly the case that cannot show whether the partial usage was priced.
#[derive(Debug)]
struct BurnsTokensThenFails;

#[async_trait::async_trait]
impl aether_model::client::ModelClient for BurnsTokensThenFails {
    fn adapter(&self) -> &str {
        "burns-tokens-then-fails/1"
    }

    async fn stream(
        &self,
        _request: aether_core::model::ModelRequest,
        _cancel: tokio_util::sync::CancellationToken,
    ) -> Result<aether_model::client::ModelStream, aether_core::model::ModelError> {
        use aether_core::model::{BlockStart, StreamEvent, Usage};
        let events: Vec<Result<StreamEvent, aether_core::model::ModelError>> = vec![
            Ok(StreamEvent::MessageStart {
                provider_message_id: Some("partial".into()),
                model: Some("partial".into()),
            }),
            Ok(StreamEvent::BlockStart {
                index: 0,
                block: BlockStart::Text {
                    text: String::new(),
                },
            }),
            Ok(StreamEvent::TextDelta {
                index: 0,
                text: "thinking about it".into(),
            }),
            Ok(StreamEvent::Usage {
                usage: Usage {
                    input_tokens: 1_000,
                    output_tokens: 500,
                    ..Default::default()
                },
            }),
            Err(aether_core::model::ModelError::new(
                ModelErrorClass::Server,
                "the provider gave up mid-answer",
            )),
        ];
        Ok(Box::pin(futures::stream::iter(events)))
    }
}

/// A failed invocation's tokens were recorded with no price, so a budget
/// undercounted exactly the spend a fallback chain adds.
#[tokio::test]
async fn a_failed_invocation_prices_the_tokens_it_burned() {
    let workspace = tempfile::tempdir().expect("tempdir");
    let config_dir = tempfile::tempdir().expect("config dir");
    let store: Arc<dyn Store> = Arc::new(SqliteStore::open_in_memory().expect("store"));
    let models = Arc::new(Models::load(Some(config_dir.path().to_path_buf())).expect("models"));
    let runtime = Runtime::start_with(
        RuntimeConfig {
            holder: "test".into(),
            max_concurrent_turns: 1,
            lease_ttl: Duration::from_secs(30),
            // A model with a price profile, so there is a cost to compute.
            default_model: Some(ModelRef::new("anthropic", "claude-sonnet-5-5")),
            // One attempt is all this needs; retries only add backoff waits.
            max_consecutive_failures: 1,
            workspace_root: Some(workspace.path().canonicalize().expect("canonical")),
            ..Default::default()
        },
        store.clone(),
        models,
        Arc::new(ToolRegistry::with_builtins()),
        Some(Arc::new(BurnsTokensThenFails)),
    )
    .await
    .expect("runtime");
    let (agent, _) = runtime
        .create_agent(spec("burn some tokens and fail"), Actor::user())
        .expect("create");
    wait_until_with_budget(
        store.as_ref(),
        agent.id,
        |s| s.is_terminal(),
        WAIT_UNTIL_FAST_HARD_CEILING,
        WAIT_UNTIL_STALL_WINDOW,
    )
    .await;

    let failed = store
        .invocations_for_agent(agent.id)
        .expect("invocations")
        .into_iter()
        .find(|i| i.error.is_some())
        .expect("a failed invocation");
    assert!(
        failed.usage.output_tokens > 0,
        "the test proves nothing without burned tokens: {:?}",
        failed.usage
    );
    assert!(
        !failed.usage.cost_complete,
        "an unfinished stream has only a cost floor"
    );
    assert_eq!(
        failed.usage.cost_gap,
        Some(aether_core::model::CostGap::Interrupted)
    );
    let cost = failed.usage.cost_usd;
    assert!(cost > 0.0, "burned tokens priced at {cost}");
}

/// The cap has to hold on a chain's LAST link too. Two agents chained
/// anthropic,groq are both admitted on the uncapped anthropic, both hit a 429
/// there, and both walk to groq - which is capped at one. Whichever loses the
/// race must hand its turn back unspent, not carry groq past its limit.
#[tokio::test]
async fn a_capped_last_link_makes_the_second_turn_wait() {
    let h = harness_full(
        vec![],
        |_| {},
        |c| c.max_concurrent_turns = 4,
        Some("[providers.groq]\nmax_concurrent = 1\n"),
        None,
        None,
        |_| {},
    )
    .await;
    // Keep the primary unavailable for the whole test. If its backoff expires
    // while the second agent waits for the capped fallback, the fixture
    // can exhaust its two errors and return its default primary success.
    for _ in 0..2 {
        h.client.push_for(
            "anthropic",
            Err(
                aether_core::model::ModelError::new(ModelErrorClass::RateLimit, "slow down")
                    .with_retry_after(Duration::from_secs(60)),
            ),
        );
        h.client
            .push_for("groq", slow_turn("answered on the fallback"));
    }
    h.client.set_delay(Duration::from_millis(8));
    let chained = |objective: &str| CreateAgentSpec {
        model: Some(ModelRef::new("anthropic", "claude-sonnet-5-5")),
        fallbacks: vec![ModelRef::new("groq", "openai/gpt-oss-120b")],
        ..spec(objective)
    };
    let (a, _) = h
        .runtime
        .create_agent(chained("part one"), Actor::user())
        .expect("a");
    let (b, _) = h
        .runtime
        .create_agent(chained("part two"), Actor::user())
        .expect("b");
    h.wait_until(a.id, |s| s.is_terminal()).await;
    h.wait_until(b.id, |s| s.is_terminal()).await;

    assert_eq!(
        h.client.peak_in_flight("groq"),
        1,
        "the chain's last link carried more turns than its cap allows"
    );
    let sent: Vec<ModelRef> = h
        .client
        .requests()
        .iter()
        .map(|r| r.model.clone())
        .collect();
    assert_eq!(
        sent.iter().filter(|m| m.provider == "groq").count(),
        2,
        "both agents must still be answered on the fallback: {sent:?}"
    );
}

/// §4.5: a chain must not make an agent more fragile. A three-link chain that
/// fails everywhere spends one turn per walk, so the agent survives exactly as
/// many turns as it would on a single link.
#[tokio::test]
async fn a_chain_failing_everywhere_costs_one_turn_not_three() {
    let h = harness_full(
        vec![],
        |_| {},
        |c| c.max_consecutive_failures = 3,
        None,
        None,
        None,
        |_| {},
    )
    .await;
    let rate_limited = || {
        Err(
            aether_core::model::ModelError::new(ModelErrorClass::RateLimit, "slow down")
                .with_retry_after(Duration::from_millis(10)),
        )
    };
    // Three turns' worth on every link, and one to spare: the agent must fail
    // on the third turn, not before, and not run a fourth.
    for provider in ["anthropic", "groq", "xai"] {
        for _ in 0..4 {
            h.client.push_for(provider, rate_limited());
        }
    }
    let (agent, _) = h
        .runtime
        .create_agent(
            CreateAgentSpec {
                model: Some(ModelRef::new("anthropic", "claude-sonnet-5-5")),
                fallbacks: vec![
                    ModelRef::new("groq", "openai/gpt-oss-120b"),
                    ModelRef::new("xai", "grok-4"),
                ],
                ..spec("keep trying")
            },
            Actor::user(),
        )
        .expect("create");
    let status = h.wait_until(agent.id, |s| s.is_terminal()).await;
    assert!(
        matches!(status, AgentStatus::Failed { .. }),
        "the agent should end failed, not {status:?}"
    );

    let turns: std::collections::BTreeSet<u32> = h
        .store
        .invocations_for_agent(agent.id)
        .expect("invocations")
        .iter()
        .map(|i| i.turn)
        .collect();
    assert_eq!(
        turns.len(),
        3,
        "three failed turns is what one link would have bought: {turns:?}"
    );
    let failure = h
        .store
        .agent(agent.id)
        .expect("agent")
        .expect("exists")
        .status;
    let AgentStatus::Failed { error, .. } = &failure else {
        panic!("expected a failure, got {failure:?}");
    };
    assert!(
        error.contains("every link of the model chain failed") && error.contains("3 turns"),
        "the failure must name the chain, not one link: {error}"
    );
}

#[tokio::test]
async fn prestream_error_status_keeps_complete_zero() {
    let error =
        aether_core::model::ModelError::new(ModelErrorClass::InvalidRequest, "fixture rejection");
    let h = harness(vec![Err(error)]).await;
    let (agent, _) = h
        .runtime
        .create_agent(spec("rejected request"), Actor::user())
        .unwrap();
    h.wait_until(agent.id, |status| status.is_terminal()).await;
    let invocations = h.store.invocations_for_agent(agent.id).unwrap();
    assert_eq!(invocations.len(), 1);
    assert_eq!(invocations[0].usage, aether_core::model::Usage::default());
    h.runtime.shutdown().await;
}
