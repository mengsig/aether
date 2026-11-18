//! An agent's own `compaction_tokens` reaches its engine launch; a sibling on
//! the same model keeps the model's threshold.
use super::*;

fn agent_on(h: &Harness, model: &ModelRef, compaction_tokens: Option<u32>) -> AgentId {
    let (agent, _) = h
        .runtime
        .create_agent(
            CreateAgentSpec {
                role: "assistant".into(),
                start: true,
                model: Some(model.clone()),
                config: Some(AgentConfig {
                    long_lived: true,
                    approval: ApprovalPolicy::none(),
                    compaction_tokens,
                    ..Default::default()
                }),
                ..Default::default()
            },
            Actor::user(),
        )
        .expect("create");
    assert_eq!(agent.config.compaction_tokens, compaction_tokens);
    agent.id
}

/// The `auto_compact_tokens` of every launch an agent's prompt reached.
fn launched_thresholds(turns: &Mutex<Vec<EngineTurn>>, prompt: &str) -> Vec<Option<u32>> {
    turns
        .lock()
        .unwrap()
        .iter()
        .filter(|turn| turn.prompt.contains(prompt))
        .map(|turn| turn.auto_compact_tokens)
        .collect()
}

#[tokio::test]
async fn an_agents_own_threshold_reaches_its_launch_and_not_its_siblings() {
    for model in [
        ModelRef::new("claude-code", "opus"),
        ModelRef::new("codex", "gpt-6-luna"),
    ] {
        let turns = Arc::new(Mutex::new(Vec::new()));
        let engine = Arc::new(CheckpointEngine {
            turns: turns.clone(),
            valid: true,
        });
        let h = harness_with_engine(engine, Arc::default(), Arc::default()).await;
        let capabilities = h
            .runtime
            .models()
            .resolve(&model)
            .expect("model")
            .model
            .capabilities;
        let window = capabilities.context_window;
        assert!(window > 160_000, "{model}: 160,000 sits inside the window");
        // The model's own launch value: below the window, or left to the engine.
        let default = capabilities.compaction_tokens.filter(|t| *t < window);
        if model.provider == "claude-code" {
            assert_eq!(default, Some(256_000), "the Claude default");
        }

        let own = agent_on(&h, &model, Some(160_000));
        let sibling = agent_on(&h, &model, None);
        let above = agent_on(&h, &model, Some(window + 1));
        engine_turn(&h, own, "own threshold").await;
        engine_turn(&h, sibling, "model threshold").await;
        engine_turn(&h, above, "beyond the window").await;

        assert_eq!(
            launched_thresholds(&turns, "own threshold"),
            [Some(160_000)],
            "{model}"
        );
        assert_eq!(
            launched_thresholds(&turns, "model threshold"),
            [default],
            "{model}"
        );
        assert_eq!(
            launched_thresholds(&turns, "beyond the window"),
            [None],
            "{model}: bounded by the window, where the engine folds anyway"
        );
    }
}

#[tokio::test]
async fn a_zero_threshold_is_refused_at_creation() {
    let h = harness(Reply::ToolRun).await;
    let error = h
        .runtime
        .create_agent(
            CreateAgentSpec {
                role: "assistant".into(),
                config: Some(AgentConfig {
                    compaction_tokens: Some(0),
                    ..Default::default()
                }),
                ..Default::default()
            },
            Actor::user(),
        )
        .expect_err("zero never compacts sensibly");
    assert!(error.to_string().contains("compaction_tokens"), "{error}");
}
