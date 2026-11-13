#![allow(clippy::expect_used, clippy::unwrap_used, clippy::panic)]

//! `append_note` over a real socket: stored as an event, never a turn.

#[path = "remote/phone.rs"]
mod phone;
mod support;

use phone::{Host, Phone};
use serde_json::{Value, json};
use support::RawConn;

async fn idle_agent(host: &mut Host) -> Value {
    let agent = host
        .ok(
            json!({"cmd":"create_agent", "role":"note-test", "start":false,
            "model":{"provider":"claude-code","model":"haiku"}}),
        )
        .await;
    agent["id"].clone()
}

async fn agent_events(host: &mut Host, agent: &Value) -> Vec<Value> {
    let events = host
        .ok(json!({"cmd":"get_events", "after":0, "limit":10000, "query":{"agent":agent}}))
        .await;
    events.as_array().expect("a list of events").clone()
}

fn kinds(events: &[Value]) -> Vec<&str> {
    events
        .iter()
        .map(|event| event["kind"]["type"].as_str().expect("an event type"))
        .collect()
}

/// The feature is advertised; a note lands as one `note.appended` event,
/// starts nothing and replays before the first turn; an oversized one is
/// refused with the limit named.
#[tokio::test]
async fn a_note_is_recorded_without_starting_a_turn() {
    let mut host = Host::enabled().await;
    let mut conn = RawConn::connect(&host.daemon.socket).await.unwrap();
    let welcome = conn.hello(aether_protocol::PROTOCOL_VERSION).await.unwrap();
    let features = welcome["features"].as_array().unwrap();
    assert!(features.contains(&json!("notes")), "{welcome}");
    let agent = idle_agent(&mut host).await;
    let before = agent_events(&mut host, &agent).await;

    let appended = host
        .ok(
            json!({"cmd":"append_note", "agent":agent, "text":"3 items shipped",
            "links":["https://example.com/pr/1"], "context":"digest", "level":"applied"}),
        )
        .await;
    let seq = appended["seq"].as_u64().expect("the note's seq");
    let after = agent_events(&mut host, &agent).await;
    assert_eq!(after.len(), before.len() + 1, "{after:?}");
    let note = after.last().unwrap();
    assert_eq!(note["seq"], seq);
    assert_eq!(note["kind"]["name"], "note.appended", "{note}");
    assert_eq!(note["kind"]["payload"]["text"], "3 items shipped");
    assert_eq!(note["kind"]["payload"]["context"], "digest");
    assert_eq!(note["kind"]["payload"]["level"], "applied");
    assert!(!kinds(&after).contains(&"agent.started"), "{after:?}");
    let replay = host.ok(json!({"cmd":"replay", "agent":agent})).await;
    assert_eq!(replay[0]["turn"], 0, "{replay}");
    assert_eq!(replay[0]["outcome"]["outcome"], "not_started");
    assert_eq!(replay[0]["notes"][0]["text"], "3 items shipped");

    let oversized = host
        .err(json!({"cmd":"append_note", "agent":agent, "text":"x".repeat(2049)}))
        .await;
    assert_eq!(oversized["code"], "invalid_argument", "{oversized}");
    assert!(
        oversized["message"].as_str().unwrap().contains("2048"),
        "{oversized}"
    );
    assert_eq!(agent_events(&mut host, &agent).await.len(), after.len());
    host.daemon.stop().await;
}

/// An operator device may append a note; an observer may not.
#[tokio::test]
async fn an_observer_device_cannot_append_a_note() {
    let mut host = Host::enabled().await;
    let agent = idle_agent(&mut host).await;
    let note = json!({"cmd":"append_note", "agent":agent, "text":"from the phone"});
    for (role, allowed) in [("operator", true), ("observer", false)] {
        let phone = Phone::new();
        let device = host.pair(&phone, role, role).await;
        let mut socket = host.connect(&phone, &device).await;
        let answer = socket.call(note.clone()).await;
        match allowed {
            true => assert!(answer["result"].get("Ok").is_some(), "{role}: {answer}"),
            false => assert_eq!(answer["result"]["Err"]["code"], "forbidden", "{answer}"),
        }
    }
    let events = agent_events(&mut host, &agent).await;
    let notes = events
        .iter()
        .filter(|event| event["kind"]["name"] == "note.appended")
        .count();
    assert_eq!(notes, 1, "{events:?}");
    host.daemon.stop().await;
}

/// An important note is a post: `get_conversation` keeps it with the flag the
/// host set, and `important_only` leaves out the plain note beside it.
#[tokio::test]
async fn an_important_post_is_read_back_from_the_conversation() {
    let mut host = Host::enabled().await;
    let mut conn = RawConn::connect(&host.daemon.socket).await.unwrap();
    let welcome = conn.hello(aether_protocol::PROTOCOL_VERSION).await.unwrap();
    let features = welcome["features"].as_array().unwrap();
    assert!(features.contains(&json!("conversation")), "{welcome}");
    let agent = idle_agent(&mut host).await;
    host.ok(json!({"cmd":"append_note", "agent":agent, "text":"cache pruned"}))
        .await;
    let post = "**Item shipped**\n\n".to_owned() + &"x".repeat(4000);
    let appended = host
        .ok(json!({"cmd":"append_note", "agent":agent, "text":post, "important":true}))
        .await;

    let all = host
        .ok(json!({"cmd":"get_conversation", "agent":agent}))
        .await;
    let posts: Vec<(&str, bool)> = all["entries"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|entry| entry["kind"] == "post")
        .map(|entry| (entry["text"].as_str().unwrap(), entry["important"] == true))
        .collect();
    assert_eq!(
        posts,
        [("cache pruned", false), (post.as_str(), true)],
        "{all}"
    );
    assert_eq!(all["last_seq"], appended["seq"], "{all}");

    let important = host
        .ok(json!({"cmd":"get_conversation", "agent":agent, "important_only":true}))
        .await;
    let entries = important["entries"].as_array().unwrap();
    assert!(
        entries.iter().all(|entry| entry["important"] == true),
        "{important}"
    );
    assert!(
        entries.iter().any(|entry| entry["text"] == post.as_str()),
        "{important}"
    );
    assert!(!entries.iter().any(|entry| entry["text"] == "cache pruned"));

    let older = host
        .ok(json!({"cmd":"get_conversation", "agent":agent, "before":important["to_seq"], "limit":1}))
        .await;
    assert_eq!(older["entries"][0]["text"], "cache pruned", "{older}");
    let both = host
        .err(json!({"cmd":"get_conversation", "agent":agent, "after":1, "before":2}))
        .await;
    assert_eq!(both["code"], "invalid_argument", "{both}");
    host.daemon.stop().await;
}
