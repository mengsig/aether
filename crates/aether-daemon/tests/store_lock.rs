#![allow(clippy::expect_used, clippy::unwrap_used, clippy::panic)]

//! Another process holding the database lock. A write waits out SQLite's busy
//! timeout and is refused as busy; nothing else the daemon answers waits
//! behind it.

mod support;

use std::time::{Duration, Instant};

use aether_protocol::PROTOCOL_VERSION;
use serde_json::{Value, json};
use support::{RawConn, TestDaemon};

/// Well under the store's five-second busy timeout.
const ANSWER_BOUND: Duration = Duration::from_secs(3);

fn new_agent() -> Value {
    json!({
        "cmd": "create_agent", "role": "assistant", "start": false,
        "model": {"provider": "claude-code", "model": "haiku"}
    })
}

/// One worker, so a write that blocks it while the lock is held, or one
/// waiting behind it for the commit gate, would stall every other
/// connection: this is what `aether agents` hanging looked like.
#[tokio::test(flavor = "multi_thread", worker_threads = 1)]
async fn a_held_database_lock_refuses_a_write_as_busy_and_stalls_no_read() {
    let daemon = TestDaemon::start().await;
    let mut writer = RawConn::connect(&daemon.socket).await.unwrap();
    writer.hello(PROTOCOL_VERSION).await.unwrap();
    let created = writer.call(new_agent()).await;
    assert!(
        created["result"]["Ok"]["data"]["id"].is_string(),
        "{created}"
    );

    let holder = rusqlite::Connection::open(daemon.store_path()).unwrap();
    holder
        .execute_batch("BEGIN EXCLUSIVE; UPDATE meta SET value = value WHERE key = 'last_seq';")
        .unwrap();
    let started = Instant::now();
    let write = writer.request(new_agent()).await;
    // Long enough for the daemon to be inside the write's busy wait; the
    // second write then waits for the first to let go of the commit gate.
    tokio::time::sleep(Duration::from_millis(300)).await;
    let mut queued = RawConn::connect(&daemon.socket).await.unwrap();
    queued.hello(PROTOCOL_VERSION).await.unwrap();
    let behind = queued.request(new_agent()).await;
    tokio::time::sleep(Duration::from_millis(300)).await;

    // What `aether agents` does: connect, greet, list.
    let client = tokio::time::timeout(ANSWER_BOUND, aether_client::Client::connect(&daemon.socket))
        .await
        .expect("the hello waited behind the locked write")
        .unwrap();
    let agents = tokio::time::timeout(ANSWER_BOUND, client.list_agents(Default::default()))
        .await
        .expect("the list waited behind the locked write")
        .unwrap();
    assert_eq!(agents.len(), 1, "{agents:?}");
    assert!(
        started.elapsed() < ANSWER_BOUND,
        "answered only after {:?}",
        started.elapsed()
    );

    for refused in [
        writer.await_reply(write).await,
        queued.await_reply(behind).await,
    ] {
        let error = &refused["result"]["Err"];
        assert_eq!(error["code"], "unavailable", "{refused}");
        assert!(
            error["message"]
                .as_str()
                .is_some_and(|m| m.starts_with("store busy:")),
            "{refused}"
        );
    }
    drop(holder);
    let landed = writer.call(new_agent()).await;
    assert!(landed["result"]["Ok"]["data"]["id"].is_string(), "{landed}");
    daemon.stop().await;
}
