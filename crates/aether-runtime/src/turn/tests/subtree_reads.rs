//! The per-turn and per-delegation fleet reads load only the agents they
//! need, and agree exactly with what reading the whole fleet gave.
use super::*;
use crate::RunningTurn;
use crate::scheduler::{CapHolder, cap_holder, on_provider_descendants};
use aether_core::agent::AgentStatus;
use aether_core::budget::subtree_usage;
use aether_store::AgentFilter;
use std::time::Instant;

async fn quiet_runtime() -> (Arc<Runtime>, tempfile::TempDir) {
    let (runtime, dir) = crate::test_runtime(|_| {}).await;
    runtime.shutdown.cancel();
    (runtime, dir)
}

fn put(runtime: &Runtime, agent: &Agent) {
    runtime
        .store
        .commit(Commit::new().mutate(Mutation::UpsertAgent(agent.clone())))
        .expect("store the row");
}

/// A live stored agent under `parent` that has spent `tokens`.
fn member(runtime: &Runtime, parent: Option<AgentId>, tokens: u64) -> Agent {
    let (created, _) = runtime
        .create_agent(
            CreateAgentSpec {
                role: "worker".into(),
                parent,
                config: Some(aether_core::agent::AgentConfig {
                    effort: Some(aether_core::model::Effort::Low),
                    ..Default::default()
                }),
                ..Default::default()
            },
            Actor::user(),
        )
        .expect("agent");
    let mut row = runtime.store.agent(created.id).expect("read").expect("row");
    row.stats.usage.output_tokens = tokens;
    put(runtime, &row);
    row
}

/// Moves `id` to `status` once its children exist: a terminal agent cannot be a parent.
fn settle(runtime: &Runtime, id: AgentId, status: AgentStatus) -> Agent {
    let mut row = runtime.store.agent(id).expect("read").expect("row");
    row.status = status;
    put(runtime, &row);
    row
}

fn hold_turn(runtime: &Runtime, agent: AgentId) {
    runtime.sched.lock().running.insert(
        agent,
        RunningTurn {
            task: None,
            cancel: CancellationToken::new(),
            cancel_request: None,
            stop_reason: None,
            interrupt: CancellationToken::new(),
            addressed: false,
            provider: "anthropic".into(),
            engine: false,
            on_provider: true,
        },
    );
}

fn completed() -> AgentStatus {
    AgentStatus::Completed { summary: None }
}

fn cancelled() -> AgentStatus {
    AgentStatus::Cancelled { reason: None }
}

/// What `turn_refused` read before: the roll-up over every stored agent.
fn refused_over_the_whole_fleet(runtime: &Runtime, agent: &Agent) -> Option<String> {
    let all = runtime
        .store
        .agents(&AgentFilter::default())
        .expect("fleet");
    let usage = subtree_usage(&all)[&agent.id];
    agent.budget_exceeded(&usage)
}

fn budgeted(runtime: &Runtime, id: AgentId, tokens: u64) -> Agent {
    let mut row = runtime.store.agent(id).expect("read").expect("row");
    row.config.token_budget = Some(tokens);
    put(runtime, &row);
    row
}

#[tokio::test]
async fn a_turn_is_refused_on_the_subtree_roll_up_exactly_as_on_the_whole_fleet() {
    let (runtime, _dir) = quiet_runtime().await;
    let ancestor = member(&runtime, None, 10_000);
    let root = member(&runtime, Some(ancestor.id), 50);
    let done = member(&runtime, Some(root.id), 200);
    let stopped = member(&runtime, Some(root.id), 300);
    let paused = member(&runtime, Some(root.id), 100);
    let archived = member(&runtime, Some(root.id), 150);
    let failed = member(&runtime, Some(stopped.id), 100);
    member(&runtime, Some(done.id), 0);
    settle(
        &runtime,
        failed.id,
        AgentStatus::Failed { error: "x".into() },
    );
    settle(&runtime, done.id, completed());
    settle(&runtime, stopped.id, cancelled());
    let mut row = settle(&runtime, paused.id, AgentStatus::Ready);
    row.paused = true;
    put(&runtime, &row);
    let mut row = settle(&runtime, archived.id, completed());
    row.archived = true;
    put(&runtime, &row);
    for _ in 0..12 {
        let outside = member(&runtime, None, 9_999);
        settle(&runtime, outside.id, completed());
    }

    // Own 50 plus descendants 200 + 300 + 100 + 150 + 100 = 900.
    for limit in [1, 899, 900, 901, 10_049, 10_050] {
        let agent = budgeted(&runtime, root.id, limit);
        let expected = refused_over_the_whole_fleet(&runtime, &agent);
        let got = turn_refused(&runtime, &agent, 1).expect("refusal");
        assert_eq!(got, expected, "limit {limit}");
        assert_eq!(got.is_some(), limit <= 900, "limit {limit}");
    }
    let agent = budgeted(&runtime, root.id, 900);
    assert_eq!(
        turn_refused(&runtime, &agent, 1).expect("refusal"),
        Some("token budget of 900: 900 used".into())
    );
}

fn describe(status: &CapStatus) -> String {
    match status {
        CapStatus::Counted {
            running,
            cap,
            source,
            queued,
        } => format!("counted {running}/{cap} {source} queued={queued}"),
        CapStatus::Unknown { cap, source, error } => format!("unknown {cap} {source} {error}"),
    }
}

/// What `cap_status` computed before: the same predicates over the whole fleet.
fn cap_over_the_whole_fleet(runtime: &Runtime, agent: &Agent, engine_cap: Option<usize>) -> String {
    let live = runtime
        .store
        .agents(&AgentFilter::default())
        .expect("fleet");
    let sched = runtime.sched.lock();
    let global = runtime.config.max_concurrent_turns;
    let status = match cap_holder(agent.id, &live, &sched, global, engine_cap) {
        CapHolder::Spent {
            agent: holder,
            running,
            cap,
        } => {
            let source = match holder {
                Some(id) if id == agent.id => "your concurrency cap",
                Some(_) => "an ancestor's concurrency cap",
                None => "the daemon's global concurrency limit",
            };
            CapStatus::Counted {
                running,
                cap,
                source,
                queued: true,
            }
        }
        CapHolder::EngineCap { running, cap } => CapStatus::Counted {
            running,
            cap,
            source: "the daemon's engine-turn cap",
            queued: true,
        },
        CapHolder::Clear => {
            let (cap, source) = effective_cap(runtime, agent);
            CapStatus::Counted {
                running: on_provider_descendants(agent.id, &live, &sched),
                cap,
                source,
                queued: false,
            }
        }
    };
    describe(&status)
}

#[tokio::test]
async fn the_cap_advisory_counts_exactly_what_the_whole_fleet_gives() {
    let (runtime, _dir) = quiet_runtime().await;
    let top = member(&runtime, None, 0);
    let delegator = member(&runtime, Some(top.id), 0);
    let sibling = member(&runtime, Some(top.id), 0);
    let finished = member(&runtime, Some(delegator.id), 0);
    let deep = member(&runtime, Some(finished.id), 0);
    let live = member(&runtime, Some(delegator.id), 0);
    let stranger = member(&runtime, None, 0);
    // A running turn whose stored status is already terminal still holds its slot.
    let done_but_running = member(&runtime, Some(delegator.id), 0);
    settle(&runtime, finished.id, completed());
    settle(&runtime, done_but_running.id, completed());
    for _ in 0..20 {
        let outside = member(&runtime, None, 0);
        settle(&runtime, outside.id, completed());
    }

    let runs = [
        vec![],
        vec![live.id],
        vec![live.id, deep.id],
        vec![live.id, deep.id, done_but_running.id, sibling.id],
        vec![deep.id, stranger.id],
        vec![
            live.id,
            deep.id,
            done_but_running.id,
            sibling.id,
            stranger.id,
        ],
    ];
    for running in runs {
        runtime.sched.lock().running.clear();
        for id in &running {
            hold_turn(&runtime, *id);
        }
        for (top_cap, own_cap) in [
            (None, None),
            (Some(1), None),
            (Some(2), Some(9)),
            (Some(9), Some(2)),
        ] {
            let mut top_row = runtime.store.agent(top.id).expect("read").expect("row");
            top_row.config.concurrency = top_cap;
            put(&runtime, &top_row);
            let mut own = runtime
                .store
                .agent(delegator.id)
                .expect("read")
                .expect("row");
            own.config.concurrency = own_cap;
            put(&runtime, &own);
            let expected = cap_over_the_whole_fleet(&runtime, &own, None);
            let got = describe(&cap_status(&runtime, &own, false));
            assert_eq!(
                got, expected,
                "running {running:?} caps {top_cap:?}/{own_cap:?}"
            );
        }
    }
}

/// `cargo test -p aether-runtime --release -- --ignored --nocapture fleet_read_timing`
#[tokio::test]
#[ignore = "timing bench; run by hand"]
async fn fleet_read_timing() {
    const OUTSIDE: usize = 20_000;
    const ROUNDS: u32 = 5;
    let (runtime, _dir) = quiet_runtime().await;
    let root = member(&runtime, None, 10);
    let child = member(&runtime, Some(root.id), 20);
    let grandchild = member(&runtime, Some(child.id), 30);
    settle(&runtime, grandchild.id, cancelled());
    let child = settle(&runtime, child.id, completed());
    let agent = budgeted(&runtime, root.id, 1_000_000);
    let mut template = child.clone();
    template.parent = None;
    template.archived = true;
    for batch in 0..(OUTSIDE / 1_000) {
        let mut commit = Commit::new();
        for _ in 0..1_000 {
            let mut row = template.clone();
            row.id = AgentId::new();
            row.created_at += chrono::Duration::milliseconds(batch as i64);
            commit = commit.mutate(Mutation::UpsertAgent(row));
        }
        runtime.store.commit(commit).expect("bulk");
    }
    hold_turn(&runtime, child.id);

    let time = |work: &dyn Fn()| {
        let start = Instant::now();
        for _ in 0..ROUNDS {
            work();
        }
        start.elapsed() / ROUNDS
    };
    let refused_old = time(&|| {
        refused_over_the_whole_fleet(&runtime, &agent);
    });
    let refused_new = time(&|| {
        turn_refused(&runtime, &agent, 1).expect("refusal");
    });
    let cap_old = time(&|| {
        cap_over_the_whole_fleet(&runtime, &agent, None);
    });
    let cap_new = time(&|| {
        cap_status(&runtime, &agent, false);
    });
    println!(
        "turn_refused: before {refused_old:?}, after {refused_new:?} ({OUTSIDE} agents outside)"
    );
    println!("cap_status:   before {cap_old:?}, after {cap_new:?}");
    assert!(refused_new < Duration::from_millis(50), "{refused_new:?}");
    assert!(cap_new < Duration::from_millis(50), "{cap_new:?}");
}
