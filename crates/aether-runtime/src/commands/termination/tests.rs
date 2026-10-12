use super::*;
use aether_core::idempotency::Submission;
use tokio_util::sync::CancellationToken;

fn running(interrupt: CancellationToken) -> crate::RunningTurn {
    crate::RunningTurn {
        task: None,
        cancel: CancellationToken::new(),
        cancel_request: None,
        interrupt,
        addressed: false,
        provider: "anthropic".into(),
        engine: false,
        on_provider: true,
    }
}

#[tokio::test]
async fn replaying_an_interrupt_never_cancels_a_later_turn() {
    let (runtime, _config) = crate::test_runtime(|_| {}).await;
    runtime.shutdown.cancel();
    let (agent, _) = runtime
        .create_agent(
            crate::CreateAgentSpec {
                role: "worker".into(),
                start: false,
                ..Default::default()
            },
            Actor::user(),
        )
        .unwrap();
    let first = CancellationToken::new();
    runtime
        .sched
        .lock()
        .running
        .insert(agent.id, running(first.clone()));
    let submission = Submission::new("interrupt", &serde_json::json!({"agent":agent.id})).unwrap();
    assert!(
        runtime
            .interrupt_agent_keyed(agent.id, Actor::user(), Some(submission.clone()))
            .unwrap()
    );
    assert!(first.is_cancelled());
    let later = CancellationToken::new();
    runtime
        .sched
        .lock()
        .running
        .insert(agent.id, running(later.clone()));
    assert!(
        runtime
            .interrupt_agent_keyed(agent.id, Actor::user(), Some(submission))
            .unwrap()
    );
    assert!(!later.is_cancelled());
    let changed =
        Submission::new("interrupt", &serde_json::json!({"agent":AgentId::new()})).unwrap();
    assert!(
        runtime
            .interrupt_agent_keyed(agent.id, Actor::user(), Some(changed))
            .is_err()
    );
    assert!(!later.is_cancelled());
    runtime.sched.lock().running.clear();
    runtime.shutdown().await;
}
