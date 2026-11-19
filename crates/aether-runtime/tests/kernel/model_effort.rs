//! Per-model default effort, and alias resolution, on the API path.
use super::*;
use aether_core::model::Effort;

/// GPT-6.1/6.2 Sol's request carries its high default when nothing asked
/// otherwise; an explicit effort wins, and a model without a default sends none.
/// The provider is sent the canonical id, never Aether's alias: the Chat
/// adapter's GPT-6 tool guard matches on it, and the API rejects `sol`.
#[tokio::test]
async fn gpt_6_1_sol_requests_high_unless_an_effort_was_asked_for() {
    for (provider, model, configured, expected, wire) in [
        (
            "openai-responses",
            "sol",
            None,
            Some(Effort::High),
            "gpt-6.2-sol",
        ),
        ("openai", "sol", None, Some(Effort::High), "gpt-6.2-sol"),
        (
            "openai-responses",
            "gpt-6.1-sol",
            Some(Effort::Low),
            Some(Effort::Low),
            "gpt-6.1-sol",
        ),
        ("openai-responses", "luna", None, None, "gpt-6-luna"),
        ("openai-responses", "astra", None, None, "gpt-6.2-astra"),
        ("openai-responses", "gpt-6-astra", None, None, "gpt-6-astra"),
    ] {
        let h = harness(vec![ScriptedClient::tool_call(
            "complete_task",
            json!({"summary": "done"}),
        )])
        .await;
        let mut spec = spec("check the effort");
        spec.model = Some(ModelRef::new(provider, model));
        if let Some(config) = spec.config.as_mut() {
            config.effort = configured;
        }
        let (agent, _) = h.runtime.create_agent(spec, Actor::user()).expect("create");
        h.wait_until(agent.id, |s| s.is_terminal()).await;

        let case = format!("{provider}/{model} configured={configured:?}");
        let requests = h.client.requests();
        let first = requests.first().expect("one request");
        assert_eq!(first.effort, expected, "{case}");
        assert_eq!(first.model, ModelRef::new(provider, wire), "{case}");
        let persisted = h.store.agent(agent.id).expect("agent").expect("exists");
        assert_eq!(persisted.config.effort, configured, "{case}");
        assert_eq!(
            persisted.model,
            ModelRef::new(provider, model),
            "{case}: the configured alias is kept"
        );
    }
}

#[tokio::test]
async fn api_turn_records_the_requested_effort_and_the_sent_override() {
    use Effort::{High, Max, XHigh};

    for (provider, model, requested, effective) in [
        ("openai", "sol", Some(XHigh), Some(High)),
        ("openai-responses", "astra", Some(Max), Some(High)),
        ("openai-responses", "sol", None, Some(High)),
        ("openai-responses", "luna", None, None),
        ("anthropic", "opus", Some(XHigh), Some(XHigh)),
        ("groq", "llama-3.3-70b-versatile", Some(Max), None),
    ] {
        let h = harness(vec![ScriptedClient::tool_call(
            "complete_task",
            json!({"summary": "done"}),
        )])
        .await;
        let mut spec = spec("check the turn record");
        spec.model = Some(ModelRef::new(provider, model));
        spec.config.as_mut().unwrap().effort = requested;
        let (agent, _) = h.runtime.create_agent(spec, Actor::user()).unwrap();
        h.wait_until(agent.id, |s| s.is_terminal()).await;
        let calls = h.store.invocations_for_agent(agent.id).unwrap();
        let snapshot = h.store.snapshot(calls[0].snapshot_id).unwrap().unwrap();
        let params = snapshot.params;
        assert_eq!(params.effort_requested, requested, "{provider}/{model}");
        assert_eq!(params.effort_effective, effective, "{provider}/{model}");
        assert_eq!(params.effort, h.client.requests()[0].effort);
    }
}

#[tokio::test]
async fn child_creation_requires_effort_before_writing_any_state() {
    let h = harness(vec![]).await;
    let (parent, _) = h
        .runtime
        .create_agent(
            CreateAgentSpec {
                start: false,
                config: Some(AgentConfig {
                    effort: Some(Effort::High),
                    ..Default::default()
                }),
                ..spec("parent")
            },
            Actor::user(),
        )
        .expect("parent");
    let before_agents = h.store.agents(&AgentFilter::default()).unwrap().len();
    let before_tasks = h
        .store
        .tasks(&aether_store::TaskFilter::default())
        .unwrap()
        .len();
    for config in [None, Some(AgentConfig::default())] {
        let error = h
            .runtime
            .create_agent(
                CreateAgentSpec {
                    parent: Some(parent.id),
                    config,
                    start: false,
                    ..spec("child")
                },
                Actor::user(),
            )
            .expect_err("a child must choose effort");
        assert!(
            error
                .to_string()
                .contains("a sub-agent needs an explicit effort: low, medium, high, xhigh or max"),
            "{error}"
        );
        assert_eq!(
            h.store.agents(&AgentFilter::default()).unwrap().len(),
            before_agents
        );
        assert_eq!(
            h.store
                .tasks(&aether_store::TaskFilter::default())
                .unwrap()
                .len(),
            before_tasks
        );
    }
    for effort in [
        Effort::Low,
        Effort::Medium,
        Effort::High,
        Effort::XHigh,
        Effort::Max,
    ] {
        let (child, _) = h
            .runtime
            .create_agent(
                CreateAgentSpec {
                    parent: Some(parent.id),
                    start: false,
                    config: Some(AgentConfig {
                        effort: Some(effort),
                        ..Default::default()
                    }),
                    ..spec("child")
                },
                Actor::user(),
            )
            .expect("explicit effort");
        assert_eq!(child.config.effort, Some(effort));
    }
    h.runtime.shutdown().await;
}

#[tokio::test]
async fn child_restart_and_wake_preserve_recorded_effort() {
    for restart in [false, true] {
        for recorded in [Some(Effort::Low), None] {
            let h = harness(vec![ScriptedClient::tool_call(
                "fail_task",
                json!({"error":"failed"}),
            )])
            .await;
            let (parent, _) = h
                .runtime
                .create_agent(
                    CreateAgentSpec {
                        start: false,
                        ..spec("parent")
                    },
                    Actor::user(),
                )
                .unwrap();
            let (child, _) = h
                .runtime
                .create_agent(
                    CreateAgentSpec {
                        parent: Some(parent.id),
                        ..spec("child")
                    },
                    Actor::user(),
                )
                .unwrap();
            h.wait_until(child.id, |s| s.kind() == AgentStatusKind::Failed)
                .await;
            if recorded.is_none() {
                let mut old = h.store.agent(child.id).unwrap().unwrap();
                old.config.effort = None;
                h.store
                    .commit(Commit::new().mutate(Mutation::UpsertAgent(old)))
                    .unwrap();
            }
            h.runtime
                .update_agent(
                    parent.id,
                    AgentUpdate {
                        effort: Some(Effort::Max),
                        ..Default::default()
                    },
                    Actor::user(),
                )
                .unwrap();
            let resumed = if restart {
                h.runtime.restart_agent(child.id, Actor::user()).unwrap()
            } else {
                h.runtime.wake_agent(child.id, Actor::user()).unwrap()
            };
            assert_eq!(resumed.parent, Some(parent.id));
            assert_eq!(resumed.config.effort, recorded);
            h.runtime.shutdown().await;
        }
    }
}
