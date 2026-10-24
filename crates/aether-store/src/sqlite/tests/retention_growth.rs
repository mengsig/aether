use super::*;
use crate::{RunExpiry, WriteGate};
use aether_core::{
    id::SnapshotId,
    snapshot::{InvocationParams, InvocationSnapshot},
};

#[test]
fn repeated_file_backed_expiry_reuses_payload_pages_and_survives_reopen() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("retention.db");
    let store = SqliteStore::open(&path).unwrap();
    let gate = WriteGate::new();
    let old = now() - chrono::Duration::days(14);
    let cutoff = now() - chrono::Duration::days(7);
    let live = agent();
    store
        .commit(Commit::new().mutate(Mutation::UpsertAgent(live.clone())))
        .unwrap();
    let mut warm_pages = 0;
    let mut maximum_pages = 0;
    let mut last_run = None;
    for cycle in 0..12 {
        let mut finished = agent();
        finished.status = AgentStatus::Completed { summary: None };
        finished.created_at = old;
        finished.updated_at = old;
        last_run = Some(finished.run);
        let events = (0..64)
            .map(|_| ImportedEvent {
                id: EventId::new(),
                at: old,
                draft: EventDraft::new(
                    Actor::System,
                    EventKind::Custom {
                        name: "payload".into(),
                        // Inline strings: a blob would compress the payload away.
                        payload: serde_json::json!({"text": vec!["x".repeat(1024); 16]}),
                    },
                )
                .agent(finished.id)
                .run(finished.run),
            })
            .collect();
        let snapshot = InvocationSnapshot {
            id: SnapshotId::new(),
            agent_id: finished.id,
            task_id: None,
            turn: 1,
            model: finished.model.clone(),
            adapter: None,
            capabilities: None,
            memory_ids: vec![],
            layers: vec![],
            messages: vec![aether_core::model::Message::user_text(
                "s".repeat(256 * 1024),
            )],
            tools: vec![],
            params: InvocationParams::default(),
            token_estimate: 0,
            created_at: old,
        };
        store
            .import(
                events,
                vec![
                    Mutation::UpsertAgent(finished.clone()),
                    Mutation::PutSnapshot(snapshot),
                ],
            )
            .unwrap();
        let pages: i64 = store
            .conn
            .lock()
            .query_row("PRAGMA page_count", [], |row| row.get(0))
            .unwrap();
        maximum_pages = maximum_pages.max(pages);
        if cycle == 1 {
            warm_pages = pages;
        }
        assert_eq!(
            store
                .expire_run_history(finished.run, cutoff, &gate.hold())
                .unwrap(),
            RunExpiry::Expired {
                events: 64,
                rows: 2
            }
        );
        assert!(store.agent(live.id).unwrap().is_some());
        let conn = store.conn.lock();
        let free: i64 = conn
            .query_row("PRAGMA freelist_count", [], |row| row.get(0))
            .unwrap();
        let size: i64 = conn
            .query_row("PRAGMA page_size", [], |row| row.get(0))
            .unwrap();
        // Checkpoint without VACUUM or truncation: freed pages must be reused
        // naturally rather than making the assertion pass by rebuilding SQLite.
        let (busy, log, checkpointed): (i64, i64, i64) = conn
            .query_row("PRAGMA wal_checkpoint(PASSIVE)", [], |row| {
                Ok((row.get(0)?, row.get(1)?, row.get(2)?))
            })
            .unwrap();
        assert_eq!(busy, 0);
        assert_eq!(log, checkpointed);
        println!(
            "cycle={cycle} pages={pages} free={free} page_size={size} db_bytes={} wal_bytes={}",
            std::fs::metadata(&path).unwrap().len(),
            std::fs::metadata(dir.path().join("retention.db-wal"))
                .unwrap()
                .len()
        );
        assert!(free * size >= 1024 * 1024, "payload pages were not freed");
    }
    // A small metadata allowance, not a promise of unbounded-time stability:
    // each run currently retains expiry spans and those still need a cap.
    assert!(
        maximum_pages <= warm_pages + 128,
        "payload growth continued after warmup: {warm_pages} -> {maximum_pages}"
    );
    assert_eq!(store.last_seq().unwrap(), 12 * 64);
    drop(store);
    let reopened = SqliteStore::open(&path).unwrap();
    assert_eq!(reopened.last_seq().unwrap(), 12 * 64);
    assert!(reopened.agent(live.id).unwrap().is_some());
    assert!(matches!(
        reopened.events(0, 1, &EventFilter::for_run(last_run.unwrap())),
        Err(StoreError::HistoryExpired { .. })
    ));
    let spans: i64 = reopened
        .conn
        .lock()
        .query_row("SELECT COUNT(*) FROM expired_event_spans", [], |row| {
            row.get(0)
        })
        .unwrap();
    assert_eq!(spans, 12);
}
