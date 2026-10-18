use super::*;

#[tokio::test]
async fn a_redundant_wait_cannot_disable_or_extend_a_parked_deadline() {
    for repeated in [
        json!({}),
        json!({"timeout_seconds": 3600}),
        json!({"timeout_seconds": 7200}),
    ] {
        let mut h = harness(Duration::from_secs(30)).await;
        let parent = start_coordinator(&h);
        let _child = idle_child(&h, parent);
        let turn = turn_of(&mut h, parent).await;
        let (reply, is_error) = call(&h, &turn, "wait_for", json!({"timeout_seconds": 3600})).await;
        assert!(!is_error, "{reply}");
        let (reply, is_error) = call(&h, &turn, "wait_for", repeated).await;
        assert!(is_error && reply.contains(TURN_IS_OVER), "{reply}");
        let _ = turn.end.send(());
        let parked = wait_for_park(&h, parent).await;
        assert!(
            matches!(condition_of(&parked.status), WaitCondition::Any { of }
        if of.len() == 2 && of.iter().any(|part| matches!(part, WaitCondition::Deadline { .. })))
        );
        h.runtime.shutdown().await;
    }
}

#[tokio::test]
async fn a_parked_native_turn_cannot_cancel_a_child() {
    let mut h = harness(Duration::from_secs(30)).await;
    let parent = start_coordinator(&h);
    let child = idle_child(&h, parent);
    let turn = turn_of(&mut h, parent).await;
    let (reply, is_error) = call(&h, &turn, "wait_for", json!({"timeout_seconds": 3600})).await;
    assert!(!is_error, "{reply}");
    let (reply, is_error) = call(
        &h,
        &turn,
        "cancel_agent",
        json!({"agent": child, "reason": "pretending the deadline already passed"}),
    )
    .await;
    assert!(is_error && reply.contains(TURN_IS_OVER), "{reply}");
    assert!(!h.store.agent(child).unwrap().unwrap().status.is_terminal());
    let _ = turn.end.send(());
    wait_for_park(&h, parent).await;
    assert!(!events(h.store.as_ref()).iter().any(|event| {
        event.agent_id == Some(child) && matches!(event.kind, EventKind::AgentCancelled { .. })
    }));
    h.runtime.shutdown().await;
}

/// The reported bug: `wait_for` only recorded the wait, so the engine kept
/// its turn - polling `list_tasks` every few seconds and paying for the whole
/// context each time - instead of parking until its child finished.
#[tokio::test]
async fn wait_for_ends_the_engine_turn_and_the_agent_is_woken_once() {
    let mut h = harness_full(
        Duration::from_secs(30),
        OnKill::Report(vec![cancelled_with(Usage::default())]),
        Script {
            linger_after_stop: Some(Duration::from_secs(30)),
            ..Default::default()
        },
    )
    .await;
    let coordinator = start_coordinator(&h);
    let first = turn_of(&mut h, coordinator).await;
    assert_eq!(
        first.resumed, None,
        "the first turn has no session to resume"
    );

    let (reply, is_error) = call(
        &h,
        &first,
        "delegate",
        json!({"model":"claude-code/haiku", "effort":"low", "role": "worker", "objective": "do the work", "wait": false}),
    )
    .await;
    assert!(!is_error, "{reply}");
    let task = task_of(&reply);
    let child = h
        .store
        .agents(&AgentFilter {
            parent: Some(coordinator),
            ..Default::default()
        })
        .expect("agents")
        .remove(0)
        .id;

    let (reply, is_error) = call(&h, &first, "wait_for", json!({"tasks": [task]})).await;
    assert!(!is_error, "{reply}");
    assert_eq!(
        reply,
        "Waiting. Your turn ends here - stop now and make no further tool calls. \
         You will be woken with the results when the condition is met.",
        "the reply must not read as an ordinary result an engine can follow up on"
    );
    // The graceful ask, before any deadline: stdin closed and the engine's
    // own stop verb sent.
    assert!(first.observed.stdin_closed(), "the turn's stdin stays open");
    assert!(
        first.observed.stopped(),
        "the engine was never asked to stop"
    );

    // What the live agent did next, every few seconds, for a whole turn.
    let (reply, is_error) = call(&h, &first, "list_tasks", json!({})).await;
    assert!(!is_error, "a read-only sibling call was refused: {reply}");
    assert!(
        reply.contains(TURN_IS_OVER),
        "a call served after the park must still carry the stop: {reply}"
    );

    let _ = first.end.send(());
    let parked = wait_for_park(&h, coordinator).await;
    assert_eq!(waited_on(&parked.status), vec![task]);
    assert!(
        !first.observed.cancelled(),
        "an engine that ends its own turn must not be killed"
    );
    assert_eq!(
        custom(h.store.as_ref(), coordinator, "engine.parked").expect("the wind-down is recorded")
            ["stopped_by"],
        json!("engine")
    );

    let worker = turn_of(&mut h, child).await;
    let settled = wait_until(h.store.as_ref(), coordinator, |a| a.stats.turns == 1).await;
    let invocations_before = h
        .store
        .invocations_for_agent(coordinator)
        .expect("parent invocations")
        .len();
    // Scheduler activity is free of model work while the child's result is
    // outstanding. Exercise repeated passes instead of relying on an idle sleep.
    for _ in 0..64 {
        aether_runtime::scheduler::tick(&h.runtime).expect("scheduler tick");
        tokio::task::yield_now().await;
    }
    let still_waiting = h.store.agent(coordinator).expect("agent").expect("exists");
    assert_eq!(waited_on(&still_waiting.status), vec![task]);
    assert_eq!(still_waiting.stats.turns, settled.stats.turns);
    assert_eq!(still_waiting.stats.usage, settled.stats.usage);
    assert_eq!(
        h.store
            .invocations_for_agent(coordinator)
            .expect("parent invocations")
            .len(),
        invocations_before,
        "scheduler passes must not invoke a waiting parent's model"
    );
    assert_eq!(turns_started(h.store.as_ref(), coordinator), 1);
    let (reply, is_error) = call(&h, &worker, "complete_task", json!({"summary": "worked"})).await;
    assert!(!is_error, "{reply}");
    // Completing the task does not prove the native process has stopped.
    // Hold its terminal event back while the scheduler sees the durable result.
    for _ in 0..64 {
        aether_runtime::scheduler::tick(&h.runtime).expect("scheduler tick during wind-down");
        tokio::task::yield_now().await;
    }
    let winding_down = h.store.agent(coordinator).expect("agent").expect("exists");
    assert_eq!(
        waited_on(&winding_down.status),
        vec![task],
        "a task result must not release its waiter before the producing turn stops"
    );
    assert_eq!(turns_started(h.store.as_ref(), coordinator), 1);
    let _ = worker.end.send(());

    let woken = turn_of(&mut h, coordinator).await;
    let session = h
        .store
        .agent(coordinator)
        .expect("agent")
        .expect("exists")
        .engine_session
        .expect("the parked turn kept its engine session");
    assert_eq!(
        woken.resumed.as_deref(),
        Some(session.as_str()),
        "the woken turn started a fresh session instead of resuming the parked one"
    );
    let _ = woken.end.send(());
    wait_until(h.store.as_ref(), coordinator, |a| a.stats.turns >= 2).await;

    // One turn before the wait and one after it, with the child's completion
    // between them: no polling turn in the middle, asserted from the log
    // rather than from a sleep.
    let starts = seqs(h.store.as_ref(), |e| {
        e.agent_id == Some(coordinator) && matches!(e.kind, EventKind::AgentStarted { .. })
    });
    let completed = seqs(h.store.as_ref(), |e| {
        e.task_id == Some(task) && matches!(e.kind, EventKind::TaskCompleted { .. })
    });
    assert_eq!(starts.len(), 2, "the coordinator polled instead of parking");
    assert_eq!(completed.len(), 1);
    assert!(
        starts[0] < completed[0] && completed[0] < starts[1],
        "the second turn must be the wake, not a poll: starts {starts:?} completion {completed:?}"
    );
    h.runtime.shutdown().await;
}

/// The engine is asked to stop, not trusted to. One that keeps going past the
/// grace is stopped, and the wait it already recorded still stands.
#[tokio::test]
async fn an_engine_that_ignores_the_park_is_stopped_and_the_wait_stands() {
    let mut h = harness(Duration::from_millis(200)).await;
    let coordinator = start_coordinator(&h);
    let first = turn_of(&mut h, coordinator).await;

    let (reply, is_error) = call(&h, &first, "wait_for", json!({"timeout_seconds": 3600})).await;
    assert!(!is_error, "{reply}");

    // `first.end` is never signalled: this engine never ends its own turn.
    let parked = wait_for_park(&h, coordinator).await;
    assert!(
        first.observed.cancelled(),
        "the grace expired and the process was left running"
    );
    assert!(
        matches!(
            condition_of(&parked.status),
            WaitCondition::Any { of }
                if matches!(of.as_slice(), [WaitCondition::Inbox, WaitCondition::Deadline { .. }])
        ),
        "the recorded wait is not the one wait_for asked for: {:?}",
        parked.status
    );
    assert_eq!(turns_started(h.store.as_ref(), coordinator), 1);
    let park = custom(h.store.as_ref(), coordinator, "engine.parked")
        .expect("a forced stop leaves a trace");
    assert_eq!(park["stopped_by"], json!("runtime"));
    assert!(
        custom(h.store.as_ref(), coordinator, "engine.interrupted").is_none(),
        "a stop we asked for is not an engine interruption"
    );
    h.runtime.shutdown().await;
}

/// A killed CLI often says nothing at all. The turn still ends on the wait
/// the harness recorded, not on the broken stream.
#[tokio::test]
async fn a_stopped_engine_that_never_reports_still_parks() {
    let mut h = harness_with(Duration::from_millis(200), OnKill::Silent).await;
    let coordinator = start_coordinator(&h);
    let first = turn_of(&mut h, coordinator).await;

    let (reply, is_error) = call(&h, &first, "wait_for", json!({"timeout_seconds": 3600})).await;
    assert!(!is_error, "{reply}");

    let parked = wait_for_park(&h, coordinator).await;
    assert!(first.observed.cancelled());
    assert!(
        matches!(parked.status, AgentStatus::Waiting { .. }),
        "a stop we asked for read as an engine failure: {:?}",
        parked.status
    );
    h.runtime.shutdown().await;
}

/// Engines emit a parallel tool batch as separate bridge calls, so the
/// siblings of the parking call arrive after it. Waits and the turn's own
/// first question merge into the park; a `remember` that would deliver an
/// effect and a genuine second decision are refused.
#[tokio::test]
async fn a_parallel_batch_survives_the_call_that_parks_it() {
    let mut h = harness(Duration::from_secs(30)).await;
    let coordinator = start_coordinator(&h);
    let first = turn_of(&mut h, coordinator).await;

    let (reply, is_error) = call(
        &h,
        &first,
        "delegate",
        json!({"model":"claude-code/haiku", "effort":"low", "role": "worker", "objective": "first half", "wait": true}),
    )
    .await;
    assert!(!is_error, "{reply}");
    let one = task_of(&reply);

    let (reply, is_error) = call(
        &h,
        &first,
        "delegate",
        json!({"model":"claude-code/haiku", "effort":"low", "role": "worker", "objective": "second half", "wait": true}),
    )
    .await;
    assert!(!is_error, "the second child of the batch was lost: {reply}");
    let two = task_of(&reply);
    assert!(reply.contains(TURN_IS_OVER), "{reply}");

    let (reply, is_error) = call(
        &h,
        &first,
        "remember",
        json!({"text": "the batch note must not outlive the park", "kind": "lesson"}),
    )
    .await;
    assert!(is_error, "a note was stored after the turn ended: {reply}");
    assert!(reply.starts_with(TURN_IS_OVER), "{reply}");

    // The turn's *first* question is a merging wait too, exactly like
    // the `delegate` above - it folds into the wait the batch already
    // committed rather than being refused as a second decision.
    let (reply, is_error) = call(&h, &first, "ask_user", json!({"question": "now what?"})).await;
    assert!(!is_error, "the first question was refused: {reply}");
    assert!(reply.contains("Question recorded"), "{reply}");

    // A *second* decision is the one thing that cannot run: it would
    // overwrite the wait the turn already committed.
    let (reply, is_error) = call(&h, &first, "ask_user", json!({"question": "and then?"})).await;
    assert!(
        is_error,
        "a second decision was allowed to overwrite: {reply}"
    );
    assert!(reply.starts_with(TURN_IS_OVER), "{reply}");

    let _ = first.end.send(());
    let parked = wait_for_park(&h, coordinator).await;
    assert_eq!(
        waited_on(&parked.status),
        vec![one, two],
        "the merged wait must cover both children"
    );
    assert_eq!(
        h.store
            .agents(&AgentFilter {
                parent: Some(coordinator),
                ..Default::default()
            })
            .expect("agents")
            .len(),
        2,
        "a coordinator delegating twice in one batch spawned one child"
    );
    assert_eq!(
        h.store
            .memories(&MemoryQuery {
                exact_text: Some("the batch note must not outlive the park".into()),
                ..Default::default()
            })
            .expect("memories")
            .len(),
        0
    );
    assert_eq!(
        events(h.store.as_ref())
            .iter()
            .filter(|e| e.type_name() == "question.asked")
            .count(),
        1,
        "only the first question is ever recorded, never the refused second one"
    );
    let park = custom(h.store.as_ref(), coordinator, "engine.parked").expect("recorded");
    assert_eq!(park["refused_calls"], json!(2));
    assert_eq!(park["refused_first"], json!(["remember", "ask_user"]));
    h.runtime.shutdown().await;
}

/// Terminal tools leave a bounded reporting window, never an unbounded
/// native turn. An engine that does not finish its reporting is stopped.
#[tokio::test]
async fn a_terminal_harness_action_stops_an_unresponsive_engine_after_its_reporting_window() {
    for (tool, input) in [
        ("complete_task", json!({"summary": "worked"})),
        ("fail_task", json!({"error": "could not"})),
    ] {
        let mut h = harness(Duration::from_millis(200)).await;
        let parent = start_coordinator(&h);
        let parent_turn = turn_of(&mut h, parent).await;
        let (reply, is_error) = call(
            &h,
            &parent_turn,
            "delegate",
            json!({"model":"claude-code/haiku", "effort":"low", "role": "worker", "objective": "do the work", "wait": false}),
        )
        .await;
        assert!(!is_error, "{reply}");
        let child = h
            .store
            .agents(&AgentFilter {
                parent: Some(parent),
                ..Default::default()
            })
            .expect("agents")
            .remove(0)
            .id;
        let _ = parent_turn.end.send(());

        let worker = turn_of(&mut h, child).await;
        let (reply, is_error) = call(&h, &worker, tool, input).await;
        assert!(!is_error, "{reply}");
        assert!(
            reply.contains("Your turn ends here"),
            "{tool} must tell the engine to stop: {reply}"
        );

        let child_row =
            wait_until(h.store.as_ref(), child, |a| a.status.kind().is_terminal()).await;
        assert!(
            worker.observed.cancelled(),
            "{tool} left the engine running on a turn that was over"
        );
        assert_eq!(
            worker.observed.stop_grace(),
            None,
            "terminal reporting is not interrupted before its deadline"
        );
        assert_eq!(
            child_row.status.kind() == AgentStatusKind::Completed,
            tool == "complete_task",
            "{tool} recorded {:?}",
            child_row.status
        );
        h.runtime.shutdown().await;
    }
}

/// A session the engine reports "lost" because we killed its process must
/// not restart the turn with a fresh one: the wait is already recorded and a
/// retry would run the whole turn again, children and all.
#[tokio::test]
async fn a_parked_turn_is_not_retried_when_the_stop_loses_the_session() {
    let lost = EngineOutcome {
        is_error: true,
        session_lost: true,
        error: Some("cancelled".into()),
        ..Default::default()
    };
    let mut h = harness_with(Duration::from_millis(200), OnKill::Report(vec![lost])).await;
    let coordinator = start_coordinator(&h);
    // The retry only applies to a turn that asked to resume a session, so the
    // parked turn has to be the second one.
    let first = turn_of(&mut h, coordinator).await;
    let _ = first.end.send(());
    wait_for_park(&h, coordinator).await;
    h.runtime
        .send_message(
            coordinator,
            vec![ContentBlock::text("carry on")],
            Actor::user(),
        )
        .expect("message");

    let second = turn_of(&mut h, coordinator).await;
    assert!(
        second.resumed.is_some(),
        "the second turn resumes a session"
    );
    second
        .events
        .upgrade()
        .unwrap()
        .send(EngineEvent::Assistant {
            content: vec![ContentBlock::text("Continuing while the child works.")],
            usage: None,
            model: None,
        })
        .await
        .unwrap();
    wait_for_event(h.store.as_ref(), |event| matches!(&event.kind,
        EventKind::AgentMessage { content, .. }
            if content.iter().any(|block| matches!(block, ContentBlock::Text { text } if text == "Continuing while the child works."))
    )).await;
    let (reply, is_error) = call(&h, &second, "wait_for", json!({"timeout_seconds": 3600})).await;
    assert!(!is_error, "{reply}");

    let parked = wait_for_park(&h, coordinator).await;
    assert!(second.observed.cancelled());
    assert!(matches!(parked.status, AgentStatus::Waiting { .. }));
    assert!(
        h.started.try_recv().is_err(),
        "the parked turn was run again with a fresh session"
    );
    assert_eq!(turns_started(h.store.as_ref(), coordinator), 2);
    h.runtime.shutdown().await;
}

/// Nothing a person sends may reach a turn that is over. The engine here is
/// blocked on a question, so without the guard the reply would be handed
/// straight to a process about to be killed - consumed, and lost with it.
#[tokio::test]
async fn a_message_during_the_park_stays_in_the_inbox() {
    let mut h = harness_full(
        Duration::from_millis(400),
        OnKill::Report(vec![cancelled_with(Usage::default())]),
        Script {
            question: true,
            ..Default::default()
        },
    )
    .await;
    let coordinator = start_coordinator(&h);
    let first = turn_of(&mut h, coordinator).await;
    // The question has to be in hand before the park, or the guard under test
    // is never reached.
    wait_for_event(h.store.as_ref(), |e| {
        matches!(&e.kind, EventKind::AgentMessage { content, .. }
            if content.iter().any(|b| matches!(b, ContentBlock::Text { text } if text == QUESTION)))
    })
    .await;

    let (reply, is_error) = call(&h, &first, "wait_for", json!({"timeout_seconds": 3600})).await;
    assert!(!is_error, "{reply}");

    h.runtime
        .send_message(
            coordinator,
            vec![ContentBlock::text(MESSAGE)],
            Actor::user(),
        )
        .expect("message");

    wait_until_stopped(&first.observed, Duration::from_secs(2)).await;
    assert!(
        first.observed.decided().is_empty(),
        "the person's reply was handed to a process being killed: {:?}",
        first.observed.decided()
    );
    // The delivery wakes the waiting agent, and the message has to be there
    // for that turn rather than consumed by the one that was already over.
    let woken = turn_of(&mut h, coordinator).await;
    assert!(
        woken.prompt.contains(MESSAGE),
        "the parked turn swallowed the message: {}",
        woken.prompt
    );
    let _ = woken.end.send(());
    h.runtime.shutdown().await;
}

/// Between the stop and the process actually going, the bearer token is
/// still valid and the bridge may still be reachable. Nothing served in that
/// window could be reported to the engine anyway, so a call that would commit
/// - a `remember`, a `send_message` - must not run at all.
#[tokio::test]
async fn a_bridge_call_after_the_stop_commits_nothing() {
    let note = "written for a process being killed";
    let mut h = harness_full(
        Duration::from_millis(200),
        OnKill::Report(vec![cancelled_with(Usage::default())]),
        Script {
            linger_after_stop: Some(Duration::from_secs(3)),
            ..Default::default()
        },
    )
    .await;
    let coordinator = start_coordinator(&h);
    let first = turn_of(&mut h, coordinator).await;

    let (reply, is_error) = call(&h, &first, "wait_for", json!({"timeout_seconds": 3600})).await;
    assert!(!is_error, "{reply}");
    wait_until_stopped(&first.observed, Duration::from_secs(2)).await;

    let (reply, is_error) = call(
        &h,
        &first,
        "remember",
        json!({"text": note, "kind": "lesson"}),
    )
    .await;
    assert!(is_error, "a call ran for a process being killed: {reply}");
    assert!(reply.starts_with(TURN_IS_OVER), "{reply}");
    assert!(
        h.store
            .memories(&MemoryQuery {
                exact_text: Some(note.into()),
                ..Default::default()
            })
            .expect("memories")
            .is_empty(),
        "the call committed anyway"
    );

    let parked = wait_for_park(&h, coordinator).await;
    assert!(matches!(parked.status, AgentStatus::Waiting { .. }));
    h.runtime.shutdown().await;
}
