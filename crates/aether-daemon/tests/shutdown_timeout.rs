#![allow(clippy::expect_used, clippy::unwrap_used, clippy::panic)]

mod support;

use std::io::Write;
use std::sync::{Arc, Mutex};

use aether_store::{SqliteStore, Store};
use serde_json::json;
use support::{RawConn, TestDaemon, await_lock_free};

struct Log(Arc<Mutex<Vec<u8>>>);

impl Write for Log {
    fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
        self.0.lock().unwrap().write(bytes)
    }

    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_reader_delaying_final_sync_does_not_hold_up_shutdown_or_lose_commits() {
    let output = Arc::new(Mutex::new(Vec::new()));
    let writer = output.clone();
    tracing_subscriber::fmt()
        .with_ansi(false)
        .with_writer(move || Log(writer.clone()))
        .try_init()
        .unwrap();
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
    let reply = connection
        .call(json!({
            "cmd": "create_agent", "role": "committed", "start": false,
            "model": {"provider": "claude-code", "model": "haiku"}
        }))
        .await;
    let id: aether_core::id::AgentId = reply["result"]["Ok"]["data"]["id"]
        .as_str()
        .unwrap()
        .parse()
        .unwrap();
    drop(connection);
    let dir = daemon.stop_serving().await;
    let log = String::from_utf8(output.lock().unwrap().clone()).unwrap();
    assert!(
        log.contains("final store sync") && log.contains("exceeded its deadline"),
        "{log}"
    );
    let observer =
        rusqlite::Connection::open_with_flags(&path, rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY)
            .unwrap();
    let count: i64 = observer
        .query_row(
            "SELECT count(*) FROM agents WHERE id = ?1",
            [id.to_string()],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(count, 1, "the committed row remains visible in the WAL");
    drop(observer);
    reader.execute_batch("ROLLBACK").unwrap();
    drop(reader);
    await_lock_free(&path.with_extension("db.lock")).await;
    let reopened = SqliteStore::open(&path).unwrap();
    assert!(reopened.agent(id).unwrap().is_some());
    drop(reopened);
    drop(dir);
}
