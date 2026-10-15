use super::*;

async fn reported_wait(h: &mut Harness) -> (AgentId, TurnStarted) {
    let parent = start_coordinator(h);
    let turn = turn_of(h, parent).await;
    turn.events
        .upgrade()
        .unwrap()
        .send(EngineEvent::Assistant {
            content: vec![ContentBlock::ToolUse {
                id: "wait-reply".into(),
                name: "mcp__aether__wait_for".into(),
                input: json!({}),
            }],
            usage: None,
            model: None,
        })
        .await
        .unwrap();
    wait_for_event(h.store.as_ref(), |event| {
        matches!(&event.kind, EventKind::ToolRequested { call, .. }
            if call.provider_call_id == "wait-reply")
    })
    .await;
    let (reply, error) = raw_call(h, &turn, "wait_for", json!({})).await;
    assert!(!error, "{reply}");
    (parent, turn)
}

#[tokio::test]
async fn parking_waits_for_the_native_tool_reply_before_interrupting() {
    let mut h = harness(Duration::from_secs(5)).await;
    let (parent, turn) = reported_wait(&mut h).await;
    turn.events
        .upgrade()
        .unwrap()
        .send(EngineEvent::ToolResult {
            tool_use_id: "unrelated".into(),
            content: "ok".into(),
            is_error: false,
        })
        .await
        .unwrap();
    tokio::time::sleep(Duration::from_millis(30)).await;
    assert!(!turn.observed.stopped());
    assert!(!turn.observed.stdin_closed());
    turn.events
        .upgrade()
        .unwrap()
        .send(EngineEvent::ToolResult {
            tool_use_id: "wait-reply".into(),
            content: "Waiting for your next message.".into(),
            is_error: false,
        })
        .await
        .unwrap();
    wait_until_asked_to_stop(&turn.observed, Duration::from_secs(2)).await;
    assert!(events(h.store.as_ref()).iter().any(|event| {
        matches!(&event.kind, EventKind::ToolSucceeded { outcome, .. }
            if !outcome.is_error && outcome.content == "Waiting for your next message.")
    }));
    let _ = turn.end.send(());
    wait_for_park(&h, parent).await;
    h.runtime.shutdown().await;
}

#[tokio::test]
async fn a_missing_native_reply_cannot_prevent_the_bounded_park_stop() {
    let mut h = harness(Duration::from_millis(100)).await;
    let (parent, turn) = reported_wait(&mut h).await;
    wait_until_stopped(&turn.observed, Duration::from_secs(2)).await;
    drop(turn.events);
    wait_for_park(&h, parent).await;
    let recorded = events(h.store.as_ref());
    assert!(recorded.iter().any(|event| matches!(&event.kind,
        EventKind::ToolSucceeded { outcome, .. }
        if !outcome.is_error && outcome.content.contains("Waiting"))));
    assert!(!recorded.iter().any(|event| matches!(&event.kind,
        EventKind::Custom { name, .. } if name == "tool.interrupted")));
    assert!(!recorded.iter().any(|event| matches!(
        event.kind,
        EventKind::ToolCancelled { .. } | EventKind::ToolDenied { .. }
    )));
    h.runtime.shutdown().await;
}

#[tokio::test]
async fn completed_bridge_replies_survive_missing_or_rejected_native_echoes() {
    for (name, input) in [
        ("wait_for", json!({})),
        ("ask_user", json!({"question": "which half?"})),
        ("complete_task", json!({"summary": "finished"})),
    ] {
        for (provider, model) in [("claude-code", "haiku"), ("codex", "default")] {
            for late_announcement in [false, true] {
                let mut h = harness(Duration::from_millis(100)).await;
                let parent = start_agent(
                    &h,
                    CreateAgentSpec {
                        model: Some(ModelRef::new(provider, model)),
                        ..coordinator_spec()
                    },
                );
                let turn = turn_of(&mut h, parent).await;
                let stream = turn.events.upgrade().unwrap();
                let announcement = EngineEvent::Assistant {
                    content: vec![ContentBlock::ToolUse {
                        id: "completed-call".into(),
                        name: format!("mcp__aether__{name}"),
                        input: input.clone(),
                    }],
                    usage: None,
                    model: None,
                };
                if !late_announcement {
                    stream.send(announcement.clone()).await.unwrap();
                }
                let (reply, error) = raw_call(&h, &turn, name, input.clone()).await;
                assert!(!error, "{reply}");
                if late_announcement {
                    stream.send(announcement).await.unwrap();
                    stream.send(EngineEvent::ToolResult {
                        tool_use_id: "completed-call".into(),
                        content: "The user doesn't want to proceed with this tool use. The tool use was rejected.".into(),
                        is_error: true,
                    }).await.unwrap();
                }
                wait_until_stopped(&turn.observed, Duration::from_secs(2)).await;
                drop(stream);
                drop(turn.events);
                wait_for_park(&h, parent).await;
                let recorded = events(h.store.as_ref());
                assert!(
                    recorded.iter().any(|event| matches!(&event.kind,
                    EventKind::ToolSucceeded { outcome, .. }
                    if !outcome.is_error && outcome.content == reply)),
                    "{name}, {provider}, late={late_announcement}"
                );
                assert!(!recorded.iter().any(|event| matches!(&event.kind,
                    EventKind::Custom { name, .. } if name == "tool.interrupted")));
                assert!(!recorded.iter().any(|event| matches!(
                    event.kind,
                    EventKind::ToolCancelled { .. } | EventKind::ToolDenied { .. }
                )));
                h.runtime.shutdown().await;
            }
        }
    }
}

#[tokio::test]
async fn a_completed_parking_call_does_not_hide_an_interrupted_native_sibling() {
    let mut h = harness(Duration::from_millis(100)).await;
    let (parent, turn) = reported_wait(&mut h).await;
    turn.events
        .upgrade()
        .unwrap()
        .send(EngineEvent::Assistant {
            content: vec![ContentBlock::ToolUse {
                id: "native-sibling".into(),
                name: "Bash".into(),
                input: json!({"command": "make ci"}),
            }],
            usage: None,
            model: None,
        })
        .await
        .unwrap();
    wait_until_stopped(&turn.observed, Duration::from_secs(2)).await;
    drop(turn.events);
    wait_for_park(&h, parent).await;
    let recorded = events(h.store.as_ref());
    let interrupted: Vec<_> = recorded
        .iter()
        .filter_map(|event| match &event.kind {
            EventKind::Custom { name, payload } if name == "tool.interrupted" => Some(payload),
            _ => None,
        })
        .collect();
    assert_eq!(interrupted.len(), 1);
    assert_eq!(interrupted[0]["tool"], "Bash");
    assert_eq!(interrupted[0]["cause"], "harness ended the turn");
    assert!(recorded.iter().any(|event| matches!(&event.kind,
        EventKind::ToolSucceeded { outcome, .. }
        if !outcome.is_error && outcome.content.contains("Waiting"))));
    h.runtime.shutdown().await;
}

#[tokio::test]
async fn bridge_requests_can_arrive_before_their_native_stream_events() {
    let mut h = harness(Duration::from_secs(5)).await;
    let parent = start_coordinator(&h);
    let turn = turn_of(&mut h, parent).await;
    let (reply, error) = raw_call(&h, &turn, "wait_for", json!({})).await;
    assert!(!error, "{reply}");
    assert!(!turn.observed.stopped());
    let stream = turn.events.upgrade().unwrap();
    stream
        .send(EngineEvent::Assistant {
            content: vec![ContentBlock::ToolUse {
                id: "late-request".into(),
                name: "mcp__aether__wait_for".into(),
                input: json!({}),
            }],
            usage: None,
            model: None,
        })
        .await
        .unwrap();
    stream
        .send(EngineEvent::ToolResult {
            tool_use_id: "late-request".into(),
            content: reply.clone(),
            is_error: false,
        })
        .await
        .unwrap();
    wait_until_asked_to_stop(&turn.observed, Duration::from_secs(2)).await;
    drop(stream);
    let _ = turn.end.send(());
    wait_for_park(&h, parent).await;
    h.runtime.shutdown().await;
}
