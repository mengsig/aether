#![allow(clippy::expect_used, clippy::unwrap_used, clippy::panic)]

//! ARCHITECTURE.md §g, the off-by-default guarantees, one test each, over a
//! real daemon: Noesis is off unless configured on, and while off it
//! registers, starts, opens and adds nothing, and refuses its commands.

mod noesis_support;
#[path = "remote/phone.rs"]
mod phone;
mod support;

use std::collections::BTreeSet;
use std::time::Duration;

use noesis_support::faux::offered_tools;
use noesis_support::{Home, assert_no_noesis_db, assert_noesis_disabled, holds_open};
use serde_json::{Value, json};

/// How long an idle daemon is watched for work it must not start.
const QUIET: Duration = Duration::from_millis(1500);

/// Create an ordinary agent on the faux model and return the first request it made.
async fn ordinary_agent_request(home: &mut Home) -> Value {
    let before = home.model.requests().len();
    let created = home
        .ok(json!({"cmd": "create_agent", "role": "worker", "name": "plain", "objective": "Say hello.",
            "model": {"provider": "mock", "model": "mock-1"}, "start": true}))
        .await;
    assert!(created.to_string().contains("worker"), "{created}");
    let requests = home.requests_reach(before + 1).await;
    requests[before].clone()
}

/// One valid frame for every Noesis command but status and configure.
fn refused_commands() -> Vec<Value> {
    vec![
        json!({"cmd": "noesis_create_session", "goal": "Learn limits", "mode": "teach", "idempotency_key": "k1"}),
        json!({"cmd": "noesis_send", "session": "ns_x", "text": "hello", "idempotency_key": "k2"}),
        json!({"cmd": "noesis_answer", "session": "ns_x", "question": "q_x", "answer": {"dont_know": true}, "idempotency_key": "k3"}),
        json!({"cmd": "noesis_get_session", "session": "ns_x"}),
        json!({"cmd": "noesis_list_sessions"}),
        json!({"cmd": "noesis_delete_session", "session": "ns_x", "graph": "preserve", "idempotency_key": "k4"}),
        json!({"cmd": "noesis_graph"}),
        json!({"cmd": "noesis_delete_proposition", "proposition": "p_x", "idempotency_key": "k5"}),
        json!({"cmd": "noesis_delete_source", "source": "s_x", "graph": "preserve", "idempotency_key": "k6"}),
        json!({"cmd": "noesis_extract", "session": "ns_x", "idempotency_key": "k7"}),
        json!({"cmd": "noesis_import_source", "name": "notes", "artifact": "art_01ARZ3NDEKTSV4RRFFQ69G5FAV", "idempotency_key": "k8"}),
        json!({"cmd": "noesis_jobs"}),
        json!({"cmd": "noesis_cancel_job", "job": "j_x", "idempotency_key": "k9"}),
        json!({"cmd": "noesis_learner"}),
        json!({"cmd": "noesis_learner_evidence", "concept": "k_x"}),
        json!({"cmd": "noesis_correct_learner", "concept": "k_x", "state": "exposed", "reason": "checked", "idempotency_key": "k10"}),
    ]
}

/// Every `noesis_*` command the protocol schema names.
fn schema_noesis_commands() -> BTreeSet<String> {
    fn walk(value: &Value, names: &mut BTreeSet<String>) {
        match value {
            Value::Object(fields) => {
                if let Some(name) = fields
                    .get("properties")
                    .and_then(|p| p.get("cmd"))
                    .and_then(|cmd| cmd.get("const"))
                    .and_then(Value::as_str)
                    && name.starts_with("noesis_")
                {
                    names.insert(name.to_owned());
                }
                fields.values().for_each(|v| walk(v, names));
            }
            Value::Array(items) => items.iter().for_each(|v| walk(v, names)),
            _ => {}
        }
    }
    let mut names = BTreeSet::new();
    walk(&aether_protocol::schema::schema(), &mut names);
    names
}

#[tokio::test(flavor = "multi_thread")]
async fn g01_noesis_is_disabled_by_default() {
    let mut home = Home::start("").await;
    let status = home.ok(json!({"cmd": "noesis_status"})).await;
    assert_eq!(status["enabled"], false, "{status}");
    assert_eq!(status["settings"]["enabled"], false, "{status}");
    assert_eq!(status["effective"]["enabled"], false, "{status}");
    assert_eq!(status["settings"]["engineering_context"], false, "{status}");
    assert_eq!(
        status["settings"]["extraction"]["sessions"], "manual",
        "{status}"
    );
    assert_eq!(
        status["settings"]["extraction"]["engineering"], "off",
        "{status}"
    );
    assert_eq!(status["jobs_running"], 0, "{status}");
    assert!(
        !home.config_toml().contains("[noesis]"),
        "the default needs no [noesis] table"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn g02_no_noesis_tool_is_offered_to_any_agent_while_disabled() {
    let mut home = Home::start("").await;
    let request = ordinary_agent_request(&mut home).await;
    let tools = offered_tools(&request);
    assert!(
        !tools.is_empty(),
        "the ordinary agent is offered its tools: {request}"
    );
    let noesis: Vec<&String> = tools
        .iter()
        .filter(|name| name.starts_with("noesis"))
        .collect();
    assert!(
        noesis.is_empty(),
        "Noesis tools offered while disabled: {noesis:?}"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn g03_no_noesis_role_can_be_created_while_disabled() {
    let mut home = Home::start("").await;
    let before = home.last_seq().await;
    for role in [
        "noesis.prober",
        "noesis.teacher",
        "noesis.practitioner",
        "noesis.extractor",
        "noesis.auditor",
    ] {
        let refused = home
            .err(
                json!({"cmd": "create_agent", "role": role, "name": "posing", "objective": "teach",
                "model": {"provider": "mock", "model": "mock-1"}, "start": true}),
            )
            .await;
        assert_eq!(refused["code"], "invalid_argument", "{role}: {refused}");
    }
    assert_eq!(
        home.last_seq().await,
        before,
        "no refused creation committed anything"
    );
    let agents = home.ok(json!({"cmd": "list_agents", "query": {}})).await;
    assert!(
        !agents.to_string().contains("noesis."),
        "no Noesis agent exists: {agents}"
    );
    assert!(home.model.requests().is_empty(), "nothing called the model");
}

#[tokio::test(flavor = "multi_thread")]
async fn g04_no_worker_or_timer_starts_while_disabled() {
    let stored = "\n[noesis]\nengineering_context = true\n[noesis.extraction]\nsessions = \"auto\"\nengineering = \"auto\"\n";
    let mut home = Home::start(stored).await;
    let first = home.last_seq().await;
    tokio::time::sleep(QUIET).await;
    let status = home.ok(json!({"cmd": "noesis_status"})).await;
    assert_eq!(
        status["settings"]["extraction"]["sessions"], "auto",
        "the stored switch is read: {status}"
    );
    assert_eq!(
        status["effective"]["extraction"]["sessions"], "manual",
        "{status}"
    );
    assert_eq!(status["jobs_running"], 0, "{status}");
    assert!(
        home.model.requests().is_empty(),
        "no worker called the model"
    );
    assert_eq!(
        home.last_seq().await,
        first,
        "no timer committed anything while idle"
    );
    let tree = home
        .ok(json!({"cmd": "get_tree", "include_archived": true, "role_prefix": "noesis."}))
        .await;
    assert!(
        !tree.to_string().contains("noesis."),
        "no Noesis agent was started: {tree}"
    );
    assert_no_noesis_db(&home, "by an idle disabled daemon");
}

#[tokio::test(flavor = "multi_thread")]
async fn g05_noesis_db_is_never_opened_or_created_while_disabled() {
    let mut home = Home::start("").await;
    assert_no_noesis_db(&home, "at startup");
    for command in refused_commands() {
        let refused = home.err(command.clone()).await;
        assert_noesis_disabled(&refused, &command.to_string());
    }
    assert_no_noesis_db(&home, "by a refused command");
    home.ok(json!({"cmd": "noesis_status"})).await;
    home.configure(json!({"engineering_context": true})).await;
    assert_no_noesis_db(&home, "by status or a configure that keeps Noesis off");
    let mut home = home.restart().await;
    assert_no_noesis_db(&home, "by a restart while disabled");

    home.enable().await;
    assert!(
        home.noesis_db().exists(),
        "enabling opens noesis.db, so the checks above could fail"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn g06_no_context_layer_is_added_to_any_agent_while_disabled() {
    let mut home = Home::start("").await;
    let request = ordinary_agent_request(&mut home).await;
    let messages = request["messages"].to_string().to_lowercase();
    assert!(
        messages.contains("say hello"),
        "the request carries the agent's own context: {request}"
    );
    for word in ["noesis", "learner", "teaching"] {
        assert!(
            !messages.contains(word),
            "a Noesis context layer ({word}) reached an agent: {messages}"
        );
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn g07_every_noesis_command_but_status_and_configure_is_refused_with_no_side_effect() {
    let mut home = Home::start("").await;
    let commands = refused_commands();
    let sampled: BTreeSet<String> = commands
        .iter()
        .map(|c| c["cmd"].as_str().unwrap().to_owned())
        .collect();
    let mut expected = schema_noesis_commands();
    assert!(
        expected.remove("noesis_status") && expected.remove("noesis_configure"),
        "{expected:?}"
    );
    assert_eq!(
        sampled, expected,
        "every Noesis command of the protocol is sampled"
    );

    let (seq, config) = (home.last_seq().await, home.config_toml());
    for command in commands {
        let refused = home.err(command.clone()).await;
        assert_noesis_disabled(&refused, &command.to_string());
    }
    assert_eq!(
        home.last_seq().await,
        seq,
        "no refused command committed an event"
    );
    assert_eq!(
        home.config_toml(),
        config,
        "no refused command wrote the config"
    );
    assert!(
        home.model.requests().is_empty(),
        "no refused command called the model"
    );
    assert_no_noesis_db(&home, "by a refused command");
}

#[tokio::test(flavor = "multi_thread")]
async fn g08_configure_while_disabled_only_validates_and_writes_the_config() {
    let mut home = Home::start("").await;
    let invalid = home
        .err(json!({"cmd": "noesis_configure", "patch": {"concurrency": {"interactive": 9}}}))
        .await;
    assert_eq!(
        invalid["code"], "invalid_argument",
        "an invalid value is refused, not clamped: {invalid}"
    );
    assert!(
        !home.config_toml().contains("[noesis"),
        "a refused patch writes nothing"
    );

    let status = home
        .configure(json!({"engineering_context": true, "concurrency": {"interactive": 3}}))
        .await;
    assert_eq!(status["enabled"], false, "{status}");
    assert_eq!(status["settings"]["engineering_context"], true, "{status}");
    let config = home.config_toml();
    assert!(
        config.contains("[noesis") && config.contains("engineering_context = true"),
        "{config}"
    );
    assert!(
        home.model.requests().is_empty(),
        "configure called no model"
    );
    assert_no_noesis_db(&home, "by a configure while disabled");
}

#[tokio::test(flavor = "multi_thread")]
async fn g09_enabling_writes_first_then_opens_noesis_db() {
    let mut home = Home::start("").await;
    assert_no_noesis_db(&home, "before enabling");
    let status = home.enable().await;
    assert_eq!(status["effective"]["enabled"], true, "{status}");
    assert!(
        home.config_toml().contains("enabled = true"),
        "the switch is written"
    );
    assert!(home.noesis_db().exists(), "the service opened noesis.db");
    let page = home
        .ok(json!({"cmd": "get_events", "after": 0, "limit": 1000, "query": {}}))
        .await;
    assert!(
        page.to_string().contains("noesis.settings_changed"),
        "the change is announced: {page}"
    );
    let restarted = home.restart().await;
    assert!(
        holds_open(&restarted.noesis_db()),
        "an enabled service reopens noesis.db at boot"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn g13_switches_are_effective_only_with_their_prerequisites() {
    let mut home = Home::start("").await;
    let stored = home
        .configure(json!({"engineering_context": true, "extraction": {"sessions": "auto", "engineering": "auto"}}))
        .await;
    assert_eq!(
        stored["settings"]["extraction"]["engineering"], "auto",
        "{stored}"
    );
    assert_eq!(
        stored["effective"]["engineering_context"], false,
        "{stored}"
    );
    assert_eq!(
        stored["effective"]["extraction"]["sessions"], "manual",
        "{stored}"
    );
    assert_eq!(
        stored["effective"]["extraction"]["engineering"], "off",
        "{stored}"
    );

    let on = home.enable().await;
    assert_eq!(on["effective"]["engineering_context"], true, "{on}");
    assert_eq!(on["effective"]["extraction"]["sessions"], "auto", "{on}");
    assert_eq!(on["effective"]["extraction"]["engineering"], "auto", "{on}");
    let no_context = home.configure(json!({"engineering_context": false})).await;
    assert_eq!(
        no_context["effective"]["extraction"]["engineering"], "off",
        "{no_context}"
    );
    assert_eq!(
        no_context["settings"]["extraction"]["engineering"], "auto",
        "the stored value stays: {no_context}"
    );
    assert_eq!(
        no_context["effective"]["extraction"]["sessions"], "auto",
        "{no_context}"
    );

    let off = home.disable().await;
    assert_eq!(
        off["effective"]["extraction"]["sessions"], "manual",
        "{off}"
    );
    assert_eq!(off["settings"]["extraction"]["sessions"], "auto", "{off}");
}

#[tokio::test(flavor = "multi_thread")]
async fn g14_disabling_refuses_new_work_at_once_and_reports_the_policy() {
    let mut home = Home::start("").await;
    let status = home.ok(json!({"cmd": "noesis_status"})).await;
    assert_eq!(status["disable_policy"]["grace_seconds"], 60, "{status}");
    assert_eq!(
        status["disable_policy"]["background"], "cancel_at_model_boundary",
        "{status}"
    );

    home.enable().await;
    home.model.text("Welcome. What do you already know?");
    let created = home.create_session("teach", "Learn limits").await;
    let id = created["id"].as_str().expect("a session id").to_owned();
    home.requests_reach(1).await;
    home.disable().await;
    let asked = home.model.requests().len();
    let send = json!({"cmd": "noesis_send", "session": id, "text": "Nothing yet.", "idempotency_key": "send-1"});
    let refused = home.err(send).await;
    assert_noesis_disabled(&refused, "a message right after the disable");
    let session = home
        .err(json!({"cmd": "noesis_get_session", "session": id}))
        .await;
    assert_noesis_disabled(&session, "a read right after the disable");
    tokio::time::sleep(QUIET).await;
    assert_eq!(
        home.model.requests().len(),
        asked,
        "no model call after the disable"
    );
}
