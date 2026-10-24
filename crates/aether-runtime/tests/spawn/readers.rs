use super::*;
use aether_store::TaskFilter;

/// How long the foreign writer keeps the database's write lock: under the
/// store's 5 s busy timeout, so a commit queued behind it still lands.
const WRITE_HELD: Duration = Duration::from_secs(3);
/// What a read tool may take while that write is held.
const READ_BUDGET: Duration = Duration::from_secs(1);

/// Takes the database's write lock from another connection, as a slow
/// commit on a loaded disk does, and keeps it for `WRITE_HELD`.
fn hold_the_write_lock(path: std::path::PathBuf) -> std::sync::mpsc::Receiver<()> {
    let (held_tx, held) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        let conn = rusqlite::Connection::open(&path).expect("writer connection");
        conn.execute_batch(
            "BEGIN IMMEDIATE; UPDATE meta SET value = value WHERE key = 'instance_id';",
        )
        .expect("write lock taken");
        held_tx.send(()).expect("test waiting");
        std::thread::sleep(WRITE_HELD);
        conn.execute_batch("COMMIT").expect("write released");
    });
    held
}

#[tokio::test]
async fn read_tools_answer_while_a_long_write_holds_the_database() {
    let dir = tempfile::tempdir().expect("tempdir");
    let path = dir.path().join("aether.db");
    let store: Arc<dyn Store> = Arc::new(SqliteStore::open(&path).expect("file store"));
    let mut h = harness_over(Some(store), 10, "", None).await;
    let coordinator = start_coordinator(&h, None);
    let bridge = h.ready.recv().await.expect("coordinator started");
    let task = h
        .store
        .tasks(&TaskFilter {
            owner: Some(coordinator),
            ..Default::default()
        })
        .expect("tasks")
        .first()
        .expect("the coordinator has a task")
        .id;

    let held = hold_the_write_lock(path);
    held.recv_timeout(Duration::from_secs(5))
        .expect("the writer took the lock");
    let started = tokio::time::Instant::now();
    for (tool, input) in [
        ("list_agents", json!({})),
        ("list_tasks", json!({})),
        ("read_task", json!({"task": task})),
        ("inspect_state", json!({})),
    ] {
        let answered = tokio::time::timeout(
            READ_BUDGET,
            h.runtime
                .engine_tool(coordinator, &bridge.token, tool.into(), input),
        )
        .await
        .unwrap_or_else(|_| panic!("{tool} waited behind the write"));
        let (reply, is_error) = answered.expect("bridge response");
        assert!(!is_error, "{tool}: {reply}");
    }
    assert!(
        started.elapsed() < WRITE_HELD,
        "the reads finished only after the write: {:?}",
        started.elapsed()
    );
    h.runtime.shutdown().await;
}
