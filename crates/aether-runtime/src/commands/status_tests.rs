use super::*;
use crate::{CreateAgentSpec, RuntimeConfig};
use aether_core::event::{Actor, EventDraft};
use aether_core::id::EventId;
use aether_core::model::{ModelRef, Usage};
use aether_store::{ImportedEvent, Mutation};
use std::sync::Arc;

async fn runtime(dir: &std::path::Path) -> Arc<Runtime> {
    let store = aether_store::SqliteStore::open_in_memory().expect("store");
    runtime_on(dir, Arc::new(store)).await
}

async fn runtime_on(dir: &std::path::Path, store: Arc<dyn aether_store::Store>) -> Arc<Runtime> {
    Runtime::start_with_engines(
        RuntimeConfig {
            default_model: Some(ModelRef::new("anthropic", "claude-sonnet-5-5")),
            workspace_root: Some(dir.to_path_buf()),
            ..Default::default()
        },
        store,
        Arc::new(aether_model::Models::load(Some(dir.to_path_buf())).expect("models")),
        Arc::new(aether_tools::ToolRegistry::with_builtins()),
        None,
        Default::default(),
    )
    .await
    .expect("runtime")
}

fn billed(cost: f64) -> Usage {
    Usage {
        input_tokens: 1,
        cost_usd: cost,
        ..Default::default()
    }
}

/// One agent whose receipts land `minutes_ago`, each the agent's running total
/// and the running total of its turn, and whose row bills `total` now.
fn bill(runtime: &Runtime, receipts: &[(i64, f64, f64)], total: f64) -> Agent {
    bill_in(runtime, None, receipts, total)
}

/// [`bill`], in `run` when one is given.
fn bill_in(
    runtime: &Runtime,
    run: Option<RunId>,
    receipts: &[(i64, f64, f64)],
    total: f64,
) -> Agent {
    let (mut agent, _) = runtime
        .create_agent(
            CreateAgentSpec {
                role: "worker".into(),
                ..Default::default()
            },
            Actor::user(),
        )
        .expect("agent");
    agent.run = run.unwrap_or(agent.run);
    let at = now();
    let events = receipts
        .iter()
        .map(|&(minutes_ago, total, turn)| ImportedEvent {
            id: EventId::new(),
            draft: EventDraft::new(
                Actor::user(),
                EventKind::AgentUsageUpdated {
                    total: billed(total),
                    turn_usage: billed(turn),
                    billing: None,
                },
            )
            .agent(agent.id)
            .run(agent.run),
            at: at - chrono::Duration::minutes(minutes_ago),
        })
        .collect();
    agent.created_at = at - chrono::Duration::hours(2);
    agent.stats.usage = billed(total);
    runtime
        .store
        .import(events, vec![Mutation::UpsertAgent(agent.clone())])
        .expect("import");
    agent
}

fn run_of(runtime: &Runtime, agent: &Agent) -> RunSummary {
    let status = runtime.status().expect("status");
    status
        .runs
        .into_iter()
        .find(|r| r.run == agent.run)
        .expect("the live run is summarised")
}

#[tokio::test]
async fn a_run_is_rated_on_what_it_billed_since_the_window_opened() {
    let dir = tempfile::tempdir().expect("tempdir");
    let runtime = runtime(dir.path()).await;
    // $10 before the window; inside it the turn that had billed $1 by the
    // last earlier receipt reaches $3, so $3 of its total is recent.
    let agent = bill(
        &runtime,
        &[
            (60, 10.0, 10.0),
            (20, 11.0, 1.0),
            (10, 12.0, 2.0),
            (5, 14.0, 4.0),
        ],
        14.0,
    );
    let run = run_of(&runtime, &agent);
    assert!((run.rate_usd_per_hour - 12.0).abs() < 1e-9, "{run:?}");
    assert!((run.forecast_usd_next_hour - 26.0).abs() < 1e-9, "{run:?}");
    runtime.shutdown().await;
}

#[tokio::test]
async fn a_run_whose_last_receipt_is_older_than_the_window_is_idle() {
    let dir = tempfile::tempdir().expect("tempdir");
    let runtime = runtime(dir.path()).await;
    let agent = bill(
        &runtime,
        &[
            (90, 400.0, 400.0),
            (RATE_WINDOW_MINUTES + 1, 843.18, 443.18),
        ],
        843.18,
    );
    let run = run_of(&runtime, &agent);
    assert_eq!(run.rate_usd_per_hour, 0.0, "{run:?}");
    assert_eq!(run.forecast_usd_next_hour, 843.18);
    runtime.shutdown().await;
}

/// With no earlier receipt kept, the window opens where the billing turn
/// began: never at zero, which would rate the agent's whole life as recent.
#[tokio::test]
async fn without_an_earlier_receipt_the_window_opens_at_the_turn() {
    let dir = tempfile::tempdir().expect("tempdir");
    let runtime = runtime(dir.path()).await;
    let agent = bill(&runtime, &[(10, 100.0, 1.0), (5, 102.0, 3.0)], 102.0);
    let run = run_of(&runtime, &agent);
    assert!((run.rate_usd_per_hour - 12.0).abs() < 1e-9, "{run:?}");
    runtime.shutdown().await;
}

#[test]
fn every_section_is_answered_by_exactly_one_reader() {
    let answered: Vec<&str> = READERS
        .iter()
        .flat_map(|r| r.sections.iter().copied())
        .collect();
    assert_eq!(
        answered, SECTIONS,
        "readers answer the sections, once each, in order"
    );
    let keys: BTreeSet<&str> = READERS.iter().map(|r| r.sections[0]).collect();
    assert_eq!(
        keys.len(),
        READERS.len(),
        "each reader has its own in-flight key"
    );
}

#[tokio::test]
async fn a_status_with_patience_reads_every_section_of_a_quiet_store() {
    let dir = tempfile::tempdir().expect("dir");
    let runtime = runtime(dir.path()).await;
    // Generous, so a loaded test host cannot make a quiet store look late.
    let status = runtime
        .status_within(Duration::from_secs(10))
        .expect("status");
    assert_eq!(
        status.unavailable,
        Vec::new(),
        "nothing is late on a quiet store"
    );
    // A reader leaves the set just after it answers, so give it a moment.
    let deadline = Instant::now() + Duration::from_secs(5);
    while !runtime.status_in_flight.lock().is_empty() {
        assert!(
            Instant::now() < deadline,
            "every reader left the in-flight set"
        );
        std::thread::sleep(Duration::from_millis(5));
    }
}

#[tokio::test]
async fn a_held_scheduler_costs_only_the_sections_behind_it() {
    let dir = tempfile::tempdir().expect("dir");
    let runtime = runtime(dir.path()).await;
    let held = runtime.sched.lock();
    let started = Instant::now();
    let status = runtime
        .status_within(Duration::from_secs(2))
        .expect("status");
    assert!(
        started.elapsed() < Duration::from_secs(7),
        "status kept to its deadline with the scheduler held"
    );
    drop(held);
    let late: Vec<&str> = status
        .unavailable
        .iter()
        .map(|u| u.section.as_str())
        .collect();
    assert!(late.contains(&section::ENGINES), "{late:?}");
    for read in [
        section::USAGE_TOTAL,
        section::PENDING_APPROVALS,
        section::TASKS_ACTIVE,
    ] {
        assert!(
            !late.contains(&read),
            "{read} does not wait on the scheduler: {late:?}"
        );
    }
    let deadline = Instant::now() + Duration::from_secs(10);
    while !runtime.status_in_flight.lock().is_empty() {
        assert!(
            Instant::now() < deadline,
            "the held readers ended once released"
        );
        std::thread::sleep(Duration::from_millis(5));
    }
    runtime.shutdown().await;
}

#[tokio::test]
async fn a_status_right_after_another_reads_every_section_again() {
    let dir = tempfile::tempdir().expect("dir");
    let runtime = runtime(dir.path()).await;
    for attempt in 0..50 {
        let status = runtime.status().expect("status");
        assert_eq!(
            status.unavailable,
            Vec::new(),
            "attempt {attempt}: a reader that answered is no longer in flight"
        );
    }
    runtime.shutdown().await;
}

/// Before any read has finished, every section is named unavailable, so no
/// client reads its zeros as counts.
#[tokio::test]
async fn an_unread_status_names_every_section_unavailable() {
    let dir = tempfile::tempdir().expect("dir");
    let runtime = runtime(dir.path()).await;
    let unread = runtime.status_unread();
    let named: Vec<&str> = unread
        .unavailable
        .iter()
        .map(|u| u.section.as_str())
        .collect();
    assert_eq!(named, SECTIONS, "every section, in order");
    assert!(
        unread
            .unavailable
            .iter()
            .all(|u| u.reason == section::TIMED_OUT)
    );
    assert_eq!(
        unread.max_concurrent_turns,
        runtime.config.max_concurrent_turns
    );
    runtime.shutdown().await;
}

/// Retention removed the receipt before the window: the rate opens at the
/// billing turn, as with none kept, rather than failing the whole `status`.
#[tokio::test]
async fn a_receipt_removed_by_retention_opens_the_window_at_the_turn() {
    let dir = tempfile::tempdir().expect("tempdir");
    let path = dir.path().join("aether.db");
    let store = aether_store::SqliteStore::open(&path).expect("store");
    let runtime = runtime_on(dir.path(), Arc::new(store)).await;
    let agent = bill(
        &runtime,
        &[(90, 400.0, 400.0), (10, 12.0, 2.0), (5, 14.0, 4.0)],
        14.0,
    );
    let conn = rusqlite::Connection::open(&path).expect("raw connection");
    let oldest: i64 = conn
        .query_row(
            "SELECT MIN(seq) FROM events WHERE agent_id = ?1 AND type = 'agent.usage_updated'",
            [agent.id.to_string()],
            |row| row.get(0),
        )
        .expect("the oldest receipt");
    conn.execute(
        "INSERT INTO expired_event_spans SELECT run, COALESCE(agent_id,''), \
         COALESCE(parent_agent_id,''), COALESCE(task_id,''), type, family, seq, seq \
         FROM events WHERE seq = ?1",
        [oldest],
    )
    .expect("expiry span");
    assert_eq!(
        conn.execute("DELETE FROM events WHERE seq = ?1", [oldest])
            .unwrap(),
        1
    );
    let run = run_of(&runtime, &agent);
    // From the turn's start at $10 to $14, in a quarter of an hour.
    assert!((run.rate_usd_per_hour - 16.0).abs() < 1e-9, "{run:?}");
    assert!(
        !run.rate_approximate,
        "every kept receipt was read: {run:?}"
    );
    runtime.shutdown().await;
}

/// A run with more billers in the window than a `status` looks up is rated
/// on those it did, and says the rate is a floor.
#[tokio::test]
async fn a_run_with_more_billers_than_one_read_looks_up_is_a_floor() {
    let dir = tempfile::tempdir().expect("tempdir");
    let runtime = runtime(dir.path()).await;
    let first = bill(&runtime, &[(5, 2.0, 1.0)], 2.0);
    for _ in 0..BASELINE_LOOKUPS {
        bill_in(&runtime, Some(first.run), &[(5, 2.0, 1.0)], 2.0);
    }
    let run = run_of(&runtime, &first);
    assert!(run.rate_approximate, "{run:?}");
    let looked_up = BASELINE_LOOKUPS as f64 * 4.0;
    assert!((run.rate_usd_per_hour - looked_up).abs() < 1e-9, "{run:?}");
    runtime.shutdown().await;
}
