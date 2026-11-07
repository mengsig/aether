#![allow(clippy::expect_used, clippy::unwrap_used, clippy::panic)]

//! ARCHITECTURE.md §g, "one runtime guard covers every path to execution",
//! over a real daemon: a Noesis agent takes no work from any generic agent
//! command while Noesis is off, a restart resumes it nowhere, it is offered
//! nothing to delegate with, and every call it makes is charged to Noesis.

mod noesis_support;
#[path = "remote/phone.rs"]
mod phone;
mod support;

use std::time::Duration;

use noesis_support::faux::offered_tools;
use noesis_support::{Home, assert_noesis_disabled, holds_open};
use serde_json::{Value, json};

/// How long an idle daemon is watched for work it must not start.
const QUIET: Duration = Duration::from_millis(1500);

/// A teaching session whose agent posed a question, so it holds a parked call.
async fn session_with_open_question(home: &mut Home) -> (String, String) {
    home.enable().await;
    let question = json!({"kind": "recall", "prompt": "What is the derivative of x squared?",
        "answer_key": ["two x"], "explanation": "The power rule."});
    home.model.tool("A first question.", "noesis_ask", question);
    let created = home.create_session("teach", "Learn derivatives").await;
    let id = created["id"].as_str().expect("a session id").to_owned();
    let posed = home
        .session_until(&id, "a question", |s| !s["open_question"].is_null())
        .await;
    let agent = posed["agent"]
        .as_str()
        .expect("the session's agent")
        .to_owned();
    (id, agent)
}

/// Whether `command` refuses an agent in `row`'s state before it reaches the
/// guard: wake takes only a failed agent, which a Noesis agent never becomes
/// (its service drives its turns), restart a failed or cancelled one, and a
/// subtree resume a paused subtree.
fn state_refuses_first(command: &Value, row: &Value) -> bool {
    let state = row["status"]["state"].as_str().expect("an agent state");
    match command["cmd"].as_str().expect("a command name") {
        "wake" => state != "failed",
        "restart" => !matches!(state, "failed" | "cancelled"),
        "resume" if command["subtree"] == true => row["paused"] != true,
        _ => false,
    }
}

/// Every generic command that would give `agent` work.
fn generic_commands(agent: &str) -> Vec<Value> {
    vec![
        json!({"cmd": "send_message", "to": agent, "content": "hello"}),
        json!({"cmd": "send_message", "to": agent, "content": "steer the work", "subtree": true}),
        json!({"cmd": "reply_to_question", "to": agent, "question_seq": 1, "content": "an answer"}),
        json!({"cmd": "wake", "agent": agent}),
        json!({"cmd": "resume", "agent": agent}),
        json!({"cmd": "resume", "agent": agent, "subtree": true}),
        json!({"cmd": "restart", "agent": agent}),
        json!({"cmd": "update_agent", "id": agent, "patch": {"model": {"provider": "mock", "model": "mock-1"}}}),
        json!({"cmd": "compact", "agent": agent}),
        json!({"cmd": "trigger", "agent": agent, "source": "test", "payload": {"x": 1}}),
    ]
}

async fn agent_row(home: &mut Home, agent: &str) -> Value {
    let detail = home.ok(json!({"cmd": "get_agent", "id": agent})).await;
    let row = detail.get("agent").cloned().unwrap_or(detail);
    assert_eq!(row["id"], agent, "{row}");
    row
}

/// Each generic command is refused, by the guard wherever the agent's state
/// lets the command reach it, and changes nothing.
async fn assert_every_command_refused(home: &mut Home, agent: &str, when: &str) {
    let (seq, asked) = (home.last_seq().await, home.model.requests().len());
    let row = agent_row(home, agent).await;
    for command in generic_commands(agent) {
        let refused = home.err(command.clone()).await;
        if state_refuses_first(&command, &row) {
            assert_eq!(
                refused["code"], "invalid_argument",
                "{command} {when}: {refused}"
            );
        } else {
            assert_noesis_disabled(&refused, &format!("{command} {when}"));
        }
    }
    assert_eq!(
        home.last_seq().await,
        seq,
        "no refused command committed anything {when}"
    );
    assert_eq!(
        agent_row(home, agent).await,
        row,
        "the agent is unchanged {when}"
    );
    let inbox = home
        .ok(json!({"cmd": "get_inbox", "agent": agent, "after": 0}))
        .await;
    assert!(
        !inbox.to_string().contains("steer the work"),
        "nothing was queued {when}: {inbox}"
    );
    tokio::time::sleep(QUIET).await;
    assert_eq!(home.model.requests().len(), asked, "no model call {when}");
}

#[tokio::test(flavor = "multi_thread")]
async fn g10_every_generic_agent_command_is_refused_for_a_parked_noesis_agent_while_disabled() {
    let mut home = Home::start("").await;
    let (_, agent) = session_with_open_question(&mut home).await;
    home.disable().await;
    assert_every_command_refused(&mut home, &agent, "while disabled").await;
    home.ok(json!({"cmd": "pause", "agent": agent})).await;
    assert_eq!(
        agent_row(&mut home, &agent).await["paused"],
        true,
        "pausing is allowed while disabled"
    );
    assert_every_command_refused(&mut home, &agent, "paused, while disabled").await;

    let mut plain = home
        .ok(json!({"cmd": "create_agent", "role": "worker", "name": "plain", "objective": "Say hello.",
            "model": {"provider": "mock", "model": "mock-1"}, "start": true}))
        .await;
    plain = plain.get("agent").cloned().unwrap_or(plain);
    assert!(
        plain["id"].is_string(),
        "an ordinary agent is untouched by the guard: {plain}"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn g10_a_cancelled_noesis_agent_is_not_restarted_while_disabled() {
    let mut home = Home::start("").await;
    let (_, agent) = session_with_open_question(&mut home).await;
    home.ok(json!({"cmd": "cancel", "agent": agent, "reason": "test"}))
        .await;
    assert_eq!(
        agent_row(&mut home, &agent).await["status"]["state"],
        "cancelled"
    );
    home.disable().await;
    let agents = home.ok(json!({"cmd": "list_agents", "query": {}})).await;
    let (seq, asked) = (home.last_seq().await, home.model.requests().len());
    let refused = home.err(json!({"cmd": "restart", "agent": agent})).await;
    assert_noesis_disabled(&refused, "restarting a cancelled Noesis agent");
    assert_eq!(
        home.last_seq().await,
        seq,
        "the refused restart committed nothing"
    );
    assert_eq!(
        home.ok(json!({"cmd": "list_agents", "query": {}})).await,
        agents,
        "no successor was created"
    );
    tokio::time::sleep(QUIET).await;
    assert_eq!(home.model.requests().len(), asked, "no model call");
}

#[tokio::test(flavor = "multi_thread")]
async fn g11_a_restart_with_a_parked_noesis_agent_while_disabled_resumes_nothing() {
    let mut home = Home::start("").await;
    let (id, agent) = session_with_open_question(&mut home).await;
    home.disable().await;
    let asked = home.model.requests().len();
    let mut home = home.restart().await;
    tokio::time::sleep(QUIET).await;
    let row = agent_row(&mut home, &agent).await;
    assert_ne!(
        row["status"]["state"], "running",
        "nothing resumed the agent at boot: {row}"
    );
    assert_eq!(
        home.model.requests().len(),
        asked,
        "no model call after the restart"
    );
    assert!(
        !holds_open(&home.noesis_db()),
        "a restart while disabled opened noesis.db"
    );
    assert_every_command_refused(&mut home, &agent, "after a restart while disabled").await;

    home.enable().await;
    tokio::time::sleep(QUIET).await;
    assert_eq!(
        home.model.requests().len(),
        asked,
        "re-enabling resumes nothing by surprise"
    );
    let session = home.session(&id).await;
    assert!(
        !session["open_question"].is_null(),
        "the parked question is still open: {session}"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn g12_a_noesis_agent_is_offered_no_tool_to_delegate_with() {
    let mut home = Home::start("").await;
    session_with_open_question(&mut home).await;
    let requests = home.model.requests();
    assert!(!requests.is_empty(), "the Noesis agent called the model");
    for request in &requests {
        let tools = offered_tools(request);
        assert!(
            tools.iter().any(|name| name == "noesis_ask"),
            "a Noesis agent's request: {tools:?}"
        );
        let other: Vec<&String> = tools
            .iter()
            .filter(|name| !name.starts_with("noesis_"))
            .collect();
        assert!(
            other.is_empty(),
            "a Noesis agent was offered Aether tools: {other:?}"
        );
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn g15_every_noesis_invocation_is_charged_to_the_noesis_budget() {
    let mut home = Home::start("").await;
    session_with_open_question(&mut home).await;
    let calls = home.model.requests().len() as u64;
    assert!(calls >= 1, "the session called the model");
    let status = home.ok(json!({"cmd": "noesis_status"})).await;
    let interactive = &status["spend_today"]["interactive"];
    assert_eq!(
        interactive["input_tokens"],
        10 * calls,
        "each call's input is charged: {status}"
    );
    assert_eq!(
        interactive["output_tokens"],
        5 * calls,
        "each call's output is charged: {status}"
    );
    assert_eq!(
        status["spend_today"]["background"]["input_tokens"], 0,
        "{status}"
    );
}
