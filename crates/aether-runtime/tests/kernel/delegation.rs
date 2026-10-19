//! delegation scenarios and fixtures.
use super::*;

#[tokio::test]
async fn delegate_leaf_setting_reaches_the_child_config() {
    for leaf in [None, Some(false), Some(true)] {
        let mut input = json!({"role":"worker", "objective":"check", "model":"anthropic/claude-sonnet-5-5", "effort":"low", "wait":true});
        if let Some(leaf) = leaf {
            input["leaf"] = json!(leaf);
        }
        let h = harness(vec![
            ScriptedClient::tool_call("delegate", input),
            ScriptedClient::tool_call("complete_task", json!({"summary":"done"})),
            ScriptedClient::tool_call("complete_task", json!({"summary":"done"})),
        ])
        .await;
        let (parent, _) = h
            .runtime
            .create_agent(spec("delegate"), Actor::user())
            .unwrap();
        h.wait_until(parent.id, |s| s.is_terminal()).await;
        let children = h
            .store
            .agents(&AgentFilter {
                parent: Some(parent.id),
                ..Default::default()
            })
            .unwrap();
        assert_eq!(children.len(), 1);
        assert_eq!(children[0].config.leaf, leaf.unwrap_or(false));
        assert!(!h.store.agent(parent.id).unwrap().unwrap().config.leaf);
    }
}

#[tokio::test]
async fn delegate_host_context_inherits_the_parent_unless_set() {
    use aether_core::agent::HostContext;
    for (parent_context, requested, expected) in [
        (HostContext::Full, None, HostContext::Full),
        (HostContext::Lean, None, HostContext::Lean),
        (HostContext::Lean, Some("full"), HostContext::Full),
        (HostContext::Full, Some("lean"), HostContext::Lean),
    ] {
        let mut input = json!({"role":"worker", "objective":"check", "model":"anthropic/claude-sonnet-5-5", "effort":"low", "wait":true});
        if let Some(host_context) = requested {
            input["host_context"] = json!(host_context);
        }
        let h = harness(vec![
            ScriptedClient::tool_call("delegate", input),
            ScriptedClient::tool_call("complete_task", json!({"summary":"done"})),
            ScriptedClient::tool_call("complete_task", json!({"summary":"done"})),
        ])
        .await;
        let mut parent_spec = spec("delegate");
        parent_spec.config.as_mut().unwrap().host_context = parent_context;
        let (parent, _) = h.runtime.create_agent(parent_spec, Actor::user()).unwrap();
        h.wait_until(parent.id, |s| s.is_terminal()).await;
        let children = h
            .store
            .agents(&AgentFilter {
                parent: Some(parent.id),
                ..Default::default()
            })
            .unwrap();
        assert_eq!(children.len(), 1);
        assert_eq!(children[0].config.host_context, expected);
        let stored_parent = h.store.agent(parent.id).unwrap().unwrap();
        assert_eq!(stored_parent.config.host_context, parent_context);
    }
}

#[tokio::test]
async fn leaf_cannot_delegate_even_when_the_tool_is_called_directly() {
    let h = harness(vec![
        ScriptedClient::tool_calls(&[
            ("delegate", json!({"role":"worker", "objective":"check", "model":"anthropic/claude-sonnet-5-5", "effort":"low", "leaf":false})),
            ("integrate_workspace", json!({"child_agent_id": AgentId::new().to_string()})),
            ("list_agents", json!({})),
            ("swarm_playbook", json!({})),
            ("list_peers", json!({})),
        ], None),
        ScriptedClient::tool_call("complete_task", json!({"summary":"done"})),
    ]).await;
    let mut leaf_spec = spec("do the work yourself");
    leaf_spec.config = Some(AgentConfig {
        leaf: true,
        ..Default::default()
    });
    let (leaf, _) = h.runtime.create_agent(leaf_spec, Actor::user()).unwrap();
    h.wait_until(leaf.id, |s| s.is_terminal()).await;
    let children = h
        .store
        .agents(&AgentFilter {
            parent: Some(leaf.id),
            ..Default::default()
        })
        .unwrap();
    assert!(children.is_empty());
    let events = h
        .store
        .events(0, 10_000, &EventFilter::for_agent(leaf.id))
        .unwrap();
    for (name, expected) in [
        ("delegate", "Leaf agents cannot delegate children"),
        ("integrate_workspace", "withheld from leaf agents"),
        ("list_agents", "withheld from leaf agents"),
        ("swarm_playbook", "withheld from leaf agents"),
        ("list_peers", "withheld from root agents"),
    ] {
        let call = events
            .iter()
            .find_map(|event| match &event.kind {
                EventKind::ToolRequested { call, .. } if call.name == name => Some(call.id),
                _ => None,
            })
            .expect("scripted tool call was requested");
        assert!(
            events.iter().any(|event| matches!(
                &event.kind,
                EventKind::ToolFailed { call: failed, error, .. }
                    if *failed == call && error.contains(expected)
            )),
            "{name} must be refused for a leaf"
        );
    }
}

#[tokio::test]
async fn leaf_api_requests_remove_only_coordination_tools() {
    let h = harness(vec![
        ScriptedClient::tool_call("complete_task", json!({"summary":"root done"})),
        ScriptedClient::tool_call("complete_task", json!({"summary":"leaf done"})),
    ])
    .await;
    let (root, _) = h.runtime.create_agent(spec("root"), Actor::user()).unwrap();
    h.wait_until(root.id, |status| status.is_terminal()).await;

    let mut leaf_spec = spec("leaf");
    leaf_spec.config.as_mut().unwrap().leaf = true;
    let (leaf, _) = h.runtime.create_agent(leaf_spec, Actor::user()).unwrap();
    h.wait_until(leaf.id, |status| status.is_terminal()).await;

    let requests = h.client.requests();
    assert_eq!(requests.len(), 2);
    let root_tools = &requests[0].tools;
    let leaf_tools = &requests[1].tools;
    let removed = [
        "delegate",
        "integrate_workspace",
        "cancel_agent",
        "create_schedule",
        "update_schedule",
        "list_schedules",
        "list_agents",
        "list_tasks",
        "read_task",
        "list_models",
        "provider_usage",
        "refresh_provider_usage",
        "swarm_playbook",
        "notify",
    ];
    for name in removed {
        assert!(root_tools.iter().any(|tool| tool.name == name), "{name}");
        assert!(!leaf_tools.iter().any(|tool| tool.name == name), "{name}");
    }
    let expected = root_tools
        .iter()
        .filter(|tool| !removed.contains(&tool.name.as_str()))
        .collect::<Vec<_>>();
    assert_eq!(
        serde_json::to_value(leaf_tools).unwrap(),
        serde_json::to_value(expected).unwrap()
    );
    assert!(leaf_tools.iter().any(|tool| tool.name == "complete_task"));
    for tools in [root_tools, leaf_tools] {
        assert!(
            !tools.iter().any(|tool| tool.name == "list_peers"),
            "a root agent has no peers"
        );
    }
}

#[tokio::test]
async fn leaf_cannot_create_children_through_the_runtime() {
    let h = harness(vec![]).await;
    let mut leaf_spec = spec("leaf");
    leaf_spec.start = false;
    leaf_spec.config = Some(AgentConfig {
        leaf: true,
        ..Default::default()
    });
    let (leaf, _) = h.runtime.create_agent(leaf_spec, Actor::user()).unwrap();
    let mut child_spec = spec("child");
    child_spec.parent = Some(leaf.id);
    child_spec.config = Some(AgentConfig {
        effort: Some(aether_core::model::Effort::Low),
        ..Default::default()
    });
    let error = h
        .runtime
        .create_agent(child_spec, Actor::agent(leaf.id))
        .unwrap_err();
    assert!(
        error
            .to_string()
            .contains("Leaf agents cannot delegate children"),
        "{error}"
    );
    assert!(
        h.store
            .agents(&AgentFilter {
                parent: Some(leaf.id),
                ..Default::default()
            })
            .unwrap()
            .is_empty()
    );
}

#[tokio::test]
async fn delegated_effort_is_explicit_and_never_inherited() {
    for explicit in [true, false] {
        let mut input = json!({"role":"worker", "objective":"check", "model":"anthropic/claude-sonnet-5-5", "wait":true});
        if explicit {
            input["effort"] = json!("low");
        }
        let h = harness(vec![
            ScriptedClient::tool_call("delegate", input),
            ScriptedClient::tool_call("complete_task", json!({"summary":"done"})),
            ScriptedClient::tool_call("complete_task", json!({"summary":"done"})),
        ])
        .await;
        let mut parent_spec = spec("delegate");
        parent_spec.config = Some(AgentConfig {
            effort: Some(aether_core::model::Effort::High),
            ..Default::default()
        });
        let (parent, _) = h.runtime.create_agent(parent_spec, Actor::user()).unwrap();
        h.wait_until(parent.id, |s| s.is_terminal()).await;
        let children = h
            .store
            .agents(&aether_store::AgentFilter {
                parent: Some(parent.id),
                ..Default::default()
            })
            .unwrap();
        if explicit {
            assert_eq!(children.len(), 1);
            assert_eq!(
                children[0].config.effort,
                Some(aether_core::model::Effort::Low)
            );
        } else {
            assert!(
                children.is_empty(),
                "missing effort must not create a child"
            );
        }
    }
}

#[tokio::test]
async fn delegation_does_not_copy_parent_task_instructions_into_leaves() {
    for instructions in [None, Some("Read the README; report the heading.")] {
        let mut input = json!({"role":"worker", "objective":"read the docs", "model":"anthropic/claude-sonnet-5-5", "effort":"low", "wait":true});
        if let Some(text) = instructions {
            input["instructions"] = json!(text);
        }
        let h = harness(vec![
            ScriptedClient::tool_call("delegate", input),
            ScriptedClient::tool_call("complete_task", json!({"summary":"child done"})),
            ScriptedClient::tool_call("complete_task", json!({"summary":"parent done"})),
        ])
        .await;
        let mut parent_spec = spec("delegate a documentation check");
        parent_spec.config = Some(AgentConfig {
            instructions: Some("You must spawn exactly one Sonnet child.".into()),
            ..Default::default()
        });
        let (parent, _) = h.runtime.create_agent(parent_spec, Actor::user()).unwrap();
        h.wait_until(parent.id, |s| s.is_terminal()).await;
        let children = h
            .store
            .agents(&AgentFilter {
                parent: Some(parent.id),
                ..Default::default()
            })
            .unwrap();
        assert_eq!(children.len(), 1);
        assert_eq!(
            children[0].config.instructions.as_deref(),
            Some(instructions.unwrap_or_default())
        );
        assert_eq!(
            h.store
                .agent(parent.id)
                .unwrap()
                .unwrap()
                .config
                .instructions,
            parent.config.instructions
        );
    }
}

#[tokio::test]
async fn parent_sleeps_while_child_runs_and_wakes_exactly_once() {
    let h = harness(vec![
        // parent turn 1: delegate and wait
        ScriptedClient::tool_call(
            "delegate",
            json!({"model":"anthropic/claude-sonnet-5-5", "effort":"low", "role": "worker", "objective": "do the sub task", "wait": true}),
        ),
        // child turn 1: complete
        ScriptedClient::tool_call(
            "complete_task",
            json!({"summary": "child finished the sub task"}),
        ),
        // parent turn 2: complete with the child's result
        ScriptedClient::tool_call("complete_task", json!({"summary": "parent done"})),
    ])
    .await;
    let (parent, _) = h
        .runtime
        .create_agent(spec("split the work"), Actor::user())
        .expect("create");
    let status = h.wait_until(parent.id, |s| s.is_terminal()).await;
    assert_eq!(status.kind(), AgentStatusKind::Completed);

    let t = types(h.store.as_ref(), parent.id);
    let resumed = t.iter().filter(|t| *t == "agent.resumed").count();
    assert_eq!(resumed, 1, "parent must wake exactly once: {t:?}");
    assert!(t.contains(&"agent.waiting".to_string()));
    let parent_turns = h.store.invocations_for_agent(parent.id).expect("inv");
    assert_eq!(
        parent_turns.len(),
        2,
        "parent used two turns, none while sleeping"
    );
    // The wake-up turn saw the child's result in its user message.
    let snap = h
        .store
        .snapshot(parent_turns[1].snapshot_id)
        .expect("snap")
        .expect("exists");
    let last_user = snap
        .messages
        .iter()
        .rev()
        .find(|m| m.role == aether_core::model::MessageRole::User)
        .expect("user");
    assert!(
        last_user.text().contains("child finished the sub task"),
        "{}",
        last_user.text()
    );
    // Child completed and its task is terminal.
    let children = h
        .store
        .agents(&aether_store::AgentFilter {
            parent: Some(parent.id),
            ..Default::default()
        })
        .expect("kids");
    assert_eq!(children.len(), 1);
    assert_eq!(children[0].status.kind(), AgentStatusKind::Completed);
    assert_eq!(h.client.remaining(), 0);
}

#[tokio::test]
async fn a_person_gets_a_turn_while_children_fill_the_slots() {
    use aether_core::model::{BlockStart, StopReason, StreamEvent};
    // The child's reply streams slowly, so it holds the only slot for a while.
    let mut slow = vec![
        StreamEvent::MessageStart {
            provider_message_id: Some("scripted".into()),
            model: Some("scripted".into()),
        },
        StreamEvent::BlockStart {
            index: 0,
            block: BlockStart::Text {
                text: String::new(),
            },
        },
    ];
    slow.extend((0..30).map(|i| StreamEvent::TextDelta {
        index: 0,
        text: format!("w{i} "),
    }));
    slow.push(StreamEvent::BlockStop { index: 0 });
    slow.push(StreamEvent::Stop {
        reason: StopReason::EndTurn,
    });
    slow.push(StreamEvent::MessageStop);
    let h = harness(vec![
        // parent turn 1: delegate and wait
        ScriptedClient::tool_call(
            "delegate",
            json!({"model":"anthropic/claude-sonnet-5-5", "effort":"low", "role": "worker", "objective": "do the sub task", "wait": true}),
        ),
        // child turn 1: the slow reply
        Ok(slow),
        // parent turn 2: woken by the person while the child still runs
        ScriptedClient::text("The worker is still busy; I will report when it is done."),
        // child turn 2: nudged to close its task
        ScriptedClient::tool_call("complete_task", json!({"summary": "child finished"})),
        // parent turn 3: finish with the result
        ScriptedClient::tool_call("complete_task", json!({"summary": "parent done"})),
    ])
    .await;
    assert_eq!(h.runtime.config().max_concurrent_turns, 1);
    h.client.set_delay(Duration::from_millis(80));
    let (parent, _) = h
        .runtime
        .create_agent(spec("split the work"), Actor::user())
        .expect("create");
    let child = loop {
        let kids = h
            .store
            .agents(&AgentFilter {
                parent: Some(parent.id),
                ..Default::default()
            })
            .expect("agents");
        if let Some(kid) = kids
            .into_iter()
            .find(|k| k.status.kind() == AgentStatusKind::Running)
            && h.client.requests().len() >= 2
        {
            break kid;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    };

    // The child holds the only slot; a person speaks to the waiting parent.
    h.runtime
        .send_message(
            parent.id,
            vec![ContentBlock::text("status?")],
            Actor::user(),
        )
        .expect("send");
    let deadline = tokio::time::Instant::now() + Duration::from_secs(15);
    loop {
        let p = h.store.agent(parent.id).expect("agent").expect("exists");
        if p.stats.turns >= 2 && matches!(p.status, AgentStatus::Waiting { .. }) {
            break;
        }
        assert!(
            tokio::time::Instant::now() < deadline,
            "parent never answered: {:?}",
            p.status
        );
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    // Event order proves the parent answered before the child's first turn
    // ended, not merely after it.
    let seq_of = |agent: AgentId, kind: &str, nth: usize| {
        h.store
            .events(0, 10_000, &EventFilter::for_agent(agent))
            .expect("events")
            .into_iter()
            .filter(|e| e.type_name() == kind)
            .nth(nth)
            .map(|e| e.seq)
    };
    let parent_answered = seq_of(parent.id, "agent.turn_completed", 1).expect("parent turn 2");
    let child_first_done = loop {
        if let Some(seq) = seq_of(child.id, "agent.turn_completed", 0) {
            break seq;
        }
        assert!(
            tokio::time::Instant::now() < deadline,
            "child never finished"
        );
        tokio::time::sleep(Duration::from_millis(10)).await;
    };
    assert!(
        parent_answered < child_first_done,
        "parent turn 2 ended at {parent_answered}, after the child's first turn at {child_first_done}"
    );
    assert!(
        h.client
            .requests()
            .iter()
            .any(|r| format!("{r:?}").contains("status?")),
        "the message reached the model"
    );

    let status = h.wait_until(parent.id, |s| s.is_terminal()).await;
    assert_eq!(status.kind(), AgentStatusKind::Completed);
    assert_eq!(h.client.remaining(), 0);
}
