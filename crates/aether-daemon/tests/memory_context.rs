#![allow(clippy::expect_used, clippy::unwrap_used, clippy::panic)]

//! `memories_in_context` over a real socket: served to the host, refused to
//! paired devices whatever their role.

#[path = "remote/phone.rs"]
mod phone;
mod support;

use phone::{Host, Phone};
use serde_json::json;
use support::RawConn;

#[tokio::test]
async fn the_host_is_served_and_a_paired_device_is_refused() {
    let mut host = Host::enabled().await;
    let mut conn = RawConn::connect(&host.daemon.socket).await.unwrap();
    let welcome = conn.hello(aether_protocol::PROTOCOL_VERSION).await.unwrap();
    let features = welcome["features"].as_array().unwrap();
    assert!(features.contains(&json!("memory_context")));

    let task = host
        .ok(json!({"cmd":"create_task", "objective":"Weigh the release memories"}))
        .await;
    let query = json!({"cmd":"memories_in_context", "task":task["id"]});
    assert_eq!(
        host.ok(query.clone()).await,
        json!({"snapshots":[], "truncated":false}),
        "a task with no model calls yet saw no memories"
    );
    let neither = host.err(json!({"cmd":"memories_in_context"})).await;
    assert_eq!(neither["code"], "invalid_argument");
    let unknown =
        json!({"cmd":"memories_in_context", "invocation":"inv_01M1Z1AEC7ATEWCBH91PZ2NQKV"});
    assert_eq!(host.err(unknown).await["code"], "not_found");

    for role in ["operator", "observer"] {
        let phone = Phone::new();
        let device = host.pair(&phone, role, role).await;
        let mut socket = host.connect(&phone, &device).await;
        let refused = socket.call(query.clone()).await;
        assert_eq!(
            refused["result"]["Err"]["code"], "forbidden",
            "{role}: {refused}"
        );
    }
    host.daemon.stop().await;
}
