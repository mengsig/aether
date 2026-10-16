#![allow(clippy::expect_used, clippy::unwrap_used, clippy::panic)]

//! The daemon launches its MCP bridge from its own published build: status
//! names it, storage GC keeps it while the daemon runs, and a clean shutdown
//! removes it.

mod support;

use std::path::{Path, PathBuf};

use aether_protocol::PROTOCOL_VERSION;
use serde_json::{Value, json};
use support::{RawConn, TestDaemon};

async fn call(daemon: &TestDaemon, frame: Value) -> Value {
    let mut conn = RawConn::connect(&daemon.socket).await.expect("connect");
    conn.hello(PROTOCOL_VERSION).await.expect("hello");
    let answer = conn.call(frame).await;
    answer["result"]["Ok"]["data"].clone()
}

async fn bridge_of(daemon: &TestDaemon) -> PathBuf {
    let status = call(daemon, json!({"cmd": "status"})).await;
    let bridge = status["bridge"].as_str();
    PathBuf::from(bridge.unwrap_or_else(|| panic!("status names no bridge: {status}")))
}

fn action_on(report: &Value, path: &Path) -> String {
    let entries = report["bridge_copies"]["entries"].as_array();
    let entry = entries
        .into_iter()
        .flatten()
        .find(|entry| entry["path"].as_str() == path.to_str());
    let entry = entry.unwrap_or_else(|| panic!("{} is not in {report}", path.display()));
    entry["action"].as_str().unwrap().to_owned()
}

#[tokio::test]
async fn the_bridge_is_the_daemons_own_copy_until_a_clean_shutdown() {
    let daemon = TestDaemon::start().await;
    let data = std::fs::canonicalize(daemon.store_path().parent().unwrap()).unwrap();
    let bin = data.join("bin");
    let bridge = bridge_of(&daemon).await;
    assert_eq!(bridge.parent(), Some(bin.as_path()));
    let name = bridge.file_name().unwrap().to_str().unwrap();
    assert!(
        name.starts_with(&format!("aether-{}-", env!("CARGO_PKG_VERSION"))),
        "{name}"
    );

    // A daemon that died: the OS released its lock and left its copy.
    let image = std::env::current_exe().unwrap();
    let dead = aether_core::bridge_bin::publish(&bin, &image, "0.0.1").unwrap();
    let dead_path = dead.path().to_path_buf();
    drop(dead);
    let report = call(&daemon, json!({"cmd": "storage_gc", "dry_run": false})).await;
    assert_eq!(action_on(&report, &bridge), "kept");
    assert_eq!(action_on(&report, &dead_path), "removed");
    assert!(bridge.exists() && !dead_path.exists());

    let daemon = daemon.restart().await;
    assert!(!bridge.exists(), "a clean shutdown removes the copy");
    let next = bridge_of(&daemon).await;
    assert_ne!(next, bridge);
    assert!(next.exists());
    daemon.stop().await;
}
