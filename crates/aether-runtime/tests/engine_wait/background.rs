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

fn one_turn_slot(config: &mut RuntimeConfig) {
    config.max_concurrent_turns = 1;
    config.max_concurrent_engine_turns = Some(1);
}

/// A held delegate waits on its child, so its slot has to go to the child:
/// at one turn slot, holding it would leave the child queued for good.
#[tokio::test]
async fn a_held_delegate_lets_its_child_run_at_a_one_turn_limit() {
    let script = Script {
        background: Some("build".into()),
        ..Default::default()
    };
    let h = harness_configured(Duration::from_millis(100), script, one_turn_slot).await;
    let request = json!({"model": "claude-code/haiku", "effort": "low", "role": "worker",
                         "objective": "first half", "wait": true});
    let (mut h, turn, pending) = held_call(h, "delegate", request).await;
    let child = only_child(&h, turn.agent);
    let child_turn = turn_of(&mut h, child).await;
    let summary = json!({"summary": "left half sorted"});
    let (done, is_error) = call(&h, &child_turn, "complete_task", summary).await;
    assert!(!is_error, "{done}");
    let reply = reply_of(pending).await;
    assert!(
        reply.starts_with("Wait ended: the delegated task finished.")
            && reply.contains("left half sorted"),
        "{reply}"
    );
    assert!(!turn.observed.stopped() && !turn.observed.stdin_closed());
    h.runtime.shutdown().await;
}

/// A person reaches a waiting agent at once; a held delegate is waiting too.
#[tokio::test]
async fn a_user_message_releases_a_held_delegate_while_its_child_runs() {
    let request = json!({"model": "claude-code/haiku", "effort": "low", "role": "worker",
                         "objective": "first half", "wait": true});
    let (mut h, turn, pending) = held_call(job_harness().await, "delegate", request).await;
    let child = only_child(&h, turn.agent);
    let _child_turn = turn_of(&mut h, child).await;
    let change = vec![ContentBlock::text("stop: do the right half instead")];
    h.runtime
        .send_message(turn.agent, change, Actor::user())
        .unwrap();
    let reply = reply_of(pending).await;
    assert!(
        reply.starts_with("Wait ended: message from the user.")
            && reply.contains("do the right half instead")
            && reply.contains(&format!("Delegated to agent {child}")),
        "{reply}"
    );
    let running = h.store.agent(child).unwrap().unwrap();
    assert!(!running.status.is_terminal(), "{:?}", running.status);
    h.runtime.shutdown().await;
}

fn short_check_in(config: &mut RuntimeConfig) {
    config.wait_timeout = Some(Duration::from_millis(1_000));
}

/// `wait_timeout` gives every waiting agent a check-in; a held delegate too.
#[tokio::test]
async fn a_held_delegate_returns_at_the_configured_check_in() {
    let script = Script {
        background: Some("build".into()),
        ..Default::default()
    };
    let h = harness_configured(Duration::from_millis(100), script, short_check_in).await;
    let request = json!({"model": "claude-code/haiku", "effort": "low", "role": "worker",
                         "objective": "first half", "wait": true});
    let (h, turn, pending) = held_call(h, "delegate", request).await;
    let child = only_child(&h, turn.agent);
    let reply = reply_of(pending).await;
    assert!(
        reply.starts_with(
            "Wait ended: the check-in time came; the delegated task is still running."
        ) && reply.contains(&format!("Delegated to agent {child}")),
        "{reply}"
    );
    assert!(!turn.observed.stopped() && !turn.observed.stdin_closed());
    h.runtime.shutdown().await;
}

fn one_turn_and_short_check_in(config: &mut RuntimeConfig) {
    config.max_concurrent_turns = 1;
    config.wait_timeout = Some(Duration::from_millis(1_000));
}

/// A wait's end takes back a turn slot first, so the reply waits while the
/// child holds the only one, and says what ended the wait by then.
#[tokio::test]
async fn a_held_reply_waits_for_a_free_turn_slot() {
    let script = Script {
        background: Some("build".into()),
        ..Default::default()
    };
    let h = harness_configured(
        Duration::from_millis(100),
        script,
        one_turn_and_short_check_in,
    )
    .await;
    let request = json!({"model": "claude-code/haiku", "effort": "low", "role": "worker",
                         "objective": "first half", "wait": true});
    let (mut h, turn, pending) = held_call(h, "delegate", request).await;
    let child = only_child(&h, turn.agent);
    let child_turn = turn_of(&mut h, child).await;
    // Past the check-in, while the child's turn holds the only slot.
    tokio::time::sleep(Duration::from_millis(1_500)).await;
    assert!(!pending.is_finished(), "{:?}", pending.await);
    let summary = json!({"summary": "left half sorted"});
    let (done, is_error) = call(&h, &child_turn, "complete_task", summary).await;
    assert!(!is_error, "{done}");
    let reply = reply_of(pending).await;
    assert!(
        reply.starts_with("Wait ended: the delegated task finished.")
            && reply.contains("left half sorted"),
        "{reply}"
    );
    h.runtime.shutdown().await;
}

/// A deadline reached while another agent holds the engine slot replies
/// only once that slot is free again.
#[tokio::test]
async fn a_native_wait_deadline_replies_once_its_slot_is_back() {
    let script = Script {
        background: Some("build".into()),
        ..Default::default()
    };
    let mut h = harness_configured(Duration::from_millis(100), script, one_turn_slot).await;
    let parent = start_coordinator(&h);
    let turn = turn_of(&mut h, parent).await;
    let runtime = h.runtime.clone();
    let token = turn.token.clone();
    let input = json!({"timeout_seconds": 1});
    let pending = tokio::spawn(async move {
        runtime
            .engine_tool(parent, &token, "wait_for".into(), input)
            .await
    });
    let other = start_coordinator(&h);
    let _other_turn = turn_of(&mut h, other).await;
    tokio::time::sleep(Duration::from_millis(1_500)).await;
    assert!(!pending.is_finished(), "{:?}", pending.await);
    h.runtime
        .cancel_agent(other, Some("stop".into()), Actor::user())
        .unwrap();
    let reply = reply_of(pending).await;
    assert!(reply.contains("deadline"), "{reply}");
    assert!(!turn.observed.stopped() && !turn.observed.cancelled());
    h.runtime.shutdown().await;
}

fn one_turn_slot_short_retake(config: &mut RuntimeConfig) {
    one_turn_slot(config);
    config.resource_wait_timeout = Duration::from_millis(300);
}

/// The slot retake is bounded for a resuming park, but a native wait keeps
/// queueing past that bound instead of going on over both caps.
#[tokio::test]
async fn a_native_wait_keeps_queueing_past_the_retake_bound() {
    let script = Script {
        background: Some("build".into()),
        ..Default::default()
    };
    let mut h = harness_configured(
        Duration::from_millis(100),
        script,
        one_turn_slot_short_retake,
    )
    .await;
    let parent = start_coordinator(&h);
    let turn = turn_of(&mut h, parent).await;
    let runtime = h.runtime.clone();
    let token = turn.token.clone();
    let input = json!({"timeout_seconds": 1});
    let pending = tokio::spawn(async move {
        runtime
            .engine_tool(parent, &token, "wait_for".into(), input)
            .await
    });
    let other = start_coordinator(&h);
    let _other_turn = turn_of(&mut h, other).await;
    // The deadline (1 s) plus several retake bounds (0.3 s each) have passed.
    tokio::time::sleep(Duration::from_millis(2_500)).await;
    assert!(!pending.is_finished(), "{:?}", pending.await);
    h.runtime
        .cancel_agent(other, Some("stop".into()), Actor::user())
        .unwrap();
    let reply = reply_of(pending).await;
    assert!(reply.contains("deadline"), "{reply}");
    assert!(!turn.observed.stopped() && !turn.observed.cancelled());
    h.runtime.shutdown().await;
}

/// Starts a 1 s native `wait_for` in `first`'s turn, lets the agent that
/// `second` starts take the slot it gave away, and checks that the reply
/// waits past the deadline until that agent is cancelled.
async fn wait_reply_queues_behind(mut h: Harness, first: AgentId, second: fn(&Harness) -> AgentId) {
    let turn = turn_of(&mut h, first).await;
    let runtime = h.runtime.clone();
    let token = turn.token.clone();
    let input = json!({"timeout_seconds": 1});
    let pending = tokio::spawn(async move {
        runtime
            .engine_tool(first, &token, "wait_for".into(), input)
            .await
    });
    let other = second(&h);
    let _other_turn = turn_of(&mut h, other).await;
    tokio::time::sleep(Duration::from_millis(2_000)).await;
    assert!(!pending.is_finished(), "{:?}", pending.await);
    h.runtime
        .cancel_agent(other, Some("stop".into()), Actor::user())
        .unwrap();
    let reply = reply_of(pending).await;
    assert!(reply.contains("deadline"), "{reply}");
    assert!(!turn.observed.stopped() && !turn.observed.cancelled());
    h.runtime.shutdown().await;
}

/// Global and engine limits have room; the provider's own cap does not.
#[tokio::test]
async fn a_native_wait_retakes_only_under_its_provider_cap() {
    let script = Script {
        background: Some("build".into()),
        ..Default::default()
    };
    let providers = "[providers.claude-code]\nmax_concurrent = 1\n";
    let h = harness_with_providers(script, providers, |_| {}).await;
    let first = start_coordinator(&h);
    wait_reply_queues_behind(h, first, start_coordinator).await;
}

fn capped_root(h: &Harness) -> AgentId {
    let mut spec = coordinator_spec();
    spec.start = false;
    spec.config.as_mut().expect("config").concurrency = Some(1);
    start_agent(h, spec)
}

fn child_of_first_root(h: &Harness) -> AgentId {
    let roots = h.store.agents(&AgentFilter::default()).expect("agents");
    let root = roots
        .iter()
        .find(|agent| agent.config.concurrency == Some(1))
        .expect("the capped root");
    let mut spec = coordinator_spec();
    spec.parent = Some(root.id);
    spec.config.as_mut().expect("config").effort = Some(aether_core::model::Effort::Low);
    start_agent(h, spec)
}

/// Global and engine limits have room; a supervisor's subtree cap does not.
#[tokio::test]
async fn a_native_wait_retakes_only_under_its_supervisor_cap() {
    let script = Script {
        background: Some("build".into()),
        ..Default::default()
    };
    let h = harness_configured(Duration::from_millis(100), script, |_| {}).await;
    capped_root(&h);
    let first = child_of_first_root(&h);
    wait_reply_queues_behind(h, first, child_of_first_root).await;
}
