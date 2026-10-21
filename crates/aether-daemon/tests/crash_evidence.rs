#![allow(clippy::expect_used, clippy::unwrap_used)]

mod support;

use aether_core::crash::{self, CrashKind};
use support::{RawConn, TestDaemon};

#[tokio::test]
async fn status_keeps_crash_evidence_after_startup_acknowledgment() {
    let mut daemon = TestDaemon::start().await;
    crash::record(
        &daemon.store_path(),
        CrashKind::Panic,
        "prior failure",
        Some("worker.rs:12:3"),
    )
    .unwrap();
    for restart in 0..3 {
        let mut connection = RawConn::connect(&daemon.socket).await.unwrap();
        connection
            .hello(aether_protocol::PROTOCOL_VERSION)
            .await
            .unwrap();
        let reply = connection.call(serde_json::json!({"cmd":"status"})).await;
        let problems = reply["result"]["Ok"]["data"]["problems"]
            .as_array()
            .unwrap();
        assert!(
            problems
                .iter()
                .any(|p| p.as_str().unwrap().contains("prior failure")),
            "{reply}"
        );
        let record = crash::read(&daemon.store_path()).unwrap().unwrap();
        assert_eq!(record.acknowledged, restart > 0);
        if restart > 0 {
            assert!(
                crash::acknowledge_startup(&daemon.store_path())
                    .unwrap()
                    .is_none()
            );
        }
        drop(connection);
        daemon = daemon.restart().await;
    }
    daemon.stop().await;
}
