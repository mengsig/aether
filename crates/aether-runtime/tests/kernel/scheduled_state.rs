//! scheduled state scenarios and fixtures.
use super::*;

#[tokio::test]
async fn scheduled_wake_survives_restart_and_explains_why_it_woke() {
    let at = now() + chrono::Duration::hours(1);
    let h = harness(vec![ScriptedClient::tool_call(
        "wait_for",
        json!({"wake_at":at.to_rfc3339()}),
    )])
    .await;
    let (agent, _) = h
        .runtime
        .create_agent(spec("follow up later"), Actor::user())
        .unwrap();
    h.wait_until(agent.id, |s| matches!(s, AgentStatus::Waiting { .. }))
        .await;
    h.runtime.shutdown().await;
    let parked = h.store.agent(agent.id).unwrap().unwrap();
    assert!(
        matches!(&parked.status, AgentStatus::Waiting { condition } if condition.earliest_deadline() == Some(at))
    );
    let restarted = harness_full(
        vec![],
        |_| {},
        |_| {},
        None,
        None,
        Some(h.store.clone()),
        |_| {},
    )
    .await;
    assert_eq!(
        restarted.store.agent(agent.id).unwrap().unwrap().status,
        parked.status
    );
    restarted.runtime.shutdown().await;
    let mut due = parked;
    due.status = AgentStatus::Waiting {
        condition: aether_core::agent::WaitCondition::Any {
            of: vec![
                aether_core::agent::WaitCondition::Inbox,
                aether_core::agent::WaitCondition::Deadline {
                    at: now() - chrono::Duration::seconds(1),
                },
            ],
        },
    };
    h.store
        .commit(Commit::new().mutate(Mutation::UpsertAgent(due)))
        .unwrap();
    let resumed = harness_full(
        vec![ScriptedClient::tool_call(
            "complete_task",
            json!({"summary":"follow-up done"}),
        )],
        |_| {},
        |_| {},
        None,
        None,
        Some(h.store.clone()),
        |_| {},
    )
    .await;
    resumed.wait_until(agent.id, |s| s.is_terminal()).await;
    let requests = resumed.client.requests();
    assert!(
        requests
            .iter()
            .flat_map(|r| r.messages.iter())
            .any(|m| m.text().contains("requested wake time has arrived"))
    );
}

#[tokio::test]
async fn state_tools_report_runtime_facts_without_mutating_task_state() {
    let h = harness(vec![
        ScriptedClient::tool_call("inspect_state", json!({})),
        ScriptedClient::tool_call("list_models", json!({"provider":"codex"})),
        ScriptedClient::tool_call("complete_task", json!({"summary":"done"})),
    ])
    .await;
    let (agent, _) = h
        .runtime
        .create_agent(spec("inspect the harness"), Actor::user())
        .unwrap();
    h.wait_until(agent.id, |s| s.is_terminal()).await;
    let outputs: Vec<_> = h
        .store
        .events(0, 10_000, &EventFilter::for_agent(agent.id))
        .unwrap()
        .into_iter()
        .filter_map(|e| match e.kind {
            EventKind::ToolSucceeded { outcome, .. } => outcome.structured,
            _ => None,
        })
        .collect();
    let state = outputs
        .iter()
        .find(|v| v.get("compaction").is_some())
        .expect("state result");
    assert_eq!(state["model"], json!(agent.model));
    assert!(state["compaction"]["threshold_tokens"].is_u64());
    assert_eq!(state["compaction"]["effort"], "medium");
    let catalog = outputs
        .iter()
        .find(|v| v.get("models").is_some())
        .expect("catalog result");
    assert!(!catalog["models"].as_array().unwrap().is_empty());
    let models = catalog["models"].as_array().unwrap();
    assert_eq!(models[0]["model"]["model"], "gpt-6.2-astra");
    assert_eq!(models[1]["model"]["model"], "gpt-6.1-terra");
    assert_eq!(models[2]["model"]["model"], "gpt-6.2-sol");
    assert_eq!(models[3]["model"]["model"], "gpt-6-luna");
    assert_eq!(models[1]["api_equivalent_cost"]["input_per_mtok"], 1.0);
    assert_eq!(
        models[1]["compaction_threshold_tokens"], 272_000,
        "the catalog says where Codex compacts"
    );
    assert!(
        catalog["guidance"]
            .as_str()
            .unwrap()
            .contains("capability first")
    );
    assert!(
        catalog["models"]
            .as_array()
            .unwrap()
            .iter()
            .all(|m| m["model"]["provider"] == "codex")
    );
    assert!(catalog["providers"]["codex"]["configured"].is_boolean());
    for model in catalog["models"].as_array().unwrap() {
        assert_eq!(
            model["configured"],
            catalog["providers"]["codex"]["configured"]
        );
        assert!(model["provider_name"].is_string());
    }
    for alternative in catalog["configured_alternatives"].as_array().unwrap() {
        assert_eq!(alternative["configured"], true);
        assert_ne!(alternative["model"]["provider"], "codex");
        assert!(
            catalog["models"]
                .as_array()
                .unwrap()
                .iter()
                .any(|model| model["name"] == alternative["name"])
        );
    }
}

#[tokio::test]
async fn inspect_state_reports_cost_budget_turn_limit_and_deadline() {
    let h = harness(vec![
        ScriptedClient::tool_call("inspect_state", json!({})),
        ScriptedClient::tool_call("complete_task", json!({"summary": "done"})),
    ])
    .await;
    let deadline = now() + chrono::Duration::days(365);
    let mut limited = spec("report your limits");
    limited.config = limited.config.map(|config| AgentConfig {
        cost_budget: Some(0.5),
        max_turns: Some(7),
        deadline: Some(deadline),
        ..config
    });
    let (agent, _) = h.runtime.create_agent(limited, Actor::user()).unwrap();
    h.wait_until(agent.id, |s| s.is_terminal()).await;
    let state = h
        .store
        .events(0, 10_000, &EventFilter::for_agent(agent.id))
        .unwrap()
        .into_iter()
        .find_map(|e| match e.kind {
            EventKind::ToolSucceeded { outcome, .. } => {
                outcome.structured.filter(|v| v.get("compaction").is_some())
            }
            _ => None,
        })
        .expect("inspect_state result");
    assert_eq!(state["cost_budget"], 0.5);
    assert_eq!(state["max_turns"], 7);
    assert_eq!(state["deadline"], deadline.to_rfc3339());
    let left = state["deadline_minutes_left"]
        .as_i64()
        .expect("minutes left");
    assert!(
        (365 * 24 * 60 - 5..=365 * 24 * 60).contains(&left),
        "a year out, read moments ago: {left}"
    );
    assert!(state["turns_taken"].is_u64());
    assert!(state["subtree_cost_spent_usd"].is_number());
    assert!(state["subtree_cost_is_a_floor"].is_boolean());
}

#[tokio::test]
async fn inspect_state_measures_context_against_the_compaction_threshold() {
    let h = harness(vec![
        ScriptedClient::tool_call("inspect_state", json!({})),
        ScriptedClient::tool_call("complete_task", json!({"summary": "done"})),
    ])
    .await;
    let (agent, _) = h
        .runtime
        .create_agent(spec("report the threshold"), Actor::user())
        .unwrap();
    h.wait_until(agent.id, |s| s.is_terminal()).await;
    let state = h
        .store
        .events(0, 10_000, &EventFilter::for_agent(agent.id))
        .unwrap()
        .into_iter()
        .find_map(|e| match e.kind {
            EventKind::ToolSucceeded { outcome, .. } => {
                outcome.structured.filter(|v| v.get("compaction").is_some())
            }
            _ => None,
        })
        .expect("inspect_state result");
    let capabilities = h
        .runtime
        .models()
        .resolve(&agent.model)
        .unwrap()
        .model
        .capabilities;
    let threshold = capabilities.compaction_tokens.expect("a Claude threshold");
    assert!(threshold < capabilities.context_window);
    assert_eq!(state["context"]["window"], capabilities.context_window);
    assert_eq!(state["context"]["compaction_threshold_tokens"], threshold);
    assert_eq!(state["compaction"]["threshold_tokens"], threshold);
    assert_eq!(
        state["compaction"]["threshold_percent_of_window"],
        f64::from(threshold) / f64::from(capabilities.context_window) * 100.0
    );
    assert!(
        state["compaction"]
            .get("pressure_threshold_percent")
            .is_none()
    );
}

/// A caller confirms an agent's own threshold took effect from inspect_state.
#[tokio::test]
async fn inspect_state_reports_an_agents_own_compaction_threshold() {
    let h = harness(vec![
        ScriptedClient::tool_call("inspect_state", json!({})),
        ScriptedClient::tool_call("complete_task", json!({"summary": "done"})),
    ])
    .await;
    let mut own = spec("report my threshold");
    if let Some(config) = own.config.as_mut() {
        config.compaction_tokens = Some(120_000);
    }
    let (agent, _) = h.runtime.create_agent(own, Actor::user()).unwrap();
    h.wait_until(agent.id, |s| s.is_terminal()).await;
    let state = h
        .store
        .events(0, 10_000, &EventFilter::for_agent(agent.id))
        .unwrap()
        .into_iter()
        .find_map(|e| match e.kind {
            EventKind::ToolSucceeded { outcome, .. } => {
                outcome.structured.filter(|v| v.get("compaction").is_some())
            }
            _ => None,
        })
        .expect("inspect_state result");
    let capabilities = h
        .runtime
        .models()
        .resolve(&agent.model)
        .unwrap()
        .model
        .capabilities;
    assert!(capabilities.context_window > 120_000);
    assert_ne!(capabilities.compaction_tokens, Some(120_000));
    assert_eq!(state["compaction"]["agent_threshold_tokens"], 120_000);
    assert_eq!(state["compaction"]["threshold_tokens"], 120_000);
    assert_eq!(state["context"]["compaction_threshold_tokens"], 120_000);
}

#[tokio::test]
async fn inspect_state_reports_unset_limits_as_null() {
    let h = harness(vec![
        ScriptedClient::tool_call("inspect_state", json!({})),
        ScriptedClient::tool_call("complete_task", json!({"summary": "done"})),
    ])
    .await;
    let (agent, _) = h
        .runtime
        .create_agent(spec("report no limits"), Actor::user())
        .unwrap();
    h.wait_until(agent.id, |s| s.is_terminal()).await;
    let state = h
        .store
        .events(0, 10_000, &EventFilter::for_agent(agent.id))
        .unwrap()
        .into_iter()
        .find_map(|e| match e.kind {
            EventKind::ToolSucceeded { outcome, .. } => {
                outcome.structured.filter(|v| v.get("compaction").is_some())
            }
            _ => None,
        })
        .expect("inspect_state result");
    for field in ["cost_budget", "max_turns", "deadline"] {
        assert!(state[field].is_null(), "{field}: {state}");
    }
}

#[tokio::test]
async fn an_api_batch_cannot_delegate_or_message_after_a_wait() {
    let h = harness(vec![ScriptedClient::tool_calls(
        &[
            ("wait_for", json!({"wake_at": (now() + chrono::Duration::hours(1)).to_rfc3339()})),
            (
                "delegate",
                json!({"model":"anthropic/claude-sonnet-5-5", "effort":"low", "role": "worker", "objective": "late work", "wait": false}),
            ),
            (
                "send_message",
                json!({"to": AgentId::new().to_string(), "content": "late"}),
            ),
        ],
        None,
    )])
    .await;
    let (agent, _) = h
        .runtime
        .create_agent(spec("wait then act"), Actor::user())
        .unwrap();
    h.wait_until(agent.id, |s| matches!(s, AgentStatus::Waiting { .. }))
        .await;
    let children = h
        .store
        .agents(&AgentFilter {
            parent: Some(agent.id),
            ..Default::default()
        })
        .unwrap();
    assert!(children.is_empty(), "a child was spawned after the wait");
    let failed = h
        .store
        .events(0, 10_000, &EventFilter::for_agent(agent.id))
        .unwrap()
        .into_iter()
        .filter(|e| matches!(&e.kind, EventKind::ToolFailed { error, .. } if error.contains("turn is over")))
        .count();
    assert_eq!(failed, 2);
}

/// The direct API loop requests compaction at the agent's own threshold,
/// while a sibling on the same model with the same context does not.
#[tokio::test]
async fn the_api_loop_compacts_at_an_agents_own_threshold() {
    let h = harness(vec![
        ScriptedClient::tool_call("complete_task", json!({"summary": "done"})),
        ScriptedClient::tool_call("complete_task", json!({"summary": "done"})),
    ])
    .await;
    // About 115,000 estimated tokens: past 110,000, inside the model's own.
    let brief = "x".repeat(415_000);
    let create = |tokens| {
        let mut spec = spec(&brief);
        spec.model = Some(ModelRef::new("openai", "gpt-6.1-sol"));
        if let Some(config) = spec.config.as_mut() {
            config.compaction_tokens = tokens;
        }
        h.runtime.create_agent(spec, Actor::user()).unwrap().0
    };
    let own = create(Some(110_000));
    let sibling = create(None);
    assert_eq!(own.model, sibling.model);
    let capabilities = h
        .runtime
        .models()
        .resolve(&own.model)
        .unwrap()
        .model
        .capabilities;
    let model_threshold = aether_context::ContextBudget::for_profile(&capabilities, None);
    assert!(
        model_threshold.threshold_tokens() > 130_000 && !capabilities.compacts_on_server(),
        "the fixture model folds in Aether above the brief: {capabilities:?}"
    );
    // The threshold request commits before the model is called, so a
    // completed invocation means the decision has been recorded.
    let invoked = |events: &[aether_core::event::Event]| {
        events
            .iter()
            .any(|e| matches!(e.kind, EventKind::InvocationCompleted { .. }))
    };
    let own_events = wait_until_events(h.store.as_ref(), own.id, invoked).await;
    let sibling_events = wait_until_events(h.store.as_ref(), sibling.id, invoked).await;
    let threshold_requests = |events: &[aether_core::event::Event]| {
        events
            .iter()
            .filter_map(|e| match &e.kind {
                EventKind::CompactionRequested {
                    trigger: CompactionTrigger::TokenThreshold { tokens, threshold },
                } => Some((*tokens, *threshold)),
                _ => None,
            })
            .collect::<Vec<_>>()
    };
    let own_requests = threshold_requests(&own_events);
    assert!(!own_requests.is_empty(), "the agent's own threshold fired");
    for (tokens, threshold) in own_requests {
        assert_eq!(threshold, 110_000);
        assert!(tokens >= 110_000, "{tokens}");
    }
    assert_eq!(
        threshold_requests(&sibling_events),
        [],
        "the sibling keeps the model's"
    );
}
