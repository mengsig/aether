#![allow(clippy::expect_used, clippy::unwrap_used, clippy::panic)]

//! The notification center over a real socket: a key dedupes, a restart keeps
//! everything, an answer is an event a service reads, and an old client is
//! served unchanged.

mod support;

use aether_protocol::PROTOCOL_VERSION;
use aether_protocol::compat::MIN_PROTOCOL_VERSION;
use serde_json::{Value, json};
use support::{RawConn, TestDaemon};

fn ok(answer: &Value) -> &Value {
    let data = &answer["result"]["Ok"]["data"];
    assert!(!data.is_null(), "a reply with data: {answer}");
    data
}

async fn current(daemon: &TestDaemon) -> RawConn {
    let mut conn = RawConn::connect(&daemon.socket).await.unwrap();
    let welcome = conn.hello(PROTOCOL_VERSION).await.unwrap();
    let features = welcome["features"].as_array().unwrap();
    assert!(features.contains(&json!("notifications")), "{welcome}");
    conn
}

fn question(title: &str) -> Value {
    json!({"cmd":"post_notification", "source":"archon", "notification":{
        "kind":"question", "title":title, "body":"Release **1.3** is staged.",
        "key":"q-1", "actions":[{"id":"ship","label":"Ship it"},{"id":"hold","label":"Hold"}],
        "free_text":true}})
}

async fn notification_events(conn: &mut RawConn) -> Vec<Value> {
    let events = conn
        .call(json!({"cmd":"get_events", "after":0, "limit":10000, "query":{"types":["custom"]}}))
        .await;
    ok(&events)
        .as_array()
        .unwrap()
        .iter()
        .filter(|e| {
            e["kind"]["name"]
                .as_str()
                .is_some_and(|n| n.starts_with("notification."))
        })
        .cloned()
        .collect()
}

/// A repost under the key updates the one notification, it survives a
/// restart, and the answer lands as `notification.answered` with the choice.
#[tokio::test]
async fn a_keyed_notification_dedupes_survives_a_restart_and_carries_its_answer() {
    let daemon = TestDaemon::start().await;
    let mut conn = current(&daemon).await;
    let first = conn.call(question("Ship 1.3?")).await;
    let id = ok(&first)["id"].clone();
    assert_eq!(ok(&first)["state"], "unread", "{first}");
    let again = conn.call(question("Ship 1.3 today?")).await;
    assert_eq!(ok(&again)["id"], id, "the key names the same notification");
    let listed = conn.call(json!({"cmd":"list_notifications"})).await;
    assert_eq!(ok(&listed).as_array().unwrap().len(), 1, "{listed}");

    let daemon = daemon.restart().await;
    let mut conn = current(&daemon).await;
    let unread = conn
        .call(json!({"cmd":"list_notifications", "states":["unread"]}))
        .await;
    assert_eq!(ok(&unread)[0]["id"], id, "{unread}");
    assert_eq!(ok(&unread)[0]["title"], "Ship 1.3 today?");

    let answered = conn
        .call(
            json!({"cmd":"answer_notification", "id":id, "action":"ship", "text":"after the docs"}),
        )
        .await;
    assert_eq!(answered["result"]["Ok"]["reply"], "notification_answered");
    assert_eq!(ok(&answered)["notification"]["state"], "resolved");
    assert!(ok(&answered).get("delivered_to").is_none(), "{answered}");
    let events = notification_events(&mut conn).await;
    let names: Vec<_> = events.iter().map(|e| e["kind"]["name"].clone()).collect();
    assert_eq!(
        names,
        [
            "notification.posted",
            "notification.updated",
            "notification.answered"
        ]
    );
    let answer = &events[2]["kind"]["payload"];
    assert_eq!(answer["key"], "q-1");
    assert_eq!(answer["answer"]["action"], "ship");
    assert_eq!(answer["answer"]["text"], "after the docs");

    let refused = conn
        .call(json!({"cmd":"answer_notification", "id":id, "action":"hold"}))
        .await;
    assert_eq!(
        refused["result"]["Err"]["code"], "invalid_argument",
        "{refused}"
    );
    let both = conn
        .call(json!({"cmd":"resolve_notification", "id":id, "source":"archon", "key":"q-1"}))
        .await;
    assert_eq!(both["result"]["Err"]["code"], "invalid_argument", "{both}");
    daemon.stop().await;
}

/// A client at the oldest served version decodes notification changes as
/// custom events in its stream, and its own commands still work.
#[tokio::test]
async fn an_old_client_reads_notification_changes_as_custom_events() {
    let daemon = TestDaemon::start().await;
    let mut old = RawConn::connect(&daemon.socket).await.unwrap();
    old.hello(MIN_PROTOCOL_VERSION).await.unwrap();
    let subscribed = old
        .call(json!({"cmd":"subscribe", "after":0, "query":{}, "deltas":false}))
        .await;
    assert_eq!(
        subscribed["result"]["Ok"]["reply"], "subscribed",
        "{subscribed}"
    );

    let mut conn = current(&daemon).await;
    let posted = conn.call(question("Ship 1.3?")).await;
    let id = ok(&posted)["id"].clone();
    let mut seen = None;
    for _ in 0..64 {
        let frame = old.recv().await.expect("the stream stays open");
        if frame["frame"] == "event" && frame["event"]["kind"]["type"] == "custom" {
            seen = Some(frame);
            break;
        }
    }
    let frame = seen.expect("the notification reached the old subscriber");
    assert_eq!(frame["event"]["kind"]["name"], "notification.posted");
    assert_eq!(frame["event"]["kind"]["payload"]["id"], id);
    let status = old.call(json!({"cmd":"status"})).await;
    assert!(status["result"]["Ok"].is_object(), "{status}");
    daemon.stop().await;
}
