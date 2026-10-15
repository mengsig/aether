#![allow(clippy::expect_used, clippy::unwrap_used, clippy::panic)]

//! `alias_repository` over a real socket, and refused to a paired device.

#[path = "remote/phone.rs"]
mod phone;
mod support;

use phone::{Host, Phone};
use serde_json::{Value, json};
use support::RawConn;

const CANONICAL: &str = "github.com/mengsig/archon";

fn alias(dry_run: bool) -> Value {
    json!({"cmd":"alias_repository", "alias":"archon", "repository":CANONICAL, "dry_run":dry_run})
}

fn under(repository: &str) -> Value {
    json!({"cmd":"memories", "query":{"scope":{"scope":"repository","id":repository}, "limit":10}})
}

/// An agent and a memory under the old name, as Archon 1.2.1 leaves them.
async fn filed_under_archon(host: &mut Host) -> Value {
    host.ok(
        json!({"cmd":"create_agent", "role":"alias-test", "start":false,
        "model":{"provider":"claude-code","model":"haiku"},
        "scope":{"repository":"archon"}}),
    )
    .await;
    host.ok(json!({"cmd":"create_memory", "spec":{
        "text":"Archon's tests run under cargo nextest", "kind":"fact",
        "scope":{"scope":"repository","repository":"archon"}}}))
        .await
}

/// The feature is advertised; a dry run counts and moves nothing; the real
/// run moves the same; a query naming the old id finds the moved memory.
#[tokio::test]
async fn a_dry_run_counts_what_the_alias_then_moves() {
    let mut host = Host::enabled().await;
    let mut conn = RawConn::connect(&host.daemon.socket).await.unwrap();
    let welcome = conn.hello(aether_protocol::PROTOCOL_VERSION).await.unwrap();
    let features = welcome["features"].as_array().unwrap();
    assert!(features.contains(&json!("repository_aliases")), "{welcome}");
    let memory = filed_under_archon(&mut host).await;

    let preview = host.ok(alias(true)).await;
    assert_eq!(
        (&preview["dry_run"], &preview["existing"]),
        (&json!(true), &json!(false))
    );
    assert_eq!(preview["counts"]["repository"], 1, "{preview}");
    assert_eq!(preview["counts"]["agents"], 1, "{preview}");
    assert_eq!(
        host.ok(under(CANONICAL)).await,
        json!([]),
        "a dry run moves nothing"
    );
    assert_eq!(
        host.ok(json!({"cmd":"list_repository_aliases"})).await,
        json!([])
    );

    let moved = host.ok(alias(false)).await;
    assert_eq!(moved["dry_run"], false);
    assert_eq!(moved["counts"], preview["counts"]);
    let aliases = host.ok(json!({"cmd":"list_repository_aliases"})).await;
    assert_eq!(aliases[0]["alias"], "archon");
    assert_eq!(aliases[0]["canonical"], CANONICAL);

    let found = host.ok(under("archon")).await;
    assert_eq!(found[0]["id"], memory["id"], "{found}");
    assert_eq!(found[0]["scope"]["id"], CANONICAL);
    assert_eq!(found, host.ok(under(CANONICAL)).await);

    let again = host.ok(alias(false)).await;
    assert_eq!(
        (&again["existing"], &again["counts"]["repository"]),
        (&json!(true), &json!(0))
    );
    host.daemon.stop().await;
}

/// The move is irreversible and the host's alone: a paired device may list
/// aliases but not register one, and its attempt moves nothing.
#[tokio::test]
async fn a_paired_device_cannot_alias_a_repository() {
    let mut host = Host::enabled().await;
    filed_under_archon(&mut host).await;
    let phone = Phone::new();
    let device = host.pair(&phone, "phone", "operator").await;
    let mut socket = host.connect(&phone, &device).await;

    for dry_run in [true, false] {
        let refused = socket.call(alias(dry_run)).await;
        assert_eq!(refused["result"]["Err"]["code"], "forbidden", "{refused}");
    }
    let listed = socket.call(json!({"cmd":"list_repository_aliases"})).await;
    assert_eq!(listed["result"]["Ok"]["data"], json!([]), "{listed}");
    assert_eq!(host.ok(under("archon")).await.as_array().unwrap().len(), 1);
    host.daemon.stop().await;
}
