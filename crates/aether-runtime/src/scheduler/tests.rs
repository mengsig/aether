use super::dispatch::pick;
use crate::RunningTurn;
use aether_core::{event::Actor, model::ModelRef};
use std::{
    collections::{HashMap, HashSet},
    time::Instant,
};
use tokio_util::sync::CancellationToken;

use super::*;
use crate::commands::CreateAgentSpec;

async fn engine_runtime() -> (Arc<Runtime>, tempfile::TempDir) {
    let (runtime, config_dir) = crate::test_runtime(|c| {
        c.max_concurrent_turns = 4;
        c.max_concurrent_engine_turns = Some(1);
    })
    .await;
    // These tests read and write scheduler state by hand; the background
    // loop would race their asserts with its own tick.
    runtime.shutdown.cancel();
    (runtime, config_dir)
}

fn engine_agent(runtime: &Runtime) -> Agent {
    runtime
        .create_agent(
            CreateAgentSpec {
                role: "worker".into(),
                model: Some(ModelRef::new("claude-code", "haiku")),
                ..Default::default()
            },
            Actor::user(),
        )
        .expect("agent")
        .0
}

fn admits(runtime: &Runtime, agent: &Agent, addressed: bool) -> bool {
    let ready = [Runnable {
        agent: agent.clone(),
        addressed,
    }];
    let engines = HashSet::from(["claude-code".to_owned()]);
    let sched = runtime.sched.lock();
    pick(
        runtime,
        &ready,
        &sched,
        &Lineage::of(&[]),
        &HashMap::new(),
        &engines,
    )
    .is_some()
}

#[tokio::test]
async fn operator_admission_survives_backoff_only_for_its_reserved_link() {
    let (runtime, _config_dir) = engine_runtime().await;
    let agent = engine_agent(&runtime);
    for provider in ["claude-code", "codex"] {
        backoff(
            &runtime,
            provider,
            "subscription window 96% spent".into(),
            Duration::from_secs(60),
        )
        .expect("backoff");
    }
    assert!(!admits(&runtime, &agent, false));
    assert!(admits(&runtime, &agent, true));
    runtime.sched.lock().running.insert(
        agent.id,
        RunningTurn {
            task: None,
            cancel: CancellationToken::new(),
            cancel_request: None,
            stop_reason: None,
            interrupt: CancellationToken::new(),
            addressed: true,
            provider: "claude-code".into(),
            engine: true,
            on_provider: true,
        },
    );
    assert!(claim_link(&runtime, agent.id, "claude-code").is_ok());
    assert!(runtime.sched.lock().backoff.contains_key("claude-code"));
    assert!(
        claim_link(&runtime, agent.id, "codex").is_err(),
        "unreserved fallback bypassed backoff"
    );
    runtime
        .sched
        .lock()
        .running
        .get_mut(&agent.id)
        .unwrap()
        .addressed = false;
    assert!(
        claim_link(&runtime, agent.id, "claude-code").is_err(),
        "background turn bypassed backoff"
    );
    {
        let mut sched = runtime.sched.lock();
        let turn = sched.running.get_mut(&agent.id).unwrap();
        turn.addressed = true;
        turn.on_provider = false;
    }
    assert!(
        claim_link(&runtime, agent.id, "claude-code").is_err(),
        "released slot retained its exception"
    );
}

/// A turn resuming from a park queues for a slot holding its resource
/// keys, so the slot it is waiting for is not the next fresh turn's to
/// take: everyone behind those keys is waiting out that queue too.
#[tokio::test]
async fn a_queued_retaker_reserves_the_engine_slot_from_a_fresh_turn() {
    let (runtime, _config_dir) = engine_runtime().await;
    let waiter = engine_agent(&runtime);
    let newcomer = engine_agent(&runtime);
    assert!(
        admits(&runtime, &newcomer, false),
        "an idle engine cap admits a fresh turn"
    );
    runtime.sched.lock().engine_retakers.insert(waiter.id);
    assert!(
        !admits(&runtime, &newcomer, false),
        "the only slot was promised to a turn already queued for it"
    );
    assert!(
        admits(&runtime, &newcomer, true),
        "a person's message passes the engine cap like every limit below 5"
    );
    runtime.sched.lock().engine_retakers.remove(&waiter.id);
    assert!(
        admits(&runtime, &newcomer, false),
        "the reservation ends with the queue"
    );
}

/// The hold that stops a turn no link would carry being re-offered on the
/// same tick: one scheduler period, and never a person's message.
#[tokio::test]
async fn an_entry_refusal_holds_a_turn_briefly_but_never_a_persons() {
    let (runtime, _config_dir) = engine_runtime().await;
    let agent = engine_agent(&runtime);
    assert!(admits(&runtime, &agent, false));
    hold_after_entry_refusal(&runtime, agent.id);
    assert!(
        !admits(&runtime, &agent, false),
        "a refused turn was re-dispatched on the same tick"
    );
    assert!(
        admits(&runtime, &agent, true),
        "a person's message must bypass the hold"
    );
    // The filter honours the deadline itself, not only the prune that
    // happens to run before it.
    let expired = Instant::now()
        .checked_sub(Duration::from_millis(1))
        .expect("the clock has been up a millisecond");
    runtime
        .sched
        .lock()
        .entry_refused_until
        .insert(agent.id, expired);
    assert!(
        admits(&runtime, &agent, false),
        "an expired hold parked the agent until something pruned it"
    );
}

#[tokio::test]
async fn exhaustion_canary_cannot_overlap_a_usage_refresh_claim() {
    let (runtime, _dir) = engine_runtime().await;
    let agent = engine_agent(&runtime);
    runtime.sched.lock().usage_refresh.insert(
        "codex".into(),
        crate::provider_refresh::RefreshState {
            last_attempt: Instant::now(),
            in_flight: true,
            quarantined: false,
        },
    );
    assert!(!providers::claim_canary(&runtime, "codex", agent.id));
    assert!(!runtime.sched.lock().probing.contains_key("codex"));
    runtime
        .sched
        .lock()
        .usage_refresh
        .get_mut("codex")
        .unwrap()
        .in_flight = false;
    assert!(providers::claim_canary(&runtime, "codex", agent.id));
    assert_eq!(runtime.sched.lock().probing["codex"], agent.id);
}

#[tokio::test]
async fn quarantined_usage_refresh_does_not_prevent_exhaustion_recovery() {
    let (runtime, _dir) = engine_runtime().await;
    let mut agent = engine_agent(&runtime);
    agent.status = AgentStatus::Blocked {
        reason: BlockReason::Exhausted {
            model: ModelRef::new("claude-code", "haiku"),
            class: aether_core::agent::ExhaustionClass::UsageLimit,
            since: now(),
            reported_reset: None,
            next_probe: now(),
            probes: 0,
            detail: "spent".into(),
        },
    };
    runtime
        .store
        .commit(
            aether_store::Commit::new().mutate(aether_store::Mutation::UpsertAgent(agent.clone())),
        )
        .unwrap();
    runtime.sched.lock().usage_refresh.insert(
        "claude-code".into(),
        crate::provider_refresh::RefreshState {
            last_attempt: Instant::now(),
            in_flight: false,
            quarantined: true,
        },
    );
    assert_eq!(
        due_canaries(&runtime, &[agent.clone()], now())["claude-code"],
        agent.id
    );
    tick(&runtime).unwrap();
    let woken = runtime.store.agent(agent.id).unwrap().unwrap();
    assert!(!matches!(
        woken.status,
        AgentStatus::Blocked {
            reason: BlockReason::Exhausted { .. }
        }
    ));
    assert_eq!(runtime.sched.lock().probing["claude-code"], agent.id);
}

#[tokio::test]
async fn no_budget_warning_follows_the_stop_of_the_same_agent() {
    let (runtime, _config_dir) = engine_runtime().await;
    let agent = engine_agent(&runtime);
    let usage = aether_core::model::Usage::default();
    let warnings = |runtime: &Runtime| {
        runtime
            .store
            .events(0, 1000, &aether_store::EventFilter::for_agent(agent.id))
            .unwrap()
            .into_iter()
            .filter(|e| matches!(e.kind, aether_core::event::EventKind::BudgetWarning { .. }))
            .count()
    };
    warn_once(&runtime, &agent, usage, "token budget of 5: 4 used".into()).unwrap();
    assert_eq!(warnings(&runtime), 1, "a live agent is warned");

    stop_for_budget(
        &runtime,
        agent.id,
        usage,
        "token budget of 5: 6 used",
        Actor::Scheduler,
    )
    .unwrap();
    warn_once(&runtime, &agent, usage, "token budget of 5: 6 used".into()).unwrap();
    assert_eq!(
        warnings(&runtime),
        1,
        "a stopped agent is not warned afterwards"
    );
}

async fn quiet_runtime() -> (Arc<Runtime>, tempfile::TempDir) {
    let (runtime, config_dir) = crate::test_runtime(|_| {}).await;
    runtime.shutdown.cancel();
    (runtime, config_dir)
}

fn started_agent(runtime: &Runtime) -> Agent {
    runtime
        .create_agent(
            CreateAgentSpec {
                role: "worker".into(),
                start: true,
                ..Default::default()
            },
            Actor::user(),
        )
        .expect("agent")
        .0
}

/// A ready agent with nothing to do is parked by the scheduling pass that
/// follows the maintenance steps.
fn parked(runtime: &Runtime, agent: &Agent) -> bool {
    let stored = runtime.store.agent(agent.id).expect("read").expect("agent");
    matches!(stored.status, AgentStatus::Waiting { .. })
}

#[tokio::test]
async fn failing_early_steps_do_not_stop_scheduling() {
    let (runtime, _dir) = quiet_runtime().await;
    let agent = started_agent(&runtime);
    runtime.sched.lock().step_faults.extend([
        ("drain cancellations", StepFault::FailOnce),
        ("expire backoff", StepFault::FailAlways),
        ("enforce budgets", StepFault::FailAlways),
        ("gate dependencies", StepFault::FailAlways),
    ]);
    tick(&runtime).expect_err("the first step failure is still reported");
    assert!(parked(&runtime, &agent), "later steps were skipped");
    assert!(
        !runtime
            .sched
            .lock()
            .step_faults
            .contains_key("drain cancellations"),
        "the once-failing step never ran"
    );
}

#[tokio::test]
async fn a_panicking_step_is_counted_and_the_tick_still_schedules() {
    let (runtime, _dir) = quiet_runtime().await;
    let agent = started_agent(&runtime);
    runtime
        .sched
        .lock()
        .step_faults
        .insert("enforce budgets", StepFault::Panic);
    tick(&runtime).expect("tick survives a panicking step");
    tick(&runtime).expect("and the next tick");
    assert_eq!(runtime.sched.lock().step_panics, 2);
    assert!(parked(&runtime, &agent));
}

#[tokio::test]
async fn a_panicking_tick_does_not_end_the_scheduler_loop() {
    let (runtime, _dir) = crate::test_runtime(|_| {}).await;
    runtime
        .sched
        .lock()
        .step_faults
        .insert("tick", StepFault::Panic);
    let deadline = Instant::now() + Duration::from_secs(10);
    while runtime.sched.lock().step_panics < 2 {
        assert!(Instant::now() < deadline, "the loop stopped after a panic");
        runtime.notify.notify_one();
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    runtime.shutdown.cancel();
}

#[tokio::test]
async fn a_failed_limit_step_withholds_dispatch_but_other_steps_do_not() {
    let (runtime, _dir) = quiet_runtime().await;
    let fail = || Err(crate::RuntimeError::Internal("boom".into()));
    let mut faults = Faults::default();
    isolated(&runtime, &mut faults, "expire backoff", fail);
    assert_eq!(faults.errors.len(), 1);
    assert!(!faults.withhold_dispatch);
    guarded(&runtime, &mut faults, "enforce deadlines", fail);
    assert!(faults.withhold_dispatch);

    let mut faults = Faults::default();
    runtime
        .sched
        .lock()
        .step_faults
        .insert("drain cancellations", StepFault::Panic);
    guarded(&runtime, &mut faults, "drain cancellations", || Ok(()));
    assert!(
        faults.withhold_dispatch,
        "a panicking limit step fails closed"
    );
}

#[tokio::test]
async fn operator_priority_does_not_bypass_unreadable_ancestry() {
    let (runtime, _config) = engine_runtime().await;
    let mut agent = engine_agent(&runtime);
    agent.parent = Some(aether_core::id::AgentId::new());
    let lineage = Lineage::from_store(&[agent.clone()], &runtime);
    assert!(lineage.has_unreadable_ancestor(agent.id));
    let ready = [Runnable {
        agent,
        addressed: true,
    }];
    assert!(
        pick(
            &runtime,
            &ready,
            &runtime.sched.lock(),
            &lineage,
            &HashMap::new(),
            &HashSet::from(["claude-code".into()])
        )
        .is_none()
    );
    assert!(
        runtime
            .problems()
            .unwrap()
            .iter()
            .any(|problem| problem.contains("Ancestor"))
    );
}
