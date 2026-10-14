#![allow(clippy::expect_used, clippy::unwrap_used, clippy::panic)]

mod support;

use aether_protocol::{PROTOCOL_VERSION, compat::MIN_PROTOCOL_VERSION};
use serde_json::{Value, json};
use support::{RawConn, TestDaemon};

#[tokio::test]
async fn protocol_child_creation_requires_explicit_effort_including_keyed_requests() {
    let daemon = TestDaemon::start().await;
    let mut conn = RawConn::connect(&daemon.socket).await.unwrap();
    conn.hello(PROTOCOL_VERSION).await.unwrap();
    let root = conn
        .call(json!({
            "cmd": "create_agent", "role": "parent", "start": false,
            "model": {"provider": "claude-code", "model": "haiku"}
        }))
        .await;
    let parent = &root["result"]["Ok"]["data"];
    assert!(parent["id"].is_string(), "{root}");
    assert!(parent["config"]["effort"].is_null(), "{root}");
    for config in [Value::Null, json!({})] {
        for keyed in [false, true] {
            let mut request = json!({
                "cmd": "create_agent", "role": "child", "parent": parent["id"],
                "objective": "must not be written", "config": config, "start": false
            });
            if keyed {
                request["idempotency_key"] = json!("missing-child-effort");
            }
            let refused = conn.call(request).await;
            let error = &refused["result"]["Err"];
            assert_eq!(error["code"], "invalid_argument", "{refused}");
            assert!(
                error["message"].as_str().unwrap().contains(
                    "a sub-agent needs an explicit effort: low, medium, high, xhigh or max"
                ),
                "{refused}"
            );
        }
    }
    let agents = conn.call(json!({"cmd":"list_agents", "query": {}})).await;
    assert_eq!(agents["result"]["Ok"]["data"].as_array().unwrap().len(), 1);
    let tasks = conn.call(json!({"cmd":"list_tasks", "query": {}})).await;
    assert!(tasks["result"]["Ok"]["data"].as_array().unwrap().is_empty());
    let child = conn
        .call(json!({
            "cmd": "create_agent", "role": "child", "parent": parent["id"],
            "config": {"effort": "low"}, "start": false,
            "idempotency_key": "missing-child-effort"
        }))
        .await;
    assert_eq!(
        child["result"]["Ok"]["data"]["config"]["effort"], "low",
        "{child}"
    );
    daemon.stop().await;
}

#[tokio::test]
async fn protocol_create_agent_keeps_a_lean_host_context_and_omits_full() {
    let daemon = TestDaemon::start().await;
    let mut oldest = RawConn::connect(&daemon.socket).await.unwrap();
    let welcome = oldest.hello(MIN_PROTOCOL_VERSION).await.unwrap();
    let features = welcome["features"].as_array().unwrap();
    assert!(features.contains(&json!("host_context")), "{welcome}");
    assert!(
        features.contains(&json!("operator_memory_roles")),
        "{welcome}"
    );
    let created = oldest
        .call(json!({
            "cmd": "create_agent", "role": "root", "start": false,
            "config": {"host_context": "lean"},
            "model": {"provider": "claude-code", "model": "haiku"}
        }))
        .await;
    let config = &created["result"]["Ok"]["data"]["config"];
    assert_eq!(config["host_context"], "lean", "{created}");
    let mut conn = RawConn::connect(&daemon.socket).await.unwrap();
    let welcome = conn.hello(PROTOCOL_VERSION).await.unwrap();
    let features = welcome["features"].as_array().unwrap();
    assert!(features.contains(&json!("host_context")), "{welcome}");
    assert!(
        features.contains(&json!("operator_memory_roles")),
        "{welcome}"
    );
    for (config, expected) in [
        (json!({"host_context": "lean"}), json!("lean")),
        (json!({"host_context": "full"}), Value::Null),
        (json!({}), Value::Null),
    ] {
        let created = conn
            .call(json!({
                "cmd": "create_agent", "role": "root", "start": false, "config": config,
                "model": {"provider": "claude-code", "model": "haiku"}
            }))
            .await;
        let agent = &created["result"]["Ok"]["data"];
        assert_eq!(agent["config"]["host_context"], expected, "{created}");
        let read = conn
            .call(json!({"cmd": "get_agent", "id": agent["id"]}))
            .await;
        let config = &read["result"]["Ok"]["data"]["config"];
        assert_eq!(config["host_context"], expected, "{read}");
    }
    let refused = conn
        .call(json!({
            "cmd": "create_agent", "role": "root", "start": false,
            "config": {"host_context": "minimal"}
        }))
        .await;
    assert!(refused["result"]["Err"].is_object(), "{refused}");
    daemon.stop().await;
}
