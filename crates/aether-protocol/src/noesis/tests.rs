use super::*;
use crate::introduced::Introduced;
use crate::{ErrorCode, ErrorReply, features};
use serde_json::{Value, json};

fn command(wire: Value) -> Command {
    serde_json::from_value(wire.clone()).unwrap_or_else(|error| panic!("{wire}: {error}"))
}

fn artifact() -> String {
    aether_core::id::ArtifactId::new().to_string()
}

/// Every command with the fields §6.2 requires, and the class it needs.
fn commands() -> Vec<(Value, Access)> {
    let bundle = json!({"title": "t", "items": [
        {"ref": {"kind": "pr", "id": "7", "url": "https://x/7"}, "excerpt": "e", "status": "verified"}]});
    vec![
        (json!({"cmd": "noesis_status"}), Access::Observe),
        (
            json!({"cmd": "noesis_configure", "patch": {"concurrency": {"background": 0}}}),
            Access::Operate,
        ),
        (
            json!({"cmd": "noesis_create_session", "goal": "learn rust", "mode": "teach",
                   "context": bundle, "practice_targets": ["p_1"]}),
            Access::Operate,
        ),
        (
            json!({"cmd": "noesis_send", "session": "ns_1", "text": "hi"}),
            Access::Operate,
        ),
        (
            json!({"cmd": "noesis_answer", "session": "ns_1", "question": "q_1",
                   "answer": {"choices": ["a"], "text": "t", "dont_know": false,
                              "confidence": 70, "thinking": "because"}}),
            Access::Operate,
        ),
        (
            json!({"cmd": "noesis_get_session", "session": "ns_1"}),
            Access::Observe,
        ),
        (
            json!({"cmd": "noesis_list_sessions", "after": "ns_0", "limit": 20}),
            Access::Observe,
        ),
        (
            json!({"cmd": "noesis_delete_session", "session": "ns_1", "graph": "delete_nodes"}),
            Access::Operate,
        ),
        (
            json!({"cmd": "noesis_graph", "query": "q", "session": "ns_1", "source": "s_1",
                   "limit": 50}),
            Access::Observe,
        ),
        (
            json!({"cmd": "noesis_delete_proposition", "proposition": "p_1"}),
            Access::Operate,
        ),
        (
            json!({"cmd": "noesis_delete_source", "source": "s_1", "graph": "delete_unsupported"}),
            Access::Operate,
        ),
        (
            json!({"cmd": "noesis_extract", "session": "ns_1"}),
            Access::Operate,
        ),
        (
            json!({"cmd": "noesis_import_source", "name": "notes", "artifact": artifact()}),
            Access::Operate,
        ),
        (
            json!({"cmd": "noesis_jobs", "state": "running"}),
            Access::Observe,
        ),
        (
            json!({"cmd": "noesis_cancel_job", "job": "j_1"}),
            Access::Operate,
        ),
        (
            json!({"cmd": "noesis_learner", "concepts": ["k_1"], "query": "q", "limit": 5}),
            Access::Observe,
        ),
        (
            json!({"cmd": "noesis_learner_evidence", "concept": "k_1", "before": 9, "limit": 5}),
            Access::Observe,
        ),
        (
            json!({"cmd": "noesis_correct_learner", "concept": "k_1", "state": "fragile",
                   "reason": "I forgot it"}),
            Access::Operate,
        ),
    ]
}

#[test]
fn every_command_round_trips_from_its_wire_json() {
    let all = commands();
    assert_eq!(all.len(), 18, "one entry per Noesis command");
    for (wire, _) in all {
        let decoded = command(wire.clone());
        assert_eq!(serde_json::to_value(&decoded).unwrap(), wire);
        assert_eq!(validate(&decoded), Ok(()), "{wire}");
    }
}

#[test]
fn every_command_is_classed_and_introduced_by_the_feature() {
    for (wire, access) in commands() {
        let decoded = command(wire);
        assert_eq!(decoded.access(), access, "{}", decoded.name());
        assert_eq!(
            decoded.introduced(),
            Introduced::Feature(features::NOESIS),
            "{}",
            decoded.name()
        );
        assert!(decoded.is_noesis(), "{}", decoded.name());
    }
}

#[test]
fn only_status_and_configure_work_without_the_service() {
    for (wire, _) in commands() {
        let decoded = command(wire);
        let standalone = matches!(
            decoded.name().as_str(),
            "noesis_status" | "noesis_configure"
        );
        assert_eq!(
            decoded.needs_noesis_service(),
            !standalone,
            "{}",
            decoded.name()
        );
    }
    assert!(!Command::Status.is_noesis());
    assert!(!Command::Status.needs_noesis_service());
}

#[test]
fn a_keyed_noesis_mutation_fingerprints_and_a_read_does_not() {
    for (wire, access) in commands() {
        let unkeyed = command(wire.clone());
        assert!(unkeyed.keyed_request().is_none(), "{}", unkeyed.name());
        let mut keyed_wire = wire.clone();
        keyed_wire["idempotency_key"] = json!("k-1");
        let keyed = serde_json::from_value::<Command>(keyed_wire.clone());
        if access == Access::Observe {
            assert!(keyed.is_err() || keyed.unwrap().keyed_request().is_none());
            continue;
        }
        let keyed = keyed.expect("a mutation takes a key");
        let request = keyed.keyed_request().expect("keyed").expect("fields");
        assert_eq!(request.key, "k-1");
        assert_eq!(request.fields["cmd"], wire["cmd"]);
        assert!(request.fields.get("idempotency_key").is_none());
    }
}

#[test]
fn the_fingerprint_follows_the_arguments() {
    let view = |text: &str| {
        command(
            json!({"cmd": "noesis_send", "session": "ns_1", "text": text,
                       "idempotency_key": "k"}),
        )
        .keyed_request()
        .expect("keyed")
        .expect("fields")
        .fields
    };
    assert_ne!(view("a"), view("b"));
    assert_eq!(view("a"), view("a"));
}

fn patch(wire: Value) -> NoesisSettingsPatch {
    serde_json::from_value(wire).expect("a patch")
}

#[test]
fn host_only_keys_make_the_patch_host_only() {
    for wire in [
        json!({"enabled": true}),
        json!({"enabled": false}),
        json!({"budget": {"interactive_usd_daily": 0.0}}),
        json!({"budget": {"background_usd_daily": 2.0}}),
        json!({"budget": {"background_tokens_daily": 10}}),
        json!({"billing": {"allow_api": true}}),
        json!({"prompts": {"overrides": true}}),
        json!({"engineering_context": true, "billing": {"allow_api": false}}),
    ] {
        assert_eq!(
            configure_access(&patch(wire.clone())),
            Access::HostOnly,
            "{wire}"
        );
    }
}

#[test]
fn the_other_keys_and_an_empty_patch_are_operate() {
    for wire in [
        json!({}),
        json!({"budget": {}}),
        json!({"engineering_context": true}),
        json!({"extraction": {"sessions": "auto", "engineering": "auto"}}),
        json!({"concurrency": {"interactive": 3, "background": 2}}),
        json!({"priority": {"background": "normal"}}),
        json!({"models": {"strong": "m", "cheap": "n"}}),
        json!({"disable": {"grace_seconds": 5}}),
    ] {
        assert_eq!(
            configure_access(&patch(wire.clone())),
            Access::Operate,
            "{wire}"
        );
    }
}

#[test]
fn configure_takes_its_class_from_the_patch() {
    let host = command(json!({"cmd": "noesis_configure", "patch": {"enabled": true}}));
    assert_eq!(host.access(), Access::HostOnly);
    let resolving = Command::ResolveSubmission {
        command: Box::new(host),
        horizon: None,
    };
    assert_eq!(resolving.access(), Access::HostOnly);
}

#[test]
fn a_patch_replaces_only_the_keys_it_names() {
    let base = NoesisSettingsView::default();
    let next = apply_patch(
        &base,
        &patch(json!({"enabled": true, "concurrency": {"background": 3},
                      "models": {"strong": "opus"}, "budget": {"background_usd_daily": 2.5}})),
    );
    assert!(next.enabled);
    assert_eq!(next.concurrency.background, 3);
    assert_eq!(next.concurrency.interactive, base.concurrency.interactive);
    assert_eq!(next.models.strong.as_deref(), Some("opus"));
    assert_eq!(next.models.cheap, None);
    assert_eq!(next.budget.background_usd_daily, 2.5);
    assert_eq!(next.budget.interactive_usd_daily, 5.0);
    assert_eq!(apply_patch(&base, &NoesisSettingsPatch::default()), base);
}

fn invalid(wire: Value) -> String {
    validate(&command(wire.clone())).expect_err(&format!("{wire} is invalid"))
}

#[test]
fn text_lengths_are_counted_in_characters() {
    let send = |text: String| json!({"cmd": "noesis_send", "session": "ns_1", "text": text});
    assert!(validate(&command(send("é".repeat(20_000)))).is_ok());
    assert!(invalid(send("é".repeat(20_001))).contains("text"));
    assert!(invalid(send(String::new())).contains("1..=20000"));
    let create =
        |goal: String| json!({"cmd": "noesis_create_session", "goal": goal, "mode": "practice"});
    assert!(validate(&command(create("g".repeat(2000)))).is_ok());
    assert!(invalid(create("g".repeat(2001))).contains("goal"));
    assert!(invalid(create(String::new())).contains("goal"));
}

#[test]
fn names_and_reasons_have_their_bounds() {
    let import =
        |name: String| json!({"cmd": "noesis_import_source", "name": name, "artifact": artifact()});
    assert!(validate(&command(import("n".repeat(200)))).is_ok());
    assert!(invalid(import("n".repeat(201))).contains("name"));
    assert!(invalid(import(String::new())).contains("name"));
    let correct = |reason: String| json!({"cmd": "noesis_correct_learner", "concept": "k_1", "state": "unseen", "reason": reason});
    assert!(validate(&command(correct("r".repeat(500)))).is_ok());
    assert!(invalid(correct("r".repeat(501))).contains("reason"));
    assert!(invalid(correct(String::new())).contains("reason"));
}

#[test]
fn limits_are_bounded_per_command() {
    for (cmd, field, max) in [
        ("noesis_list_sessions", None, 200),
        ("noesis_graph", None, 5000),
        ("noesis_learner", None, 1000),
        ("noesis_learner_evidence", Some(("concept", "k_1")), 500),
    ] {
        let wire = |limit: u32| {
            let mut wire = json!({"cmd": cmd, "limit": limit});
            if let Some((name, value)) = field {
                wire[name] = json!(value);
            }
            wire
        };
        assert!(validate(&command(wire(1))).is_ok(), "{cmd} 1");
        assert!(validate(&command(wire(max))).is_ok(), "{cmd} {max}");
        assert!(invalid(wire(0)).contains("limit"), "{cmd} 0");
        assert!(
            invalid(wire(max + 1)).contains("limit"),
            "{cmd} {}",
            max + 1
        );
    }
}

#[test]
fn extract_takes_exactly_one_of_session_and_source() {
    assert!(
        validate(&command(
            json!({"cmd": "noesis_extract", "session": "ns_1"})
        ))
        .is_ok()
    );
    assert!(validate(&command(json!({"cmd": "noesis_extract", "source": "s_1"}))).is_ok());
    for wire in [
        json!({"cmd": "noesis_extract"}),
        json!({"cmd": "noesis_extract", "session": "ns_1", "source": "s_1"}),
    ] {
        assert!(invalid(wire).contains("exactly one"));
    }
}

#[test]
fn only_the_documented_confidences_are_accepted() {
    let answer = |confidence: u8| {
        json!({"cmd": "noesis_answer", "session": "ns_1", "question": "q_1",
        "answer": {"dont_know": false, "confidence": confidence}})
    };
    for confidence in CONFIDENCE_LEVELS {
        assert!(
            validate(&command(answer(confidence))).is_ok(),
            "{confidence}"
        );
    }
    assert!(invalid(answer(80)).contains("confidence"));
}

#[test]
fn an_empty_id_is_refused_at_the_boundary() {
    assert!(invalid(json!({"cmd": "noesis_get_session", "session": ""})).contains("session"));
    assert!(invalid(json!({"cmd": "noesis_cancel_job", "job": ""})).contains("job"));
}

fn bundle_of(excerpts: &[usize]) -> EvidenceBundle {
    EvidenceBundle {
        title: "t".into(),
        items: excerpts
            .iter()
            .map(|size| EvidenceItem {
                reference: EvidenceRef {
                    kind: EvidenceRefKind::File,
                    id: "f".into(),
                    url: None,
                },
                excerpt: "x".repeat(*size),
                status: EvidenceStatus::Preliminary,
            })
            .collect(),
    }
}

#[test]
fn the_excerpts_of_a_bundle_share_32_kib() {
    assert!(validate_bundle(&bundle_of(&[16 * 1024, 16 * 1024])).is_ok());
    let error = validate_bundle(&bundle_of(&[16 * 1024, 16 * 1024, 1])).unwrap_err();
    assert!(error.contains("32768"), "{error}");
    let create = Command::NoesisCreateSession {
        goal: "g".into(),
        mode: SessionMode::Teach,
        context: Some(bundle_of(&[MAX_BUNDLE_EXCERPT_BYTES + 1])),
        practice_targets: None,
        idempotency_key: None,
    };
    assert!(validate(&create).is_err());
}

#[test]
fn settings_ranges_follow_the_table() {
    let ok = |wire: Value| assert_eq!(validate_patch(&patch(wire.clone())), Ok(()), "{wire}");
    let bad = |wire: Value, key: &str| {
        let error = validate_patch(&patch(wire.clone())).expect_err(&wire.to_string());
        assert!(error.contains(key), "{error}");
    };
    ok(json!({"concurrency": {"interactive": 1, "background": 0}}));
    ok(json!({"concurrency": {"interactive": 4, "background": 3}}));
    bad(
        json!({"concurrency": {"interactive": 0}}),
        "concurrency.interactive",
    );
    bad(
        json!({"concurrency": {"interactive": 5}}),
        "concurrency.interactive",
    );
    bad(
        json!({"concurrency": {"background": 4}}),
        "concurrency.background",
    );
    ok(json!({"disable": {"grace_seconds": 300}}));
    bad(
        json!({"disable": {"grace_seconds": 301}}),
        "disable.grace_seconds",
    );
    ok(json!({"budget": {"interactive_usd_daily": 0.0, "background_usd_daily": 0.0}}));
    bad(
        json!({"budget": {"interactive_usd_daily": -0.5}}),
        "interactive_usd_daily",
    );
    bad(
        json!({"budget": {"background_usd_daily": -1.0}}),
        "background_usd_daily",
    );
    bad(json!({"models": {"strong": ""}}), "models.strong");
}

#[test]
fn a_non_finite_budget_is_refused() {
    let nan = NoesisSettingsPatch {
        budget: Some(BudgetPatch {
            interactive_usd_daily: Some(f64::NAN),
            ..Default::default()
        }),
        ..Default::default()
    };
    assert!(validate_patch(&nan).is_err());
}

#[test]
fn other_commands_pass_validation_untouched() {
    assert_eq!(validate(&Command::Status), Ok(()));
}

#[test]
fn the_status_reply_carries_the_documented_shape() {
    let view = NoesisSettingsView::default();
    let status = NoesisStatus {
        enabled: false,
        settings: view.clone(),
        effective: view,
        jobs_running: 0,
        spend_today: SpendToday {
            interactive: Usage::default(),
            background: Usage::default(),
            subscription: Usage::default(),
            api: Usage::default(),
            unknown: Usage::default(),
        },
        disable_policy: DisablePolicy {
            grace_seconds: 60,
            background: BackgroundDisable::CancelAtModelBoundary,
        },
        prompt_overrides: vec!["01-teacher".into()],
        models: vec![NoesisModelOffer {
            id: "claude-code/haiku".into(),
            billing: NoesisBilling::Subscription,
            ready: true,
        }],
        teaching_model: Some(NoesisModelChoice {
            model: Some("claude-code/haiku".into()),
            refusal: None,
        }),
    };
    let wire = serde_json::to_value(crate::Reply::NoesisStatus(status.clone())).unwrap();
    assert_eq!(wire["reply"], "noesis_status");
    assert_eq!(
        wire["data"]["models"],
        json!([{"id": "claude-code/haiku", "billing": "subscription", "ready": true}])
    );
    assert_eq!(
        wire["data"]["teaching_model"],
        json!({"model": "claude-code/haiku"})
    );
    let mut older = wire.clone();
    let data = older["data"].as_object_mut().unwrap();
    data.remove("models");
    data.remove("teaching_model");
    let crate::Reply::NoesisStatus(older) = serde_json::from_value(older).unwrap() else {
        panic!("a status reply");
    };
    assert!(older.models.is_empty() && older.teaching_model.is_none());
    assert_eq!(
        wire["data"]["disable_policy"],
        json!({"grace_seconds": 60, "background": "cancel_at_model_boundary"})
    );
    assert!(wire["data"]["spend_today"]["interactive"].is_object());
    let back: crate::Reply = serde_json::from_value(wire).unwrap();
    assert_eq!(back, crate::Reply::NoesisStatus(status));
}

#[test]
fn the_replies_carry_their_wire_tags() {
    use crate::Reply;
    let job = json!({"id": "j_1", "kind": "extract_source", "target": "s_1", "state": "queued",
        "usage": serde_json::to_value(Usage::default()).unwrap(),
        "created_at": "2026-01-01T00:00:00Z", "updated_at": "2026-01-01T00:00:00Z"});
    let concept = json!({"concept": {"id": "k_1", "kind": "topic", "ref": "t", "title": "T"},
        "state": "exposed", "strength": 0.5, "misconceptions": [], "as_of_evidence": 3});
    for (reply, data) in [
        (
            "noesis_sessions",
            json!({"sessions": [], "next_after": "ns_9"}),
        ),
        (
            "noesis_graph",
            json!({"nodes": [], "chains": [], "clusters": [], "truncated": false}),
        ),
        ("noesis_job", json!({"job": job})),
        ("noesis_import", json!({"source": "s_1", "job": job})),
        ("noesis_jobs", json!({"jobs": [job]})),
        ("noesis_learner", json!({"concepts": [concept]})),
        ("noesis_evidence", json!({"evidence": [], "next_before": 4})),
        ("concept_state", concept.clone()),
    ] {
        let wire = json!({"reply": reply, "data": data});
        let decoded: Reply =
            serde_json::from_value(wire.clone()).unwrap_or_else(|e| panic!("{reply}: {e}"));
        assert_eq!(serde_json::to_value(&decoded).unwrap(), wire, "{reply}");
    }
}

#[test]
fn the_refusals_name_their_reason_and_keep_their_code() {
    let disabled = ErrorReply::noesis_disabled();
    assert_eq!(disabled.code, ErrorCode::Unavailable);
    assert_eq!(disabled.reason.as_deref(), Some(crate::NOESIS_DISABLED));
    assert_eq!(
        disabled.message,
        "Noesis is disabled; enable it with noesis_configure"
    );
    let conflict = ErrorReply::conflict("a job on s_1 is running");
    assert_eq!(conflict.code, ErrorCode::InvalidTransition);
    assert_eq!(conflict.reason.as_deref(), Some("conflict"));
    let wire = serde_json::to_value(&disabled).unwrap();
    assert_eq!(wire["reason"], "noesis_disabled");
    let plain = serde_json::to_value(ErrorReply::invalid("x")).unwrap();
    assert!(
        plain.get("reason").is_none(),
        "an ordinary error carries no reason"
    );
    assert_eq!(
        serde_json::from_value::<ErrorReply>(wire).unwrap(),
        disabled
    );
}
