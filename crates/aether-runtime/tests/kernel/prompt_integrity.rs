use super::*;

async fn small_context_harness(responses: Vec<aether_model::testing::ScriptedResponse>) -> Harness {
    harness_full(
        responses,
        |_| {},
        |config| config.default_model = Some(ModelRef::new("small", "m")),
        Some(
            r#"
[providers.small]
protocol = "openai-chat-completions"
base_url = "https://invalid.test"
[[providers.small.models]]
id = "m"
[providers.small.models.capabilities]
context_window = 100000
max_output_tokens = 80000
"#,
        ),
        None,
        None,
        |_| {},
    )
    .await
}

#[tokio::test]
async fn memory_retrieval_failure_fails_the_child_and_notifies_its_parent() {
    let inner: Arc<dyn Store> = Arc::new(SqliteStore::open_in_memory().expect("store"));
    let store = Arc::new(CommitsMidPass::new(inner, Hook::MemoryRetrievalRead));
    let h = harness_full(
        vec![ScriptedClient::text("must not be invoked")],
        |_| {},
        |_| {},
        None,
        None,
        Some(store.clone()),
        |_| {},
    )
    .await;
    let (parent, _) = h
        .runtime
        .create_agent(
            CreateAgentSpec {
                start: false,
                ..idle_spec()
            },
            Actor::user(),
        )
        .expect("parent");
    store.arm(Box::new(|_| {
        Err(aether_store::StoreError::Corrupt(
            "simulated memory retrieval failure".into(),
        ))
    }));
    let (child, task) = h
        .runtime
        .create_agent(
            CreateAgentSpec {
                parent: Some(parent.id),
                ..spec("retrieve required context")
            },
            Actor::agent(parent.id),
        )
        .expect("child");
    let status = h.wait_until(child.id, |status| status.is_terminal()).await;
    let AgentStatus::Failed { error } = status else {
        panic!("expected failure, got {status:?}");
    };
    assert!(
        error.contains("simulated memory retrieval failure"),
        "{error}"
    );
    assert!(store.fired());
    assert!(h.client.requests().is_empty());
    let task = task.expect("task");
    assert_eq!(
        h.store.task(task.id).unwrap().unwrap().status.kind(),
        TaskStatusKind::Failed
    );
    let inbox = h.store.inbox(parent.id, 0).expect("parent inbox");
    assert!(
        inbox.iter().any(|item| matches!(
            &item.kind,
            aether_core::inbox::InboxKind::TaskFailed { task: id, error, .. }
                if *id == task.id && error.contains("simulated memory retrieval failure")
        )),
        "{inbox:?}"
    );
    h.runtime.shutdown().await;
}

#[tokio::test]
async fn oversized_api_input_is_rejected_before_consuming_the_inbox() {
    let h = small_context_harness(vec![ScriptedClient::text("must not be invoked")]).await;
    let (agent, _) = h
        .runtime
        .create_agent(idle_spec(), Actor::user())
        .expect("agent");
    let profile = h
        .runtime
        .models()
        .resolve(&agent.model)
        .expect("model")
        .model
        .capabilities;
    h.runtime
        .send_message(
            agent.id,
            vec![ContentBlock::text(
                "x".repeat(profile.context_window as usize * 4),
            )],
            Actor::user(),
        )
        .expect("oversized message");
    let status = h.wait_until(agent.id, |status| status.is_terminal()).await;
    let AgentStatus::Failed { error } = status else {
        panic!("expected failure, got {status:?}");
    };
    assert!(error.contains("context budget exceeded"), "{error}");
    assert!(h.client.requests().is_empty());
    assert_eq!(h.store.agent(agent.id).unwrap().unwrap().inbox_cursor, 0);
    assert_eq!(h.store.inbox(agent.id, 0).unwrap().len(), 1);
    let invocations = h.store.invocations_for_agent(agent.id).unwrap();
    assert_eq!(invocations.len(), 1);
    assert!(
        invocations[0]
            .error
            .as_ref()
            .unwrap()
            .contains("context budget exceeded")
    );
    assert_eq!(invocations[0].usage, aether_core::model::Usage::default());
    h.runtime.shutdown().await;
}

#[tokio::test]
async fn oversized_fixed_context_is_rejected_before_invoking_the_provider() {
    let h = small_context_harness(vec![ScriptedClient::text("must not be invoked")]).await;
    let model = ModelRef::new("small", "m");
    let window = h
        .runtime
        .models()
        .resolve(&model)
        .unwrap()
        .model
        .capabilities
        .context_window;
    let (agent, task) = h
        .runtime
        .create_agent(spec(&"x".repeat(window as usize * 4)), Actor::user())
        .expect("agent");
    let status = h.wait_until(agent.id, |status| status.is_terminal()).await;
    let AgentStatus::Failed { error } = status else {
        panic!("expected failure, got {status:?}");
    };
    assert!(error.contains("context budget exceeded"), "{error}");
    assert!(h.client.requests().is_empty());
    assert_eq!(
        h.store
            .task(task.unwrap().id)
            .unwrap()
            .unwrap()
            .status
            .kind(),
        TaskStatusKind::Failed
    );
    h.runtime.shutdown().await;
}

#[tokio::test]
async fn api_input_budget_reserves_the_configured_output_tokens() {
    let h = small_context_harness(vec![ScriptedClient::text("must not be invoked")]).await;
    let mut specification = idle_spec();
    specification.config.as_mut().unwrap().max_output_tokens = Some(80_000);
    let (agent, _) = h
        .runtime
        .create_agent(specification, Actor::user())
        .unwrap();
    h.runtime
        .send_message(
            agent.id,
            vec![ContentBlock::text("x".repeat(80_000))],
            Actor::user(),
        )
        .unwrap();
    let status = h.wait_until(agent.id, |status| status.is_terminal()).await;
    let AgentStatus::Failed { error } = status else {
        panic!("expected failure, got {status:?}");
    };
    assert!(error.contains("context budget exceeded"), "{error}");
    assert!(
        error.contains("available 20000 after reserving output"),
        "{error}"
    );
    assert!(h.client.requests().is_empty());
    assert_eq!(h.store.agent(agent.id).unwrap().unwrap().inbox_cursor, 0);
    h.runtime.shutdown().await;
}

#[tokio::test]
async fn oversized_api_context_tries_a_larger_fallback_without_calling_the_small_model() {
    let h = small_context_harness(vec![ScriptedClient::tool_call(
        "complete_task",
        json!({"summary": "larger fallback carried the turn"}),
    )])
    .await;
    let mut specification = spec(&"x".repeat(80_000));
    specification.config.as_mut().unwrap().max_output_tokens = Some(80_000);
    specification.fallbacks = vec![ModelRef::new("anthropic", "claude-sonnet-5-5")];
    let (agent, _) = h
        .runtime
        .create_agent(specification, Actor::user())
        .unwrap();
    let status = h.wait_until(agent.id, |status| status.is_terminal()).await;
    assert_eq!(status.kind(), AgentStatusKind::Completed, "{status:?}");
    let requests = h.client.requests();
    assert_eq!(requests.len(), 1);
    assert_eq!(requests[0].model.provider, "anthropic");
    let events = h
        .store
        .events(0, 10_000, &EventFilter::own_events(agent.id))
        .unwrap();
    assert!(events.iter().any(|event| matches!(
        event.kind,
        EventKind::ProviderFallback {
            class: Some(ModelErrorClass::ContextLimit),
            ..
        }
    )));
    assert!(
        !events
            .iter()
            .any(|event| matches!(event.kind, EventKind::ProviderBackoff { .. }))
    );
    h.runtime.shutdown().await;
}

#[tokio::test]
async fn local_context_exhaustion_is_reported_directly_to_every_ancestor() {
    let h = small_context_harness(vec![ScriptedClient::text("must not be invoked")]).await;
    let (root, _) = h
        .runtime
        .create_agent(
            CreateAgentSpec {
                start: false,
                ..idle_spec()
            },
            Actor::user(),
        )
        .unwrap();
    let (parent, _) = h
        .runtime
        .create_agent(
            CreateAgentSpec {
                parent: Some(root.id),
                start: false,
                ..idle_spec()
            },
            Actor::agent(root.id),
        )
        .unwrap();
    let window = h
        .runtime
        .models()
        .resolve(&parent.model)
        .unwrap()
        .model
        .capabilities
        .context_window;
    let (child, _) = h
        .runtime
        .create_agent(
            CreateAgentSpec {
                parent: Some(parent.id),
                ..spec(&"x".repeat(window as usize * 4))
            },
            Actor::agent(parent.id),
        )
        .unwrap();
    let status = h.wait_until(child.id, |status| status.is_terminal()).await;
    let AgentStatus::Failed { error } = status else {
        panic!("expected failure, got {status:?}");
    };
    assert!(error.contains("context_limit"), "{error}");
    assert!(error.contains("context budget exceeded"), "{error}");
    assert!(h.client.requests().is_empty());
    for ancestor in [parent.id, root.id] {
        let failures = child_failures(h.store.as_ref(), ancestor);
        assert_eq!(failures.len(), 1, "{failures:?}");
        assert!(failures[0].contains("context_limit"));
        assert!(failures[0].contains(&child.id.to_string()));
    }
    let events = h
        .store
        .events(0, 10_000, &EventFilter::own_events(child.id))
        .unwrap();
    assert!(events.iter().any(|event| matches!(
        event.kind,
        EventKind::InvocationFailed {
            class: Some(ModelErrorClass::ContextLimit),
            ..
        }
    )));
    h.runtime.shutdown().await;
}
