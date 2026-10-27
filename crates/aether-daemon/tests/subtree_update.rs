#![allow(clippy::expect_used, clippy::unwrap_used, clippy::panic)]

//! `update_agent` with `subtree` over a real socket (#323).

mod support;

use aether_protocol::PROTOCOL_VERSION;
use serde_json::{Value, json};
use support::{RawConn, TestDaemon};

const OLD: &str = "2099-01-01T00:00:00Z";
const NEW: &str = "2099-06-01T00:00:00Z";

async fn ok(conn: &mut RawConn, frame: Value) -> Value {
    let reply = conn.call(frame.clone()).await;
    let data = &reply["result"]["Ok"]["data"];
    assert!(!data.is_null(), "{frame} -> {reply}");
    data.clone()
}

async fn idle(conn: &mut RawConn, parent: Option<&Value>) -> Value {
    let mut frame = json!({
        "cmd": "create_agent", "role": "worker", "start": false,
        "config": {"deadline": OLD, "long_lived": true, "effort": "low"},
        "model": {"provider": "claude-code", "model": "haiku"}
    });
    if let Some(parent) = parent {
        frame["parent"] = parent.clone();
    }
    ok(conn, frame).await["id"].clone()
}

/// The daemon announces the feature, and a subtree extension answers the
/// named agent while its child moves to the new deadline too.
#[tokio::test]
async fn a_subtree_update_is_announced_and_reaches_the_child() {
    let daemon = TestDaemon::start().await;
    let mut conn = RawConn::connect(&daemon.socket).await.unwrap();
    let welcome = conn.hello(PROTOCOL_VERSION).await.unwrap();
    let features = welcome["features"].as_array().unwrap();
    assert!(features.contains(&json!("subtree_update")), "{welcome}");
    let parent = idle(&mut conn, None).await;
    let child = idle(&mut conn, Some(&parent)).await;

    let updated = ok(
        &mut conn,
        json!({"cmd": "update_agent", "id": parent, "subtree": true,
               "patch": {"deadline": {"set": NEW}}}),
    )
    .await;
    assert_eq!(updated["id"], parent, "{updated}");
    assert_eq!(updated["config"]["deadline"], NEW, "{updated}");
    let child = ok(&mut conn, json!({"cmd": "get_agent", "id": child})).await;
    assert_eq!(child["config"]["deadline"], NEW, "{child}");
    daemon.stop().await;
}
