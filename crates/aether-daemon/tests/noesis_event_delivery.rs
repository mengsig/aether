#![allow(clippy::expect_used, clippy::unwrap_used, clippy::panic)]

//! Why the desktop client never saw a reveal: the daemon sends `noesis.*`
//! events typed only to a client whose hello lists `noesis`, and as `custom`
//! to any other. A subscription made as the web client makes it receives
//! `noesis.answer_recorded` and `noesis.practice_concluded` typed.

mod noesis_support;
#[path = "remote/phone.rs"]
mod phone;
mod support;

use std::time::Duration;

use noesis_support::{Home, answer_of};
use serde_json::{Value, json};

/// The features a browser client says hello with.
const WEB_FEATURES: &[&str] = &[
    "deltas",
    "artifact_bytes",
    "delta_order",
    "memory_v2",
    "noesis",
];

/// A connection that says hello with `features` and subscribes as the web
/// client does: from the last seq it knows, every event, no deltas.
async fn subscribed(home: &mut Home, features: &[&str]) -> support::RawConn {
    let after = home.last_seq().await;
    let mut conn = support::RawConn::connect(&home.host.daemon.socket)
        .await
        .expect("connect");
    let mut hello = support::hello_frame(aether_protocol::PROTOCOL_VERSION, None);
    hello["features"] = json!(features);
    support::read_answer(conn.call(hello).await).expect("welcome");
    let subscribe = json!({"cmd": "subscribe", "after": after, "query": {}, "deltas": false});
    answer_of(&conn.call(subscribe).await).expect("subscribed");
    conn
}

/// The kinds of the events `conn`'s subscription has pushed.
fn pushed_kinds(conn: &support::RawConn) -> Vec<Value> {
    conn.pushed
        .iter()
        .filter(|frame| frame["frame"] == "event")
        .map(|frame| frame["event"]["kind"].clone())
        .collect()
}

/// Ask until `conn`'s subscription has pushed an event of `kind` accepts.
async fn await_kind(
    conn: &mut support::RawConn,
    what: &str,
    kind: impl Fn(&Value) -> bool,
) -> Value {
    for _ in 0..400 {
        if let Some(found) = pushed_kinds(conn).into_iter().find(|seen| kind(seen)) {
            return found;
        }
        conn.call(json!({"cmd": "ping"})).await;
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
    panic!(
        "the subscription never pushed {what}: {:?}",
        pushed_kinds(conn)
    );
}

/// A teach session's question answered, then a practice problem concluded.
async fn answer_and_conclude(home: &mut Home) {
    let question = json!({"kind": "recall", "prompt": "Roman name of Paris?",
        "answer_key": ["Lutetia"], "explanation": "Its Latin name."});
    home.model.tool("A first question.", "noesis_ask", question);
    let created = home.create_session("teach", "Learn French history").await;
    let id = created["id"].as_str().expect("a session id").to_owned();
    let posed = home
        .session_until(&id, "a question", |s| !s["open_question"].is_null())
        .await;
    let answer = json!({"cmd": "noesis_answer", "session": id,
        "question": posed["open_question"]["id"], "idempotency_key": "answer-1",
        "answer": {"text": "Lutetia", "dont_know": false}});
    home.model.tool(
        "",
        "noesis_judge_recall",
        json!({"question": posed["open_question"]["id"], "correct": true}),
    );
    home.model.text("Noted.");
    let asked = home.model.requests().len();
    home.ok(answer).await;
    home.requests_reach(asked + 2).await;

    let problem = json!({"statement": "Differentiate x^5.", "mode": "recall",
        "targets": [], "solution": "5x^4"});
    home.model.tool("Try this one.", "noesis_problem", problem);
    home.model.text("Over to you.");
    let created = home
        .create_session("practice", "Practise derivatives")
        .await;
    let id = created["id"].as_str().expect("a session id").to_owned();
    home.session_until(&id, "a problem", |s| !s["active_problem"].is_null())
        .await;
    home.model.tool(
        "Correct.",
        "noesis_conclude",
        json!({"outcome": "solved", "rung": 0, "retest": false}),
    );
    home.model.text("Well done.");
    let send =
        json!({"cmd": "noesis_send", "session": id, "text": "5x^4?", "idempotency_key": "send-1"});
    home.ok(send).await;
}

#[tokio::test(flavor = "multi_thread")]
async fn a_web_subscription_receives_the_reveals_typed_and_one_without_the_feature_as_custom() {
    let mut home = Home::start("").await;
    home.enable().await;
    let mut web = subscribed(&mut home, WEB_FEATURES).await;
    let mut legacy = subscribed(&mut home, &["deltas"]).await;
    answer_and_conclude(&mut home).await;

    for name in ["noesis.answer_recorded", "noesis.practice_concluded"] {
        let typed = await_kind(&mut web, name, |kind| kind["type"] == name).await;
        assert!(
            typed["session"].is_string(),
            "{name} carries its fields: {typed}"
        );
        let custom = await_kind(&mut legacy, name, |kind| {
            kind["type"] == "custom" && kind["name"] == name
        })
        .await;
        assert!(custom["payload"].is_object(), "{custom}");
    }
    let revealed = await_kind(&mut web, "the reveal", |kind| {
        kind["type"] == "noesis.answer_recorded"
    })
    .await;
    assert_eq!(revealed["result"]["answer_key"], json!("Lutetia"));
    let disguised = pushed_kinds(&web)
        .into_iter()
        .filter(|kind| kind["type"] == "custom")
        .filter(|kind| {
            kind["name"]
                .as_str()
                .is_some_and(|name| name.starts_with("noesis."))
        })
        .count();
    assert_eq!(
        disguised, 0,
        "a client listing `noesis` gets every Noesis event typed"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn recall_judgment_reaches_a_web_subscriber() {
    let mut home = Home::start("").await;
    home.enable().await;
    let mut web = subscribed(&mut home, WEB_FEATURES).await;
    home.model.tool(
        "A question.",
        "noesis_ask",
        json!({"kind": "recall", "prompt": "Name the capital of France", "answer_key": "Paris"}),
    );
    let created = home.create_session("teach", "Geography").await;
    let id = created["id"].as_str().unwrap();
    let posed = home
        .session_until(id, "a question", |s| s["open_question"]["id"].is_string())
        .await;
    let question = posed["open_question"]["id"].as_str().unwrap();
    home.model.tool(
        "",
        "noesis_judge_recall",
        json!({"question": question, "correct": false, "misconception": "Confuses countries"}),
    );
    home.model.text("Let's review.");
    home.ok(json!({"cmd": "noesis_answer", "session": id, "question": question, "idempotency_key": "recall-answer", "answer": {"text": "London", "dont_know": false}})).await;
    let judged = await_kind(&mut web, "judgment", |kind| {
        kind["type"] == "noesis.answer_recorded" && kind["result"]["correct"] == false
    })
    .await;
    assert_eq!(judged["result"]["question"], question);
    assert_eq!(judged["result"]["misconception"], "Confuses countries");
}
