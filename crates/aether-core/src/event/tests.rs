use crate::{
    id::{InvocationId, SnapshotId},
    model::ModelRef,
};

use super::*;

#[test]
fn operator_origin_preserves_legacy_users_and_import_authority() {
    let legacy: super::Actor = serde_json::from_str(r#"{"actor":"user"}"#).unwrap();
    assert_eq!(legacy, super::Actor::user());
    assert_eq!(
        serde_json::to_string(&legacy).unwrap(),
        r#"{"actor":"user"}"#
    );
    let actor = super::Actor::connected_user(super::OperatorOrigin::Tcp {
        peer: "[::1]:54321".parse().unwrap(),
        connection: "server-connection".into(),
    });
    let roundtrip: super::Actor =
        serde_json::from_str(&serde_json::to_string(&actor).unwrap()).unwrap();
    assert_eq!(roundtrip, actor);
    assert_eq!(actor.label(), "user:tcp:[::1]:54321#server-connection");
    let imported = super::Actor::Imported {
        was: Box::new(actor.clone()),
    };
    assert!(!matches!(imported, super::Actor::User { .. }));
    assert_eq!(imported.original(), &actor);
    assert_eq!(
        imported.label(),
        "imported:user:tcp:[::1]:54321#server-connection"
    );
}

#[test]
fn a_device_origin_names_the_device_and_keeps_user_standing() {
    let device = crate::id::DeviceId::new();
    let actor = super::Actor::connected_user(super::OperatorOrigin::Device {
        device,
        name: "phone".into(),
        connection: "c1".into(),
    });
    assert!(matches!(actor, super::Actor::User { .. }));
    assert_eq!(actor.label(), format!("user:device:phone({device})#c1"));
    let json = serde_json::to_value(&actor).unwrap();
    assert_eq!(json["origin"]["transport"], "device");
    let back: super::Actor = serde_json::from_value(json).unwrap();
    assert_eq!(back, actor);
}

#[test]
fn type_names_are_dotted() {
    assert_eq!(EventKind::AgentReady.type_name(), "agent.ready");
    assert_eq!(EventKind::AgentReady.family(), "agent");
    assert_eq!(EventKind::LeaseReleased.type_name(), "lease.released");
    assert_eq!(EventKind::TaskStarted.family(), "task");
}

#[test]
fn invocation_failure_class_is_optional_for_older_logs() {
    let event = EventKind::InvocationFailed {
        invocation: InvocationId::new(),
        snapshot: SnapshotId::new(),
        error: "Auth: insufficient_quota".into(),
        class: Some(crate::model::ModelErrorClass::Auth),
        retryable: false,
        cancelled: false,
        partial: Vec::new(),
    };
    let mut wire = serde_json::to_value(&event).unwrap();
    assert_eq!(wire["class"], "auth");
    assert_eq!(
        serde_json::from_value::<EventKind>(wire.clone()).unwrap(),
        event
    );
    wire.as_object_mut().unwrap().remove("class");
    let old: EventKind = serde_json::from_value(wire).unwrap();
    assert!(matches!(
        old,
        EventKind::InvocationFailed { class: None, .. }
    ));
    assert!(serde_json::to_value(old).unwrap().get("class").is_none());
}

/// Every variant this phase reshaped, in the form a store written before
/// the change holds it. Events are JSON in SQLite and the log is
/// append-only, so an old body that stops decoding is history lost.
#[test]
fn events_written_before_the_shapes_changed_still_decode() {
    let older = [
        // `agent.paused` was a unit variant.
        (r#"{"type":"agent.paused"}"#, "agent.paused"),
        // `agent.resumed` had no `via`.
        (
            r#"{"type":"agent.resumed","reason":"unpaused by operator"}"#,
            "agent.resumed",
        ),
        // `agent.config_updated` had no `scope`.
        (
            r#"{"type":"agent.config_updated","config":{}}"#,
            "agent.config_updated",
        ),
        // `agent.restarted` had neither `inbox_carried` nor
        // `dropped_deadline`.
        (
            r#"{"type":"agent.restarted","from":"agt_01M1Z1AEC7ATEWCBH91PZ2NQKV",
                "to":"agt_01M1Z1AEC7ATEWCBH91PZ2NQKW","transcript_tail":3}"#,
            "agent.restarted",
        ),
    ];
    for (body, type_name) in older {
        let kind: EventKind = serde_json::from_str(body)
            .unwrap_or_else(|e| panic!("{type_name} must still decode: {e}"));
        assert_eq!(kind.type_name(), type_name);
        match &kind {
            EventKind::AgentPaused { via } => assert_eq!(*via, None),
            EventKind::AgentResumed { reason, via } => {
                assert_eq!(reason, "unpaused by operator");
                assert_eq!(*via, None);
            }
            EventKind::AgentConfigUpdated { scope, model, .. } => {
                assert!(scope.is_none());
                assert!(model.is_none());
            }
            EventKind::AgentRestarted {
                transcript_tail,
                transcript_dropped,
                inbox_carried,
                dropped_deadline,
                ..
            } => {
                assert_eq!(*transcript_tail, 3);
                assert_eq!(*transcript_dropped, 0);
                assert_eq!(*inbox_carried, 0);
                assert_eq!(*dropped_deadline, None);
            }
            other => panic!("unexpected {other:?}"),
        }
    }
}

/// The generated clients declare `duration_ms` as `number | undefined` /
/// `Optional[int]`, never a literal `null` - so `None` must serialise
/// with the key absent, not present as `null`.
#[test]
fn tool_failed_duration_ms_is_absent_on_the_wire_for_none_and_present_for_some() {
    let call = crate::id::ToolCallId::new();
    let none_json = serde_json::to_value(EventKind::ToolFailed {
        call,
        error: "boom".into(),
        retryable: false,
        duration_ms: None,
    })
    .unwrap();
    assert!(
        !none_json.as_object().unwrap().contains_key("duration_ms"),
        "{none_json}"
    );

    let some_json = serde_json::to_value(EventKind::ToolFailed {
        call,
        error: "boom".into(),
        retryable: false,
        duration_ms: Some(150),
    })
    .unwrap();
    assert_eq!(some_json["duration_ms"], serde_json::json!(150));
}

#[test]
fn event_roundtrip() {
    let draft = EventDraft::new(Actor::user(), EventKind::AgentReady).agent(AgentId::new());
    let event = Event::from_draft(draft, EventId::new(), 7, crate::time::now());
    let json = serde_json::to_string(&event).expect("ser");
    assert!(json.contains("\"type\":\"agent.ready\""));
    let back: Event = serde_json::from_str(&json).expect("de");
    assert_eq!(back, event);
}

#[test]
fn the_phase_two_event_kinds_roundtrip_under_their_documented_names() {
    let call = crate::id::ToolCallId::new();
    let rule = crate::id::ApprovalRuleId::new();
    let artifact = crate::id::ArtifactId::new();
    let kinds = vec![
        (
            "tool.replayed",
            EventKind::ToolReplayed {
                call,
                key: crate::idempotency::IdempotencyKey("bash:h:aa".into()),
                original: crate::id::ToolCallId::new(),
            },
        ),
        (
            "tool.retried",
            EventKind::ToolRetried {
                call,
                attempt: 1,
                of: 3,
                error: "transient".into(),
            },
        ),
        (
            "approval.rule_added",
            EventKind::ApprovalRuleAdded {
                rule: crate::approval::ApprovalRule {
                    id: rule,
                    decision: crate::tool::ApprovalDecision::Denied,
                    matcher: crate::approval::RuleMatch::tool("bash"),
                    scope: crate::approval::RuleScope::Everywhere,
                    created_at: crate::time::now(),
                    created_by: Actor::user(),
                    note: None,
                    revoked: None,
                },
            },
        ),
        (
            "approval.rule_revoked",
            EventKind::ApprovalRuleRevoked {
                rule,
                revocation: crate::approval::Revocation {
                    at: crate::time::now(),
                    by: Actor::user(),
                    reason: None,
                },
            },
        ),
        (
            "artifact.deduplicated",
            EventKind::ArtifactDeduplicated {
                artifact,
                shared_with: Some(crate::id::ArtifactId::new()),
                digest: "abc".into(),
            },
        ),
        (
            "artifact.evicted",
            EventKind::ArtifactEvicted {
                artifact,
                size_bytes: 12,
                reason: "retention".into(),
            },
        ),
        (
            "daemon.stopped",
            EventKind::DaemonStopped {
                background_processes_stopped: 2,
            },
        ),
        (
            "provider.fallback",
            EventKind::ProviderFallback {
                from: ModelRef::new("openai", "gpt-5"),
                to: ModelRef::new("anthropic", "claude-opus-5"),
                reason: "rate limited".into(),
                class: Some(crate::model::ModelErrorClass::RateLimit),
            },
        ),
    ];
    for (name, kind) in kinds {
        assert_eq!(kind.type_name(), name);
        let draft = EventDraft::new(Actor::Rule { rule }, kind.clone());
        let event = Event::from_draft(draft, EventId::new(), 7, crate::time::now());
        let json = serde_json::to_string(&event).expect("ser");
        assert!(json.contains(&format!("\"type\":\"{name}\"")), "{json}");
        assert_eq!(
            serde_json::from_str::<Event>(&json).expect("de"),
            event,
            "{name} does not survive the log"
        );
    }
}

#[test]
fn a_rule_is_named_as_the_actor_that_decided() {
    let rule = crate::id::ApprovalRuleId::new();
    assert_eq!(Actor::Rule { rule }.label(), format!("rule:{rule}"));
}

#[test]
fn an_event_written_before_this_version_still_reads() {
    // `daemon.stopped` carried no fields, and the new one is
    // `#[serde(default)]`, which is what keeps a pre-branch store
    // readable rather than a parse error on open.
    let old = r#"{"id":"evt_01ARZ3NDEKTSV4RRFFQ69G5FAV","seq":1,
        "at":"2026-01-01T00:00:00Z","actor":{"actor":"system"},
        "kind":{"type":"daemon.stopped"}}"#;
    let event: Event = serde_json::from_str(old).expect("an old event still reads");
    assert_eq!(
        event.kind,
        EventKind::DaemonStopped {
            background_processes_stopped: 0
        }
    );
}

#[test]
fn compaction_counts_preserve_old_logs_and_omit_unknown_values() {
    let legacy = serde_json::json!({
        "type": "compaction.completed",
        "checkpoint": crate::id::CheckpointId::new(),
        "version": 1,
        "from_seq": 2,
        "to_seq": 10,
    });
    let decoded: EventKind = serde_json::from_value(legacy.clone()).unwrap();
    assert!(matches!(
        decoded,
        EventKind::CompactionCompleted {
            before_tokens: None,
            after_tokens: None,
            ..
        }
    ));
    assert_eq!(serde_json::to_value(decoded).unwrap(), legacy);
    let mut counted = legacy;
    counted["before_tokens"] = 123.into();
    counted["after_tokens"] = 45.into();
    let decoded: EventKind = serde_json::from_value(counted.clone()).unwrap();
    assert_eq!(serde_json::to_value(decoded).unwrap(), counted);
}
