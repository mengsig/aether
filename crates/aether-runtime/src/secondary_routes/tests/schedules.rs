//! An agent's spawn schedule naming a secondary route: each occurrence runs,
//! and is checked on, the route `delegate` would give its child then.

use super::*;
use aether_core::schedule::{
    BusyPolicy, CreateScheduleSpec, MissedRunPolicy, Recurrence, Schedule, ScheduleOutcome,
    ScheduleTarget, SpawnSpec,
};
use std::num::NonZeroU64;

struct Antigravity;

#[async_trait::async_trait]
impl Engine for Antigravity {
    fn kind(&self) -> EngineKind {
        EngineKind::ANTIGRAVITY
    }

    fn adapter(&self) -> String {
        "antigravity/test".into()
    }

    async fn run(
        &self,
        _turn: aether_engine::EngineTurn,
        _cancel: tokio_util::sync::CancellationToken,
    ) -> std::result::Result<aether_engine::EngineRun, aether_engine::EngineError> {
        Err(aether_engine::EngineError::Unsupported(
            "a test engine".into(),
        ))
    }
}

/// A lead off any secondary route, so no turn of its own reads the sign-ins.
fn api_lead(runtime: &Runtime, workers: &[ModelRef]) -> Agent {
    let mut lead = lead(runtime, workers);
    lead.model = ModelRef::new("anthropic", "claude-opus-5-5");
    runtime
        .apply(Commit::new().mutate(Mutation::UpsertAgent(lead.clone())))
        .expect("lead moved");
    lead
}

fn spawn_spec(model: ModelRef) -> CreateScheduleSpec {
    CreateScheduleSpec {
        target: ScheduleTarget::Spawn(SpawnSpec {
            role: "poller".into(),
            name: None,
            objective: "Check the queue".into(),
            model: Some(model),
            fallbacks: Vec::new(),
            scope: None,
            config: Some(AgentConfig {
                effort: Some(Effort::Low),
                ..Default::default()
            }),
            approval: None,
        }),
        name: "Poll".into(),
        recurrence: Recurrence::Interval {
            seconds: NonZeroU64::new(3600).expect("non-zero"),
        },
        missed: MissedRunPolicy::Coalesce,
        busy: BusyPolicy::Skip,
        payload: serde_json::Value::Null,
        max_deliveries: None,
    }
}

fn schedule(runtime: &Runtime, lead: &Agent, model: ModelRef) -> crate::Result<Schedule> {
    runtime.create_schedule(spawn_spec(model), Actor::agent(lead.id))
}

/// Tick the schedule's due occurrence once, and return it as that left it.
fn tick(runtime: &Runtime, id: aether_core::id::ScheduleId) -> Schedule {
    let job = runtime.store.schedule(id).expect("read").expect("stored");
    let due = job.status.next_due().expect("active");
    crate::schedules::tick(runtime, due).expect("tick");
    runtime.store.schedule(id).expect("read").expect("stored")
}

/// The model of the child the schedule's last occurrence spawned.
fn spawned_model(runtime: &Runtime, job: &Schedule) -> ModelRef {
    let last = job.last.as_ref().expect("an occurrence ran");
    let ScheduleOutcome::Spawned { agent, .. } = &last.outcome else {
        panic!("expected a spawned child, got {:?}", last.outcome);
    };
    let child = runtime.store.agent(*agent).expect("read").expect("child");
    assert!(child.parent.is_some(), "a scheduled child has its creator");
    child.model
}

/// Fire the due occurrence, reading the sign-ins again whenever a pass holds
/// it for them, as the scheduler does: a slow test may outlast a read.
async fn fire(runtime: &Runtime, id: aether_core::id::ScheduleId) -> Schedule {
    for _ in 0..5 {
        let before = runtime.store.schedule(id).expect("read").expect("stored");
        let after = tick(runtime, id);
        if after != before {
            return after;
        }
        runtime.refresh_signed_in_originals().await;
    }
    panic!("the occurrence never fired");
}

/// Fire an occurrence after a sign-in change no read has seen: the pass holds
/// it, due, until the sign-ins are read again, and a pass after fires it.
async fn fire_after_sign_in_change(runtime: &Runtime, id: aether_core::id::ScheduleId) -> Schedule {
    runtime.age_sign_ins();
    let before = runtime.store.schedule(id).expect("read").expect("stored");
    assert_eq!(tick(runtime, id), before, "held for the sign-ins");
    let fired = fire(runtime, id).await;
    assert!(fired.last.is_some(), "the read lets it fire");
    fired
}

#[tokio::test]
async fn a_spawn_schedule_runs_on_the_original_while_agy_is_unavailable() {
    let (runtime, _dir) = runtime(false).await;
    let lead = api_lead(&runtime, &[]);
    runtime
        .engines
        .finish_with(EngineKind::CLAUDE_CODE, Arc::new(ClaudeCode));
    runtime.refresh_signed_in_originals().await;
    let job = schedule(&runtime, &lead, agy("claude-sonnet-5-5"))
        .expect("the original's engine is checked, not agy's");
    let fired = fire(&runtime, job.id).await;
    assert_eq!(
        spawned_model(&runtime, &fired),
        ModelRef::new("claude-code", "claude-sonnet-5-5")
    );
    runtime.shutdown().await;
}

#[tokio::test]
async fn an_agy_only_policy_checks_and_runs_the_agy_route() {
    let (runtime, _dir) = runtime(false).await;
    let lead = api_lead(&runtime, &[agy("claude-sonnet-5-5")]);
    runtime
        .engines
        .finish_with(EngineKind::CLAUDE_CODE, Arc::new(ClaudeCode));
    runtime.refresh_signed_in_originals().await;
    let error = schedule(&runtime, &lead, agy("claude-sonnet-5-5"))
        .expect_err("agy is the only route admitted, and it cannot run")
        .to_string();
    assert!(
        error.contains("antigravity/claude-sonnet-5-5 cannot run this child"),
        "{error}"
    );
    runtime
        .engines
        .mark_usable(EngineKind::ANTIGRAVITY, Arc::new(Antigravity));
    let job = schedule(&runtime, &lead, agy("claude-sonnet-5-5")).expect("agy can run it");
    let fired = fire(&runtime, job.id).await;
    assert_eq!(spawned_model(&runtime, &fired), agy("claude-sonnet-5-5"));
    runtime.shutdown().await;
}

#[tokio::test]
async fn a_login_between_creation_and_firing_routes_the_child_to_the_api() {
    let signed_in = Arc::new(AtomicBool::new(false));
    let (runtime, _dir) = runtime_signed_in(signed_in.clone()).await;
    let lead = api_lead(&runtime, &[]);
    // Codex serves no Claude model, so only the API could take the route.
    runtime
        .engines
        .finish_with(EngineKind::CODEX, Arc::new(ClaudeCode));
    runtime
        .engines
        .mark_usable(EngineKind::ANTIGRAVITY, Arc::new(Antigravity));
    runtime.refresh_signed_in_originals().await;
    let job = schedule(&runtime, &lead, agy("claude-sonnet-5-5")).expect("created on agy");
    signed_in.store(true, Ordering::SeqCst);
    let fired = fire_after_sign_in_change(&runtime, job.id).await;
    assert_eq!(
        spawned_model(&runtime, &fired),
        ModelRef::new("anthropic", "claude-sonnet-5-5")
    );
    runtime.shutdown().await;
}

#[tokio::test]
async fn a_logout_between_creation_and_firing_keeps_the_child_on_agy() {
    let signed_in = Arc::new(AtomicBool::new(true));
    let (runtime, _dir) = runtime_signed_in(signed_in.clone()).await;
    let lead = api_lead(&runtime, &[]);
    runtime
        .engines
        .finish_with(EngineKind::CODEX, Arc::new(ClaudeCode));
    runtime
        .engines
        .mark_usable(EngineKind::ANTIGRAVITY, Arc::new(Antigravity));
    runtime.refresh_signed_in_originals().await;
    let job = schedule(&runtime, &lead, agy("claude-sonnet-5-5")).expect("created on the API");
    signed_in.store(false, Ordering::SeqCst);
    let fired = fire_after_sign_in_change(&runtime, job.id).await;
    assert_eq!(
        spawned_model(&runtime, &fired),
        agy("claude-sonnet-5-5"),
        "a stale sign-in must not route to an API signed out of"
    );
    runtime.shutdown().await;
}
