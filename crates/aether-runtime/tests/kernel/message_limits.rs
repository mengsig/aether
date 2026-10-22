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

#[tokio::test]
async fn an_oversized_trigger_or_schedule_payload_is_refused() {
    let h = harness(vec![]).await;
    let (agent, _) = h
        .runtime
        .create_agent(idle_spec(), Actor::user())
        .expect("agent");
    let payload = serde_json::json!({"log": oversized()});
    let trigger = h
        .runtime
        .trigger_agent(agent.id, "ci".into(), payload.clone(), Actor::user());
    assert!(
        trigger
            .expect_err("trigger")
            .to_string()
            .contains("artifact")
    );
    let spec = aether_core::schedule::CreateScheduleSpec {
        name: "nightly".into(),
        target: aether_core::schedule::ScheduleTarget::Agent(agent.id),
        recurrence: aether_core::schedule::Recurrence::Interval {
            seconds: std::num::NonZeroU64::new(3600).expect("nonzero"),
        },
        missed: Default::default(),
        busy: Default::default(),
        payload,
        max_deliveries: None,
    };
    let schedule = h.runtime.create_schedule(spec, Actor::user());
    assert!(
        schedule
            .expect_err("schedule")
            .to_string()
            .contains("schedule payload")
    );
}

#[tokio::test]
async fn a_spawn_schedule_whose_runs_would_be_oversized_is_refused() {
    let h = harness(vec![]).await;
    let limit = aether_core::limits::MAX_MESSAGE_BYTES;
    let template = aether_core::schedule::SpawnSpec {
        role: "worker".into(),
        name: None,
        objective: "x".repeat(limit - 100),
        model: None,
        fallbacks: vec![],
        scope: None,
        config: None,
        approval: None,
    };
    let spec = aether_core::schedule::CreateScheduleSpec {
        name: "nightly".into(),
        target: aether_core::schedule::ScheduleTarget::Spawn(template),
        recurrence: aether_core::schedule::Recurrence::Interval {
            seconds: std::num::NonZeroU64::new(3600).expect("nonzero"),
        },
        missed: Default::default(),
        busy: Default::default(),
        payload: serde_json::Value::Null,
        max_deliveries: None,
    };
    let refusal = h.runtime.create_schedule(spec, Actor::user());
    assert!(
        refusal
            .expect_err("schedule")
            .to_string()
            .contains("scheduled run objective")
    );
}
