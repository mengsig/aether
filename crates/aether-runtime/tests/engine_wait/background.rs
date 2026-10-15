use super::*;

async fn background_wait(
    input: Value,
) -> (
    Harness,
    AgentId,
    TurnStarted,
    tokio::task::JoinHandle<aether_runtime::Result<(String, bool)>>,
) {
    let mut h = harness_full(
        Duration::from_millis(100),
        OnKill::Report(vec![cancelled_with(Usage::default())]),
        Script {
            background: Some("build".into()),
            ..Default::default()
        },
    )
    .await;
    let parent = start_coordinator(&h);
    let turn = turn_of(&mut h, parent).await;
    let runtime = h.runtime.clone();
    let token = turn.token.clone();
    let pending = tokio::spawn(async move {
        runtime
            .engine_tool(parent, &token, "wait_for".into(), input)
            .await
    });
    wait_for_event(h.store.as_ref(), |event| {
        matches!(&event.kind,
        EventKind::Custom { name, .. } if name == "engine.native_wait_started")
    })
    .await;
    (h, parent, turn, pending)
}

#[tokio::test]
async fn native_jobs_survive_wait_until_their_completion_event() {
    let (h, parent, turn, pending) = background_wait(json!({})).await;
    turn.events
        .upgrade()
        .unwrap()
        .send(EngineEvent::Assistant {
            content: vec![ContentBlock::ToolUse {
                id: "background-wait".into(),
                name: "mcp__aether__wait_for".into(),
                input: json!({}),
            }],
            usage: None,
            model: None,
        })
        .await
        .unwrap();
    let before = h.store.agent(parent).unwrap().unwrap().stats;
    tokio::time::sleep(Duration::from_millis(350)).await;
    assert!(!pending.is_finished());
    assert!(!turn.observed.stopped());
    assert!(!turn.observed.stdin_closed());
    assert!(!turn.observed.cancelled());
    let after = h.store.agent(parent).unwrap().unwrap().stats;
    assert_eq!(before.usage, after.usage);
    assert_eq!(before.turns, after.turns);
    turn.events
        .upgrade()
        .unwrap()
        .send(EngineEvent::BackgroundTask {
            id: "build".into(),
            running: false,
        })
        .await
        .unwrap();
    let (reply, error) = tokio::time::timeout(Duration::from_secs(2), pending)
        .await
        .unwrap()
        .unwrap()
        .unwrap();
    assert!(!error && reply.contains("build finished"), "{reply}");
    assert!(!turn.observed.stopped());
    let _ = turn.end.send(());
    wait_for_park(&h, parent).await;
    let recorded = events(h.store.as_ref());
    assert!(recorded.iter().any(|event| matches!(&event.kind,
        EventKind::ToolSucceeded { outcome, .. }
        if !outcome.is_error && outcome.content == reply)));
    assert!(!recorded.iter().any(|event| matches!(&event.kind,
        EventKind::Custom { name, .. } if name == "tool.interrupted")));
    h.runtime.shutdown().await;
}

#[tokio::test]
async fn a_native_wait_deadline_returns_without_killing_its_jobs() {
    let (h, _, turn, pending) = background_wait(json!({"timeout_seconds": 1})).await;
    let (reply, error) = tokio::time::timeout(Duration::from_secs(3), pending)
        .await
        .unwrap()
        .unwrap()
        .unwrap();
    assert!(!error && reply.contains("deadline"), "{reply}");
    assert!(!turn.observed.stopped());
    assert!(!turn.observed.cancelled());
    h.runtime.shutdown().await;
    assert!(turn.observed.cancelled());
}

#[tokio::test]
async fn only_a_tracked_completion_releases_wait_and_other_jobs_survive() {
    let (h, _, turn, pending) = background_wait(json!({})).await;
    let events = turn.events.upgrade().unwrap();
    for (id, running) in [("monitor", true), ("unrelated", false)] {
        events
            .send(EngineEvent::BackgroundTask {
                id: id.into(),
                running,
            })
            .await
            .unwrap();
    }
    tokio::time::sleep(Duration::from_millis(150)).await;
    assert!(!pending.is_finished());
    events
        .send(EngineEvent::BackgroundTask {
            id: "build".into(),
            running: false,
        })
        .await
        .unwrap();
    let (reply, error) = tokio::time::timeout(Duration::from_secs(2), pending)
        .await
        .unwrap()
        .unwrap()
        .unwrap();
    assert!(!error && reply.contains("build finished"), "{reply}");
    assert!(!turn.observed.stopped());
    assert!(!turn.observed.stdin_closed());
    h.runtime.shutdown().await;
}

#[tokio::test]
async fn cancelling_a_native_wait_still_stops_its_engine() {
    let (h, parent, turn, pending) = background_wait(json!({"timeout_seconds": 3600})).await;
    h.runtime
        .cancel_agent(parent, Some("operator cancelled".into()), Actor::user())
        .unwrap();
    wait_until_stopped(&turn.observed, Duration::from_secs(2)).await;
    let result = tokio::time::timeout(Duration::from_secs(2), pending)
        .await
        .expect("cancellation releases the pending request")
        .expect("the request task must not panic");
    assert!(
        result
            .unwrap_err()
            .to_string()
            .contains("dropped the request")
    );
    h.runtime.shutdown().await;
}

#[tokio::test]
async fn a_message_releases_native_wait_without_ending_the_session() {
    let (h, parent, turn, pending) = background_wait(json!({})).await;
    h.runtime
        .send_message(
            parent,
            vec![ContentBlock::text("status please")],
            Actor::user(),
        )
        .unwrap();
    let (reply, error) = tokio::time::timeout(Duration::from_secs(2), pending)
        .await
        .unwrap()
        .unwrap()
        .unwrap();
    assert!(!error && reply.contains("message"), "{reply}");
    assert!(!turn.observed.stopped());
    assert!(!turn.observed.stdin_closed());
    h.runtime.shutdown().await;
}

/// The grace has to bind the party it bounds. A parked engine that keeps the
/// drive loop busy inside one slow bridge call would otherwise buy itself
/// unrestricted time - its own shell and network are not gated by the park at
/// all - simply by never letting the loop reach a deadline.
#[tokio::test]
async fn a_stalled_bridge_call_cannot_postpone_the_park_s_stop() {
    let stall = Duration::from_secs(5);
    let mut h = harness_with_tool(Duration::from_millis(200), Arc::new(SlowTool(stall))).await;
    let coordinator = start_coordinator(&h);
    let first = turn_of(&mut h, coordinator).await;

    let (reply, is_error) = call(&h, &first, "wait_for", json!({"timeout_seconds": 3600})).await;
    assert!(!is_error, "{reply}");

    // Issued after the park and answered only long after the grace: while it
    // runs, the drive loop is suspended inside it.
    let runtime = h.runtime.clone();
    let (agent, token) = (first.agent, first.token.clone());
    let stalled = tokio::spawn(async move {
        runtime
            .engine_tool(agent, &token, "list_tasks".into(), json!({}))
            .await
    });

    let took = wait_until_stopped(&first.observed, Duration::from_secs(2)).await;
    assert!(
        !stalled.is_finished(),
        "the call returned on its own; it stalled nothing and the test proves nothing"
    );
    assert!(
        took < stall,
        "the stop waited for the stalled call: {took:?}"
    );

    let parked = wait_for_park(&h, coordinator).await;
    assert!(matches!(parked.status, AgentStatus::Waiting { .. }));
    assert_eq!(
        custom(h.store.as_ref(), coordinator, "engine.parked").expect("recorded")["stopped_by"],
        json!("runtime")
    );
    let _ = stalled.await.expect("the stalled call is still answered");
    h.runtime.shutdown().await;
}
