#![allow(clippy::expect_used, clippy::panic)]
use super::*;
use crate::{Client, Transport, outbox::Outbox};
use serde_json::json;
use std::collections::BTreeSet;

fn commands() -> Vec<Command> {
    let agent = aether_core::id::AgentId::new();
    let schedule = aether_core::id::ScheduleId::new();
    let approval = aether_core::id::ApprovalId::new();
    let wires = [
        json!({"cmd":"create_agent", "role":"worker"}),
        json!({"cmd":"create_task", "objective":"ship"}),
        json!({"cmd":"send_message", "to":agent, "content":"hi"}),
        json!({"cmd":"reply_to_question", "to":agent, "question_seq":1, "content":"yes"}),
        json!({"cmd":"trigger", "agent":agent, "source":"test"}),
        json!({"cmd":"create_schedule", "spec": {"name":"timer", "target":agent, "recurrence":{"kind":"interval", "seconds":60}, "missed":"skip", "busy":"skip", "payload":null, "max_deliveries":null}}),
        json!({"cmd":"restart", "agent":agent}),
        json!({"cmd":"resolve_approval", "id":approval, "decision":"approved"}),
        json!({"cmd":"cancel", "agent":agent}),
        json!({"cmd":"interrupt", "agent":agent}),
        json!({"cmd":"pause", "agent":agent}),
        json!({"cmd":"resume", "agent":agent}),
        json!({"cmd":"archive", "target":"all_finished"}),
        json!({"cmd":"update_agent", "id":agent, "patch":{}}),
        json!({"cmd":"set_approvals", "agent":agent, "policy": aether_core::agent::ApprovalPolicy::default()}),
        json!({"cmd":"update_schedule", "id":schedule, "action":"pause"}),
        json!({"cmd":"create_memory", "spec":{"text":"remember", "kind":"fact", "scope":{"scope":"global"}}}),
        json!({"cmd":"add_approval_rule", "decision":"approved", "match":{}, "scope":{"scope":"everywhere"}}),
    ];
    wires
        .into_iter()
        .map(|wire| {
            serde_json::from_value(wire.clone()).unwrap_or_else(|error| panic!("{wire}: {error}"))
        })
        .collect()
}

#[tokio::test]
async fn every_keyed_schema_command_assigns_preserves_and_records_its_key() {
    let schema = aether_protocol::schema::schema();
    let variants = schema["$defs"]["ClientFrame"]["oneOf"]
        .as_array()
        .expect("variants");
    let expected: BTreeSet<_> = variants
        .iter()
        .filter(|variant| {
            let body = match variant["$ref"].as_str() {
                Some(reference) => schema
                    .pointer(reference.strip_prefix('#').expect("local reference"))
                    .expect("resolved schema"),
                None => variant,
            };
            body["properties"].get("idempotency_key").is_some()
        })
        .map(|variant| {
            variant["properties"]["cmd"]["const"]
                .as_str()
                .expect("tag")
                .to_owned()
        })
        .collect();
    let commands = commands();
    let actual: BTreeSet<_> = commands.iter().map(Command::name).collect();
    assert!(!expected.is_empty());
    assert_eq!(
        actual, expected,
        "update fixtures for every new keyed command"
    );
    let directory = tempfile::tempdir().expect("directory");
    let outbox = Outbox::open(directory.path().into()).await.expect("outbox");
    for mut command in commands {
        assert!(command.keyed_request().is_none());
        assert!(key_command(&mut command), "{}", command.name());
        let first = command.clone();
        assert!(key_command(&mut command));
        assert_eq!(command, first, "preserve existing keys");
        outbox
            .record(&command, None)
            .await
            .expect("record keyed operation");
    }
    assert_eq!(
        outbox.pending().await.expect("pending").entries.len(),
        expected.len()
    );
}

#[tokio::test]
async fn version_17_only_records_commands_supported_by_that_host() {
    let (transport, _peer) = Transport::memory(4);
    let mut client = Client::from_transport(transport);
    let (cancel, _cancelled) = tokio::sync::oneshot::channel();
    client.carrier = Some(std::sync::Arc::new(crate::ssh::Carrier::test_carrier(
        cancel,
    )));
    let mut welcome = crate::tests::welcome();
    welcome.daemon_protocol_version = Some(17);
    client.welcome = Some(welcome);
    let legacy = [
        "create_agent",
        "create_task",
        "send_message",
        "reply_to_question",
        "trigger",
        "create_schedule",
    ];
    for mut command in commands() {
        key_command(&mut command);
        let supported = legacy.contains(&command.name().as_str());
        assert_eq!(
            client
                .prepare_remote_command(&mut command)
                .expect("prepare"),
            supported
        );
        assert_eq!(
            command.keyed_request().is_some(),
            supported,
            "{}",
            command.name()
        );
    }
}
