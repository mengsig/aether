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

type Pending = tokio::task::JoinHandle<aether_runtime::Result<(String, bool)>>;

/// The coordinator's call `name` while its engine still runs the `build`
/// job: a parking call held for the native wait instead.
async fn held_call(mut h: Harness, name: &str, input: Value) -> (Harness, TurnStarted, Pending) {
    let parent = start_coordinator(&h);
    let turn = turn_of(&mut h, parent).await;
    let pending = hold(&h, &turn, name, input).await;
    (h, turn, pending)
}

/// Send `turn`'s call `name` and check that it is held, not parked.
async fn hold(h: &Harness, turn: &TurnStarted, name: &str, input: Value) -> Pending {
    let parent = turn.agent;
    turn.events
        .upgrade()
        .unwrap()
        .send(EngineEvent::Assistant {
            content: vec![ContentBlock::ToolUse {
                id: "held".into(),
                name: format!("mcp__aether__{name}"),
                input: input.clone(),
            }],
            usage: None,
            model: None,
        })
        .await
        .unwrap();
    let runtime = h.runtime.clone();
    let (token, name) = (turn.token.clone(), name.to_owned());
    let pending =
        tokio::spawn(async move { runtime.engine_tool(parent, &token, name, input).await });
    wait_for_event(h.store.as_ref(), |event| {
        matches!(&event.kind,
        EventKind::Custom { name, .. } if name == "engine.native_wait_started")
    })
    .await;
    // Many park graces: a parked turn would have asked the engine to stop.
    tokio::time::sleep(Duration::from_millis(350)).await;
    assert!(!pending.is_finished(), "the wait ended the turn");
    assert!(!turn.observed.stopped() && !turn.observed.cancelled());
    assert!(!turn.observed.stdin_closed());
    pending
}

/// The child's id once `parent`'s delegate created it.
fn only_child(h: &Harness, parent: AgentId) -> AgentId {
    let mut children = h
        .store
        .agents(&AgentFilter {
            parent: Some(parent),
            ..Default::default()
        })
        .unwrap();
    assert_eq!(children.len(), 1, "{children:?}");
    children.remove(0).id
}

async fn job_harness() -> Harness {
    harness_full(
        Duration::from_millis(100),
        OnKill::Report(vec![cancelled_with(Usage::default())]),
        Script {
            background: Some("build".into()),
            ..Default::default()
        },
    )
    .await
}

async fn reply_of(pending: Pending) -> String {
    let (reply, error) = tokio::time::timeout(Duration::from_secs(2), pending)
        .await
        .unwrap()
        .unwrap()
        .unwrap();
    assert!(!error, "{reply}");
    reply
}

/// The operator's answer is the reply, and the job outlives the question.
#[tokio::test]
async fn ask_user_keeps_running_native_jobs_until_the_answer() {
    let (h, turn, pending) = held_call(
        job_harness().await,
        "ask_user",
        json!({"question": "which half?"}),
    )
    .await;
    // One wait at a time, and the question's task stays open under it.
    for (name, input) in [
        ("wait_for", json!({})),
        ("complete_task", json!({"summary": "done"})),
    ] {
        let (refused, is_error) = raw_call(&h, &turn, name, input).await;
        assert!(is_error && refused.contains("pending"), "{name}: {refused}");
    }
    h.runtime
        .send_message(
            turn.agent,
            vec![ContentBlock::text("the left half")],
            Actor::user(),
        )
        .unwrap();
    let reply = reply_of(pending).await;
    assert!(
        reply.starts_with("Wait ended: a message arrived.")
            && reply.contains("Delivered with this reply:")
            && reply.contains("the left half")
            && reply.ends_with("Question recorded for the operator to see."),
        "{reply}"
    );
    assert!(!turn.observed.stopped() && !turn.observed.stdin_closed());
    // Answered inside the turn: a second question is held, not refused.
    let runtime = h.runtime.clone();
    let (agent, token) = (turn.agent, turn.token.clone());
    let again = tokio::spawn(async move {
        let input = json!({"question": "and then?"});
        runtime
            .engine_tool(agent, &token, "ask_user".into(), input)
            .await
    });
    tokio::time::sleep(Duration::from_millis(200)).await;
    assert!(!again.is_finished(), "{:?}", again.await);
    h.runtime.shutdown().await;
}

/// The child's result ends the wait, and the job outlives the child.
#[tokio::test]
async fn a_waiting_delegate_keeps_running_native_jobs_until_the_child_finishes() {
    let request = json!({"model": "claude-code/haiku", "effort": "low", "role": "worker",
                         "objective": "first half", "wait": true});
    let (mut h, turn, pending) = held_call(job_harness().await, "delegate", request).await;
    let child = only_child(&h, turn.agent);
    let child_turn = turn_of(&mut h, child).await;
    let (done, is_error) = call(
        &h,
        &child_turn,
        "complete_task",
        json!({"summary": "left half sorted"}),
    )
    .await;
    assert!(!is_error, "{done}");
    let reply = reply_of(pending).await;
    assert!(
        reply.starts_with("Wait ended: the delegated task finished.")
            && reply.contains("Delivered with this reply:")
            && reply.contains("left half sorted")
            && reply.contains(&format!("Delegated to agent {child}")),
        "{reply}"
    );
    assert!(!reply.contains("Your turn ends here"), "{reply}");
    assert!(!turn.observed.stopped() && !turn.observed.stdin_closed());
    h.runtime.shutdown().await;
}

/// A message steered before the hold may still be refused. The held reply
/// must not consume what arrived after it, or the refusal would skip it.
#[tokio::test]
async fn a_held_reply_never_consumes_past_an_unacknowledged_steer() {
    let mut h = job_harness().await;
    let parent = start_coordinator(&h);
    let turn = turn_of(&mut h, parent).await;
    let steered = vec![ContentBlock::text("use the left half")];
    h.runtime
        .send_message(parent, steered, Actor::user())
        .unwrap();
    // The fixture takes steered input without acknowledging it.
    tokio::time::sleep(Duration::from_millis(200)).await;
    let request = json!({"model": "claude-code/haiku", "effort": "low", "role": "worker",
                         "objective": "first half", "wait": true});
    let pending = hold(&h, &turn, "delegate", request).await;
    let child = only_child(&h, parent);
    let child_turn = turn_of(&mut h, child).await;
    let summary = json!({"summary": "left half sorted"});
    let (done, is_error) = call(&h, &child_turn, "complete_task", summary).await;
    assert!(!is_error, "{done}");
    let reply = reply_of(pending).await;
    let events = turn.events.upgrade().unwrap();
    let refused = EngineEvent::InputRejected {
        reason: "busy".into(),
    };
    events.send(refused).await.unwrap();
    tokio::time::sleep(Duration::from_millis(100)).await;
    let cursor = h.store.agent(parent).unwrap().unwrap().inbox_cursor;
    let unread = h.store.inbox(parent, cursor).unwrap();
    assert!(
        unread
            .iter()
            .any(|item| format!("{:?}", item.kind).contains("use the left half")),
        "the steered message was skipped: {unread:?}"
    );
    assert!(
        reply.starts_with("Wait ended: the delegated task finished.")
            && reply.contains("What arrived follows as your next input.")
            && !reply.contains("Delivered with this reply:"),
        "{reply}"
    );
    h.runtime.shutdown().await;
}

/// Once its last job finished, nothing is left to keep attached: the held
/// question parks the turn as it would have at once.
#[tokio::test]
async fn a_held_wait_parks_once_its_last_native_job_finishes() {
    let (h, turn, pending) = held_call(
        job_harness().await,
        "ask_user",
        json!({"question": "which half?"}),
    )
    .await;
    turn.events
        .upgrade()
        .unwrap()
        .send(EngineEvent::BackgroundTask {
            id: "build".into(),
            running: false,
        })
        .await
        .unwrap();
    let reply = reply_of(pending).await;
    assert!(
        reply.starts_with("Native background jobs finished while you waited (build)")
            && reply.contains("Question recorded")
            && reply.contains("Your turn ends here"),
        "{reply}"
    );
    let events = turn.events.upgrade().unwrap();
    let result = EngineEvent::ToolResult {
        tool_use_id: "held".into(),
        content: reply,
        is_error: false,
    };
    events.send(result).await.unwrap();
    wait_until_asked_to_stop(&turn.observed, Duration::from_secs(2)).await;
    let parked = wait_for_park(&h, turn.agent).await;
    assert!(
        condition_contains(condition_of(&parked.status), &WaitCondition::Inbox),
        "{:?}",
        parked.status
    );
    h.runtime.shutdown().await;
}
