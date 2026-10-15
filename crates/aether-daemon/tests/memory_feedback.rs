#![allow(clippy::expect_used, clippy::unwrap_used, clippy::panic)]

#[path = "remote/phone.rs"]
mod phone;
mod support;

use aether_core::event::{Actor, EventDraft, EventKind};
use aether_core::id::{AgentId, MemoryId, SnapshotId, TaskId};
use aether_core::model::ModelRef;
use aether_core::snapshot::{ContextLayer, InvocationSnapshot, LayerKind, LayerPlacement};
use aether_core::time::now;
use aether_store::{Commit, Mutation, SqliteStore, Store};
use phone::{Host, Phone};
use serde_json::{Value, json};
use support::RawConn;

async fn feedback_request(host: &mut Host, shown: bool) -> Value {
    let memory = host
        .ok(json!({"cmd":"create_memory", "spec":{
            "text":"Always verify releases with the checked-in script", "kind":"lesson",
            "scope":{"scope":"global"},
        }}))
        .await;
    let task = host
        .ok(json!({"cmd":"create_task", "objective":"Verify release"}))
        .await;
    let request = json!({"cmd":"record_memory_feedback", "memory":memory["id"],
        "task":task["id"], "verdict":"helped", "evidence":"review", "reason":"Release passed"});
    if shown {
        show_memory(
            host,
            serde_json::from_value(memory["id"].clone()).unwrap(),
            serde_json::from_value(task["id"].clone()).unwrap(),
        );
    }
    assert!(request["memory"].is_string());
    assert!(request["task"].is_string());
    request
}

fn show_memory(host: &Host, memory: MemoryId, task: TaskId) {
    let snapshot = InvocationSnapshot {
        id: SnapshotId::new(),
        agent_id: AgentId::new(),
        task_id: Some(task),
        turn: 1,
        model: ModelRef::new("mock", "m"),
        adapter: None,
        capabilities: None,
        memory_ids: vec![memory],
        layers: vec![ContextLayer {
            kind: LayerKind::RetrievedMemory,
            text: None,
            tokens: 0,
            items: 1,
            placement: LayerPlacement::System,
            memories: vec![memory],
        }],
        messages: vec![],
        tools: vec![],
        params: Default::default(),
        token_estimate: 0,
        created_at: now(),
    };
    let id = snapshot.id;
    let event = EventDraft::new(
        Actor::System,
        EventKind::SnapshotCreated {
            snapshot: id,
            turn: 1,
            model: snapshot.model.clone(),
            token_estimate: 0,
            layers: snapshot.layers.clone(),
        },
    )
    .task(task);
    let store = SqliteStore::open(host.daemon.store_path()).unwrap();
    store
        .commit(
            Commit::new()
                .event(event)
                .mutate(Mutation::PutSnapshot(snapshot)),
        )
        .unwrap();
    let found = store.snapshot_memories(&[id]).unwrap();
    assert_eq!(found[0].memories, vec![memory]);
    assert_eq!(found[0].task, Some(task));
}

#[tokio::test]
async fn feedback_in_context_refusal_is_an_invalid_argument_without_writes() {
    let mut host = Host::start().await;
    let request = feedback_request(&mut host, false).await;
    let error = host.err(request.clone()).await;
    assert_eq!(error["code"], "invalid_argument");
    assert!(
        error["message"]
            .as_str()
            .unwrap()
            .contains("was not in context")
    );
    let list = host
        .ok(json!({"cmd":"memory_feedback", "memory":request["memory"]}))
        .await;
    assert_eq!(list, json!({"records":[], "truncated":false}));
    assert!(host.audit("memory.feedback").await.is_empty());
    host.daemon.stop().await;
}

#[tokio::test]
async fn feedback_retry_writes_one_record_and_one_event_and_conflicts_on_changed_fields() {
    let mut host = Host::start().await;
    let mut request = feedback_request(&mut host, true).await;
    request["idempotency_key"] = json!("feedback-review");
    let first = host.ok(request.clone()).await;
    assert!(first["id"].as_str().unwrap().starts_with("mfb_"));
    assert_eq!(host.ok(request.clone()).await, first);
    let events = host.audit("memory.feedback").await;
    assert_eq!(events.len(), 1);
    assert_eq!(events[0]["feedback"], first["id"]);
    assert_eq!(events[0]["task"], request["task"]);
    assert_eq!(events[0]["evidence"], "review");
    let list = host
        .ok(json!({"cmd":"memory_feedback", "memory":request["memory"]}))
        .await;
    assert_eq!(list["records"], json!([first]));
    request["verdict"] = json!("hurt");
    assert_eq!(host.err(request).await["code"], "invalid_argument");
    host.daemon.stop().await;
}

#[tokio::test]
async fn feedback_retraction_is_persistent_and_repeatable_without_another_event() {
    let mut host = Host::start().await;
    let request = feedback_request(&mut host, true).await;
    let first = host.ok(request.clone()).await;
    let retract = json!({"cmd":"retract_memory_feedback", "feedback":first["id"], "reason":"Review withdrawn"});
    let withdrawn = host.ok(retract.clone()).await;
    assert!(withdrawn["retracted_at"].is_string());
    assert_eq!(withdrawn["retraction_reason"], "Review withdrawn");
    assert_eq!(host.ok(retract).await, withdrawn);
    assert_eq!(host.audit("memory.feedback_retracted").await.len(), 1);
    host.daemon = host.daemon.restart().await;
    let mut conn = RawConn::connect(&host.daemon.socket).await.unwrap();
    conn.hello(aether_protocol::PROTOCOL_VERSION).await.unwrap();
    host.conn = conn;
    let found = host
        .ok(json!({"cmd":"memory_feedback", "memory":request["memory"]}))
        .await;
    assert_eq!(found["records"], json!([withdrawn]));
    host.daemon.stop().await;
}

#[tokio::test]
async fn feedback_paired_device_refusal_covers_all_commands_and_both_roles() {
    let mut host = Host::enabled().await;
    let mut conn = RawConn::connect(&host.daemon.socket).await.unwrap();
    let welcome = conn.hello(aether_protocol::PROTOCOL_VERSION).await.unwrap();
    assert!(
        welcome["features"]
            .as_array()
            .unwrap()
            .contains(&json!("memory_feedback"))
    );
    let request = feedback_request(&mut host, true).await;
    let first = host.ok(request.clone()).await;
    let commands = [
        request.clone(),
        json!({"cmd":"retract_memory_feedback", "feedback":first["id"], "reason":"Phone opinion"}),
        json!({"cmd":"memory_feedback", "memory":request["memory"]}),
    ];
    for role in ["operator", "observer"] {
        let phone = Phone::new();
        let device = host.pair(&phone, role, role).await;
        let mut socket = host.connect(&phone, &device).await;
        for command in &commands {
            let refused = socket.call(command.clone()).await;
            assert_eq!(
                refused["result"]["Err"]["code"], "forbidden",
                "{role}: {refused}"
            );
        }
    }
    assert_eq!(host.audit("memory.feedback").await.len(), 1);
    assert!(host.audit("memory.feedback_retracted").await.is_empty());
    host.daemon.stop().await;
}
