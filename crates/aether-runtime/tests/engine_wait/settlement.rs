use super::*;

/// `ask_user` parks the same way: the person's answer arrives in the next
/// turn, so this one is over.
#[tokio::test]
async fn ask_user_ends_the_engine_turn() {
    let mut h = harness(Duration::from_millis(200)).await;
    let coordinator = start_coordinator(&h);
    let first = turn_of(&mut h, coordinator).await;

    let (reply, is_error) = call(&h, &first, "ask_user", json!({"question": "which one?"})).await;
    assert!(!is_error, "{reply}");
    assert!(first.observed.stdin_closed());

    let parked = wait_for_park(&h, coordinator).await;
    assert!(matches!(condition_of(&parked.status), WaitCondition::Inbox));
    assert!(
        first.observed.cancelled(),
        "the grace expired without a stop"
    );
    h.runtime.shutdown().await;
}

/// An engine that asks and then completes in the same turn must not get
/// both. The question promises the agent is waiting for an answer, and an
/// agent that has ended cannot be sent one - `send_message` refuses it - so
/// the completion is the call that is refused, and the agent parks.
///
/// Asking parks the turn at once here, so it is the park gate that refuses the
/// completion (`apply_action` is never reached); it borrows the question-aware
/// refusal, which is what the engine needs to be told to do instead.
#[tokio::test]
async fn completing_after_ask_user_in_one_engine_turn_is_refused() {
    let mut h = harness(Duration::from_secs(30)).await;
    let worker = start_agent(
        &h,
        CreateAgentSpec {
            role: "worker".into(),
            objective: Some("paint it".into()),
            start: true,
            config: Some(AgentConfig {
                approval: ApprovalPolicy::none(),
                ..Default::default()
            }),
            ..Default::default()
        },
    );
    let turn = turn_of(&mut h, worker).await;

    let (reply, is_error) = call(&h, &turn, "ask_user", json!({"question": "which colour?"})).await;
    assert!(!is_error, "{reply}");

    let (reply, is_error) = call(&h, &turn, "complete_task", json!({"summary": "guessed"})).await;
    assert!(
        is_error,
        "completing under an unanswered question was allowed: {reply}"
    );
    assert!(
        reply.contains("which colour?"),
        "the refusal names the question it is protecting: {reply}"
    );
    assert!(
        reply.contains("woken with their reply"),
        "and tells the engine to wait for the answer instead: {reply}"
    );

    let _ = turn.end.send(());
    let parked = wait_for_park(&h, worker).await;
    assert!(
        matches!(condition_of(&parked.status), WaitCondition::Inbox),
        "the asker must be waiting for the answer: {:?}",
        parked.status
    );

    let recorded = events(h.store.as_ref());
    assert_eq!(
        recorded
            .iter()
            .filter(|e| e.type_name() == "question.asked")
            .count(),
        1,
        "the question stands"
    );
    assert!(
        !recorded.iter().any(|e| e.type_name() == "task.completed"),
        "the task must not have ended under the open question"
    );
    h.runtime
        .send_message(worker, vec![ContentBlock::text("blue")], Actor::user())
        .expect("the operator's reply still reaches the asker");
    h.runtime.shutdown().await;
}

/// The refusal quotes the open question, so it reaches the engine masked.
#[tokio::test]
async fn a_refusal_quoting_the_open_question_is_masked() {
    const KNOWN: &str = "fleet-token-0123456789abcdef";
    let mut h = harness_configured(Duration::from_secs(30), Script::default(), |config| {
        config.redactor = aether_tools::Redactor::with_known_secrets([KNOWN]).expect("compiles");
    })
    .await;
    let worker = start_agent(
        &h,
        CreateAgentSpec {
            role: "worker".into(),
            objective: Some("rotate it".into()),
            start: true,
            config: Some(AgentConfig {
                approval: ApprovalPolicy::none(),
                ..Default::default()
            }),
            ..Default::default()
        },
    );
    let turn = turn_of(&mut h, worker).await;
    let question = format!("rotate {KNOWN} now?");
    let (reply, is_error) = call(&h, &turn, "ask_user", json!({"question": question})).await;
    assert!(!is_error, "{reply}");

    let (reply, is_error) = call(&h, &turn, "complete_task", json!({"summary": "rotated"})).await;
    assert!(
        is_error,
        "completing under an open question was allowed: {reply}"
    );
    assert!(!reply.contains(KNOWN), "{reply}");
    assert!(
        reply.contains("rotate [REDACTED:known_secret] now?"),
        "the refusal still names the question: {reply}"
    );
    let _ = turn.end.send(());
    h.runtime.shutdown().await;
}

/// The same guard on the engine-bridge path, which serves one
/// tool call per request instead of a batch. A `long_lived` agent's settle
/// only parks it on `Inbox` rather than ending it, so the park gate must
/// admit it - and merge the settle's wait into the question's, not replace
/// it, so the question survives and stays answerable.
#[tokio::test]
async fn completing_after_ask_user_on_the_engine_bridge_is_admitted_for_a_long_lived_agent() {
    let mut h = harness(Duration::from_secs(30)).await;
    let worker = start_agent(
        &h,
        CreateAgentSpec {
            role: "worker".into(),
            objective: Some("paint it".into()),
            start: true,
            config: Some(AgentConfig {
                approval: ApprovalPolicy::none(),
                long_lived: true,
                ..Default::default()
            }),
            ..Default::default()
        },
    );
    let turn = turn_of(&mut h, worker).await;

    let (reply, is_error) = call(&h, &turn, "ask_user", json!({"question": "which colour?"})).await;
    assert!(!is_error, "{reply}");

    let (reply, is_error) = call(
        &h,
        &turn,
        "complete_task",
        json!({"summary": "guessed blue"}),
    )
    .await;
    assert!(
        !is_error,
        "a long_lived agent's settle after a park must be admitted: {reply}"
    );
    assert!(
        !reply.contains("cannot be completed"),
        "no refusal text reaches the engine: {reply}"
    );

    let _ = turn.end.send(());
    let parked = wait_for_park(&h, worker).await;
    assert!(
        matches!(condition_of(&parked.status), WaitCondition::Inbox),
        "the agent parks on the reply rather than ending: {:?}",
        parked.status
    );

    let recorded = events(h.store.as_ref());
    assert!(
        recorded.iter().any(|e| e.type_name() == "task.completed"),
        "the settle really landed"
    );
    assert_eq!(
        recorded
            .iter()
            .filter(|e| e.type_name() == "question.asked")
            .count(),
        1,
        "the question stands"
    );
    h.runtime
        .send_message(worker, vec![ContentBlock::text("blue")], Actor::user())
        .expect("the question is still answerable after the settle");
    h.runtime.shutdown().await;
}

/// The mirror of the completion case: `fail_task` gets the same admission
/// and merge on the engine-bridge path.
#[tokio::test]
async fn failing_after_ask_user_on_the_engine_bridge_is_admitted_for_a_long_lived_agent() {
    let mut h = harness(Duration::from_secs(30)).await;
    let worker = start_agent(
        &h,
        CreateAgentSpec {
            role: "worker".into(),
            objective: Some("paint it".into()),
            start: true,
            config: Some(AgentConfig {
                approval: ApprovalPolicy::none(),
                long_lived: true,
                ..Default::default()
            }),
            ..Default::default()
        },
    );
    let turn = turn_of(&mut h, worker).await;

    let (reply, is_error) = call(
        &h,
        &turn,
        "ask_user",
        json!({"question": "retry or give up?"}),
    )
    .await;
    assert!(!is_error, "{reply}");

    let (reply, is_error) = call(
        &h,
        &turn,
        "fail_task",
        json!({"error": "could not paint it"}),
    )
    .await;
    assert!(
        !is_error,
        "a long_lived agent's settle after a park must be admitted: {reply}"
    );
    assert!(
        !reply.contains("cannot be failed"),
        "no refusal text reaches the engine: {reply}"
    );

    let _ = turn.end.send(());
    let parked = wait_for_park(&h, worker).await;
    assert!(
        matches!(condition_of(&parked.status), WaitCondition::Inbox),
        "the agent parks on the reply rather than ending: {:?}",
        parked.status
    );

    let recorded = events(h.store.as_ref());
    assert!(
        recorded.iter().any(|e| e.type_name() == "task.failed"),
        "the settle really landed"
    );
    assert_eq!(
        recorded
            .iter()
            .filter(|e| e.type_name() == "question.asked")
            .count(),
        1,
        "the question stands"
    );
    h.runtime
        .send_message(worker, vec![ContentBlock::text("give up")], Actor::user())
        .expect("the question is still answerable after the settle");
    h.runtime.shutdown().await;
}

/// The two tests above cannot tell a real `merge_wait` from a
/// plain `effects.next` overwrite, because `ask_user` and the settle both
/// record the identical bare `Inbox` condition. Parking on a *different*
/// wait first (a `delegate(wait: true)`, which records `Task { id }`) makes
/// a clobber observable: only a merge keeps the task wait alive alongside
/// the settle's `Inbox`.
#[tokio::test]
async fn settling_after_a_task_wait_merges_it_instead_of_replacing_it_for_a_long_lived_agent() {
    let mut h = harness(Duration::from_secs(30)).await;
    let worker = start_agent(
        &h,
        CreateAgentSpec {
            role: "worker".into(),
            objective: Some("paint it".into()),
            start: true,
            config: Some(AgentConfig {
                approval: ApprovalPolicy::none(),
                long_lived: true,
                ..Default::default()
            }),
            ..Default::default()
        },
    );
    let turn = turn_of(&mut h, worker).await;

    let (reply, is_error) = call(
        &h,
        &turn,
        "delegate",
        json!({"model": "claude-code/haiku", "effort": "low", "role": "helper", "objective": "mix paint", "wait": true}),
    )
    .await;
    assert!(!is_error, "{reply}");
    let child_task = task_of(&reply);

    let (reply, is_error) = call(
        &h,
        &turn,
        "complete_task",
        json!({"summary": "guessed blue"}),
    )
    .await;
    assert!(
        !is_error,
        "a long_lived agent's settle after a task wait must be admitted: {reply}"
    );

    let _ = turn.end.send(());
    let parked = wait_for_park(&h, worker).await;
    let condition = condition_of(&parked.status);
    assert_eq!(
        waited_on(&parked.status),
        vec![child_task],
        "the delegate's task wait must survive the settle: {condition:?}"
    );
    assert!(
        condition_contains(condition, &WaitCondition::Inbox),
        "the settle's own Inbox wait must also be present, merged rather than \
         replacing the task wait: {condition:?}"
    );
    h.runtime.shutdown().await;
}

/// The mirror of the two `ask_user`-then-settle tests above, in the
/// order the bug report named - a `long_lived` settle first, then a question
/// on the engine bridge. `apply_action`'s own `AskUser` guard already admits
/// this on the batch path (it only refuses a genuine terminal
/// `Completed`/`Failed`, which a long_lived settle never records); the
/// engine-bridge park gate had a stricter, inconsistent rule that refused
/// every question once anything had parked the turn.
#[tokio::test]
async fn asking_after_a_long_lived_settle_on_the_engine_bridge_is_admitted() {
    let mut h = harness(Duration::from_secs(30)).await;
    let worker = start_agent(
        &h,
        CreateAgentSpec {
            role: "worker".into(),
            objective: Some("paint it".into()),
            start: true,
            config: Some(AgentConfig {
                approval: ApprovalPolicy::none(),
                long_lived: true,
                ..Default::default()
            }),
            ..Default::default()
        },
    );
    let turn = turn_of(&mut h, worker).await;

    let (reply, is_error) =
        call(&h, &turn, "complete_task", json!({"summary": "painted it"})).await;
    assert!(!is_error, "{reply}");

    let (reply, is_error) = raw_call(
        &h,
        &turn,
        "ask_user",
        json!({"question": "what colour next?"}),
    )
    .await;
    assert!(
        !is_error,
        "the first question after a long_lived settle must be admitted, not treated \
         as a second decision: {reply}"
    );
    assert!(
        reply.contains("Question recorded"),
        "no refusal text reaches the engine: {reply}"
    );

    let _ = turn.end.send(());
    let parked = wait_for_park(&h, worker).await;
    assert!(
        matches!(condition_of(&parked.status), WaitCondition::Inbox),
        "the agent parks on the reply rather than ending: {:?}",
        parked.status
    );

    let recorded = events(h.store.as_ref());
    assert!(
        recorded.iter().any(|e| e.type_name() == "task.completed"),
        "the settle really landed"
    );
    assert_eq!(
        recorded
            .iter()
            .filter(|e| e.type_name() == "question.asked")
            .count(),
        1,
        "the question was actually recorded, not silently dropped"
    );
    h.runtime
        .send_message(worker, vec![ContentBlock::text("red")], Actor::user())
        .expect("the question is answerable after the settle");
    h.runtime.shutdown().await;
}
