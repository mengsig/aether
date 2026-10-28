use super::*;
use aether_core::inbox::InboxKind;

/// The 2026-10 store-lock report: a turn failed internally before it recorded
/// an invocation, and the operator woke the agent.
#[tokio::test]
async fn a_woken_turn_after_an_internal_failure_is_told_only_the_cause_and_numbered_next() {
    let (runtime, _dir) = crate::test_runtime(|_| {}).await;
    runtime.shutdown().await;
    let (agent, _) = runtime
        .create_agent(
            CreateAgentSpec {
                objective: Some("reply LOCKTEST".into()),
                start: true,
                ..Default::default()
            },
            Actor::user(),
        )
        .expect("agent");
    let (ctx, _) = begin_turn(&runtime, agent.id, &CancellationToken::new())
        .expect("begin turn")
        .expect("a turn");
    assert_eq!(ctx.turn, 1);

    let cause = format!("Agent {} turn failed: sqlite: database is locked", agent.id);
    fail_agent(&runtime, agent.id, with_operator_advice(agent.id, cause)).expect("fail");
    let failed = runtime.store.agent(agent.id).unwrap().unwrap();
    assert!(
        matches!(&failed.status, AgentStatus::Failed { error } if error.contains("aether doctor")),
        "the operator lost the advice: {:?}",
        failed.status
    );
    assert_eq!(failed.stats.turns, 1, "the failed turn kept no number");

    runtime.wake_agent(agent.id, Actor::user()).expect("wake");
    let notes: Vec<String> = runtime
        .store
        .inbox(agent.id, 0)
        .unwrap()
        .into_iter()
        .filter_map(|item| match item.kind {
            InboxKind::Wake { reason } => Some(reason),
            _ => None,
        })
        .collect();
    let note = notes.last().expect("a wake note");
    assert!(note.contains("database is locked"), "{note}");
    assert!(
        !note.contains("aether doctor") && !note.contains("aether diagnose"),
        "{note}"
    );
    let (woken, _) = begin_turn(&runtime, agent.id, &CancellationToken::new())
        .expect("begin turn")
        .expect("a woken turn");
    assert_eq!(woken.turn, 2);
}

/// Messaging a failed agent is refused with the commands that revive it.
#[tokio::test]
async fn a_message_to_a_failed_agent_names_wake_and_restart() {
    let (runtime, _dir) = crate::test_runtime(|_| {}).await;
    runtime.shutdown().await;
    let (agent, _) = runtime
        .create_agent(
            CreateAgentSpec {
                objective: Some("reply".into()),
                start: true,
                ..Default::default()
            },
            Actor::user(),
        )
        .expect("agent");
    begin_turn(&runtime, agent.id, &CancellationToken::new())
        .expect("begin turn")
        .expect("a turn");
    fail_agent(&runtime, agent.id, "engine killed".into()).expect("fail");
    let refused = runtime
        .send_message(
            agent.id,
            vec![aether_core::model::ContentBlock::text("are you there?")],
            Actor::user(),
        )
        .expect_err("a failed agent takes no message")
        .to_string();
    assert!(refused.contains("is failed"), "{refused}");
    assert!(
        refused.contains(&format!("`aether wake {}`", agent.id)),
        "{refused}"
    );
    assert!(
        refused.contains(&format!("`aether restart {}`", agent.id)),
        "{refused}"
    );
}
