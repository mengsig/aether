#![allow(clippy::expect_used, clippy::unwrap_used, clippy::panic)]

//! `aether storage` reports migration backup bytes and caps how many are kept.

mod support;

use std::time::{Duration, SystemTime};

use aether_protocol::PROTOCOL_VERSION;
use serde_json::json;
use support::{RawConn, TestDaemon};

#[tokio::test]
async fn startup_storage_gc_keeps_the_newest_two_backups_and_reports_their_bytes() {
    let daemon = TestDaemon::start().await;
    let backups = daemon
        .store_path()
        .parent()
        .unwrap()
        .join("migration-backups");
    std::fs::create_dir_all(&backups).unwrap();
    let mut paths = Vec::new();
    for age in [3, 2, 1] {
        let name = format!("schema-1-before-2-{}.sqlite.zst", ulid::Ulid::generate());
        let path = backups.join(name);
        std::fs::write(&path, [0u8; 100]).unwrap();
        let modified = SystemTime::now() - Duration::from_secs(age * 60);
        let file = std::fs::File::options().write(true).open(&path).unwrap();
        file.set_modified(modified).unwrap();
        paths.push(path);
    }

    // The sweep at startup applies the default cap of two.
    let daemon = daemon.restart().await;
    let deadline = tokio::time::Instant::now() + Duration::from_secs(10);
    while paths[0].exists() {
        assert!(
            tokio::time::Instant::now() < deadline,
            "the oldest backup was kept"
        );
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    assert!(paths[1].exists() && paths[2].exists());

    let mut conn = RawConn::connect(&daemon.socket).await.expect("connect");
    conn.hello(PROTOCOL_VERSION).await.expect("hello");
    let answer = conn
        .call(json!({"cmd": "storage_gc", "dry_run": true}))
        .await;
    let report = &answer["result"]["Ok"]["data"]["backups"];
    assert_eq!(report["bytes"], 200, "{report}");
    assert_eq!(report["bytes_eligible"], 0, "{report}");
    daemon.stop().await;
}
