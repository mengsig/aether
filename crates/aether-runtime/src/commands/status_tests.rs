use super::*;
use crate::{CreateAgentSpec, RuntimeConfig};
use aether_core::event::{Actor, EventDraft};
use aether_core::id::EventId;
use aether_core::model::{ModelRef, Usage};
use aether_store::{ImportedEvent, Mutation};
use std::sync::Arc;

async fn runtime(dir: &std::path::Path) -> Arc<Runtime> {
    Runtime::start_with_engines(
        RuntimeConfig {
            default_model: Some(ModelRef::new("anthropic", "claude-sonnet-5-5")),
            workspace_root: Some(dir.to_path_buf()),
            ..Default::default()
        },
        Arc::new(aether_store::SqliteStore::open_in_memory().expect("store")),
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
    let (mut agent, _) = runtime
        .create_agent(
            CreateAgentSpec {
                role: "worker".into(),
                ..Default::default()
            },
            Actor::user(),
        )
        .expect("agent");
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
