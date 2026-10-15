use super::*;

#[test]
fn model_info_aliases_are_optional_and_roundtrip() {
    let legacy = serde_json::json!({
        "model": {"provider": "claude-code", "model": "claude-opus-5-5"},
        "name": "Claude Opus 5.5",
        "provider_name": "Claude Code",
        "capabilities": aether_core::model::CapabilityProfile::default(),
        "configured": true,
        "is_default": false
    });
    let mut info: ModelInfo = serde_json::from_value(legacy.clone()).unwrap();
    assert!(info.aliases.is_empty());
    assert_eq!(serde_json::to_value(&info).unwrap(), legacy);
    assert_eq!(decode::<ModelInfo>(&encode(&info).unwrap()).unwrap(), info);

    info.aliases = vec!["opus".into(), "opus-5-5".into()];
    let wire = serde_json::to_value(&info).unwrap();
    assert_eq!(wire["aliases"], serde_json::json!(["opus", "opus-5-5"]));
    assert_eq!(decode::<ModelInfo>(&encode(&info).unwrap()).unwrap(), info);
}

#[test]
fn operator_memory_and_rename_wire_contracts() {
    let agent = "agt_01ARZ3NDEKTSV4RRFFQ69G5FAV";
    let renamed: ClientFrame = decode(&format!(
        r#"{{"req":1,"cmd":"update_agent","id":"{agent}","patch":{{"name":"名前 café"}}}}"#
    ))
    .unwrap();
    assert!(
        matches!(&renamed.command, Command::UpdateAgent { patch, .. } if patch.name.as_deref() == Some("名前 café"))
    );
    assert_eq!(
        decode::<ClientFrame>(&encode(&renamed).unwrap()).unwrap(),
        renamed
    );
    let old: AgentPatch = serde_json::from_str("{}").unwrap();
    assert!(old.name.is_none());
    let base = serde_json::json!({"req":2,"cmd":"create_memory","spec":{"text":"Keep this durable fact across sessions.","kind":"fact","scope":{"scope":"global"}}});
    let frame: ClientFrame = serde_json::from_value(base.clone()).unwrap();
    assert_eq!(
        decode::<ClientFrame>(&encode(&frame).unwrap()).unwrap(),
        frame
    );
    for (field, value) in [
        ("provenance", serde_json::json!({"agent":agent})),
        ("kind", serde_json::json!("unknown")),
        (
            "scope",
            serde_json::json!({"scope":"repository", "id":"fabricated"}),
        ),
    ] {
        let mut invalid = base.clone();
        invalid["spec"][field] = value;
        assert!(
            serde_json::from_value::<ClientFrame>(invalid).is_err(),
            "{field}"
        );
    }
    let mut missing_scope = base;
    missing_scope["spec"]
        .as_object_mut()
        .unwrap()
        .remove("scope");
    assert!(serde_json::from_value::<ClientFrame>(missing_scope).is_err());
}

#[test]
fn memory_provenance_filter_is_optional_and_roundtrips() {
    let legacy: MemoryQuerySpec = serde_json::from_str("{}").unwrap();
    assert_eq!(legacy.provenance_agent, None);
    assert!(
        serde_json::to_value(&legacy)
            .unwrap()
            .get("provenance_agent")
            .is_none()
    );
    let agent = AgentId::new();
    let query: MemoryQuerySpec = serde_json::from_value(serde_json::json!({
        "provenance_agent": agent, "limit": 200
    }))
    .unwrap();
    let roundtrip: MemoryQuerySpec =
        serde_json::from_value(serde_json::to_value(&query).unwrap()).unwrap();
    let stored: aether_store::MemoryQuery = roundtrip.into();
    assert_eq!(stored.provenance_agent, Some(agent));
    assert_eq!(stored.limit, 200);
    assert!(stored.scope.is_none());
}

#[test]
fn memory_recorder_filter_is_optional_and_roundtrips() {
    use aether_core::memory::MemoryRecorder;
    let legacy: MemoryQuerySpec = serde_json::from_str("{}").unwrap();
    assert_eq!(legacy.recorded_by, None);
    assert!(
        serde_json::to_value(&legacy)
            .unwrap()
            .get("recorded_by")
            .is_none()
    );
    let wire =
        serde_json::json!({"recorded_by": "operator", "include_forgotten": true, "limit": 50});
    let query: MemoryQuerySpec = serde_json::from_value(wire.clone()).unwrap();
    assert_eq!(query.recorded_by, Some(MemoryRecorder::Operator));
    let echoed = serde_json::to_value(&query).unwrap();
    assert_eq!(echoed["recorded_by"], "operator");
    let stored: aether_store::MemoryQuery = query.into();
    assert_eq!(stored.recorded_by, Some(MemoryRecorder::Operator));
    assert!(stored.include_forgotten);
    // A recorder this daemon does not know is refused, not read as no filter.
    let unknown = serde_json::json!({"recorded_by": "imported_operator"});
    assert!(serde_json::from_value::<MemoryQuerySpec>(unknown).is_err());
}

#[test]
fn create_agent_repository_root_is_optional_and_roundtrips() {
    let legacy: Command = serde_json::from_value(serde_json::json!(
        {"cmd": "create_agent", "role": "worker"}
    ))
    .unwrap();
    let Command::CreateAgent(legacy) = legacy else {
        panic!("decoded as create_agent");
    };
    assert_eq!(legacy.repository_root, None);
    assert!(
        serde_json::to_value(&legacy)
            .unwrap()
            .get("repository_root")
            .is_none()
    );
    let wire = serde_json::json!(
        {"cmd": "create_agent", "role": "worker", "repository_root": "/src/widget", "start": true}
    );
    let command: Command = serde_json::from_value(wire.clone()).unwrap();
    let Command::CreateAgent(create) = &command else {
        panic!("decoded as create_agent");
    };
    assert_eq!(
        create.repository_root.as_deref(),
        Some(std::path::Path::new("/src/widget"))
    );
    assert!(create.scope.is_none(), "the root is not part of the scope");
    assert_eq!(serde_json::to_value(&command).unwrap(), wire);
}

/// The commands a client can key, each with a key set.
#[test]
fn client_frame_flattens_command() {
    let frame = ClientFrame {
        id: 7,
        command: Command::Status,
    };
    let json = serde_json::to_value(&frame).expect("ser");
    assert_eq!(json["req"], 7);
    assert_eq!(json["cmd"], "status");
    let back: ClientFrame = serde_json::from_value(json).expect("de");
    assert_eq!(back, frame);
}

#[test]
fn every_command_survives_the_frame_roundtrip() {
    let agent = AgentId::new();
    let commands = vec![
        Command::Hello(Hello::new("t", "0")),
        Command::GetAgent { id: agent },
        Command::GetTask { id: TaskId::new() },
        Command::UpdateAgent {
            id: agent,
            patch: AgentPatch::default(),
            idempotency_key: None,
        },
        Command::ResolveApproval {
            id: ApprovalId::new(),
            decision: ApprovalDecision::Approved,
            note: None,
            remember: None,
            idempotency_key: None,
        },
        Command::ResolveApproval {
            id: ApprovalId::new(),
            decision: ApprovalDecision::Approved,
            note: Some("ok".into()),
            remember: Some(RememberSpec {
                scope: Some(RuleScope::Subtree { root: agent }),
                tool_wide: true,
                note: None,
            }),
            idempotency_key: Some("answer-1".into()),
        },
        // The narrowest form: no scope, no widening.
        Command::ResolveApproval {
            id: ApprovalId::new(),
            decision: ApprovalDecision::Denied,
            note: None,
            remember: Some(RememberSpec::default()),
            idempotency_key: None,
        },
        Command::ListApprovalRules {
            include_revoked: true,
        },
        Command::ListArtifacts {
            query: ArtifactQuery::default(),
        },
        Command::McpServers {
            refresh: true,
            server: Some("files".into()),
        },
        Command::Providers,
        Command::AddApprovalRule(NewApprovalRule {
            decision: ApprovalDecision::Denied,
            matcher: RuleMatch::tool("bash").with_argument("rm -rf *"),
            scope: RuleScope::Everywhere,
            note: Some("never".into()),
            idempotency_key: None,
        }),
        Command::RevokeApprovalRule {
            id: ApprovalRuleId::new(),
            reason: None,
        },
        Command::Unsubscribe { id: 3 },
        Command::Compact { agent },
        Command::Restart {
            agent,
            idempotency_key: None,
        },
        Command::Wake { agent },
        Command::EngineTool {
            agent,
            token: "t".into(),
            name: "complete_task".into(),
            input: serde_json::json!({"summary": "done"}),
        },
        Command::GetSnapshot {
            id: SnapshotId::new(),
        },
        Command::GetInvocation {
            id: InvocationId::new(),
        },
        Command::GetCheckpoint {
            id: CheckpointId::new(),
        },
        Command::GetArtifact {
            id: ArtifactId::new(),
        },
        Command::ReadArtifact {
            id: ArtifactId::new(),
            max_bytes: 10,
        },
        Command::Subscribe {
            after: Some(1),
            query: EventQuery::default(),
            deltas: true,
        },
        Command::SendMessage {
            to: agent,
            content: "hi".into(),
            subtree: true,
            attachments: Vec::new(),
            idempotency_key: None,
        },
        Command::SendMessage {
            to: agent,
            content: "hi".into(),
            subtree: false,
            attachments: Vec::new(),
            idempotency_key: Some("outbox-7".into()),
        },
        Command::SendMessage {
            to: agent,
            content: "what is this?".into(),
            subtree: false,
            attachments: vec![
                ImageAttachment {
                    media_type: "image/png".into(),
                    data: "iVBORw0KGgo=".into(),
                    name: Some("shot.png".into()),
                },
                ImageAttachment {
                    media_type: "image/jpeg".into(),
                    data: "/9j/4AAQ".into(),
                    name: None,
                },
            ],
            idempotency_key: None,
        },
        Command::Pause {
            agent,
            subtree: true,
            idempotency_key: None,
        },
        Command::Resume {
            agent,
            subtree: false,
            idempotency_key: None,
        },
        Command::UpdateAgent {
            id: agent,
            patch: AgentPatch {
                deadline: Some(DeadlineUpdate::Clear),
                ..Default::default()
            },
            idempotency_key: None,
        },
        Command::UpdateAgent {
            id: agent,
            patch: AgentPatch {
                deadline: Some(DeadlineUpdate::Set(aether_core::time::now())),
                ..Default::default()
            },
            idempotency_key: None,
        },
        // These four shapes are non-trivial encodings that were once
        // missing from this roundtrip.
        Command::GetTree {
            run: Some(RunId::new()),
            include_archived: true,
        },
        Command::Archive {
            target: ArchiveTarget::Agent(agent),
            archived: true,
            idempotency_key: None,
        },
        Command::Archive {
            target: ArchiveTarget::Run(RunId::new()),
            archived: false,
            idempotency_key: None,
        },
        Command::Archive {
            target: ArchiveTarget::AllFinished,
            archived: true,
            idempotency_key: None,
        },
        Command::SetApprovals {
            agent,
            policy: ApprovalPolicy::none(),
            subtree: true,
            idempotency_key: None,
        },
        Command::CreateAgent(CreateAgent {
            role: "worker".into(),
            approval: Some(ApprovalPolicy::default()),
            ..Default::default()
        }),
        Command::CreateAgent(CreateAgent {
            role: "worker".into(),
            idempotency_key: Some("outbox-8".into()),
            ..Default::default()
        }),
        Command::Trigger {
            agent,
            source: "ci".into(),
            payload: serde_json::json!({"run": 1}),
            idempotency_key: Some("outbox-9".into()),
        },
    ];
    for command in commands {
        let frame = ClientFrame {
            id: 42,
            command: command.clone(),
        };
        let line = encode(&frame).expect("encode");
        let back: ClientFrame = decode(&line).expect("decode");
        assert_eq!(back.id, 42, "{line}");
        assert_eq!(back.command, command, "{line}");
    }
}

#[test]
fn server_frames_roundtrip() {
    let reply = ServerFrame::Reply {
        id: 1,
        result: Err(ErrorReply::new(
            ErrorCode::NotFound,
            "agent agt_x not found",
        )),
    };
    let line = encode(&reply).expect("encode");
    assert!(line.ends_with('\n'));
    let back: ServerFrame = decode(&line).expect("decode");
    assert_eq!(back, reply);
    let delta = ServerFrame::Delta {
        sub: 3,
        agent: AgentId::new(),
        invocation: InvocationId::new(),
        delta: Delta::Text { text: "hi".into() },
    };
    let back: ServerFrame = decode(&encode(&delta).expect("e")).expect("d");
    assert_eq!(back, delta);
}

/// The reply shapes the client decodes for pause, resume, fan-out and
/// `set`. A daemon answering a different variant is the bug this pins.
#[test]
fn the_replies_pause_resume_and_fan_out_produce_survive_the_roundtrip() {
    let agent = AgentId::new();
    for reply in [
        Reply::Paused {
            agents: Vec::new(),
            mid_turn: vec![agent],
        },
        Reply::Agents(Vec::new()),
        Reply::MessagesSent {
            deliveries: vec![MessageDelivery {
                agent,
                message: MessageId::new(),
            }],
        },
    ] {
        let frame = ServerFrame::Reply {
            id: 1,
            result: Ok(reply.clone()),
        };
        let line = encode(&frame).expect("encode");
        let back: ServerFrame = decode(&line).expect("decode");
        assert_eq!(back, frame, "{line}");
    }
}

/// A client built before the `Paused` reply changed shape must be refused
/// at hello, not left failing to decode the reply with an opaque error.
/// The version guard is a compile-time one: the shapes below and the
/// constant move together.
const _: () = assert!(PROTOCOL_VERSION >= 2);

#[test]
fn the_reply_shapes_this_version_changed_no_longer_decode() {
    for old in [
        r#"{"reply":"paused","data":{"after_current_turn":true}}"#,
        r#"{"reply":"paused","data":{}}"#,
    ] {
        assert!(
            serde_json::from_str::<Reply>(old).is_err(),
            "the old `Paused` shape must not decode: {old}"
        );
    }
    // A forward guard, not a v1 shape: `deadline` arrived with this
    // version, and a bare timestamp must never read as `Set`.
    let patch = r#"{"deadline":"2030-01-02T03:04:05Z"}"#;
    assert!(
        serde_json::from_str::<AgentPatch>(patch).is_err(),
        "a bare timestamp must not decode as a deadline update"
    );
}

/// A provider row with every optional health field populated, so the
/// additive fields are exercised on the wire.
fn provider_info() -> ProviderInfo {
    use aether_core::model::ModelErrorClass;
    use aether_core::provider::{ErrorTally, FallbackRecord, ProviderBackoff, QuotaWindow};
    ProviderInfo {
        provider: "anthropic".into(),
        name: "Anthropic".into(),
        protocol: "anthropic-messages".into(),
        family: Some("claude".into()),
        base_url: "https://api.anthropic.com".into(),
        engine: false,
        models: 4,
        open_model_list: false,
        enabled: true,
        configured: true,
        auth_source: Some("ANTHROPIC_API_KEY".into()),
        health: ProviderHealth {
            running: 1,
            limit: Some(2),
            backoff: Some(ProviderBackoff {
                reason: "rate limited".into(),
                until: aether_core::time::now(),
            }),
            quota: Some(QuotaWindow {
                used_percent: 40,
                window_minutes: Some(300),
            }),
            recent_errors: vec![ErrorTally {
                class: ModelErrorClass::RateLimit,
                count: 2,
                last_at: aether_core::time::now(),
            }],
            last_fallback: Some(FallbackRecord {
                to: ModelRef::new("claude-code", "haiku"),
                reason: "rate limited".into(),
                class: Some(ModelErrorClass::RateLimit),
                at: aether_core::time::now(),
            }),
            ..ProviderHealth::idle("anthropic")
        },
    }
}

#[test]
fn sequence_replies_encode() {
    for reply in [
        Reply::Agents(vec![]),
        Reply::Models(vec![]),
        Reply::Providers(vec![provider_info()]),
        Reply::Auth(vec![]),
        Reply::Checkpoint(None),
        Reply::Ok,
        Reply::Subscribed { id: 1, last_seq: 2 },
    ] {
        let frame = ServerFrame::Reply {
            id: 1,
            result: Ok(reply.clone()),
        };
        let line = encode(&frame).expect("encode");
        let back: ServerFrame = decode(&line).expect("decode");
        assert_eq!(back, frame);
    }
}

#[test]
fn create_agent_defaults_to_start() {
    let json = r#"{"role":"root"}"#;
    let req: CreateAgent = serde_json::from_str(json).expect("de");
    assert!(req.start);
    assert_eq!(req.role, "root");
    assert!(
        CreateAgent::default().start,
        "Rust default must match the wire default"
    );
}

#[path = "tests/attachments.rs"]
mod attachments;
#[path = "tests/idempotency.rs"]
mod idempotency;

#[path = "tests/resolution.rs"]
mod resolution;

#[test]
fn event_query_before_is_optional_and_old_wire_shape_is_unchanged() {
    let old = serde_json::json!({"families": [], "types": []});
    let query: EventQuery = serde_json::from_value(old.clone()).unwrap();
    assert_eq!(query.before, None);
    assert_eq!(serde_json::to_value(query).unwrap(), old);
    let query: EventQuery = serde_json::from_value(serde_json::json!({"before": 42})).unwrap();
    assert_eq!(query.before, Some(42));
}

#[test]
fn event_query_child_types_reach_the_store_and_older_daemons_ignore_them() {
    let agent = aether_core::id::AgentId::new();
    let wire = serde_json::json!({"agent": agent, "child_types": ["task.completed"]});
    let query: EventQuery = serde_json::from_value(wire.clone()).unwrap();
    assert_eq!(
        serde_json::to_value(&query).unwrap()["child_types"],
        wire["child_types"]
    );
    let filter: aether_store::EventFilter = query.into();
    assert_eq!(filter.agent, Some(agent));
    assert_eq!(filter.child_types, ["task.completed"]);
    assert!(!filter.exclude_children, "children stay, narrowed");
    // 1.0.5's query had these attributes without the field: an unknown key
    // is dropped, so such a daemon answers with every child event.
    let newer = serde_json::json!({"types": [], "families": [], "from_a_newer_client": [1]});
    let query: EventQuery = serde_json::from_value(newer).unwrap();
    assert_eq!(query, EventQuery::default());
}

#[test]
fn memory_proposal_commands_round_trip_and_legacy_requests_stay_immediate() {
    use serde_json::json;
    let id = "mem_01ARZ3NDEKTSV4RRFFQ69G5FAV";
    let commands = [
        json!({"cmd":"create_memory", "spec": {"text":"A durable build lesson", "kind":"lesson", "scope":{"scope":"global"}, "proposed":true, "supersedes":id}}),
        json!({"cmd":"accept_memory", "id":id, "reason":"Repeated successful builds"}),
        json!({"cmd":"reject_memory", "id":id, "reason":"Not enough evidence"}),
        json!({"cmd":"memories", "query":{"proposed":true,"include_forgotten":true}}),
    ];
    for wire in commands {
        let command: Command = serde_json::from_value(wire).unwrap();
        let encoded = serde_json::to_string(&command).unwrap();
        assert_eq!(serde_json::from_str::<Command>(&encoded).unwrap(), command);
        assert_eq!(command.access(), Access::HostOnly);
    }
    let legacy: Command = serde_json::from_value(json!({"cmd":"create_memory", "spec":{
        "text":"A durable build fact", "kind":"fact", "scope":{"scope":"global"}}}))
    .unwrap();
    assert!(matches!(legacy, Command::CreateMemory { ref spec, .. } if !spec.proposed));
    assert_eq!(legacy.access(), Access::Operate);
    let query: MemoryQuerySpec = serde_json::from_str("{}").unwrap();
    assert!(!query.proposed);
    assert!(!aether_store::MemoryQuery::from(query).proposed);
    assert_eq!(features::MEMORY_PROPOSALS, "memory_proposals");
}

#[test]
fn legacy_memory_creation_keeps_its_wire_shape_and_idempotency_fingerprint() {
    use serde_json::json;
    let wire = json!({"cmd":"create_memory", "idempotency_key":"lesson-1", "spec":{
        "text":"A durable build convention", "kind":"fact", "scope":{"scope":"global"}}});
    let mut command: Command = serde_json::from_value(wire).unwrap();
    let encoded = serde_json::to_value(&command).unwrap();
    assert!(encoded["spec"].get("proposed").is_none());
    let legacy_fields = command.keyed_request().unwrap().unwrap().fields;
    assert_eq!(
        legacy_fields,
        json!({"cmd":"create_memory","spec":{
        "text":"A durable build convention", "kind":"fact", "scope":{"scope":"global"},"tags":[]}})
    );
    if let Command::CreateMemory { spec, .. } = &mut command {
        spec.proposed = true;
    }
    let proposed = command.keyed_request().unwrap().unwrap();
    assert_eq!(proposed.fields["spec"]["proposed"], true);
    assert_ne!(proposed.fields, legacy_fields);
    let query: MemoryQuerySpec = serde_json::from_str("{}").unwrap();
    assert!(
        serde_json::to_value(query)
            .unwrap()
            .get("proposed")
            .is_none()
    );
}

#[test]
fn feedback_keys_cover_every_evidence_field_and_the_command_name() {
    let value = serde_json::json!({"cmd":"record_memory_feedback", "memory":MemoryId::new(),
        "task":TaskId::new(), "invocation":InvocationId::new(), "verdict":"helped",
        "evidence":"review", "reason":"Verified release", "idempotency_key":"feedback-review"});
    let command: Command = serde_json::from_value(value.clone()).unwrap();
    let keyed = command.keyed_request().unwrap().unwrap();
    assert_eq!(keyed.key, "feedback-review");
    assert_eq!(keyed.fields["cmd"], "record_memory_feedback");
    for (field, changed) in [
        ("memory", serde_json::json!(MemoryId::new())),
        ("task", serde_json::json!(TaskId::new())),
        ("invocation", serde_json::Value::Null),
        ("verdict", serde_json::json!("hurt")),
        ("evidence", serde_json::json!("outcome")),
        ("reason", serde_json::json!("Failed release")),
    ] {
        let mut different = value.clone();
        different[field] = changed;
        let different: Command = serde_json::from_value(different).unwrap();
        assert_ne!(
            different.keyed_request().unwrap().unwrap().fields,
            keyed.fields,
            "{field}"
        );
    }
    assert_eq!(features::MEMORY_FEEDBACK, "memory_feedback");
}

#[test]
fn a_feedback_list_without_a_ranking_decodes_as_unranked() {
    // An older daemon's reply: its feedback never moved recall.
    let older = serde_json::json!({"reply":"memory_feedback_list",
        "data":{"records":[], "truncated":false}});
    let Reply::MemoryFeedbackList(list) = serde_json::from_value(older).unwrap() else {
        panic!("expected a feedback list");
    };
    let ranking = aether_core::memory_feedback::FeedbackRanking {
        helped: 0,
        hurt: 0,
        factor: 1.0,
    };
    assert_eq!(list.ranking, ranking);
    let current = serde_json::to_value(Reply::MemoryFeedbackList(list)).unwrap();
    assert_eq!(
        current["data"]["ranking"],
        serde_json::json!({"helped":0, "hurt":0, "factor":1.0})
    );
}
