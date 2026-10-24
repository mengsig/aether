use super::*;

fn memory(text: &str) -> MemoryRecord {
    MemoryRecord::from_candidate(
        MemoryCandidate {
            scope: MemoryScope::Global,
            kind: MemoryKind::Fact,
            text: text.into(),
            structured: None,
            importance: 0.9,
            confidence: 0.9,
            tags: vec![],
            provenance: MemoryProvenance::default(),
            supersedes: None,
        },
        now(),
    )
}

fn pragma(path: &std::path::Path, name: &str) -> i64 {
    rusqlite::Connection::open(path)
        .expect("open")
        .query_row(&format!("PRAGMA {name}"), [], |row| row.get(0))
        .expect("pragma")
}

fn found(store: &SqliteStore, text: &str) -> Vec<MemoryId> {
    store
        .memories(&MemoryQuery {
            text: Some(text.into()),
            ..Default::default()
        })
        .expect("search")
        .into_iter()
        .map(|record| record.id)
        .collect()
}

fn padded_events(agent: AgentId, count: usize) -> Commit {
    (0..count).fold(Commit::new(), |commit, n| {
        commit.event(
            EventDraft::new(
                Actor::System,
                EventKind::Custom {
                    name: "padding".into(),
                    payload: serde_json::json!({"n": n, "pad": "p".repeat(1500)}),
                },
            )
            .agent(agent),
        )
    })
}

#[test]
fn a_store_from_before_incremental_vacuum_is_converted_with_its_search_intact() {
    let dir = tempfile::tempdir().expect("tempdir");
    let path = dir.path().join("state.db");
    let records = [
        memory("alpha harbour"),
        memory("beta harbour"),
        memory("gamma"),
    ];
    let agent = AgentId::new();
    {
        let store = SqliteStore::open(&path).expect("open");
        let commit = records
            .iter()
            .fold(padded_events(agent, 50), |commit, record| {
                commit.mutate(Mutation::PutMemory(record.clone()))
            });
        store.commit(commit).expect("seed");
    }
    {
        let conn = rusqlite::Connection::open(&path).expect("raw");
        conn.execute_batch("PRAGMA auto_vacuum = NONE; VACUUM;")
            .expect("old mode");
        // A gap in the implicit rowids, which a VACUUM may close.
        conn.execute(
            "DELETE FROM memories WHERE id = ?1",
            [records[0].id.to_string()],
        )
        .expect("gap");
    }
    assert_eq!(pragma(&path, "auto_vacuum"), 0, "the old store's mode");

    let store = SqliteStore::open(&path).expect("reopen converts");
    assert_eq!(pragma(&path, "auto_vacuum"), 2);
    assert_eq!(found(&store, "harbour"), vec![records[1].id]);
    assert_eq!(found(&store, "gamma"), vec![records[2].id]);
    let history = store
        .events(0, 100, &EventFilter::for_agent(agent))
        .expect("history");
    assert_eq!(history.len(), 50, "no event lost");
    drop(store);
    SqliteStore::open(&path).expect("a converted store reopens as it is");
}

#[test]
fn freed_pages_go_back_to_the_filesystem() {
    let dir = tempfile::tempdir().expect("tempdir");
    let path = dir.path().join("state.db");
    let store = SqliteStore::open(&path).expect("open");
    let agent = AgentId::new();
    store.commit(padded_events(agent, 2000)).expect("history");
    assert!(store.sync().expect("checkpoint"));
    let grown = std::fs::metadata(&path).expect("size").len();
    store
        .conn
        .lock()
        .execute("DELETE FROM events", [])
        .expect("expire");
    assert!(pragma(&path, "freelist_count") > 0, "deletion frees pages");

    let reclaimed = store.reclaim_free_pages().expect("reclaim");
    assert!(reclaimed > 0);
    assert_eq!(store.reclaim_free_pages().expect("again"), 0, "idempotent");
    assert!(store.sync().expect("checkpoint"));
    let shrunk = std::fs::metadata(&path).expect("size").len();
    assert!(shrunk < grown / 2, "{grown} bytes shrank only to {shrunk}");
}
