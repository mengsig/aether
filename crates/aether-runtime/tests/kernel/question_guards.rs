//! question guards scenarios and fixtures.
use super::*;

/// The non-long-lived case still ends the agent, so `fail_task` is refused
/// the same way `complete_task` is.
#[tokio::test]
async fn failing_a_task_after_asking_in_the_same_turn_is_refused() {
    let h = harness(vec![ScriptedClient::tool_calls(
        &[
            ("ask_user", json!({"question": "which colour?"})),
            ("fail_task", json!({"error": "could not paint it"})),
        ],
        None,
    )])
    .await;
    let (agent, task) = h
        .runtime
        .create_agent(spec("paint it"), Actor::user())
        .expect("create");
    let task = task.expect("task");

    let events = wait_until_events(h.store.as_ref(), agent.id, |events| {
        events
            .iter()
            .filter(|e| e.type_name() == "tool.succeeded")
            .count()
            >= 2
    })
    .await;

    let outcome = settled_outcome(&events, "fail_task");
    assert!(
        outcome.is_error,
        "failing while a question is outstanding must be refused: {outcome:?}"
    );
    assert!(
        outcome.content.contains("which colour?"),
        "the refusal names the question it is protecting: {outcome:?}"
    );
    assert!(
        !events.iter().any(|e| e.type_name() == "task.failed"),
        "the task must not have ended under the open question"
    );
    assert_eq!(
        h.store
            .task(task.id)
            .expect("task")
            .expect("exists")
            .status
            .kind(),
        TaskStatusKind::Active,
        "the task is still the one the answer will finish"
    );
}

/// A second `ask_user` in the same batch is never refused: it is its own
/// question, recorded under its own seq beside the first.
#[tokio::test]
async fn asking_twice_in_the_same_turn_records_two_questions() {
    let h = harness(vec![ScriptedClient::tool_calls(
        &[
            ("ask_user", json!({"question": "which colour?"})),
            ("ask_user", json!({"question": "which shade?"})),
        ],
        None,
    )])
    .await;
    let (agent, _) = h
        .runtime
        .create_agent(spec("paint it"), Actor::user())
        .expect("create");

    let events = wait_until_events(h.store.as_ref(), agent.id, |events| {
        events
            .iter()
            .filter(|e| e.type_name() == "tool.succeeded")
            .count()
            >= 2
    })
    .await;

    let outcomes: Vec<_> = events
        .iter()
        .filter_map(|e| match &e.kind {
            EventKind::ToolSucceeded { outcome, .. } => Some(outcome),
            _ => None,
        })
        .collect();
    assert!(
        outcomes.iter().all(|o| !o.is_error),
        "neither question is refused: {outcomes:?}"
    );
    h.wait_until(agent.id, |s| matches!(s, AgentStatus::Waiting { .. }))
        .await;
    let pending = aether_runtime::pending_questions(h.store.as_ref(), agent.id).expect("pending");
    let texts: Vec<&str> = pending
        .iter()
        .filter_map(|e| match &e.kind {
            EventKind::QuestionAsked { question } => Some(question.as_str()),
            _ => None,
        })
        .collect();
    assert_eq!(
        texts,
        ["which colour?", "which shade?"],
        "each is its own question"
    );
}

/// The mirror image: a question after the turn settled its task is still
/// recorded, and the asker stays alive to hear its answer.
#[tokio::test]
async fn asking_after_completing_in_the_same_turn_keeps_the_asker_for_the_answer() {
    let h = harness(vec![ScriptedClient::tool_calls(
        &[
            ("complete_task", json!({"summary": "painted"})),
            ("ask_user", json!({"question": "which colour?"})),
        ],
        None,
    )])
    .await;
    let (agent, _) = h
        .runtime
        .create_agent(spec("paint it"), Actor::user())
        .expect("create");

    let parked = h
        .wait_until(agent.id, |s| matches!(s, AgentStatus::Waiting { .. }))
        .await;
    let AgentStatus::Waiting { condition } = &parked else {
        unreachable!("{parked:?}")
    };
    assert!(
        waits_on_inbox(condition),
        "it waits for the answer: {condition:?}"
    );
    let events = h
        .store
        .events(0, 10_000, &EventFilter::for_agent(agent.id))
        .expect("events");
    let outcome = settled_outcome(&events, "ask_user");
    assert!(!outcome.is_error, "the question is recorded: {outcome:?}");
    assert!(
        outcome.content.contains("already settled"),
        "the reply says the task stands: {outcome:?}"
    );
    assert!(
        events.iter().any(|e| e.type_name() == "task.completed"),
        "the completion the agent decided on still stands"
    );
    let pending = aether_runtime::pending_questions(h.store.as_ref(), agent.id).expect("pending");
    assert_eq!(pending.len(), 1, "the question waits for its answer");
}

/// A question in a batch that also queues an approval is recorded in either
/// order. The approval wait is never interrupted by the reply, so the reply
/// says the answer is read once the approval is settled.
#[tokio::test]
async fn asking_beside_a_call_awaiting_approval_records_the_question() {
    let ask = ("ask_user", json!({"question": "which colour?"}));
    let call = ("call_outside", json!({}));
    for batch in [[ask.clone(), call.clone()], [call, ask]] {
        let h = harness_with_tools(
            vec![ScriptedClient::tool_calls(&batch, None)],
            |_| {},
            |_| {},
            |tools| tools.register(Arc::new(CallOutside)),
        )
        .await;
        let (agent, _) = h
            .runtime
            .create_agent(
                CreateAgentSpec {
                    config: Some(AgentConfig::default()),
                    approval: Some(ApprovalPolicy::default()),
                    ..spec("paint it")
                },
                Actor::user(),
            )
            .expect("create");

        let parked = h
            .wait_until(agent.id, |s| matches!(s, AgentStatus::Waiting { .. }))
            .await;
        let AgentStatus::Waiting { condition } = &parked else {
            unreachable!("{parked:?}")
        };
        assert!(
            condition.involves_approval(),
            "the turn waits for the approval it queued: {condition:?}"
        );
        let events = h
            .store
            .events(0, 10_000, &EventFilter::for_agent(agent.id))
            .expect("events");
        let outcome = settled_outcome(&events, "ask_user");
        assert!(!outcome.is_error, "the question is recorded: {outcome:?}");
        assert!(
            outcome.content.contains("approval"),
            "the reply says what comes first: {outcome:?}"
        );
        let pending =
            aether_runtime::pending_questions(h.store.as_ref(), agent.id).expect("pending");
        assert_eq!(pending.len(), 1, "the question waits for its answer");
    }
}

/// The question's own `Inbox` wait merges into a wait the same batch
/// already recorded, so a `delegate` the agent is waiting on is not dropped.
#[tokio::test]
async fn asking_after_delegating_keeps_both_waits() {
    let h = harness(vec![
        ScriptedClient::tool_calls(
            &[
                (
                    "delegate",
                    json!({"model":"anthropic/claude-sonnet-5-5", "effort":"low", "role": "worker", "objective": "do the sub task", "wait": true}),
                ),
                ("ask_user", json!({"question": "which colour?"})),
            ],
            None,
        ),
        // The child parks on a question of its own, so the parent's wait on
        // it stays unsatisfied while we inspect the parent.
        ScriptedClient::tool_call("ask_user", json!({"question": "which shade?"})),
    ])
    .await;
    let (parent, _) = h
        .runtime
        .create_agent(spec("split the work"), Actor::user())
        .expect("create");

    let parked = h
        .wait_until(parent.id, |s| matches!(s, AgentStatus::Waiting { .. }))
        .await;
    let AgentStatus::Waiting { condition } = &parked else {
        unreachable!("{parked:?}")
    };
    assert_eq!(
        condition.task_ids().len(),
        1,
        "the delegated wait survives the question: {condition:?}"
    );
    assert!(
        waits_on_inbox(condition),
        "and so does the wait the question promised: {condition:?}"
    );

    let events = h
        .store
        .events(0, 10_000, &EventFilter::for_agent(parent.id))
        .expect("events");
    assert!(
        !settled_outcome(&events, "ask_user").is_error,
        "asking after a delegate is allowed: the agent stays alive either way"
    );
    assert!(
        events.iter().any(|e| e.type_name() == "question.asked"),
        "the question was recorded"
    );
}
