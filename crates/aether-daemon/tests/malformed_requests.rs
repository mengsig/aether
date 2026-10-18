#![allow(clippy::expect_used, clippy::unwrap_used, clippy::panic)]

mod support;

use aether_protocol::{PROTOCOL_VERSION, compat::MIN_PROTOCOL_VERSION};
use serde_json::json;
use support::{RawConn, TestDaemon, hello_frame};

#[tokio::test]
async fn command_decode_errors_are_correlated_and_leave_the_connection_usable() {
    let daemon = TestDaemon::start().await;
    for version in [MIN_PROTOCOL_VERSION, PROTOCOL_VERSION] {
        let mut conn = RawConn::connect(&daemon.socket).await.unwrap();
        conn.hello(version).await.unwrap();
        let cases = [
            (
                json!({"cmd":"create_memory", "spec":{
                    "kind":"fact", "scope":{"scope":"global"}, "text":"invalid", "unexpected":true
                }}),
                "unexpected",
            ),
            (
                json!({"cmd":"create_memory", "spec":{
                    "kind":"invalid-kind", "scope":{"scope":"global"}, "text":"invalid"
                }}),
                "invalid-kind",
            ),
            (
                json!({"cmd":"create_memory", "spec":{
                    "kind":"fact", "scope":{"scope":"global"}, "text":"forged",
                    "provenance":{"agent":"agt_00000000000000000000000000"}
                }}),
                "provenance",
            ),
            (
                json!({"cmd":"forget_memory", "id":"invalid-memory-id"}),
                "invalid-memory-id",
            ),
            (json!({"cmd":"forget_memory", "id":9999}), "invalid type"),
            (json!({"cmd":"create_memory"}), "spec"),
            (
                json!({"cmd":"send_message", "to":"agt_00000000000000000000000000", "content":"  "}),
                "needs text",
            ),
            (json!({"cmd":"unknown-command"}), "unknown-command"),
        ];
        for (frame, problem) in cases {
            let reply = conn.call(frame).await;
            assert_eq!(
                reply["result"]["Err"]["code"], "invalid_argument",
                "{reply}"
            );
            assert!(
                reply["result"]["Err"]["message"]
                    .as_str()
                    .unwrap()
                    .contains(problem),
                "{reply}"
            );
            assert_eq!(
                conn.call(json!({"cmd":"ping"})).await["result"]["Ok"]["reply"],
                "ok"
            );
        }
        // Neither the command's own `id` nor floating-point rounding can
        // redirect a malformed request's reply.
        conn.send(&json!({"req":u64::MAX, "cmd":"forget_memory", "id":"invalid"}))
            .await;
        let reply = conn.await_reply(u64::MAX).await;
        assert_eq!(reply["result"]["Err"]["code"], "invalid_argument");
        assert_eq!(
            conn.call(json!({"cmd":"status"})).await["result"]["Ok"]["reply"],
            "status"
        );
        assert_eq!(
            conn.call(json!({"cmd":"memories", "query":{}})).await["result"]["Ok"]["data"],
            json!([]),
            "invalid memory requests must not write data"
        );
    }
    daemon.stop().await;
}

#[tokio::test]
async fn unreadable_correlation_sends_a_diagnostic_and_closes() {
    let daemon = TestDaemon::start().await;
    let lines = [
        "",
        "{",
        "[]",
        "[2]",
        "null",
        r#"{"cmd":"ping","id":123}"#,
        r#"{"cmd":"ping","req":0}"#,
        r#"{"cmd":"ping","req":null}"#,
        r#"{"cmd":"ping","req":true}"#,
        r#"{"cmd":"ping","req":"2"}"#,
        r#"{"cmd":"ping","req":-1}"#,
        r#"{"cmd":"ping","req":1.5}"#,
        r#"{"cmd":"ping","req":1.0}"#,
        r#"{"cmd":"ping","req":18446744073709551616}"#,
        r#"{"cmd":"ping","req":2,"req":3}"#,
        r#"{"cmd":"ping","req":2,"unexpected":[}"#,
    ];
    for version in [MIN_PROTOCOL_VERSION, PROTOCOL_VERSION] {
        for line in lines {
            let mut conn = RawConn::connect(&daemon.socket).await.unwrap();
            conn.hello(version).await.unwrap();
            conn.send_line(line).await;
            let reply = conn.await_reply(0).await;
            assert_eq!(
                reply["result"]["Err"]["code"], "invalid_argument",
                "{line}: {reply}"
            );
            assert!(
                reply["result"]["Err"]["message"]
                    .as_str()
                    .unwrap()
                    .contains("req"),
                "{reply}"
            );
            assert!(conn.is_closed().await, "left open after {line}: {reply}");
        }
    }
    daemon.stop().await;
}

#[tokio::test]
async fn malformed_hello_is_correlated_before_the_connection_closes() {
    let daemon = TestDaemon::start().await;
    let mut conn = RawConn::connect(&daemon.socket).await.unwrap();
    let mut hello = hello_frame(PROTOCOL_VERSION, None);
    hello.as_object_mut().unwrap().remove("client_name");
    let reply = conn.call(hello).await;
    assert_eq!(
        reply["result"]["Err"]["code"], "invalid_argument",
        "{reply}"
    );
    assert!(
        reply["result"]["Err"]["message"]
            .as_str()
            .unwrap()
            .contains("client_name")
    );
    assert!(conn.is_closed().await);
    daemon.stop().await;
}
