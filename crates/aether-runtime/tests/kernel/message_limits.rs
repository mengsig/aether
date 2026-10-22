//! Messages and objectives above the engine's line limit are refused.
use super::*;

fn oversized() -> String {
    "x".repeat(aether_core::limits::MAX_MESSAGE_BYTES + 1)
}

#[tokio::test]
async fn an_oversized_message_is_refused_and_never_delivered() {
    let h = harness(vec![]).await;
    let (agent, _) = h
        .runtime
        .create_agent(idle_spec(), Actor::user())
        .expect("agent");
    h.wait_until(agent.id, |s| s.kind() == AgentStatusKind::Waiting)
        .await;

    let refusal = h
        .runtime
        .send_message(
            agent.id,
            vec![ContentBlock::text(oversized())],
            Actor::user(),
        )
        .expect_err("an oversized message is refused")
        .to_string();
    assert!(
        refusal.contains("2049 KiB") && refusal.contains("artifact"),
        "{refusal}"
    );
    let fan_out = h.runtime.fan_out_message(
        agent.id,
        vec![ContentBlock::text(oversized())],
        Actor::user(),
    );
    assert!(fan_out.is_err(), "the subtree gets no oversized message");
}

#[tokio::test]
async fn an_oversized_objective_is_refused_for_tasks_and_agents() {
    let h = harness(vec![]).await;
    let task = h.runtime.create_task(
        aether_runtime::CreateTaskSpec {
            objective: oversized(),
            ..Default::default()
        },
        Actor::user(),
    );
    assert!(
        task.expect_err("task")
            .to_string()
            .contains("task objective")
    );
    let mut spec = idle_spec();
    spec.objective = Some(oversized());
    let agent = h.runtime.create_agent(spec, Actor::user());
    assert!(agent.expect_err("agent").to_string().contains("artifact"));
}
