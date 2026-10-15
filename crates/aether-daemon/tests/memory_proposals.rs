#![allow(clippy::expect_used, clippy::unwrap_used, clippy::panic)]

#[path = "remote/phone.rs"]
mod phone;
mod support;

use phone::Host;
use serde_json::{Value, json};
use support::RawConn;

fn spec(text: &str, proposed: bool, supersedes: Option<&Value>) -> Value {
    json!({"cmd":"create_memory", "spec":{
        "text":text, "kind":"lesson", "scope":{"scope":"global"},
        "proposed":proposed, "supersedes":supersedes,
    }})
}

#[tokio::test]
async fn proposals_are_advertised_persisted_and_accepted_with_deferred_replacement() {
    let mut host = Host::start().await;
    let mut conn = RawConn::connect(&host.daemon.socket).await.unwrap();
    let welcome = conn.hello(aether_protocol::PROTOCOL_VERSION).await.unwrap();
    assert!(
        welcome["features"]
            .as_array()
            .unwrap()
            .contains(&json!("memory_proposals"))
    );
    let old = host
        .ok(spec(
            "Always use the old build verification script",
            false,
            None,
        ))
        .await;
    let pending = host
        .ok(spec(
            "Always use the new build verification script",
            true,
            Some(&old["id"]),
        ))
        .await;
    assert!(pending["proposed_at"].is_string());
    assert_eq!(pending["provenance"]["actor"]["actor"], "user");
    let current = host.ok(json!({"cmd":"memories", "query":{}})).await;
    assert_eq!(current[0]["id"], old["id"]);
    assert_eq!(current.as_array().unwrap().len(), 1);
    let queue = host
        .ok(json!({"cmd":"memories", "query":{"proposed":true}}))
        .await;
    assert_eq!(queue[0]["id"], pending["id"]);
    let accept =
        json!({"cmd":"accept_memory", "id":pending["id"], "reason":"Confirmed by releases"});
    let accepted = host.ok(accept.clone()).await;
    assert!(accepted["accepted_at"].is_string());
    let history = host
        .ok(json!({"cmd":"get_events", "after":0,"limit":10000,"query":{}}))
        .await;
    assert_eq!(host.ok(accept).await, accepted);
    assert_eq!(
        host.ok(json!({"cmd":"get_events", "after":0,"limit":10000,"query":{}}))
            .await,
        history
    );
    let active = host.ok(json!({"cmd":"memories", "query":{}})).await;
    assert_eq!(active[0]["id"], accepted["id"]);
    assert_eq!(active.as_array().unwrap().len(), 1);
    let error = host
        .err(json!({"cmd":"reject_memory", "id":accepted["id"], "reason":"Too late"}))
        .await;
    assert_eq!(error["code"], "invalid_argument");
    host.daemon.stop().await;
}

#[tokio::test]
async fn keyed_proposal_creation_returns_the_same_id_and_writes_one_proposed_event() {
    let mut host = Host::start().await;
    let mut request = spec("Always verify release builds before publishing", true, None);
    request["idempotency_key"] = json!("proposed-release-verification");
    let first = host.ok(request.clone()).await;
    let second = host.ok(request).await;
    assert!(first["proposed_at"].is_string());
    assert_eq!(second["id"], first["id"]);
    assert_eq!(second, first);
    let events = host.audit("memory.proposed").await;
    assert_eq!(events.len(), 1);
    assert_eq!(events[0]["memory"], first["id"]);
    host.daemon.stop().await;
}

#[tokio::test]
async fn rejected_proposals_restore_to_review_and_report_missing_ids() {
    let mut host = Host::start().await;
    let pending = host
        .ok(spec(
            "Always validate release changes with an independent review",
            true,
            None,
        ))
        .await;
    let reject =
        json!({"cmd":"reject_memory", "id":pending["id"], "reason":"Insufficient evidence"});
    let rejected = host.ok(reject.clone()).await;
    assert!(rejected["forgotten_at"].is_string());
    let history = host
        .ok(json!({"cmd":"get_events", "after":0,"limit":10000,"query":{}}))
        .await;
    assert_eq!(host.ok(reject).await, rejected);
    assert_eq!(
        host.ok(json!({"cmd":"get_events", "after":0,"limit":10000,"query":{}}))
            .await,
        history
    );
    let error = host
        .err(json!({"cmd":"accept_memory", "id":pending["id"]}))
        .await;
    assert_eq!(error["code"], "invalid_argument");
    let restored = host
        .ok(json!({"cmd":"restore_memory", "id":pending["id"]}))
        .await;
    assert!(restored.get("forgotten_at").is_none());
    assert!(restored.get("accepted_at").is_none());
    assert_eq!(
        host.ok(json!({"cmd":"memories", "query":{}})).await,
        json!([])
    );
    for command in ["accept_memory", "reject_memory"] {
        let error = host
            .err(json!({"cmd":command, "id":aether_core::id::MemoryId::new(), "reason":"Missing"}))
            .await;
        assert_eq!(error["code"], "not_found");
    }
    host.daemon.stop().await;
}
