//! Explicit reasoning effort across launches and delegation.
use super::*;
/// Snapshots record an explicit override or None for native effort selection.
#[tokio::test]
async fn a_codex_turns_snapshot_records_only_explicit_effort() {
    for (configured, expected) in [
        (None, None),
        (Some(Effort::Low), Some(Effort::Low)),
        (Some(Effort::Max), Some(Effort::Max)),
    ] {
        let h = harness(Reply::ToolRun).await;
        let (agent, _) = h
            .runtime
            .create_agent(
                CreateAgentSpec {
                    role: "assistant".into(),
                    start: true,
                    model: Some(ModelRef::new("codex", "gpt-6-luna")),
                    config: Some(AgentConfig {
                        long_lived: true,
                        approval: ApprovalPolicy::none(),
                        effort: configured,
                        ..Default::default()
                    }),
                    ..Default::default()
                },
                Actor::user(),
            )
            .expect("create");
        let id = agent.id;
        wait_for(h.store.as_ref(), id, |a| {
            a.status.kind() == AgentStatusKind::Waiting
        })
        .await;
        h.runtime
            .send_message(id, vec![ContentBlock::text("what is here?")], Actor::user())
            .expect("send");
        wait_for(h.store.as_ref(), id, |a| {
            a.status.kind() == AgentStatusKind::Waiting && a.stats.turns >= 1
        })
        .await;

        let snapshot_id = h
            .store
            .events(0, 10_000, &EventFilter::for_agent(id))
            .expect("events")
            .into_iter()
            .find_map(|e| match e.kind {
                EventKind::SnapshotCreated { snapshot, .. } => Some(snapshot),
                _ => None,
            })
            .expect("the engine turn recorded a snapshot");
        let stored = h
            .store
            .snapshot(snapshot_id)
            .expect("snapshot")
            .expect("every announced snapshot is stored");
        assert_eq!(stored.params.effort, expected, "configured={configured:?}");
    }
}

/// `CapabilityProfile.effort` gates other engine features,
/// not a configured effort. A `providers.toml`-registered model that omits
/// it (default `false`, e.g. a freshly added Codex slug) must not silently
/// downgrade a Max agent on Codex, nor drop `--effort` on Claude Code -
/// `turn_engine` must read the agent's own config, never the capability
/// flag. Mutation: reverting to reading `caps.effort`-gated `snapshot.params
/// .effort` directly (this test's fix) makes this test fail with `Medium`
/// on Codex and `None` on Claude Code instead of `Max`.
#[tokio::test]
async fn effort_reaches_the_launch_even_when_the_registry_model_does_not_advertise_it() {
    let providers_toml = r#"
[[providers.codex.add_models]]
id = "unadvertised-codex"
[providers.codex.add_models.capabilities]
context_window = 200000
max_output_tokens = 100000

[[providers.claude-code.add_models]]
id = "unadvertised-claude"
[providers.claude-code.add_models.capabilities]
context_window = 200000
max_output_tokens = 100000
"#;
    for (provider, model_id) in [
        ("codex", "unadvertised-codex"),
        ("claude-code", "unadvertised-claude"),
    ] {
        let prompts = Arc::new(Mutex::new(Vec::new()));
        let systems = Arc::new(Mutex::new(Vec::new()));
        let engine: Arc<dyn Engine> = Arc::new(ScriptedEngine {
            reply: Reply::ToolRun,
            prompts: prompts.clone(),
            costs: Arc::new(Mutex::new(Default::default())),
            systems: systems.clone(),
            tool_gate: None,
        });
        let h =
            harness_with_engine_and_providers_toml(engine, prompts, systems, Some(providers_toml))
                .await;
        let resolved = h
            .runtime
            .models()
            .resolve(&ModelRef::new(provider, model_id))
            .expect("registered model resolves");
        assert!(
            !resolved.model.capabilities.effort,
            "fixture model must not advertise effort support"
        );

        let (agent, _) = h
            .runtime
            .create_agent(
                CreateAgentSpec {
                    role: "assistant".into(),
                    start: true,
                    model: Some(ModelRef::new(provider, model_id)),
                    config: Some(AgentConfig {
                        long_lived: true,
                        approval: ApprovalPolicy::none(),
                        effort: Some(Effort::Max),
                        ..Default::default()
                    }),
                    ..Default::default()
                },
                Actor::user(),
            )
            .expect("create");
        let id = agent.id;
        wait_for(h.store.as_ref(), id, |a| {
            a.status.kind() == AgentStatusKind::Waiting
        })
        .await;
        h.runtime
            .send_message(id, vec![ContentBlock::text("what is here?")], Actor::user())
            .expect("send");
        wait_for(h.store.as_ref(), id, |a| {
            a.status.kind() == AgentStatusKind::Waiting && a.stats.turns >= 1
        })
        .await;

        let snapshot_id = h
            .store
            .events(0, 10_000, &EventFilter::for_agent(id))
            .expect("events")
            .into_iter()
            .find_map(|e| match e.kind {
                EventKind::SnapshotCreated { snapshot, .. } => Some(snapshot),
                _ => None,
            })
            .expect("the engine turn recorded a snapshot");
        let stored = h
            .store
            .snapshot(snapshot_id)
            .expect("snapshot")
            .expect("every announced snapshot is stored");
        assert_eq!(
            stored.params.effort,
            Some(Effort::Max),
            "provider={provider}: an unadvertised registry model must not gate the agent's own configured effort"
        );
    }
}

/// `Max` is a real Codex level (mapped to `"max"`), so a Codex agent
/// moved onto it after the fact must not be refused - and the snapshot,
/// which `turn_engine` also feeds the launch arg from, must carry `Max`
/// unchanged rather than the old silent downgrade to medium.
#[tokio::test]
async fn max_effort_survives_a_model_move_onto_codex() {
    let h = harness(Reply::ToolRun).await;
    let (agent, _) = h
        .runtime
        .create_agent(
            CreateAgentSpec {
                role: "assistant".into(),
                start: true,
                model: Some(ModelRef::new("claude-code", "haiku")),
                config: Some(AgentConfig {
                    long_lived: true,
                    approval: ApprovalPolicy::none(),
                    effort: Some(Effort::Max),
                    ..Default::default()
                }),
                ..Default::default()
            },
            Actor::user(),
        )
        .expect("Max is a valid effort on claude-code");
    let id = agent.id;
    wait_for(h.store.as_ref(), id, |a| {
        a.status.kind() == AgentStatusKind::Waiting
    })
    .await;

    h.runtime
        .update_agent(
            id,
            aether_runtime::AgentUpdate {
                model: Some(ModelRef::new("codex", "gpt-6.1-sol")),
                ..Default::default()
            },
            Actor::user(),
        )
        .expect("moving models must not be refused for an effort Codex now supports");
    let moved = h.store.agent(id).expect("agent").expect("exists");
    assert_eq!(
        moved.config.effort,
        Some(Effort::Max),
        "the model move must not have touched effort"
    );

    h.runtime
        .send_message(id, vec![ContentBlock::text("what is here?")], Actor::user())
        .expect("send");
    wait_for(h.store.as_ref(), id, |a| {
        a.status.kind() == AgentStatusKind::Waiting && a.stats.turns >= 1
    })
    .await;

    let snapshot_id = h
        .store
        .events(0, 10_000, &EventFilter::for_agent(id))
        .expect("events")
        .into_iter()
        .find_map(|e| match e.kind {
            EventKind::SnapshotCreated { snapshot, .. } => Some(snapshot),
            _ => None,
        })
        .expect("the engine turn recorded a snapshot");
    let stored = h
        .store
        .snapshot(snapshot_id)
        .expect("snapshot")
        .expect("every announced snapshot is stored");
    assert_eq!(
        stored.params.effort,
        Some(Effort::Max),
        "Max must reach the codex turn unchanged, never downgraded to medium"
    );
}

/// Codex accepts an explicitly chosen Max effort for a child.
#[tokio::test]
async fn a_max_effort_parent_can_delegate_to_a_codex_child() {
    let h = harness(Reply::ToolRun).await;
    let (parent, _) = h
        .runtime
        .create_agent(
            CreateAgentSpec {
                role: "assistant".into(),
                start: true,
                model: Some(ModelRef::new("claude-code", "haiku")),
                config: Some(AgentConfig {
                    long_lived: true,
                    approval: ApprovalPolicy::none(),
                    effort: Some(Effort::Max),
                    ..Default::default()
                }),
                ..Default::default()
            },
            Actor::user(),
        )
        .expect("create parent");

    let (child, _) = h
        .runtime
        .create_agent(
            CreateAgentSpec {
                role: "worker".into(),
                start: false,
                parent: Some(parent.id),
                config: Some(AgentConfig {
                    effort: Some(Effort::Max),
                    ..Default::default()
                }),
                model: Some(ModelRef::new("codex", "gpt-6.1-sol")),
                ..Default::default()
            },
            Actor::user(),
        )
        .expect("a Max parent delegating to Codex must not be refused");
    assert_eq!(
        child.config.effort,
        Some(Effort::Max),
        "the child keeps its explicitly chosen effort"
    );
}

/// Records the effort each launch carries, then behaves like `inner`.
struct EffortRecorder {
    inner: ScriptedEngine,
    launched: Arc<Mutex<Vec<Option<Effort>>>>,
}

#[async_trait::async_trait]
impl Engine for EffortRecorder {
    fn kind(&self) -> EngineKind {
        self.inner.kind()
    }

    fn adapter(&self) -> String {
        self.inner.adapter()
    }

    async fn run(
        &self,
        turn: EngineTurn,
        cancel: CancellationToken,
    ) -> Result<EngineRun, EngineError> {
        self.launched.lock().unwrap().push(turn.effort);
        self.inner.run(turn, cancel).await
    }
}

async fn recording_harness() -> (Harness, Arc<Mutex<Vec<Option<Effort>>>>) {
    let prompts = Arc::new(Mutex::new(Vec::new()));
    let systems = Arc::new(Mutex::new(Vec::new()));
    let launched = Arc::new(Mutex::new(Vec::new()));
    let engine: Arc<dyn Engine> = Arc::new(EffortRecorder {
        inner: ScriptedEngine {
            reply: Reply::ToolRun,
            prompts: prompts.clone(),
            costs: Arc::new(Mutex::new(Default::default())),
            systems: systems.clone(),
            tool_gate: None,
        },
        launched: launched.clone(),
    });
    let h = harness_with_engine(engine, prompts, systems).await;
    (h, launched)
}

/// GPT-6.1 Sol launches at its high default when nothing asked otherwise
/// (Codex's own default is low); an explicit effort wins, and a model without
/// a default still leaves the choice to the native harness.
#[tokio::test]
async fn gpt_6_1_sol_launches_at_high_unless_an_effort_was_asked_for() {
    for (model, configured, expected) in [
        ("sol", None, Some(Effort::High)),
        ("gpt-6.1-sol", Some(Effort::Low), Some(Effort::Low)),
        ("sol", Some(Effort::Max), Some(Effort::Max)),
        ("gpt-6-luna", None, None),
    ] {
        let (h, launched) = recording_harness().await;
        let (agent, _) = h
            .runtime
            .create_agent(
                CreateAgentSpec {
                    role: "assistant".into(),
                    start: true,
                    model: Some(ModelRef::new("codex", model)),
                    config: Some(AgentConfig {
                        long_lived: true,
                        approval: ApprovalPolicy::none(),
                        effort: configured,
                        ..Default::default()
                    }),
                    ..Default::default()
                },
                Actor::user(),
            )
            .expect("create");
        let id = agent.id;
        wait_for(h.store.as_ref(), id, |a| {
            a.status.kind() == AgentStatusKind::Waiting
        })
        .await;
        engine_turn(&h, id, "what is here?").await;

        let case = format!("codex/{model} configured={configured:?}");
        assert_eq!(launched.lock().unwrap().first(), Some(&expected), "{case}");
        let params = latest_snapshot(&h, id).params;
        assert_eq!(params.effort, expected, "{case}");
        assert_eq!(params.effort_requested, configured, "{case}");
        assert_eq!(
            params.effort_effective, expected,
            "{case}: ledger matches launch"
        );
        let persisted = h.store.agent(id).expect("agent").expect("exists");
        assert_eq!(
            persisted.config.effort, configured,
            "{case}: the model default is applied per turn, never written to config"
        );
    }
}
