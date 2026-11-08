#![cfg(target_os = "linux")]
#![allow(clippy::expect_used, clippy::unwrap_used, clippy::panic)]

mod support;

use std::io::Write;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use serde_json::json;
use support::{RawConn, TestDaemon};
use tokio::io::{AsyncBufReadExt, BufReader};

struct Log(Arc<Mutex<Vec<u8>>>);

impl Write for Log {
    fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
        self.0.lock().unwrap().write(bytes)
    }

    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

#[test]
fn reader_child() {
    let Some(path) = std::env::var_os("AETHER_TEST_READER_DB") else {
        return;
    };
    let reader = rusqlite::Connection::open(path).unwrap();
    reader
        .execute_batch("BEGIN; SELECT count(*) FROM events;")
        .unwrap();
    println!("READER_READY");
    std::io::stdout().flush().unwrap();
    loop {
        std::thread::park();
    }
}

async fn create_agent(connection: &mut RawConn, role: &str) -> String {
    let reply = connection
        .call(json!({
            "cmd": "create_agent", "role": role, "start": false,
            "model": {"provider": "claude-code", "model": "haiku"}
        }))
        .await;
    reply["result"]["Ok"]["data"]["id"]
        .as_str()
        .unwrap()
        .to_owned()
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn daemon_reaps_live_agent_reader_before_its_final_checkpoint() {
    let output = Arc::new(Mutex::new(Vec::new()));
    let writer = output.clone();
    tracing_subscriber::fmt()
        .with_ansi(false)
        .with_writer(move || Log(writer.clone()))
        .try_init()
        .unwrap();
    let daemon = TestDaemon::start().await;
    let mut connection = RawConn::connect(&daemon.socket).await.unwrap();
    connection
        .hello(aether_protocol::PROTOCOL_VERSION)
        .await
        .unwrap();
    let agent = create_agent(&mut connection, "reader").await;
    let mut child = tokio::process::Command::new(std::env::current_exe().unwrap())
        .args(["--exact", "reader_child", "--nocapture"])
        .env("AETHER_TEST_READER_DB", daemon.store_path())
        .env("AETHER_ENGINE_AGENT", &agent)
        .stdout(std::process::Stdio::piped())
        .kill_on_drop(true)
        .spawn()
        .unwrap();
    let mut lines = BufReader::new(child.stdout.take().unwrap()).lines();
    tokio::time::timeout(Duration::from_secs(10), async {
        while let Some(line) = lines.next_line().await.unwrap() {
            if line.contains("READER_READY") {
                return;
            }
        }
        panic!("reader exited before opening its transaction");
    })
    .await
    .unwrap();
    create_agent(&mut connection, "after-reader").await;
    let monitor = rusqlite::Connection::open(daemon.store_path()).unwrap();
    let (_, frames, copied): (i64, i64, i64) = monitor
        .query_row("PRAGMA wal_checkpoint(PASSIVE)", [], |row| {
            Ok((row.get(0)?, row.get(1)?, row.get(2)?))
        })
        .unwrap();
    assert!(
        frames > copied,
        "the reader must hold back the later commit"
    );
    drop(connection);
    let _dir = daemon.stop_cleanly().await;
    let status = tokio::time::timeout(Duration::from_secs(3), child.wait())
        .await
        .unwrap()
        .unwrap();
    assert!(!status.success(), "the agent reader was still running");
    let (_, frames, copied): (i64, i64, i64) = monitor
        .query_row("PRAGMA wal_checkpoint(NOOP)", [], |row| {
            Ok((row.get(0)?, row.get(1)?, row.get(2)?))
        })
        .unwrap();
    assert_eq!(frames, copied, "the final checkpoint copied all frames");
    let log = String::from_utf8(output.lock().unwrap().clone()).unwrap();
    assert!(
        !log.contains("final WAL sync left frames for a reader"),
        "{log}"
    );
    assert!(!log.contains("skipping final WAL sync"), "{log}");
    assert!(
        !log.contains("shutdown phase exceeded its deadline"),
        "{log}"
    );
}
