#![allow(clippy::expect_used, clippy::unwrap_used)]

mod support;

use aether_core::crash::{self, CrashKind};
use support::{RawConn, TestDaemon};

#[tokio::test]
async fn status_reports_crash_evidence_until_a_startup_acknowledges_it() {
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
        let status = &reply["result"]["Ok"]["data"];
        assert!(status.is_object(), "{reply}");
        // An empty problem list is left out of the reply.
        let listed = status["problems"].as_array().is_some_and(|problems| {
            problems
                .iter()
                .any(|p| p.as_str().unwrap().contains("prior failure"))
        });
        // The evidence outlives every restart, but once a start acknowledged it
        // it is history, not a problem (#267).
        let record = crash::read(&daemon.store_path()).unwrap().unwrap();
        assert_eq!(record.acknowledged, restart > 0);
        assert_eq!(listed, !record.acknowledged, "{reply}");
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
