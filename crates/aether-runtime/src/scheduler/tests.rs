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
