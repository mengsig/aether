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

/// A second `ask_user` in the same batch is refused - only one question can
/// be waited on per turn.
#[tokio::test]
async fn asking_twice_in_the_same_turn_is_refused() {
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

    let question_events = events
        .iter()
        .filter(|e| e.type_name() == "question.asked")
        .count();
    assert_eq!(
        question_events, 1,
        "only the first question was recorded: {events:?}"
    );
    let outcomes: Vec<_> = events
        .iter()
        .filter_map(|e| match &e.kind {
            EventKind::ToolSucceeded { outcome, .. } => Some(outcome),
            _ => None,
        })
        .collect();
    assert!(
        outcomes
            .iter()
            .any(|o| o.is_error && o.content.contains("which colour?")),
        "the second question is refused, naming the one already asked: {outcomes:?}"
    );
}

/// The mirror image: once a turn has ended the agent, recording a question
/// would be a promise nobody can keep, so `ask_user` is refused instead.
#[tokio::test]
async fn asking_after_completing_in_the_same_turn_is_refused() {
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

    h.wait_until(agent.id, |s| s.is_terminal()).await;
    let events = h
        .store
        .events(0, 10_000, &EventFilter::for_agent(agent.id))
        .expect("events");

    let outcome = settled_outcome(&events, "ask_user");
    assert!(
        outcome.is_error,
        "a question no one can answer must not be recorded: {outcome:?}"
    );
    assert!(
        outcome.content.contains("summary"),
        "the refusal says what to do instead: {outcome:?}"
    );
    assert!(
        !events.iter().any(|e| e.type_name() == "question.asked"),
        "no dangling question was recorded"
    );
    assert!(
        events.iter().any(|e| e.type_name() == "task.completed"),
        "the completion the agent actually decided on still stands"
    );
}

/// A question in a batch that also queues an approval would lose its
/// park - the turn waits for the approval instead, and an approval wait is
/// never interrupted by an inbox item, so the reply could not wake the asker.
/// Gating runs before any call in the batch, so the order does not matter.
#[tokio::test]
async fn asking_beside_a_call_awaiting_approval_is_refused() {
    let h = harness_with_tools(
        vec![ScriptedClient::tool_calls(
            &[
                ("ask_user", json!({"question": "which colour?"})),
                ("call_outside", json!({})),
            ],
            None,
        )],
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
    assert!(
        outcome.is_error,
        "a question the approval wait would swallow must not be recorded: {outcome:?}"
    );
    assert!(
        outcome.content.contains("approv"),
        "the refusal says what is in the way: {outcome:?}"
    );
    assert!(
        !events.iter().any(|e| e.type_name() == "question.asked"),
        "no dangling question was recorded"
    );
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
