#![allow(clippy::expect_used, clippy::unwrap_used, clippy::panic)]

//! ARCHITECTURE.md §g, "disabling refuses new work at once": over a real
//! daemon, an interactive turn in flight finishes within the grace or is
//! cancelled after it, and a background job's model call is cancelled.

mod noesis_support;
#[path = "remote/phone.rs"]
mod phone;
mod support;

use std::time::{Duration, Instant};

use noesis_support::faux::Reply;
use noesis_support::{Home, assert_noesis_disabled, await_closed, holds_open};
use serde_json::{Value, json};
use support::RawConn;
use tokio::task::JoinHandle;

const HELD_TEXT: &str = "A question held in flight.";

fn held_question() -> Reply {
    let question = json!({"kind": "recall", "prompt": "What is the limit of 1/x as x grows?",
        "answer_key": ["zero"], "explanation": "It shrinks without bound."});
    Reply::Tool {
        text: HELD_TEXT.to_owned(),
        name: "noesis_ask".to_owned(),
        arguments: question,
    }
}

/// A teaching session whose first model call is in flight, its agent, and
/// the creation's reply, which comes only once that first turn has ended.
async fn session_in_flight(home: &mut Home) -> (String, String, JoinHandle<Value>) {
    let asked = home.model.requests().len();
    let mut side = RawConn::connect(&home.host.daemon.socket)
        .await
        .expect("a second connection");
    side.hello(aether_protocol::PROTOCOL_VERSION)
        .await
        .expect("welcome");
    let create = json!({"cmd": "noesis_create_session", "goal": "Learn limits", "mode": "teach", "idempotency_key": "create-1"});
    let id = side.request(create).await;
    let created = tokio::spawn(async move { side.await_reply(id).await });
    home.requests_reach(asked + 1).await;
    // A Noesis read waits for the turn in flight; the generic reads do not.
    let events = home
        .ok(json!({"cmd": "get_events", "after": 0, "limit": 10000, "query": {}}))
        .await;
    let events = events.as_array().expect("an event page is a list");
    let opened = events
        .iter()
        .find(|e| e["kind"]["name"] == "noesis.session_created");
    let opened = opened.unwrap_or_else(|| panic!("the session's creation is recorded: {events:?}"));
    let session = &opened["kind"]["payload"]["session"];
    let (id, agent) = (session["id"].as_str(), session["agent"].as_str());
    let (Some(id), Some(agent)) = (id, agent) else {
        panic!("the created session names its id and agent: {opened}");
    };
    assert!(
        id.starts_with("ns_") && agent.starts_with("agt_"),
        "{opened}"
    );
    (id.to_owned(), agent.to_owned(), created)
}

/// Disable, and check that it answered at once and new work is refused.
async fn disable_at_once(home: &mut Home, session: &str) {
    let started = Instant::now();
    home.disable().await;
    assert!(
        started.elapsed() < Duration::from_secs(2),
        "a disable never waits for a turn"
    );
    let send = json!({"cmd": "noesis_send", "session": session, "text": "More.", "idempotency_key": "send-1"});
    assert_noesis_disabled(&home.err(send).await, "a message right after the disable");
    let read = json!({"cmd": "noesis_get_session", "session": session});
    assert_noesis_disabled(&home.err(read).await, "a read right after the disable");
}

async fn transcript_text(home: &mut Home, agent: &str) -> String {
    let page = home
        .ok(json!({"cmd": "get_transcript_page", "agent": agent}))
        .await;
    assert!(
        page.is_object() || page.is_array(),
        "a transcript page: {page}"
    );
    page.to_string()
}

#[tokio::test(flavor = "multi_thread")]
async fn g14_an_interactive_turn_in_flight_finishes_within_the_grace() {
    let mut home = Home::start("").await;
    home.enable_with_grace(10).await;
    let gate = home.model.hold(held_question());
    let (id, agent, created) = session_in_flight(&mut home).await;
    disable_at_once(&mut home, &id).await;
    tokio::time::sleep(Duration::from_millis(300)).await;
    assert!(
        holds_open(&home.noesis_db()),
        "the service stays for the turn's grace"
    );

    gate.notify_one();
    let ended = await_closed(&home.noesis_db(), Duration::from_secs(10)).await;
    assert!(
        ended < Duration::from_secs(9),
        "the service stopped once the turn finished: {ended:?}"
    );
    let text = transcript_text(&mut home, &agent).await;
    assert!(
        text.contains(HELD_TEXT),
        "the turn finished and its reply stays: {text}"
    );
    let created = created.await.expect("the creation's reply");
    assert!(
        created["result"]["Ok"].is_object(),
        "the session was created: {created}"
    );
    home.enable().await;
    let session = home.session(&id).await;
    assert!(
        !session["open_question"].is_null(),
        "the finished turn's question is posed: {session}"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn g14_an_interactive_turn_in_flight_is_cancelled_after_the_grace() {
    tokio::time::timeout(Duration::from_secs(20), cancelled_after_grace())
        .await
        .expect("the cancellation journey completes within 20 seconds");
}

async fn cancelled_after_grace() {
    let mut home = Home::start("").await;
    home.enable_with_grace(1).await;
    let _never_opened = home.model.hold(held_question());
    let (id, agent, created) = session_in_flight(&mut home).await;
    let started = Instant::now();
    disable_at_once(&mut home, &id).await;
    await_closed(&home.noesis_db(), Duration::from_secs(10)).await;
    assert!(
        started.elapsed() >= Duration::from_secs(1),
        "the turn kept its grace"
    );

    let text = transcript_text(&mut home, &agent).await;
    assert!(
        !text.contains(HELD_TEXT),
        "the cancelled call recorded no reply: {text}"
    );
    let created = created.await.expect("the creation's reply");
    assert_eq!(
        created["frame"], "reply",
        "the creation is answered once its turn is cancelled: {created}"
    );
    home.enable().await;
    let session = home.session(&id).await;
    assert!(
        session["open_question"].is_null(),
        "the cancelled turn posed nothing: {session}"
    );
}

/// A teaching session with one exchange the extractor can read.
async fn session_to_extract(home: &mut Home) -> String {
    home.model
        .text("Welcome. What do you already know about limits?");
    home.model
        .text("Good: a limit is the value a function approaches.");
    let created = home.create_session("teach", "Learn limits").await;
    let id = created["id"].as_str().expect("a session id").to_owned();
    home.requests_reach(1).await;
    let send = json!({"cmd": "noesis_send", "session": id, "text": "A limit is where f(x) heads.", "idempotency_key": "send-0"});
    home.ok(send).await;
    home.requests_reach(2).await;
    id
}

async fn job_of(home: &mut Home, job: &Value) -> Value {
    let jobs = home.ok(json!({"cmd": "noesis_jobs"})).await;
    let list = jobs
        .get("jobs")
        .unwrap_or(&jobs)
        .as_array()
        .cloned()
        .unwrap_or_else(|| panic!("a job list: {jobs}"));
    let found = list.into_iter().find(|row| row["id"] == job["id"]);
    found.unwrap_or_else(|| panic!("job {} is listed: {jobs}", job["id"]))
}

#[tokio::test(flavor = "multi_thread")]
async fn g14_a_background_job_s_model_call_is_cancelled_by_the_disable() {
    let mut home = Home::start("").await;
    home.enable_with_grace(1).await;
    let id = session_to_extract(&mut home).await;
    let _never_opened = home.model.hold(Reply::Text("{}".to_owned()));
    let extract = json!({"cmd": "noesis_extract", "session": id, "idempotency_key": "extract-1"});
    let started = home.ok(extract).await;
    let job = started.get("job").cloned().unwrap_or(started);
    assert!(job["id"].is_string(), "a job: {job}");
    home.requests_reach(3).await;
    let status = home.ok(json!({"cmd": "noesis_status"})).await;
    assert_eq!(
        status["jobs_running"], 1,
        "the job's call is in flight: {status}"
    );

    home.disable().await;
    await_closed(&home.noesis_db(), Duration::from_secs(10)).await;
    assert_eq!(
        home.model.requests().len(),
        3,
        "the job made no further model call"
    );
    home.enable().await;
    let ended = job_of(&mut home, &job).await;
    assert_eq!(
        ended["state"], "cancelled",
        "the job was cancelled, not failed: {ended}"
    );
}
