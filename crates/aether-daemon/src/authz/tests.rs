use super::*;
use aether_core::id::DeviceId;

fn device(role: DeviceRole) -> AuthContext {
    AuthContext::Device(DeviceSession {
        identity: DeviceIdentity {
            id: DeviceId::new(),
            name: "Robin's iPhone".into(),
            role,
        },
        closed: CancellationToken::new(),
    })
}

fn code(auth: &AuthContext, command: &Command) -> Option<ErrorCode> {
    auth.authorize(command).err().map(|refusal| refusal.code)
}

/// One command of each class against each context: the table in
/// docs/remote.md, row by row.
#[test]
fn every_class_is_open_to_exactly_the_contexts_the_matrix_names() {
    let contexts = [
        AuthContext::Local,
        AuthContext::Token,
        device(DeviceRole::Operator),
        device(DeviceRole::Observer),
    ];
    let interrupt = Command::Interrupt {
        agent: aether_core::id::AgentId::new(),
        idempotency_key: None,
    };
    let forbidden = Some(ErrorCode::Forbidden);
    let not_a_device = Some(ErrorCode::InvalidArgument);
    let resolving = |command: &Command| Command::ResolveSubmission {
        command: Box::new(command.clone()),
        horizon: None,
    };
    let rows = [
        (Command::ListApprovals, [None, None, None, None]),
        // A resolution is open to exactly who may send what it names.
        (resolving(&interrupt), [None, None, None, forbidden]),
        (
            resolving(&Command::Shutdown),
            [None, None, forbidden, forbidden],
        ),
        (
            resolving(&Command::ListApprovals),
            [None, None, None, forbidden],
        ),
        (interrupt, [None, None, None, forbidden]),
        (Command::Shutdown, [None, None, forbidden, forbidden]),
        (Command::GetRemote, [None, forbidden, forbidden, forbidden]),
        (Command::TestPush, [not_a_device, not_a_device, None, None]),
    ];
    for (command, expected) in rows {
        for (auth, expected) in contexts.iter().zip(expected) {
            assert_eq!(
                code(auth, &command),
                expected,
                "{} from {auth:?}",
                command.name()
            );
        }
    }
}

#[test]
fn the_shared_token_is_never_a_device_and_never_an_administrator() {
    let token = AuthContext::Token;
    assert!(token.device().is_none());
    assert!(token.closed().is_none());
    for command in [
        Command::StartPairing {
            role: DeviceRole::Operator,
        },
        Command::RevokeDevice {
            id: DeviceId::new(),
            reason: None,
        },
    ] {
        assert_eq!(code(&token, &command), Some(ErrorCode::Forbidden));
    }
}

#[test]
fn a_device_acts_as_the_operator_and_is_named_in_the_record() {
    let auth = device(DeviceRole::Operator);
    let actor = auth.actor(None, "conn".into());
    let label = actor.label();
    assert!(matches!(actor, Actor::User { .. }), "{label}");
    assert!(label.contains("device:Robin's iPhone"), "{label}");
    let local = AuthContext::Local.actor(None, "conn".into()).label();
    assert!(local.contains("local_socket"), "{local}");
}

#[test]
fn every_command_has_an_explicit_role_decision() {
    let cases: serde_json::Value =
        serde_json::from_str(include_str!("command_cases.json")).unwrap();
    let cases = cases.as_array().unwrap();
    let schema = aether_protocol::schema::schema();
    let variants = schema["$defs"]["ClientFrame"]["oneOf"].as_array().unwrap();
    let tags: std::collections::BTreeSet<_> = variants
        .iter()
        .map(|v| v["properties"]["cmd"]["const"].as_str().unwrap())
        .collect();
    let tested: std::collections::BTreeSet<_> = cases
        .iter()
        .map(|v| v["command"]["cmd"].as_str().unwrap())
        .collect();
    assert_eq!(
        tags, tested,
        "new commands need a role decision and fixture"
    );
    assert_eq!(tested.len(), cases.len(), "each variant appears once");
    let contexts = [
        AuthContext::Local,
        AuthContext::Token,
        device(DeviceRole::Operator),
        device(DeviceRole::Observer),
    ];
    for case in cases {
        let command: Command = serde_json::from_value(case["command"].clone())
            .unwrap_or_else(|error| panic!("{}: {error}", case["command"]));
        let (access, allowed) = match case["access"].as_str().unwrap() {
            "observe" => (Access::Observe, [true, true, true, true]),
            "operate" => (Access::Operate, [true, true, true, false]),
            "host_only" => (Access::HostOnly, [true, true, false, false]),
            "remote_admin" => (Access::RemoteAdmin, [true, false, false, false]),
            "device_self" => (Access::DeviceSelf, [false, false, true, true]),
            other => panic!("unknown access class {other}"),
        };
        assert_eq!(command.access(), access, "{}", command.name());
        for (context, allowed) in contexts.iter().zip(allowed) {
            assert_eq!(
                context.authorize(&command).is_ok(),
                allowed,
                "{} from {context:?}",
                command.name()
            );
        }
    }
}

#[test]
fn observers_never_get_an_operator_actor_and_revocation_precedes_reads() {
    let observer = device(DeviceRole::Observer);
    assert!(matches!(
        observer.actor(None, "connection".into()),
        Actor::Client { .. }
    ));
    observer.closed().unwrap().cancel();
    assert_eq!(
        code(&observer, &Command::GetAttention),
        Some(ErrorCode::Forbidden)
    );
}

#[test]
fn remote_audit_events_do_not_disclose_host_only_metadata() {
    let auth = device(DeviceRole::Observer);
    let event = aether_core::event::EventDraft::new(
        Actor::user(),
        aether_core::event::EventKind::Custom {
            name: "remote.device_enrolled".into(),
            payload: serde_json::json!({"fingerprint": "secret", "device": "revoked"}),
        },
    );
    let mut stored = aether_core::event::Event::from_draft(
        event,
        aether_core::id::EventId::new(),
        7,
        aether_core::time::now(),
    );
    auth.redact_event(&mut stored);
    assert_eq!(stored.seq, 7);
    assert!(!serde_json::to_string(&stored).unwrap().contains("secret"));
}

#[test]
fn submission_namespaces_follow_device_ids_not_connection_or_name() {
    let operator = device(DeviceRole::Operator);
    let mut renamed = operator.clone();
    if let AuthContext::Device(session) = &mut renamed {
        session.identity.name = "renamed phone".into();
    }
    let command = Command::SendMessage {
        to: aether_core::id::AgentId::new(),
        content: "continue".into(),
        subtree: false,
        attachments: Vec::new(),
        idempotency_key: Some("same-outbox-key".into()),
    };
    let receipt = operator.submission(&command).unwrap().unwrap();
    assert_eq!(renamed.submission(&command).unwrap().unwrap(), receipt);
    assert_ne!(
        device(DeviceRole::Operator)
            .submission(&command)
            .unwrap()
            .unwrap()
            .key,
        receipt.key
    );
    assert_ne!(
        AuthContext::Local
            .submission(&command)
            .unwrap()
            .unwrap()
            .key,
        receipt.key
    );
    let mut changed = command.clone();
    if let Command::SendMessage { content, .. } = &mut changed {
        *content = "different".into();
    }
    let changed = operator.submission(&changed).unwrap().unwrap();
    assert_eq!(changed.key, receipt.key);
    assert_ne!(changed.fingerprint, receipt.fingerprint);
}

#[test]
fn memory_proposal_authz_requires_host_authority_for_review_and_listing() {
    let id = aether_core::id::MemoryId::new();
    let spec = serde_json::from_value(
        serde_json::json!({"text":"Review this lesson", "kind":"lesson",
        "scope":{"scope":"global"}, "proposed":true}),
    )
    .unwrap();
    let query = serde_json::from_value(serde_json::json!({"proposed":true})).unwrap();
    let commands = [
        Command::CreateMemory {
            spec,
            idempotency_key: None,
        },
        Command::AcceptMemory { id, reason: None },
        Command::RejectMemory {
            id,
            reason: "Insufficient evidence".into(),
        },
        Command::Memories { query },
    ];
    for command in commands {
        assert_eq!(command.access(), Access::HostOnly);
        assert!(AuthContext::Local.authorize(&command).is_ok());
        assert!(AuthContext::Token.authorize(&command).is_ok());
        for role in [DeviceRole::Operator, DeviceRole::Observer] {
            assert_eq!(
                code(&device(role), &command),
                Some(ErrorCode::Forbidden),
                "{}",
                command.name()
            );
        }
    }
}

#[test]
fn memories_in_context_is_for_the_host_and_never_a_paired_device() {
    let command = Command::MemoriesInContext {
        task: Some(aether_core::id::TaskId::new()),
        invocation: None,
    };
    assert_eq!(command.access(), Access::HostOnly);
    assert!(AuthContext::Local.authorize(&command).is_ok());
    assert!(AuthContext::Token.authorize(&command).is_ok());
    for role in [DeviceRole::Operator, DeviceRole::Observer] {
        assert_eq!(
            code(&device(role), &command),
            Some(ErrorCode::Forbidden),
            "{role:?}"
        );
    }
}

#[test]
fn memory_feedback_requires_host_authority_even_for_reading() {
    let commands = [
        serde_json::json!({"cmd":"record_memory_feedback", "memory":aether_core::id::MemoryId::new(),
            "task":aether_core::id::TaskId::new(), "verdict":"neutral", "evidence":"operator", "reason":"No effect"}),
        serde_json::json!({"cmd":"retract_memory_feedback", "feedback":aether_core::id::MemoryFeedbackId::new(), "reason":"Withdraw"}),
        serde_json::json!({"cmd":"memory_feedback", "memory":aether_core::id::MemoryId::new()}),
    ];
    for value in commands {
        let command: Command = serde_json::from_value(value).unwrap();
        assert_eq!(command.access(), Access::HostOnly);
        assert!(AuthContext::Local.authorize(&command).is_ok());
        assert!(AuthContext::Token.authorize(&command).is_ok());
        for role in [DeviceRole::Operator, DeviceRole::Observer] {
            assert_eq!(code(&device(role), &command), Some(ErrorCode::Forbidden));
        }
    }
}

#[test]
fn storage_gc_requires_host_access_in_both_modes() {
    for dry_run in [true, false] {
        let command = Command::StorageGc { dry_run };
        assert_eq!(command.access(), Access::HostOnly);
        for context in [AuthContext::Local, AuthContext::Token] {
            assert_eq!(code(&context, &command), None);
        }
        for role in [DeviceRole::Operator, DeviceRole::Observer] {
            assert_eq!(code(&device(role), &command), Some(ErrorCode::Forbidden));
        }
    }
}
