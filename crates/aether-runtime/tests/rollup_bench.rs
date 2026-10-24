//! What one scheduler tick costs over a large fleet, what the budget
//! roll-up's skip saves, and what a run report over a deep imported chain
//! costs. Budget passes read only budgeted roots and their subtrees, including
//! terminal descendants whose spend still counts.
//!
//! Ignored by default: a debug build's timings say nothing about the daemon
//! anyone runs. `scripts/ci.sh` runs it in release; by hand:
//!
//! ```text
//! cargo test --release -p aether-runtime --test rollup_bench -- --ignored --nocapture
//! ```
//!
//! The ceilings are deliberately loose - some twenty times what the machine
//! that wrote them measured (10 ms with the roll-up, 0.9 ms skipping it, over
//! 5000 agents). They catch a pass that stopped skipping or a read that turned
//! quadratic, not a slower laptop. They are still wall-clock, so on a shared
//! runner they are skipped and only `skip < pass` is asserted - that is the
//! machine-independent claim the optimisation rests on.

use std::sync::Arc;
use std::time::{Duration, Instant};

use aether_core::agent::{Agent, AgentConfig, AgentStatus, Scope, WaitCondition};
use aether_core::event::{Actor, EventDraft, EventKind};
use aether_core::id::AgentId;
use aether_core::model::{ModelRef, Usage};
use aether_core::time::now;
use aether_model::Models;
use aether_model::testing::ScriptedClient;
use aether_runtime::{Runtime, RuntimeConfig, scheduler};
use aether_store::{Commit, Mutation, SqliteStore, Store};
use aether_tools::ToolRegistry;

/// Supervisors, each with a budget, and the children whose spend rolls up
/// into it: 5000 agents in the table the pass reads.
const SUPERVISORS: usize = 50;
const CHILDREN_EACH: usize = 99;
const ROUNDS: usize = 10;

/// One tick over the whole fleet, measured with the pass forced to run.
const PASS_CEILING: Duration = Duration::from_millis(250);
/// One tick that skips the pass, measured with nothing committed since.
const SKIP_CEILING: Duration = Duration::from_millis(25);

#[tokio::test(flavor = "multi_thread")]
#[ignore = "timing; release only - see the module docs"]
async fn a_tick_over_five_thousand_agents_costs_less_when_the_fleet_has_not_moved() {
    let store: Arc<dyn Store> = Arc::new(SqliteStore::open_in_memory().expect("store"));
    let agents = seed_fleet(store.as_ref()).expect("seed");
    assert_eq!(agents, SUPERVISORS * (CHILDREN_EACH + 1));

    let config_dir = tempfile::tempdir().expect("config dir");
    let models = Arc::new(Models::load(Some(config_dir.path().to_path_buf())).expect("models"));
    let runtime = Runtime::start_with_engines(
        RuntimeConfig {
            holder: "bench".into(),
            ..Default::default()
        },
        store.clone(),
        models,
        Arc::new(ToolRegistry::with_builtins()),
        Some(Arc::new(ScriptedClient::with_responses(Vec::new()))),
        Default::default(),
    )
    .await
    .expect("runtime");
    // The daemon's own scheduler loop would tick against the same store while
    // we are timing ticks; stop it and drive the pass by hand.
    runtime.shutdown().await;

    scheduler::tick(&runtime).expect("warm-up tick");

    // A commit before each round: the pass runs only when the fleet has moved,
    // and one event is the cheapest thing that moves it.
    let mut passes = Vec::with_capacity(ROUNDS);
    for _ in 0..ROUNDS {
        runtime
            .apply(Commit::new().event(EventDraft::new(
                Actor::System,
                EventKind::DaemonStarted {
                    version: "bench".into(),
                    pid: 0,
                },
            )))
            .expect("commit");
        let at = Instant::now();
        scheduler::tick(&runtime).expect("tick");
        passes.push(at.elapsed());
    }

    // Nothing committed in between, so every one of these skips the roll-up.
    let mut skips = Vec::with_capacity(ROUNDS);
    for _ in 0..ROUNDS {
        let at = Instant::now();
        scheduler::tick(&runtime).expect("tick");
        skips.push(at.elapsed());
    }

    let pass = median(&mut passes);
    let skip = median(&mut skips);
    println!(
        "agents={agents} tick_with_roll_up={pass:?} tick_skipped={skip:?} saved={:?}",
        pass.saturating_sub(skip)
    );

    // The ratio holds anywhere; the ceilings only where the cores are ours.
    assert!(
        skip < pass,
        "the skip is the point: {skip:?} against {pass:?}"
    );
    if let Some(why) = shared_runner() {
        println!("shared runner ({why}): wall-clock ceilings not asserted");
        return;
    }
    assert!(pass < PASS_CEILING, "tick with the roll-up took {pass:?}");
    assert!(
        skip < SKIP_CEILING,
        "tick skipping the roll-up took {skip:?}"
    );
}

/// Why this machine's wall clock cannot bound a tick, if it cannot: a hosted
/// runner shares two vCPUs with whatever else the provider put on the box, and
/// no headroom is a bound under contention.
fn shared_runner() -> Option<String> {
    match std::env::var("CI") {
        Ok(ci) if !ci.is_empty() && ci != "0" && ci != "false" => return Some(format!("CI={ci}")),
        _ => {}
    }
    let loadavg = std::fs::read_to_string("/proc/loadavg").ok()?;
    let one_minute: f64 = loadavg.split_whitespace().next()?.parse().ok()?;
    let cores = std::thread::available_parallelism().ok()?.get() as f64;
    (one_minute > cores).then(|| format!("load {one_minute:.2} over {cores:.0} cores"))
}

/// A tree of terminal children under budgeted supervisors: the shape the
/// roll-up walks, with the spend that makes it walk.
fn seed_fleet(store: &dyn Store) -> aether_store::Result<usize> {
    let at = now();
    let mut seeded = 0;
    for _ in 0..SUPERVISORS {
        let supervisor = AgentId::new();
        let mut root = row(supervisor, None, at);
        root.status = AgentStatus::Waiting {
            condition: WaitCondition::Inbox,
        };
        // High enough that the pass totals every subtree without stopping it.
        root.config.cost_budget = Some(1_000_000.0);
        let mut commit = Commit::new().mutate(Mutation::UpsertAgent(root));
        seeded += 1;
        for _ in 0..CHILDREN_EACH {
            let mut child = row(AgentId::new(), Some(supervisor), at);
            child.status = AgentStatus::Completed {
                summary: Some(
                    "Implemented the requested change and verified the affected tests. ".repeat(8),
                ),
            };
            child.stats.usage = Usage {
                input_tokens: 1_000,
                output_tokens: 500,
                cost_usd: 0.01,
                ..Default::default()
            };
            commit.push_mutation(Mutation::UpsertAgent(child));
            seeded += 1;
        }
        store.commit(commit)?;
    }
    Ok(seeded)
}

fn row(id: AgentId, parent: Option<AgentId>, at: aether_core::time::Timestamp) -> Agent {
    Agent {
        archived: false,
        id,
        parent,
        role: "worker".into(),
        name: "w".into(),
        scope: Scope::default(),
        model: ModelRef::new("anthropic", "claude-sonnet-5-5"),
        fallbacks: Vec::new(),
        status: AgentStatus::Created,
        current_task: None,
        inbox_cursor: 0,
        checkpoint: None,
        engine_session: None,
        engine_usage: None,
        engine_sent: None,
        status_since: None,
        paused: false,
        paused_via: None,
        resume_wait: None,
        restarted_from: None,
        restarted_as: None,
        engine_alive: false,
        last_event_at: None,
        config: AgentConfig {
            instructions: Some(
                "Follow the task scope, inspect the surrounding code, and verify the change. "
                    .repeat(60),
            ),
            ..Default::default()
        },
        stats: Default::default(),
        run: Default::default(),
        created_at: at,
        updated_at: at,
    }
}

fn median(samples: &mut [Duration]) -> Duration {
    samples.sort_unstable();
    samples[samples.len() / 2]
}

/// An import caps bytes and records but not tree depth, and `runs report` is
/// served on the dispatch path. The report's roll-up used to walk every
/// ancestor of every agent with a linear scan over the path it had walked,
/// which is cubic in a chain: 5000 deep is ~2e10 comparisons and the daemon
/// hangs on the first report of that run. Folded child-into-parent it is one
/// pass, and the ceiling here is orders of magnitude above what that costs.
const REPORT_CEILING: Duration = Duration::from_secs(2);

#[test]
#[ignore = "timing; release only - see the module docs"]
fn a_report_over_a_deep_chain_does_not_walk_every_ancestor() {
    use aether_core::event::Event;
    use aether_core::id::{EventId, RunId};

    const DEPTH: usize = 5_000;
    let run = RunId::new();
    let at = now();
    let ids: Vec<AgentId> = (0..DEPTH).map(|_| AgentId::new()).collect();
    let mut events: Vec<Event> = Vec::with_capacity(DEPTH * 2);
    let mut seq = 0u64;
    let mut push = |kind: EventKind, agent: AgentId, events: &mut Vec<Event>| {
        seq += 1;
        events.push(Event::from_draft(
            EventDraft::new(Actor::System, kind).agent(agent).run(run),
            EventId::new(),
            seq,
            at,
        ));
    };
    for (i, id) in ids.iter().enumerate() {
        push(
            EventKind::AgentCreated {
                role: aether_core::agent::Role::new("link"),
                name: "link".into(),
                parent: i.checked_sub(1).map(|p| ids[p]),
                model: ModelRef::new("mock", "m"),
                fallbacks: Vec::new(),
                scope: Scope::default(),
            },
            *id,
            &mut events,
        );
        push(
            EventKind::InvocationCompleted {
                invocation: aether_core::id::InvocationId::new(),
                snapshot: aether_core::id::SnapshotId::new(),
                model: ModelRef::new("mock", "m"),
                usage: Usage {
                    cost_usd: 0.5,
                    ..Default::default()
                },
                stop_reason: None,
                duration_ms: 1,
            },
            *id,
            &mut events,
        );
    }

    let started = Instant::now();
    let report = aether_core::report::build(run, &events, at);
    let took = started.elapsed();
    let root = report
        .body
        .agents
        .iter()
        .find(|a| a.id == ids[0])
        .expect("root");
    assert_eq!(root.subtree_usage.cost_usd, 0.5 * DEPTH as f64);
    assert!(
        took < REPORT_CEILING,
        "a report over a {DEPTH}-deep chain took {took:?}"
    );
}
