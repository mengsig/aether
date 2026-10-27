use super::*;

fn keyed_commands(key: &str) -> Vec<Command> {
    let agent = AgentId::new();
    vec![
        Command::CreateAgent(CreateAgent {
            role: "worker".into(),
            idempotency_key: Some(key.into()),
            ..Default::default()
        }),
        Command::CreateTask(CreateTask {
            objective: "ship".into(),
            idempotency_key: Some(key.into()),
            ..Default::default()
        }),
        Command::ReplyToQuestion {
            to: agent,
            question_seq: 42,
            content: "yes".into(),
            idempotency_key: Some(key.into()),
        },
        Command::SendMessage {
            to: agent,
            content: "hi".into(),
            subtree: false,
            attachments: Vec::new(),
            idempotency_key: Some(key.into()),
        },
        Command::Trigger {
            agent,
            source: "ci".into(),
            payload: serde_json::json!({"run": 1}),
            idempotency_key: Some(key.into()),
        },
        Command::CreateSchedule {
            spec: aether_core::schedule::CreateScheduleSpec {
                name: "nightly".into(),
                target: ScheduleTarget::Agent(agent),
                recurrence: aether_core::schedule::Recurrence::Interval {
                    seconds: std::num::NonZeroU64::new(60).expect("nonzero"),
                },
                missed: Default::default(),
                busy: Default::default(),
                payload: serde_json::Value::Null,
                max_deliveries: None,
            },
            idempotency_key: Some(key.into()),
        },
    ]
}

/// `SUBMISSION_KEY_FIELD` names the wire field, and the digest must not
/// cover it. Renaming the field on any keyed command without moving the
/// constant would otherwise leave the key inside the fingerprint with
/// every other test still green.
#[test]
fn the_key_field_is_named_the_same_on_every_keyed_command_and_is_never_digested() {
    let field = aether_core::idempotency::SUBMISSION_KEY_FIELD;
    for command in keyed_commands("k-1") {
        let wire = serde_json::to_value(&command).expect("ser");
        assert_eq!(
            wire.get(field).and_then(serde_json::Value::as_str),
            Some("k-1"),
            "{} carries its key at the top level under `{field}`",
            command.name()
        );
        let keyed = command
            .keyed_request()
            .expect("carries a key")
            .expect("view");
        assert_eq!(keyed.key, "k-1");
        assert!(
            keyed.fields.get(field).is_none(),
            "{} digests the work asked for, not the key",
            command.name()
        );
    }
}

/// The digest covers the command tag, so one key cannot be replayed
/// across two command shapes, and it changes with the work asked for.
#[test]
fn the_fingerprint_view_names_the_command_and_follows_its_arguments() {
    let mut tags: Vec<String> = Vec::new();
    for command in keyed_commands("k-1") {
        let fields = command
            .keyed_request()
            .expect("keyed")
            .expect("view")
            .fields;
        tags.push(fields["cmd"].as_str().expect("tag").to_owned());
    }
    let mut unique = tags.clone();
    unique.sort();
    unique.dedup();
    assert_eq!(unique.len(), tags.len(), "one tag per command shape");

    let agent = AgentId::new();
    let view = |content: &str| {
        Command::SendMessage {
            to: agent,
            content: content.into(),
            subtree: false,
            attachments: Vec::new(),
            idempotency_key: Some("k-1".into()),
        }
        .keyed_request()
        .expect("keyed")
        .expect("view")
        .fields
    };
    assert_ne!(view("hi"), view("ho"));
}

#[test]
fn question_reply_roundtrips_and_fingerprints_every_request_field() {
    let agent = AgentId::new();
    let wire = serde_json::json!({
        "cmd": "reply_to_question", "to": agent, "question_seq": 42, "content": "yes"
    });
    let unkeyed: Command = serde_json::from_value(wire.clone()).expect("optional key");
    assert_eq!(serde_json::to_value(&unkeyed).expect("encode"), wire);
    assert!(unkeyed.keyed_request().is_none());
    let view = |to, seq, content: &str| {
        let command = Command::ReplyToQuestion {
            to,
            question_seq: seq,
            content: content.into(),
            idempotency_key: Some("reply-1".into()),
        };
        let encoded = crate::encode(&command).expect("encode");
        assert_eq!(crate::decode::<Command>(&encoded).expect("decode"), command);
        command
            .keyed_request()
            .expect("keyed")
            .expect("fields")
            .fields
    };
    let first = view(agent, 42, "yes");
    assert_eq!(first, wire);
    assert_ne!(first, view(AgentId::new(), 42, "yes"));
    assert_ne!(first, view(agent, 43, "yes"));
    assert_ne!(first, view(agent, 42, "no"));
}

/// A delivery schedule keyed under protocol 8 must digest the same after
/// the upgrade, or its correct retry is refused as a different request.
#[test]
fn a_delivery_schedule_fingerprints_exactly_as_protocol_8_did() {
    let agent = AgentId::new();
    let command = Command::CreateSchedule {
        spec: aether_core::schedule::CreateScheduleSpec {
            name: "nightly".into(),
            target: ScheduleTarget::Agent(agent),
            recurrence: aether_core::schedule::Recurrence::Interval {
                seconds: std::num::NonZeroU64::new(60).expect("nonzero"),
            },
            missed: aether_core::schedule::MissedRunPolicy::Skip,
            busy: aether_core::schedule::BusyPolicy::Skip,
            payload: serde_json::json!({"run": 1}),
            max_deliveries: std::num::NonZeroU64::new(3),
        },
        idempotency_key: Some("k-1".into()),
    };
    let fields = command
        .keyed_request()
        .expect("keyed")
        .expect("view")
        .fields;
    // The protocol 8 view, field for field.
    let v8 = serde_json::json!({
        "cmd": "create_schedule",
        "name": "nightly",
        "target": agent,
        "recurrence": {"kind": "interval", "seconds": 60},
        "missed": "skip",
        "payload": {"run": 1},
        "max_deliveries": 3,
    });
    assert_eq!(fields, v8);
}

/// A key on a command that does not create work is not a submission.
#[test]
fn an_unkeyed_command_has_no_fingerprint_view() {
    assert!(Command::Status.keyed_request().is_none());
    assert!(
        Command::SendMessage {
            to: AgentId::new(),
            content: "hi".into(),
            subtree: false,
            attachments: Vec::new(),
            idempotency_key: None,
        }
        .keyed_request()
        .is_none()
    );
}

/// A create without a checkout keeps the digest it had before the field
/// existed, and one key cannot name two checkouts.
#[test]
fn a_create_fingerprints_its_repository_root_only_when_set() {
    let view = |root: Option<&str>| {
        Command::CreateAgent(CreateAgent {
            role: "worker".into(),
            repository_root: root.map(Into::into),
            idempotency_key: Some("k-1".into()),
            ..Default::default()
        })
        .keyed_request()
        .expect("keyed")
        .expect("view")
        .fields
    };
    let plain = view(None);
    assert!(plain.get("repository_root").is_none());
    let named = view(Some("/src/widget"));
    assert_eq!(named["repository_root"], "/src/widget");
    assert_ne!(plain, named);
    assert_ne!(named, view(Some("/src/gadget")));
}

/// A plain update keeps the digest it had before `subtree` existed, and one
/// key cannot name both an update of one agent and of its subtree.
#[test]
fn an_update_fingerprints_its_subtree_reach_only_when_set() {
    let agent = AgentId::new();
    let view = |subtree: bool| {
        Command::UpdateAgent {
            id: agent,
            patch: AgentPatch {
                cost_budget: Some(3.0),
                ..Default::default()
            },
            subtree,
            idempotency_key: Some("k-1".into()),
        }
        .keyed_request()
        .expect("keyed")
        .expect("view")
        .fields
    };
    let plain = view(false);
    assert_eq!(
        plain,
        serde_json::json!({"cmd": "update_agent", "id": agent, "patch": {"cost_budget": 3.0}})
    );
    let subtree = view(true);
    assert_eq!(subtree["subtree"], true);
    assert_ne!(plain, subtree);
}
