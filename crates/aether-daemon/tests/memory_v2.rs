#![allow(clippy::expect_used, clippy::unwrap_used, clippy::panic)]

//! Memory 2.0 on the wire: standing only for a client that lists
//! `memory_v2`, the host-only commands refused to every paired device, and
//! the migration status readable by an observer.

#[path = "remote/phone.rs"]
mod phone;
mod support;

use phone::{Host, Phone};
use serde_json::{Value, json};
use support::{RawConn, hello_frame, read_answer};

const LESSON: &str = "Always verify releases with the checked-in script";

async fn create_lesson(host: &mut Host) -> Value {
    host.ok(json!({"cmd":"create_memory", "spec":{
        "text": LESSON, "kind":"lesson", "scope":{"scope":"global"},
    }}))
    .await
}

/// A second local connection whose hello lists `memory_v2`.
async fn speaking_memory_v2(host: &Host) -> RawConn {
    let mut conn = RawConn::connect(&host.daemon.socket).await.unwrap();
    let mut hello = hello_frame(aether_protocol::PROTOCOL_VERSION, None);
    hello["features"] = json!(["memory_v2"]);
    let welcome = read_answer(conn.call(hello).await).expect("welcome");
    assert!(
        welcome["features"]
            .as_array()
            .unwrap()
            .contains(&json!("memory_v2")),
        "{welcome}"
    );
    conn
}

fn ok(answer: &Value) -> Value {
    answer["result"]
        .get("Ok")
        .unwrap_or_else(|| panic!("refused: {answer}"))["data"]
        .clone()
}

#[tokio::test]
async fn standing_reaches_only_a_client_that_lists_memory_v2() {
    let mut host = Host::start().await;
    let created = create_lesson(&mut host).await;
    assert!(
        created.get("standing").is_none(),
        "an older client: {created}"
    );
    let listed = host
        .ok(json!({"cmd":"memories", "query":{"limit": 10}}))
        .await;
    let listed = listed.as_array().unwrap();
    assert_eq!(listed.len(), 1);
    assert!(listed[0].get("standing").is_none(), "{listed:?}");

    let mut v2 = speaking_memory_v2(&host).await;
    let listed = ok(&v2
        .call(json!({"cmd":"memories", "query":{"limit": 10}}))
        .await);
    let standing = &listed[0]["standing"];
    assert_eq!(standing["tier"], "operational", "{listed}");
    assert_eq!(standing["state"], "active");
    assert_eq!(
        standing["verification"], "verified",
        "an operator stated it"
    );
    assert_eq!(standing["support"], json!({"tasks":0, "independent":0}));
    let pinned = ok(&v2
        .call(json!({"cmd":"pin_memory", "id":created["id"], "pinned":true}))
        .await);
    assert_eq!(pinned["standing"]["state"], "active", "{pinned}");
    host.daemon.stop().await;
}

#[tokio::test]
async fn memory_v2_host_commands_are_refused_to_every_paired_device_and_status_is_observable() {
    let mut host = Host::enabled().await;
    let created = create_lesson(&mut host).await;
    let refused = [
        json!({"cmd":"memory_retrieval_explain", "task":"tsk_00000000000000000000000000"}),
        json!({"cmd":"consolidate_memories", "dry_run": true}),
        json!({"cmd":"memory_export"}),
        json!({"cmd":"memory_import", "artifact":"", "dry_run": true}),
    ];
    for role in ["operator", "observer"] {
        let phone = Phone::new();
        let device = host.pair(&phone, role, role).await;
        let mut socket = host.connect(&phone, &device).await;
        for command in &refused {
            let answer = socket.call(command.clone()).await;
            assert_eq!(
                answer["result"]["Err"]["code"], "forbidden",
                "{role}: {command} -> {answer}"
            );
        }
        let status = ok(&socket.call(json!({"cmd":"memory_migration_status"})).await);
        assert_eq!(
            status["schema_version"],
            aether_store::SCHEMA_VERSION,
            "{status}"
        );
        assert_eq!(status["memories"]["total"], 1);
        let versions: Vec<_> = status["migrations"]
            .as_array()
            .unwrap()
            .iter()
            .map(|m| (m["version"].clone(), m["applied"].clone()))
            .collect();
        assert_eq!(
            versions,
            [
                (json!(30), json!(true)),
                (json!(31), json!(true)),
                (json!(32), json!(true))
            ]
        );
        for backup in status["backups"].as_array().unwrap() {
            assert!(
                !backup["name"].as_str().unwrap().contains('/'),
                "no host path"
            );
        }
    }
    let export = host.ok(json!({"cmd":"memory_export"})).await;
    assert_eq!(export["records"], 1);
    assert!(
        export["artifact"]
            .as_str()
            .unwrap()
            .contains(created["id"].as_str().unwrap())
    );
    host.daemon.stop().await;
}

#[tokio::test]
async fn an_import_over_the_wire_is_dry_run_first_and_idempotent_after() {
    let mut host = Host::start().await;
    create_lesson(&mut host).await;
    let export = host.ok(json!({"cmd":"memory_export"})).await;
    let artifact = export["artifact"].clone();
    let dry = host
        .ok(json!({"cmd":"memory_import", "artifact": artifact, "dry_run": true}))
        .await;
    assert_eq!(dry["dry_run"], true);
    assert_eq!(
        (dry["inserted"].clone(), dry["unchanged"].clone()),
        (json!(0), json!(1))
    );
    assert_eq!(dry["digest"], export["digest"]);
    let applied = host
        .ok(json!({"cmd":"memory_import", "artifact": artifact, "dry_run": false}))
        .await;
    assert_eq!(applied["unchanged"], 1);
    assert_eq!(host.audit("memory.imported").await.len(), 1);
    let status = host.ok(json!({"cmd":"memory_migration_status"})).await;
    assert_eq!(
        status["last_import"]["digest"], export["digest"],
        "{status}"
    );
    let refused = host
        .err(json!({"cmd":"memory_import", "artifact":"not an export", "dry_run": true}))
        .await;
    assert_eq!(refused["code"], "invalid_argument", "{refused}");
    host.daemon.stop().await;
}
