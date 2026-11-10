#![allow(clippy::expect_used, clippy::unwrap_used, clippy::panic)]

mod noesis_support;
#[path = "remote/phone.rs"]
mod phone;
mod support;

use std::time::Duration;

use noesis_support::{Home, faux};
use serde_json::{Value, json};

fn ask() -> Value {
    json!({"kind":"recall", "prompt":"What is a quarter of a turn?", "answer_key":"90 degrees", "plan_node":"turns"})
}

async fn wait_turn_done(home: &Home, id: &str) {
    let conn = rusqlite::Connection::open(home.noesis_db()).unwrap();
    tokio::time::timeout(Duration::from_secs(8), async {
        loop {
            let pending: bool = conn
                .query_row(
                    "SELECT turn_pending FROM sessions WHERE id = ?1",
                    [id],
                    |row| row.get(0),
                )
                .unwrap();
            if !pending {
                break;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .expect("the previous turn finishes");
}

async fn planned_lesson(home: &mut Home) -> String {
    home.model.tool("", "noesis_probe_done", json!({"strands":[{"strand":"turns", "floor":"corners", "ceiling":"angles", "start":"turns"}]}));
    home.model.tool(
        "",
        "noesis_plan",
        json!({"nodes":[{"id":"turns", "title":"Turns"}], "edges":[]}),
    );
    home.model.text("Shall we begin?");
    let created = home.create_session("teach", "Learn angles").await;
    let id = created["id"].as_str().unwrap().to_owned();
    tokio::time::timeout(Duration::from_secs(8), home.requests_reach(3))
        .await
        .unwrap();
    tokio::time::timeout(
        Duration::from_secs(8),
        home.session_until(&id, "plan", |s| !s["plan"].is_null()),
    )
    .await
    .unwrap();
    wait_turn_done(home, &id).await;
    id
}

#[tokio::test(flavor = "multi_thread")]
async fn restart_after_go_resumes_early_and_mid_turn() {
    for mid_turn in [false, true] {
        let mut home = Home::start("").await;
        home.enable().await;
        let id = planned_lesson(&mut home).await;
        if mid_turn {
            home.model
                .tool("", "noesis_graph_search", json!({"query":"angles"}));
        }
        let gate = home.model.hold(faux::Reply::Tool {
            text: "".into(),
            name: "noesis_ask".into(),
            arguments: ask(),
        });
        home.model.tool("", "noesis_ask", ask());
        home.ok(json!({"cmd":"noesis_send", "session":id, "text":"Go", "idempotency_key":"go"}))
            .await;
        if mid_turn {
            tokio::time::timeout(Duration::from_secs(8), home.requests_reach(5))
                .await
                .unwrap();
        }
        tokio::spawn(async move {
            tokio::time::sleep(Duration::from_secs(1)).await;
            gate.notify_one();
        });
        let mut home = tokio::time::timeout(Duration::from_secs(10), home.restart())
            .await
            .unwrap();
        let session = tokio::time::timeout(
            Duration::from_secs(10),
            home.session_until(&id, "resumed check", |s| {
                !s["open_question"].is_null() || !s["stalled"].is_null()
            }),
        )
        .await
        .unwrap();
        assert!(session["stalled"].is_null(), "{session}");
        assert_eq!(
            session["open_question"]["prompt"],
            "What is a quarter of a turn?"
        );
        home.host.daemon.stop_cleanly().await;
    }
}

async fn practice_problem(home: &mut Home) -> String {
    home.model.tool(
        "",
        "noesis_problem",
        json!({"statement":"What is 2 + 3?", "mode":"recall", "targets":[], "solution":"five"}),
    );
    home.model.text("Your turn.");
    let created = home.create_session("practice", "Practise addition").await;
    let id = created["id"].as_str().unwrap().to_owned();
    tokio::time::timeout(
        Duration::from_secs(8),
        home.session_until(&id, "problem", |s| !s["active_problem"].is_null()),
    )
    .await
    .unwrap();
    tokio::time::timeout(Duration::from_secs(8), home.requests_reach(2))
        .await
        .unwrap();
    wait_turn_done(home, &id).await;
    id
}

async fn restart_practice(concluded: bool) {
    let mut home = Home::start("").await;
    home.enable().await;
    let id = practice_problem(&mut home).await;
    let conclusion = json!({"outcome":"solved", "rung":0, "retest":false});
    let gate = if concluded {
        home.model.tool("", "noesis_conclude", conclusion.clone());
        home.model.hold(faux::Reply::Text("Well done.".into()))
    } else {
        home.model.hold(faux::Reply::Tool {
            text: "".into(),
            name: "noesis_conclude".into(),
            arguments: conclusion.clone(),
        })
    };
    if !concluded {
        home.model.tool("", "noesis_conclude", conclusion);
    }
    home.model.text("Well done.");
    home.ok(json!({"cmd":"noesis_send", "session":id, "text":"Five", "idempotency_key":"answer"}))
        .await;
    let count = if concluded { 4 } else { 3 };
    tokio::time::timeout(Duration::from_secs(8), home.requests_reach(count))
        .await
        .unwrap();
    tokio::spawn(async move {
        tokio::time::sleep(Duration::from_secs(1)).await;
        gate.notify_one();
    });
    let mut home = tokio::time::timeout(Duration::from_secs(10), home.restart())
        .await
        .unwrap();
    let session = tokio::time::timeout(
        Duration::from_secs(10),
        home.session_until(&id, "resumed conclusion", |s| {
            s["active_problem"].is_null() || !s["stalled"].is_null()
        }),
    )
    .await
    .unwrap();
    assert!(session["stalled"].is_null(), "{session}");
    assert!(session["active_problem"].is_null(), "{session}");
    assert_eq!(session["problems"][0]["conclusion"]["outcome"], "solved");
    wait_turn_done(&home, &id).await;
    let session = home.session(&id).await;
    assert!(session["stalled"].is_null(), "{session}");
    home.host.daemon.stop_cleanly().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn restart_during_practice_answer_resumes_without_stalling() {
    for concluded in [false, true] {
        restart_practice(concluded).await;
    }
}
