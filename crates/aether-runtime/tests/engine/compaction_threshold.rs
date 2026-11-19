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
        let at = agent_on(&h, &model, Some(window));
        let above = agent_on(&h, &model, Some(window + 1));
        engine_turn(&h, own, "own threshold").await;
        engine_turn(&h, sibling, "model threshold").await;
        engine_turn(&h, at, "at the window").await;
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
        for prompt in ["at the window", "beyond the window"] {
            assert_eq!(
                launched_thresholds(&turns, prompt),
                [Some(window)],
                "{model}: {prompt} is capped at the window, not left to the engine"
            );
        }
    }
}

/// Below 100,000 Claude Code and the Anthropic server would raise it, so the
/// agent would not compact where it asked.
#[tokio::test]
async fn a_threshold_below_what_every_path_honours_is_refused_at_creation() {
    let h = harness(Reply::ToolRun).await;
    let create = |tokens| {
        h.runtime.create_agent(
            CreateAgentSpec {
                role: "assistant".into(),
                model: Some(ModelRef::new("claude-code", "opus")),
                config: Some(AgentConfig {
                    compaction_tokens: Some(tokens),
                    ..Default::default()
                }),
                ..Default::default()
            },
            Actor::user(),
        )
    };
    for tokens in [0, 50_000, 99_999] {
        let error = create(tokens).expect_err("below the floor");
        assert!(error.to_string().contains("at least 100000"), "{error}");
    }
    let (agent, _) = create(100_000).expect("the floor itself");
    assert_eq!(agent.config.compaction_tokens, Some(100_000));
}

/// A delegated child runs at its model's threshold, not its parent's.
#[tokio::test]
async fn a_child_does_not_inherit_its_parents_threshold() {
    let h = harness(Reply::ToolRun).await;
    let model = ModelRef::new("claude-code", "opus");
    let parent = agent_on(&h, &model, Some(160_000));
    let (child, _) = h
        .runtime
        .create_agent(
            CreateAgentSpec {
                role: "worker".into(),
                parent: Some(parent),
                config: Some(AgentConfig {
                    effort: Some(Effort::Low),
                    ..Default::default()
                }),
                ..Default::default()
            },
            Actor::user(),
        )
        .expect("child");
    assert_eq!(child.parent, Some(parent));
    assert_eq!(child.model, model, "the same model as its parent");
    assert_eq!(child.config.compaction_tokens, None);
}
