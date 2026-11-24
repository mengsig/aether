use super::*;

fn model(text: &str) -> ModelRef {
    ModelRef::parse(text).expect("provider/model")
}

fn candidate(model: &ModelRef, tier: Option<ModelTier>, effort: Effort) -> Candidate<'_> {
    Candidate {
        role: "worker",
        model,
        tier,
        effort,
    }
}

fn adaptive() -> DelegationPolicy {
    DelegationPolicy {
        workers: vec![
            WorkerChoice {
                model: None,
                tier: Some(ModelTier::Balanced),
                effort: Some(Effort::Medium),
            },
            WorkerChoice {
                model: Some(model("codex/gpt-6.1-terra")),
                tier: None,
                effort: Some(Effort::High),
            },
        ],
        default_tier: Some(ModelTier::Balanced),
        max_effort: Some(Effort::High),
        max_parallel: Some(2),
        exempt_roles: Vec::new(),
        exempt_models: Vec::new(),
        require_independent_vendor: false,
    }
}

#[test]
fn a_listed_tier_or_model_within_its_effort_is_admitted() {
    let policy = adaptive();
    let sonnet = model("claude-code/sonnet");
    let terra = model("codex/gpt-6.1-terra");
    let balanced = candidate(&sonnet, Some(ModelTier::Balanced), Effort::Medium);
    assert_eq!(
        policy.admits(&balanced, Some(ModelTier::Frontier), 0),
        Ok(())
    );
    let named = candidate(&terra, Some(ModelTier::Strong), Effort::High);
    assert_eq!(policy.admits(&named, Some(ModelTier::Frontier), 1), Ok(()));
}

#[test]
fn a_child_outside_the_policy_is_refused_with_the_allowed_choices() {
    let policy = adaptive();
    let fable = model("claude-code/claude-fable-5-2");
    let clone = candidate(&fable, Some(ModelTier::Frontier), Effort::Medium);
    let err = policy
        .admits(&clone, Some(ModelTier::Frontier), 0)
        .unwrap_err();
    assert!(err.contains("model is outside"), "{err}");
    assert!(
        err.contains("any balanced-tier model at effort medium or lower"),
        "{err}"
    );
    assert!(
        err.contains("codex/gpt-6.1-terra at effort high or lower"),
        "{err}"
    );
    let sonnet = model("claude-code/sonnet");
    let too_hard = candidate(&sonnet, Some(ModelTier::Balanced), Effort::High);
    let err = policy
        .admits(&too_hard, Some(ModelTier::Frontier), 0)
        .unwrap_err();
    assert!(err.contains("effort high is above"), "{err}");
    let above_max = candidate(&sonnet, Some(ModelTier::Balanced), Effort::Max);
    assert!(policy.admits(&above_max, None, 0).is_err());
}

#[test]
fn max_parallel_counts_unfinished_children() {
    let policy = adaptive();
    let sonnet = model("claude-code/sonnet");
    let child = candidate(&sonnet, Some(ModelTier::Balanced), Effort::Low);
    let err = policy.admits(&child, None, 2).unwrap_err();
    assert!(err.contains("at most 2 unfinished"), "{err}");
    assert_eq!(policy.admits(&child, None, 1), Ok(()));
}

#[test]
fn an_empty_worker_list_bounds_by_default_tier_then_by_the_parent() {
    let sonnet = model("claude-code/sonnet");
    let opus = model("claude-code/opus");
    let tiered = DelegationPolicy {
        default_tier: Some(ModelTier::Balanced),
        ..DelegationPolicy::default()
    };
    let strong = candidate(&opus, Some(ModelTier::Strong), Effort::Low);
    assert!(
        tiered
            .admits(&strong, Some(ModelTier::Frontier), 0)
            .is_err()
    );
    let unranked = candidate(&opus, None, Effort::Low);
    assert!(
        tiered
            .admits(&unranked, Some(ModelTier::Frontier), 0)
            .is_err()
    );
    let balanced = candidate(&sonnet, Some(ModelTier::Balanced), Effort::Low);
    assert_eq!(tiered.admits(&balanced, None, 0), Ok(()));
    let empty = DelegationPolicy::default();
    assert!(empty.admits(&strong, Some(ModelTier::Balanced), 0).is_err());
    assert_eq!(empty.admits(&strong, Some(ModelTier::Strong), 0), Ok(()));
}

#[test]
fn without_a_policy_a_child_may_not_outrank_its_parent() {
    let opus = model("claude-code/opus");
    let child = candidate(&opus, Some(ModelTier::Frontier), Effort::Low);
    let err = admits_without_policy(&child, Some(ModelTier::Balanced)).unwrap_err();
    assert!(err.contains("above your own balanced tier"), "{err}");
    assert_eq!(
        admits_without_policy(&child, Some(ModelTier::Frontier)),
        Ok(())
    );
    assert_eq!(admits_without_policy(&child, None), Ok(()));
    let unranked = candidate(&opus, None, Effort::Low);
    assert_eq!(
        admits_without_policy(&unranked, Some(ModelTier::Fast)),
        Ok(())
    );
}

#[test]
fn only_the_parents_own_frontier_model_needs_a_reason() {
    let fable = model("claude-code/claude-fable-5-2");
    let opus = model("claude-code/opus");
    assert!(needs_reason(
        &candidate(&fable, Some(ModelTier::Frontier), Effort::Low),
        &fable
    ));
    assert!(!needs_reason(
        &candidate(&opus, Some(ModelTier::Frontier), Effort::Low),
        &fable
    ));
    assert!(!needs_reason(
        &candidate(&fable, Some(ModelTier::Strong), Effort::Low),
        &fable
    ));
    assert_eq!(normalize_reason(Some("  ")), Ok(None));
    assert_eq!(normalize_reason(Some(" why ")), Ok(Some("why".into())));
    let long = "x".repeat(MAX_DELEGATION_REASON_BYTES + 1);
    assert!(normalize_reason(Some(&long)).is_err());
}

#[test]
fn validation_rejects_ambiguous_entries_and_a_zero_limit() {
    let both = DelegationPolicy {
        workers: vec![WorkerChoice {
            model: Some(model("a/b")),
            tier: Some(ModelTier::Fast),
            effort: None,
        }],
        ..DelegationPolicy::default()
    };
    assert!(both.validate().unwrap_err().contains("exactly one"));
    let zero = DelegationPolicy {
        max_parallel: Some(0),
        ..DelegationPolicy::default()
    };
    assert!(zero.validate().is_err());
    assert_eq!(adaptive().validate(), Ok(()));
}

#[test]
fn the_wire_shape_round_trips_and_refuses_unknown_fields() {
    let wire = serde_json::json!({
        "workers": [
            {"tier": "balanced", "effort": "medium"},
            {"model": {"provider": "codex", "model": "gpt-6.1-terra"}, "effort": "high"}
        ],
        "default_tier": "balanced",
        "max_effort": "high",
        "max_parallel": 2
    });
    let policy: DelegationPolicy = serde_json::from_value(wire.clone()).expect("decode");
    assert_eq!(policy, adaptive());
    assert_eq!(serde_json::to_value(&policy).expect("encode"), wire);
    let unknown = serde_json::json!({"workers": [], "max_children": 3});
    assert!(serde_json::from_value::<DelegationPolicy>(unknown).is_err());
}

#[test]
fn an_exempt_role_skips_the_list_but_not_the_parents_tier() {
    let mut policy = adaptive();
    policy.exempt_roles = vec!["reviewer".into()];
    policy.exempt_models = vec![model("codex/gpt-6.2-astra")];
    let opus = model("claude-code/opus");
    let mut reviewer = candidate(&opus, Some(ModelTier::Strong), Effort::XHigh);
    reviewer.role = "reviewer";
    assert_eq!(policy.admits(&reviewer, Some(ModelTier::Strong), 5), Ok(()));
    let err = policy
        .admits(&reviewer, Some(ModelTier::Balanced), 0)
        .unwrap_err();
    assert!(err.contains("above your own balanced tier"), "{err}");
    let astra = model("codex/gpt-6.2-astra");
    let mut planned = candidate(&astra, Some(ModelTier::Frontier), Effort::XHigh);
    planned.role = "reviewer";
    assert_eq!(policy.admits(&planned, Some(ModelTier::Fast), 0), Ok(()));
    assert!(policy.names_model("reviewer", &astra));
    assert!(!policy.names_model("worker", &astra));
    let worker = candidate(&opus, Some(ModelTier::Strong), Effort::Low);
    assert!(
        policy
            .admits(&worker, Some(ModelTier::Frontier), 0)
            .is_err()
    );
}

#[test]
fn archons_wire_shape_with_exemptions_decodes() {
    let wire = serde_json::json!({
        "workers": [{"tier": "balanced", "effort": "medium"}],
        "default_tier": "balanced",
        "max_effort": "x_high",
        "max_parallel": 2,
        "exempt_roles": ["reviewer"],
        "exempt_models": [{"provider": "codex", "model": "gpt-6.1-terra"}]
    });
    let policy: DelegationPolicy = serde_json::from_value(wire.clone()).expect("decode");
    assert_eq!(policy.max_effort, Some(Effort::XHigh));
    assert!(policy.exempts("reviewer"));
    assert_eq!(policy.validate(), Ok(()));
    assert_eq!(serde_json::to_value(&policy).expect("encode"), wire);
}

#[test]
fn an_exempt_model_must_be_well_formed() {
    let mut policy = adaptive();
    policy.exempt_roles = vec!["reviewer".into()];
    policy.exempt_models = vec![ModelRef::new("codex", " ")];
    let err = policy.validate().unwrap_err();
    assert!(err.contains("exempt_models[0]"), "{err}");
    policy.exempt_models = vec![ModelRef::new("", "gpt-6.1-terra")];
    assert!(policy.validate().is_err());
}

#[test]
fn the_description_names_exempt_models_and_their_tier_exception() {
    let mut policy = adaptive();
    policy.exempt_roles = vec!["reviewer".into()];
    let plain = policy.describe();
    assert!(
        plain.contains("roles reviewer exempt from worker limits, at your tier or below"),
        "{plain}"
    );
    policy.exempt_models = vec![model("codex/gpt-6.2-astra")];
    let named = policy.describe();
    assert!(
        named.contains("any of codex/gpt-6.2-astra at any tier"),
        "{named}"
    );
    assert!(
        named.contains("other models at your tier or below"),
        "{named}"
    );
    let sonnet = model("claude-code/sonnet");
    let refused = policy
        .admits(
            &candidate(&sonnet, Some(ModelTier::Fast), Effort::Low),
            None,
            2,
        )
        .unwrap_err();
    assert!(refused.contains("at most 2 unfinished"), "{refused}");
    let outside = model("claude-code/opus");
    let refused = policy
        .admits(
            &candidate(&outside, Some(ModelTier::Strong), Effort::Low),
            None,
            0,
        )
        .unwrap_err();
    assert!(
        refused.contains("any of codex/gpt-6.2-astra at any tier"),
        "{refused}"
    );
}

#[test]
fn an_exempt_role_is_not_bound_by_max_effort_or_the_parallel_limit() {
    let mut policy = adaptive();
    policy.exempt_roles = vec!["reviewer".into()];
    let opus = model("claude-code/opus");
    let mut reviewer = candidate(&opus, Some(ModelTier::Strong), Effort::Max);
    reviewer.role = "reviewer";
    assert_eq!(policy.admits(&reviewer, Some(ModelTier::Strong), 9), Ok(()));
    let err = policy
        .admits(&reviewer, Some(ModelTier::Fast), 0)
        .unwrap_err();
    assert!(err.contains("above your own fast tier"), "{err}");
}
