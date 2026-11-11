use super::*;
use crate::{RunningTurn, commands::CreateAgentSpec};
use aether_core::{budget::HardStop, id::AgentId, model::ModelRef};
use aether_store::Mutation;
use tokio_util::sync::CancellationToken;

async fn runtime_with(stop: HardStop) -> (Arc<Runtime>, tempfile::TempDir) {
    let (runtime, dir) = crate::test_runtime(|c| c.run_hard_stop = stop).await;
    // The tests drive the pass by hand; the background loop would race them.
    runtime.shutdown.cancel();
    (runtime, dir)
}

fn agent_in(runtime: &Runtime, provider: &str) -> Agent {
    child_of(runtime, provider, None)
}

fn child_of(runtime: &Runtime, provider: &str, parent: Option<AgentId>) -> Agent {
    runtime
        .create_agent(
            CreateAgentSpec {
                role: "worker".into(),
                model: Some(ModelRef::new(provider, "haiku")),
                parent,
                config: parent.map(|_| aether_core::agent::AgentConfig {
                    effort: Some(aether_core::model::Effort::Low),
                    ..Default::default()
                }),
                ..Default::default()
            },
            Actor::user(),
        )
        .expect("agent")
        .0
}

fn spend(runtime: &Runtime, agent: &Agent, usd: f64, tokens: u64, turns: u32) -> Agent {
    let mut row = runtime
        .store
        .agent(agent.id)
        .expect("read")
        .expect("exists");
    row.stats.usage.cost_usd = usd;
    row.stats.usage.output_tokens = tokens;
    row.stats.turns = turns;
    runtime
        .store
        .commit(Commit::new().mutate(Mutation::UpsertAgent(row.clone())))
        .expect("seed spend");
    row
}

fn run_turn(runtime: &Runtime, agent: &Agent) -> CancellationToken {
    let interrupt = CancellationToken::new();
    runtime.sched.lock().running.insert(
        agent.id,
        RunningTurn {
            task: None,
            cancel: CancellationToken::new(),
            cancel_request: None,
            stop_reason: None,
            interrupt: interrupt.clone(),
            addressed: false,
            engine: false,
            provider: "p".into(),
            on_provider: true,
        },
    );
    interrupt
}

fn hard_stops(runtime: &Runtime, run: RunId) -> Vec<EventKind> {
    runtime
        .store
        .latest_events(
            10,
            &EventFilter {
                types: vec!["run.hard_stop".into()],
                ..EventFilter::for_run(run)
            },
        )
        .expect("events")
        .into_iter()
        .map(|e| e.kind)
        .collect()
}

/// One scheduler pass in which dispatch selects `candidates` in order: how
/// many of them the stop admits.
fn start_all(runtime: &Arc<Runtime>, candidates: &[&Agent]) -> usize {
    let mut reserve = enforce_hard_stops(runtime).expect("enforce");
    let started = candidates
        .iter()
        .filter(|a| reserve.try_start(a.run))
        .count();
    reserve.finish(runtime);
    started
}

#[tokio::test]
async fn crossing_the_usd_stop_interrupts_every_live_agent_and_records_it() {
    let (runtime, _dir) = runtime_with(HardStop {
        usd: Some(5.0),
        tokens: None,
    })
    .await;
    let first = agent_in(&runtime, "anthropic");
    let second = child_of(&runtime, "anthropic", Some(first.id));
    let run = first.run;
    assert_eq!(second.run, run, "a child must share its parent's run");
    spend(&runtime, &first, 6.0, 0, 1);
    let token_a = run_turn(&runtime, &first);
    let token_b = run_turn(&runtime, &second);
    assert_eq!(
        start_all(&runtime, &[&first]),
        0,
        "a held run admitted a turn"
    );
    assert!(token_a.is_cancelled() && token_b.is_cancelled());
    let events = hard_stops(&runtime, run);
    let [
        EventKind::RunHardStop {
            limit,
            usage,
            interrupted,
            ..
        },
    ] = events.as_slice()
    else {
        panic!("expected one run.hard_stop event: {events:?}");
    };
    assert_eq!(
        (limit.as_str(), usage.cost_usd, *interrupted),
        ("run_hard_stop_usd = 5", 6.0, 2)
    );
    start_all(&runtime, &[&first]);
    assert_eq!(
        hard_stops(&runtime, run).len(),
        1,
        "the crossing was recorded twice"
    );
    let still = runtime
        .store
        .agent(first.id)
        .expect("read")
        .expect("exists");
    assert!(still.is_live(), "an interrupted agent was failed");
}

#[tokio::test]
async fn the_token_stop_holds_a_run_with_no_dollar_cost() {
    let (runtime, _dir) = runtime_with(HardStop {
        usd: None,
        tokens: Some(1_000),
    })
    .await;
    let agent = agent_in(&runtime, "claude-code");
    spend(&runtime, &agent, 0.0, 1_500, 1);
    let token = run_turn(&runtime, &agent);
    assert_eq!(start_all(&runtime, &[&agent]), 0);
    assert!(token.is_cancelled());
    let events = hard_stops(&runtime, agent.run);
    assert!(
        matches!(events.as_slice(), [EventKind::RunHardStop { limit, .. }] if limit == "run_hard_stop_tokens = 1000"),
        "{events:?}"
    );
}

#[tokio::test]
async fn admission_refuses_a_turn_whose_reserve_would_cross_the_stop() {
    let (runtime, _dir) = runtime_with(HardStop {
        usd: Some(5.0),
        tokens: None,
    })
    .await;
    let agent = agent_in(&runtime, "anthropic");
    // $4 over 2 turns: under the stop, but another $2 turn would cross it.
    spend(&runtime, &agent, 4.0, 0, 2);
    assert_eq!(
        start_all(&runtime, &[&agent]),
        0,
        "a turn that would cross was admitted"
    );
    assert!(
        hard_stops(&runtime, agent.run).is_empty(),
        "nothing had crossed yet"
    );
    let problems = runtime.problems().expect("problems").join("\n");
    assert!(
        problems.contains("hard stop") && problems.contains("run_hard_stop_usd"),
        "the refusal does not say how to raise the limit: {problems}"
    );
}

#[tokio::test]
async fn without_a_configured_stop_nothing_changes() {
    let (runtime, _dir) = runtime_with(HardStop::default()).await;
    let agent = agent_in(&runtime, "anthropic");
    spend(&runtime, &agent, 1_000_000.0, 1 << 40, 2);
    let token = run_turn(&runtime, &agent);
    assert_eq!(start_all(&runtime, &[&agent]), 1);
    assert!(!token.is_cancelled());
    assert!(hard_stops(&runtime, agent.run).is_empty());
}

#[tokio::test]
async fn one_pass_reserves_each_candidate_it_admits() {
    let (runtime, _dir) = runtime_with(HardStop {
        usd: Some(10.0),
        tokens: None,
    })
    .await;
    let first = agent_in(&runtime, "anthropic");
    let second = child_of(&runtime, "anthropic", Some(first.id));
    let third = child_of(&runtime, "anthropic", Some(first.id));
    // $6 over 3 turns: one more $2 turn fits under $10, a second reaches it.
    spend(&runtime, &first, 6.0, 0, 3);
    let started = start_all(&runtime, &[&first, &second, &third]);
    assert_eq!(started, 1, "the batch was admitted past the stop");
}

#[tokio::test]
async fn a_candidate_dispatch_skips_reserves_nothing() {
    let (runtime, _dir) = runtime_with(HardStop {
        usd: Some(10.0),
        tokens: None,
    })
    .await;
    let skipped = agent_in(&runtime, "anthropic");
    let eligible = child_of(&runtime, "anthropic", Some(skipped.id));
    spend(&runtime, &skipped, 6.0, 0, 3);
    // Dispatch passed over `skipped` (say, a backed-off provider) and only
    // selects `eligible`: the budget it left unused is still there.
    assert_eq!(start_all(&runtime, &[&eligible]), 1);
}
