use super::*;

#[tokio::test]
async fn duplicate_provider_tool_ids_fail_before_dispatch() {
    let mut response = ScriptedClient::tool_calls(
        &[
            ("complete_task", json!({"summary":"first"})),
            ("complete_task", json!({"summary":"second"})),
        ],
        None,
    )
    .unwrap();
    for event in &mut response {
        if let StreamEvent::BlockStart {
            block: BlockStart::ToolUse { id, .. },
            ..
        } = event
        {
            *id = "duplicate".into();
        }
    }
    let h = harness(vec![Ok(response)]).await;
    let (agent, task) = h
        .runtime
        .create_agent(spec("reject ambiguous calls"), Actor::user())
        .unwrap();
    h.wait_until(agent.id, |status| status.is_terminal()).await;
    let task = h.store.task(task.unwrap().id).unwrap().unwrap();
    assert!(
        matches!(task.status, aether_core::task::TaskStatus::Failed { error, .. }
        if error.contains("duplicate tool-use id"))
    );
    let events = h
        .store
        .events(0, 100, &EventFilter::own_events(agent.id))
        .unwrap();
    assert!(
        !events
            .iter()
            .any(|event| matches!(event.kind, EventKind::ToolRequested { .. }))
    );
    assert!(events.iter().any(|event| matches!(
        event.kind,
        EventKind::InvocationFailed {
            class: Some(ModelErrorClass::InvalidRequest),
            ..
        }
    )));
    h.runtime.shutdown().await;
}

#[tokio::test]
async fn api_context_occupancy_saturates_large_provider_counters() {
    let mut response =
        ScriptedClient::tool_call("complete_task", json!({"summary":"done"})).unwrap();
    for event in &mut response {
        if let StreamEvent::Usage { usage } = event {
            usage.input_tokens = u64::MAX;
            usage.cache_write_tokens = 1;
        }
    }
    let h = harness(vec![Ok(response)]).await;
    let (agent, _) = h
        .runtime
        .create_agent(spec("keep imported usage"), Actor::user())
        .unwrap();
    h.wait_until(agent.id, |status| status.is_terminal()).await;
    let agent = h.store.agent(agent.id).unwrap().unwrap();
    assert_eq!(agent.status.kind(), AgentStatusKind::Completed);
    assert_eq!(agent.stats.context_tokens, Some(u64::MAX));
    h.runtime.shutdown().await;
}

#[tokio::test]
async fn cross_provider_compactor_exhaustion_reaches_root_and_keeps_task_outcome() {
    for (fallback_succeeds, class) in [
        (false, ModelErrorClass::QuotaExhausted),
        (true, ModelErrorClass::QuotaExhausted),
        (false, ModelErrorClass::ContextLimit),
        (true, ModelErrorClass::ContextLimit),
    ] {
        let checkpoint = json!({"checkpoint": {
            "objective":"keep work", "state":"ready", "decisions":[],
            "constraints":[], "open_loops":[]
        }, "memory_candidates":[]})
        .to_string();
        let fallback = || {
            if fallback_succeeds {
                ScriptedClient::text(&checkpoint)
            } else {
                ScriptedClient::error(ModelErrorClass::Unknown, "fallback unavailable")
            }
        };
        let h = harness_full(
            vec![
                ScriptedClient::tool_call("wait_for", json!({"timeout_seconds":3600})),
                ScriptedClient::error(class, "compactor provider exhausted"),
                fallback(),
            ],
            |_| {},
            |_| {},
            None,
            Some("[model]\ncompactor = \"openai-responses/luna\"\n"),
            None,
            |_| {},
        )
        .await;
        let mut parent = None;
        let mut ancestors = Vec::new();
        for role in ["root", "parent"] {
            let (agent, _) = h
                .runtime
                .create_agent(
                    CreateAgentSpec {
                        role: role.into(),
                        parent,
                        start: false,
                        config: parent.map(|_| AgentConfig {
                            effort: Some(aether_core::model::Effort::Low),
                            ..Default::default()
                        }),
                        ..Default::default()
                    },
                    parent.map(Actor::agent).unwrap_or_else(Actor::user),
                )
                .unwrap();
            ancestors.push(agent.id);
            parent = Some(agent.id);
        }
        let mut child_spec = spec("keep work");
        child_spec.parent = parent;
        child_spec.model = Some(ModelRef::new("openai", "sol"));
        let (child, task) = h
            .runtime
            .create_agent(child_spec, Actor::agent(parent.unwrap()))
            .unwrap();
        h.wait_until(child.id, |status| status.kind() == AgentStatusKind::Waiting)
            .await;
        let task_before = h.store.task(task.unwrap().id).unwrap().unwrap().status;
        let mut history = Commit::new();
        for index in 0..10 {
            history.push_event(
                EventDraft::new(
                    Actor::System,
                    EventKind::AgentMessage {
                        role: if index % 2 == 0 {
                            aether_core::model::MessageRole::User
                        } else {
                            aether_core::model::MessageRole::Assistant
                        },
                        content: vec![ContentBlock::text(format!("history {index}"))],
                        invocation: None,
                    },
                )
                .agent(child.id)
                .run(child.run),
            );
        }
        h.store.commit(history).unwrap();
        let turns = h.store.agent(child.id).unwrap().unwrap().stats.turns;
        h.runtime
            .request_compaction(child.id, Actor::user())
            .unwrap();
        h.wait_for("compaction settled", || {
            h.store.agent(child.id).unwrap().is_some_and(|agent| {
                agent.stats.turns > turns && agent.status.kind() == AgentStatusKind::Waiting
            })
        })
        .await;
        assert_eq!(
            h.store.latest_checkpoint(child.id).unwrap().is_some(),
            fallback_succeeds
        );
        assert_eq!(
            h.store
                .task(child.current_task.unwrap())
                .unwrap()
                .unwrap()
                .status,
            task_before
        );
        for ancestor in &ancestors {
            let notices = h
                .store
                .inbox(*ancestor, 0)
                .unwrap()
                .into_iter()
                .filter_map(|item| match item.kind {
                    aether_core::inbox::InboxKind::ExternalTrigger {
                        source,
                        payload,
                        operator: false,
                    } if source == "exhaustion" => Some(payload),
                    _ => None,
                })
                .collect::<Vec<_>>();
            assert_eq!(notices.len(), 1);
            assert_eq!(notices[0]["provider"], "openai-responses");
            assert_eq!(notices[0]["category"], class.as_str());
            assert_eq!(notices[0]["source_agent"], json!(child.id));
            assert_eq!(
                notices[0]["verdict"],
                if class.as_str() == "context_limit" {
                    "context limit exceeded; task outcome unchanged"
                } else {
                    "provider exhausted; task outcome unchanged"
                }
            );
        }
        if !fallback_succeeds {
            h.client
                .push(ScriptedClient::error(class, "compactor provider exhausted"));
            h.client.push(fallback());
            let turns = h.store.agent(child.id).unwrap().unwrap().stats.turns;
            h.runtime
                .request_compaction(child.id, Actor::user())
                .unwrap();
            h.wait_for("second compaction settled", || {
                h.store.agent(child.id).unwrap().is_some_and(|agent| {
                    agent.stats.turns > turns && agent.status.kind() == AgentStatusKind::Waiting
                })
            })
            .await;
            for ancestor in &ancestors {
                let notices = h.store.inbox(*ancestor, 0).unwrap();
                let count = notices.iter().filter(|item| {
                    matches!(&item.kind, aether_core::inbox::InboxKind::ExternalTrigger { source, .. }
                        if source == "exhaustion")
                }).count();
                assert_eq!(count, 1);
            }
        }
        h.runtime.shutdown().await;
    }
}
