//! An engine's reply that arrives while another process holds the database
//! lock past SQLite's busy timeout.
use super::*;
use crate::RuntimeConfig;
use aether_tools::ToolRegistry;
use std::time::{Duration, Instant};

/// Past the store's five-second busy timeout, so the first write is refused.
const LOCK_HELD: Duration = Duration::from_secs(7);

async fn runtime_over(database: &std::path::Path, config_dir: &std::path::Path) -> Arc<Runtime> {
    Runtime::start_with_engines(
        RuntimeConfig {
            holder: "unit-test".into(),
            default_model: Some(aether_core::model::ModelRef::new("claude-code", "haiku")),
            ..Default::default()
        },
        Arc::new(aether_store::SqliteStore::open(database).expect("store")),
        Arc::new(aether_model::Models::load(Some(config_dir.to_path_buf())).expect("models")),
        Arc::new(ToolRegistry::with_builtins()),
        None,
        std::collections::HashMap::new(),
    )
    .await
    .expect("runtime")
}

#[tokio::test]
async fn an_engine_reply_under_a_held_lock_lands_once_it_clears() {
    let dir = tempfile::tempdir().expect("dir");
    let database = dir.path().join("aether.db");
    let runtime = runtime_over(&database, dir.path()).await;
    let mut ctx = started_agent_ctx(&runtime).await;
    let snapshot = usage::bare_snapshot(ctx.agent.id);
    let resolved = runtime.models.resolve(&snapshot.model).unwrap();
    let mut state = StreamState::new(TurnPricing::default(), resolved);

    let (held_tx, held_rx) = std::sync::mpsc::channel();
    let holder = std::thread::spawn(move || {
        let lock = rusqlite::Connection::open(&database).expect("second connection");
        lock.execute_batch(
            "BEGIN EXCLUSIVE; UPDATE meta SET value = value WHERE key = 'last_seq';",
        )
        .expect("hold the lock");
        held_tx.send(()).expect("signal");
        std::thread::sleep(LOCK_HELD);
        lock.execute_batch("COMMIT;").expect("release the lock");
    });
    held_rx.recv().expect("the lock is held");
    let started = Instant::now();
    let reply = EngineEvent::Assistant {
        content: vec![ContentBlock::text("LOCKTEST")],
        usage: Some(Usage {
            input_tokens: 100,
            output_tokens: 20,
            ..Default::default()
        }),
        model: None,
    };
    handle_event(&runtime, &mut ctx, &snapshot, reply, &mut state)
        .expect("the reply waits the lock out");
    holder.join().expect("holder");
    assert!(
        started.elapsed() >= Duration::from_secs(5),
        "the lock was never met"
    );

    let replies: Vec<String> = runtime
        .store
        .events(0, 10_000, &EventFilter::for_agent(ctx.agent.id))
        .unwrap()
        .into_iter()
        .filter_map(|e| match e.kind {
            EventKind::AgentMessage { content, .. } => Some(format!("{content:?}")),
            _ => None,
        })
        .filter(|text| text.contains("LOCKTEST"))
        .collect();
    assert_eq!(replies.len(), 1, "{replies:?}");
    let stored = runtime.store.agent(ctx.agent.id).unwrap().unwrap();
    assert_eq!(stored.stats.usage.output_tokens, 20);
    runtime.shutdown().await;
}
