#![allow(clippy::expect_used, clippy::unwrap_used, clippy::panic)]

mod support;

use aether_store::{SqliteStore, Store};
use serde_json::json;
use support::{RawConn, TestDaemon, await_lock_free, capture_logs};

async fn create_agent(connection: &mut RawConn) -> aether_core::id::AgentId {
    let reply = connection
        .call(json!({
            "cmd": "create_agent", "role": "committed", "start": false,
            "model": {"provider": "claude-code", "model": "haiku"}
        }))
        .await;
    reply["result"]["Ok"]["data"]["id"]
        .as_str()
        .unwrap()
        .parse()
        .unwrap()
}

fn assert_visible(path: &std::path::Path, ids: &[aether_core::id::AgentId]) {
    let observer =
        rusqlite::Connection::open_with_flags(path, rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY)
            .unwrap();
    for id in ids {
        let count: i64 = observer
            .query_row(
                "SELECT count(*) FROM agents WHERE id = ?1",
                [id.to_string()],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(count, 1, "the committed row remains visible in the WAL");
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_reader_delaying_final_sync_does_not_hold_up_shutdown_or_lose_commits() {
    let output = capture_logs();
    let daemon = TestDaemon::start().await;
    let path = daemon.store_path();
    let reader = rusqlite::Connection::open(&path).unwrap();
    reader
        .execute_batch("BEGIN; SELECT count(*) FROM events;")
        .unwrap();
    let mut connection = RawConn::connect(&daemon.socket).await.unwrap();
    connection
        .hello(aether_protocol::PROTOCOL_VERSION)
        .await
        .unwrap();
    let ids = [
        create_agent(&mut connection).await,
        create_agent(&mut connection).await,
    ];
    drop(connection);
    let dir = daemon.stop_serving().await;
    let log = String::from_utf8(output.lock().unwrap().clone()).unwrap();
    assert!(
        log.contains("final store sync") && log.contains("exceeded its deadline"),
        "{log}"
    );
    assert_visible(&path, &ids);
    reader.execute_batch("ROLLBACK").unwrap();
    drop(reader);
    await_lock_free(&path.with_extension("db.lock")).await;
    let reopened = SqliteStore::open(&path).unwrap();
    for id in ids {
        assert!(reopened.agent(id).unwrap().is_some());
    }
    drop(reopened);
    drop(dir);
}
