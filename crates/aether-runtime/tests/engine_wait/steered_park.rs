//! A message the engine takes in only after a harness tool ended its turn was
//! never acted on: it stays queued and wakes the next turn (#329).
use super::*;

const TASK: &str = "here is your task: transfer the ledger";

/// A turn whose engine holds [`TASK`] unread. It is delivered after context
/// assembly, so the drive loop's first steer hands it over before the model acts.
async fn steered_but_unread(
    sender: Actor,
) -> (Harness, AgentId, TurnStarted, Arc<tokio::sync::Semaphore>) {
    let input_gate = Arc::new(tokio::sync::Semaphore::new(0));
    let run_gate = Arc::new(tokio::sync::Semaphore::new(0));
    let mut h = harness_full(
        Duration::from_secs(30),
        OnKill::Report(vec![cancelled_with(Usage::default())]),
        Script {
            input_gate: Some(input_gate.clone()),
            run_gate: Some(run_gate.clone()),
            ..Default::default()
        },
    )
    .await;
    let agent = start_coordinator(&h);
    let turn = turn_of(&mut h, agent).await;
    let cursor = h.store.agent(agent).unwrap().unwrap().inbox_cursor;
    h.runtime
        .send_message(agent, vec![ContentBlock::text(TASK)], sender)
        .expect("task message");
    run_gate.add_permits(1);
    // Served by the drive loop, so its first steer has already run.
    let (reply, is_error) = raw_call(&h, &turn, "list_agents", json!({})).await;
    assert!(!is_error, "{reply}");
    assert_eq!(
        h.store.agent(agent).unwrap().unwrap().inbox_cursor,
        cursor,
        "handed over but not taken in, so not consumed"
    );
    assert!(turn.observed.steered.lock().unwrap().is_empty());
    (h, agent, turn, input_gate)
}

/// The model's visible text before it waits: it settles any reply owed to a
/// person, as the model in #329 did before calling `wait_for`.
async fn say_before_waiting(h: &Harness, turn: &TurnStarted, text: &str) {
    let events = turn.events.upgrade().expect("the turn is live");
    events
        .send(EngineEvent::Assistant {
            content: vec![ContentBlock::text(text)],
            usage: None,
            model: None,
        })
        .await
        .expect("assistant text");
    wait_for_event(h.store.as_ref(), |e| {
        matches!(&e.kind, EventKind::AgentMessage { content, .. }
            if content.iter().any(|b| matches!(b, ContentBlock::Text { text: t } if t == text)))
    })
    .await;
}

/// Seqs of this agent's events matching `pred`, in log order.
fn agent_seqs(h: &Harness, agent: AgentId, pred: impl Fn(&EventKind) -> bool) -> Vec<u64> {
    seqs(h.store.as_ref(), |e| {
        e.agent_id == Some(agent) && pred(&e.kind)
    })
}

/// The reported race: the engine takes the steered message in after the
/// turn-ending reply, and is then stopped before the model can answer it.
#[tokio::test]
async fn a_message_taken_in_after_the_turn_ended_wakes_the_next_turn() {
    let parking = [
        ("wait_for", json!({"timeout_seconds": 3600})),
        ("ask_user", json!({"question": "anything for me?"})),
    ];
    for (tool, input) in parking {
        let (mut h, agent, first, input_gate) = steered_but_unread(Actor::user()).await;
        let task_seq = h.store.inbox_last_seq(agent).unwrap();
        say_before_waiting(&h, &first, "Nothing to do yet; waiting.").await;
        let (reply, is_error) = call(&h, &first, tool, input).await;
        assert!(!is_error, "{tool}: {reply}");
        assert!(reply.contains("Your turn ends here"), "{tool}: {reply}");

        // Claude Code folds the queued message in after the tool result.
        input_gate.add_permits(1);
        wait_until_steered(&first.observed).await;
        let _ = first.end.send(());

        // Checked at the first turn's end, so the bug fails fast instead of
        // waiting out a turn that never starts.
        wait_for_event(h.store.as_ref(), |e| {
            e.agent_id == Some(agent) && matches!(e.kind, EventKind::AgentWaiting { .. })
        })
        .await;
        let ended = agent_seqs(&h, agent, |k| matches!(k, EventKind::AgentWaiting { .. }))[0];
        let consumed = agent_seqs(
            &h,
            agent,
            |k| matches!(k, EventKind::InboxConsumed { up_to, .. } if *up_to >= task_seq),
        );
        assert!(
            consumed.iter().all(|seq| *seq > ended),
            "{tool}: consumed by the turn that had ended: {consumed:?} vs end {ended}"
        );
        let owed = agent_seqs(&h, agent, |k| {
            matches!(k, EventKind::Custom { name, payload }
                if name == "reply.owed" && payload["inbox_seq"].as_u64() >= Some(task_seq))
        });
        assert!(
            owed.iter().all(|seq| *seq > ended),
            "{tool}: the ended turn owed a reply it could not give: {owed:?}"
        );

        let woken = turn_of(&mut h, agent).await;
        assert!(
            woken.prompt.contains(TASK),
            "{tool}: the ended turn swallowed the message: {}",
            woken.prompt
        );
        let _ = woken.end.send(());
        h.runtime.shutdown().await;
    }
}

/// A message sent after the turn-ending reply is never handed over at all:
/// it stays in the inbox and wakes the parked agent.
#[tokio::test]
async fn a_message_after_the_turn_ending_reply_stays_queued_and_wakes_the_agent() {
    let mut h = harness(Duration::from_secs(30)).await;
    let agent = start_coordinator(&h);
    let first = turn_of(&mut h, agent).await;
    let (reply, is_error) = call(&h, &first, "wait_for", json!({"timeout_seconds": 3600})).await;
    assert!(!is_error, "{reply}");
    let cursor = h.store.agent(agent).unwrap().unwrap().inbox_cursor;
    h.runtime
        .send_message(agent, vec![ContentBlock::text(TASK)], Actor::System)
        .expect("task message");
    let _ = first.end.send(());

    let woken = turn_of(&mut h, agent).await;
    assert!(woken.prompt.contains(TASK), "{}", woken.prompt);
    assert!(first.observed.steered.lock().unwrap().is_empty());
    let consumed = agent_seqs(&h, agent, |k| matches!(k, EventKind::InboxConsumed { .. }));
    assert!(
        h.store.agent(agent).unwrap().unwrap().inbox_cursor > cursor && !consumed.is_empty(),
        "the woken turn consumes it"
    );
    let _ = woken.end.send(());
    h.runtime.shutdown().await;
}

/// The parked engine asks a question: answering it from the inbox would
/// consume a message for a process that is being stopped.
#[tokio::test]
async fn a_question_from_a_parked_engine_is_refused_and_the_message_kept() {
    let mut h = harness(Duration::from_secs(30)).await;
    let agent = start_coordinator(&h);
    let first = turn_of(&mut h, agent).await;
    let (reply, is_error) = call(&h, &first, "wait_for", json!({"timeout_seconds": 3600})).await;
    assert!(!is_error, "{reply}");
    let cursor = h.store.agent(agent).unwrap().unwrap().inbox_cursor;
    h.runtime
        .send_message(agent, vec![ContentBlock::text(MESSAGE)], Actor::user())
        .expect("message");
    first
        .events
        .upgrade()
        .expect("the turn is live")
        .send(EngineEvent::Question {
            request_id: "after-the-park".into(),
            prompt: QUESTION.into(),
        })
        .await
        .expect("question");
    let decided = wait_until_decided(&first.observed).await;
    assert_eq!(decided.request_id, "after-the-park");
    assert!(
        !decided.allow && decided.answer.is_none(),
        "answered from the inbox: {decided:?}"
    );
    assert!(
        decided
            .message
            .as_deref()
            .is_some_and(|m| m.contains(TURN_IS_OVER))
    );
    assert_eq!(h.store.agent(agent).unwrap().unwrap().inbox_cursor, cursor);
    let _ = first.end.send(());

    let woken = turn_of(&mut h, agent).await;
    assert!(woken.prompt.contains(MESSAGE), "{}", woken.prompt);
    let _ = woken.end.send(());
    h.runtime.shutdown().await;
}

/// With native jobs alive the wait keeps the turn, so the steered message is
/// news: the wait ends at once and the model reads the message in this turn.
#[tokio::test]
async fn a_native_wait_ends_on_a_steered_message_and_the_turn_takes_it() {
    let input_gate = Arc::new(tokio::sync::Semaphore::new(0));
    let mut h = harness_full(
        Duration::from_secs(30),
        OnKill::Report(vec![cancelled_with(Usage::default())]),
        Script {
            input_gate: Some(input_gate.clone()),
            background: Some("build".into()),
            ..Default::default()
        },
    )
    .await;
    let agent = start_coordinator(&h);
    let turn = turn_of(&mut h, agent).await;
    h.runtime
        .send_message(agent, vec![ContentBlock::text(TASK)], Actor::System)
        .expect("task message");
    wait_for_event(h.store.as_ref(), |e| {
        e.agent_id == Some(agent) && matches!(e.kind, EventKind::InboxDelivered { .. })
    })
    .await;

    let (reply, is_error) = raw_call(&h, &turn, "wait_for", json!({})).await;
    assert!(!is_error, "{reply}");
    assert!(reply.starts_with("Wait ended:"), "{reply}");
    assert!(!reply.contains("Your turn ends here"), "{reply}");
    assert!(!turn.observed.stopped() && !turn.observed.stdin_closed());

    input_gate.add_permits(1);
    wait_until_steered(&turn.observed).await;
    let last = h.store.inbox_last_seq(agent).unwrap();
    wait_until(h.store.as_ref(), agent, |a| a.inbox_cursor == last).await;
    assert_eq!(
        turns_started(h.store.as_ref(), agent),
        1,
        "taken in this turn"
    );
    let _ = turn.end.send(());
    h.runtime.shutdown().await;
}

async fn wait_until_steered(observed: &Observed) {
    let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
    while observed.steered.lock().unwrap().is_empty() {
        assert!(
            tokio::time::Instant::now() < deadline,
            "the engine never took the message in"
        );
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
}

async fn wait_until_decided(observed: &Observed) -> PermissionDecision {
    let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
    loop {
        if let Some(first) = observed.decided().into_iter().next() {
            return first;
        }
        assert!(
            tokio::time::Instant::now() < deadline,
            "the question was never answered"
        );
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
}
