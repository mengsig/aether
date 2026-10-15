use super::*;
use serde_json::json;

fn receipt(session: &str, cost: f64) -> Value {
    json!({"session_id": session, "total_cost_usd": cost,
        "modelUsage": {"sonnet": {"inputTokens": 10, "outputTokens": 384,
            "cacheReadInputTokens": 78969, "cacheCreationInputTokens": 12906}}})
}

#[test]
fn fresh_receipt_preserves_cumulative_baseline() {
    let (cost, events) = bill_after_spend(&mut SessionCost::default(), &receipt("s", 0.0712778));
    assert_eq!(cost, Some(0.0712778));
    assert!(
        matches!(&events[..], [EngineEvent::SessionUsage { session, total, .. }]
        if session == "s" && total.cost_usd == 0.0712778 && total.cost_complete
        && total.input_tokens == 10 && total.output_tokens == 384
        && total.cache_read_tokens == 78969 && total.cache_write_tokens == 12906)
    );
}

#[test]
fn resumed_receipts_subtract_initial_baseline_exactly_once() {
    let mut state = resumed(
        Some("s".into()),
        Usage {
            cost_usd: 0.0595702,
            ..Usage::default()
        },
    );
    let (cost, _) = bill_after_spend(&mut state, &receipt("s", 0.0712778));
    assert!((cost.unwrap() - 0.0117076).abs() < 1e-12);
    // A later result in the same invocation still covers the entire invocation.
    let (cost, _) = bill_after_spend(&mut state, &receipt("s", 0.08));
    assert!((cost.unwrap() - 0.0204298).abs() < 1e-12);
}

#[test]
fn unknown_or_reset_baseline_does_not_charge_old_session_history() {
    for prior in [
        Usage {
            cost_complete: false,
            ..Usage::default()
        },
        Usage {
            cost_usd: 1.0,
            ..Usage::default()
        },
    ] {
        let mut state = resumed(Some("s".into()), prior);
        let (cost, events) = bill_after_spend(&mut state, &receipt("s", 0.5));
        assert_eq!(cost, None);
        assert!(matches!(events.first(), Some(EngineEvent::ProtocolNote(_))));
        assert_unreported_gap(&events);
        assert!(
            matches!(events.last(), Some(EngineEvent::SessionUsage { total, .. })
            if total.cost_usd == 0.5)
        );
    }
}

#[test]
fn replacement_session_starts_a_new_cost_history() {
    let mut state = resumed(
        Some("old".into()),
        Usage {
            cost_usd: 5.0,
            ..Usage::default()
        },
    );
    assert_eq!(
        bill_after_spend(&mut state, &receipt("new", 0.2)).0,
        Some(0.2)
    );
}

#[test]
fn missing_or_invalid_receipt_never_invents_a_price() {
    let mut state = SessionCost::default();
    let (cost, events) = bill_after_spend(&mut state, &json!({}));
    assert_eq!(cost, None);
    assert_unreported_gap(&events);
    let (cost, events) = bill_after_spend(&mut state, &receipt("s", -1.0));
    assert_eq!(cost, None);
    assert!(matches!(events.first(), Some(EngineEvent::ProtocolNote(_))));
    assert_unreported_gap(&events);
}

fn assert_unreported_gap(events: &[EngineEvent]) {
    let gaps: Vec<_> = events
        .iter()
        .filter_map(|event| match event {
            EngineEvent::UsageDelta { usage } => Some(*usage),
            _ => None,
        })
        .collect();
    assert_eq!(gaps.len(), 1, "each unusable receipt needs one gap delta");
    assert_eq!(gaps[0].total_tokens(), 0);
    assert_eq!(gaps[0].cost_usd, 0.0);
    assert!(!gaps[0].cost_complete);
    assert_eq!(
        gaps[0].cost_gap,
        Some(aether_core::model::CostGap::Unreported)
    );
}

#[test]
fn invalid_receipt_marks_the_unreported_gap() {
    let (cost, events) = bill_after_spend(&mut SessionCost::default(), &receipt("s", -1.0));
    assert_eq!(cost, None);
    assert_unreported_gap(&events);
}

fn covered_usage(state: &mut SessionCost, value: &Value) -> Option<Usage> {
    let streamed = model_counters(value).unwrap_or_default();
    let receipt = state.receipt(value, &streamed, SpendEvidence::Seen);
    assert!(
        receipt
            .cost_usd
            .is_none_or(|cost| cost.is_finite() && cost >= 0.0)
    );
    assert!(receipt.cost_usage.is_none() || receipt.cost_usd.is_some());
    receipt.cost_usage
}

#[test]
fn model_usage_subtracts_the_process_start_on_every_result() {
    let before = Usage {
        input_tokens: 4,
        output_tokens: 80,
        cache_read_tokens: 1000,
        cache_write_tokens: 2000,
        cost_usd: 0.02,
        ..Usage::default()
    };
    let mut state = resumed(Some("s".into()), before);
    for cost in [0.0712778, 0.08] {
        let usage = covered_usage(&mut state, &receipt("s", cost)).unwrap();
        assert_eq!((usage.input_tokens, usage.output_tokens), (6, 304));
        assert_eq!(
            (usage.cache_read_tokens, usage.cache_write_tokens),
            (77969, 10906)
        );
    }
}

#[test]
fn absent_malformed_or_regressed_model_usage_cannot_prove_coverage() {
    let mut state = resumed(
        Some("s".into()),
        Usage {
            input_tokens: 5,
            cost_usd: 0.02,
            ..Usage::default()
        },
    );
    for field in [
        "inputTokens",
        "outputTokens",
        "cacheReadInputTokens",
        "cacheCreationInputTokens",
    ] {
        let mut value = receipt("s", 0.08);
        value["modelUsage"]["sonnet"][field] = Value::Null;
        assert_eq!(covered_usage(&mut state, &value), None);
    }
    let mut value = receipt("s", 0.08);
    value["modelUsage"]["sonnet"]["inputTokens"] = json!(4);
    assert_eq!(covered_usage(&mut state, &value), None);
    value.as_object_mut().unwrap().remove("modelUsage");
    assert_eq!(covered_usage(&mut state, &value), None);
}

#[test]
fn a_dollar_only_baseline_cannot_prove_a_resumed_tokens_scope() {
    let mut state = resumed(
        Some("s".into()),
        Usage {
            cost_usd: 0.02,
            ..Usage::default()
        },
    );
    assert_eq!(covered_usage(&mut state, &receipt("s", 0.08)), None);
    let usage = covered_usage(&mut state, &receipt("new", 0.08)).unwrap();
    assert_eq!(usage.input_tokens, 10);
    assert_eq!(usage.output_tokens, 384);
}

#[test]
fn native_interrupt_receipt_includes_restored_session_spend() {
    // Claude Code 2.1.299, three Haiku 5.5 processes: reply, interrupt at tool_use, resume.
    // The native receipts were $0.000491, $0.00069573 and $0.00077314.
    let mut state = resumed(
        Some("s".into()),
        Usage {
            input_tokens: 2,
            output_tokens: 4,
            cache_write_tokens: 2444,
            cost_usd: 0.000491,
            ..Usage::default()
        },
    );
    let value = json!({"session_id":"s","total_cost_usd":0.00069573,
        "modelUsage":{"claude-haiku-5-5":{"inputTokens":4,"outputTokens":86,
            "cacheReadInputTokens":1793,"cacheCreationInputTokens":3172}}});
    state.before_models = Some(BTreeMap::from([(
        "claude-haiku-5-5".into(),
        state.before.unwrap(),
    )]));
    let usage = covered_usage(&mut state, &value).unwrap();
    assert_eq!((usage.input_tokens, usage.output_tokens), (2, 82));
    assert_eq!(
        (usage.cache_read_tokens, usage.cache_write_tokens),
        (1793, 728)
    );
    assert!((bill_after_spend(&mut state, &value).0.unwrap() - 0.00020473).abs() < 1e-12);
}

#[test]
fn an_unread_counter_cannot_become_a_known_zero_baseline_on_resume() {
    let mut value = receipt("s", 0.05);
    value["modelUsage"]["sonnet"]["outputTokens"] = Value::Null;
    let (_, events) = bill_after_spend(&mut SessionCost::default(), &value);
    let [EngineEvent::SessionUsage { total, .. }] = &events[..] else {
        panic!("expected the dollar baseline: {events:?}");
    };
    assert_eq!(total.cost_usd, 0.05);
    assert_eq!(total.total_tokens(), 0);
    let mut state = resumed(Some("s".into()), *total);
    assert_eq!(covered_usage(&mut state, &receipt("s", 0.08)), None);
}

#[test]
fn zeroed_crash_receipt_keeps_the_baseline_for_the_next_resume() {
    let mut baseline = Usage {
        cost_usd: 0.05,
        input_tokens: 100,
        output_tokens: 10,
        ..Usage::default()
    };
    let mut state = resumed(Some("s".into()), baseline);
    let zeroed = json!({"session_id":"s","subtype":"error_during_execution",
        "total_cost_usd":0.0,"modelUsage":{}});
    let (cost, events) = bill_after_spend(&mut state, &zeroed);
    assert_eq!(cost, None);
    assert_unreported_gap(&events);
    for event in events {
        if let EngineEvent::SessionUsage { total, .. } = event {
            baseline = total;
        }
    }
    let mut state = resumed(Some("s".into()), baseline);
    let mut next = receipt("s", 0.08);
    next["modelUsage"]["sonnet"]["inputTokens"] = json!(200);
    let (cost, _) = bill_after_spend(&mut state, &next);
    assert!(
        (cost.unwrap() - 0.03).abs() < 1e-12,
        "next resume billed {cost:?}"
    );
    assert_eq!(baseline.cost_usd, 0.05);
    assert_eq!(covered_usage(&mut state, &next).unwrap().input_tokens, 100);
}

fn resumed(session: Option<String>, usage: Usage) -> SessionCost {
    let models = (usage.total_tokens() > 0).then(|| BTreeMap::from([("sonnet".into(), usage)]));
    SessionCost::resumed(session, usage).with_models(models)
}

/// The next run's state, resumed from the baseline this one persisted.
fn next_run(state: &SessionCost) -> SessionCost {
    let saved = state
        .latest
        .as_ref()
        .expect("a nonzero receipt was persisted");
    assert!(saved.total.cost_complete);
    SessionCost::resumed(Some(saved.session.clone()), saved.total).with_models(saved.models.clone())
}

fn bill_after_spend(state: &mut SessionCost, value: &Value) -> (Option<f64>, Vec<EngineEvent>) {
    let receipt = state.receipt(value, &BTreeMap::new(), SpendEvidence::Seen);
    (receipt.cost_usd, receipt.events)
}

#[test]
fn an_all_zero_resumed_baseline_cannot_price_session_history() {
    let mut state = SessionCost::resumed(Some("s".into()), Usage::default());
    let result = state.receipt(&receipt("s", 0.08), &BTreeMap::new(), SpendEvidence::Seen);
    assert_eq!(result.cost_usd, None);
    assert_eq!(result.cost_usage, None);
    assert!(matches!(result.events[0], EngineEvent::ProtocolNote(_)));
    assert_unreported_gap(&result.events);
    let (cost, events) = bill_after_spend(&mut state, &receipt("s", 0.1));
    assert_eq!(cost, None);
    assert_unreported_gap(&events);
    let (cost, _) = bill_after_spend(&mut next_run(&state), &receipt("s", 0.13));
    assert!((cost.unwrap() - 0.03).abs() < 1e-12);
}

#[test]
fn unusable_receipts_preserve_a_known_baseline_until_covered() {
    let prior = Usage {
        cost_usd: 0.05,
        ..Usage::default()
    };
    for value in [
        json!({}),
        receipt("s", -1.0),
        json!({"session_id":"s", "total_cost_usd":0.0, "modelUsage":{}}),
    ] {
        let mut state = resumed(Some("s".into()), prior);
        let (cost, events) = bill_after_spend(&mut state, &value);
        assert_eq!(cost, None);
        assert_unreported_gap(&events);
        assert!(state.latest.is_none());
        assert_eq!(state.before, Some(prior));
        let (cost, _) = bill_after_spend(&mut state, &receipt("s", 0.08));
        assert!((cost.unwrap() - 0.03).abs() < 1e-12);
    }
}

#[test]
fn a_zero_receipt_without_spend_preserves_the_latest_reading() {
    let mut state = SessionCost::default();
    bill_after_spend(&mut state, &receipt("s", 0.05));
    let latest = state.latest.clone();
    let zero = json!({"session_id":"s","total_cost_usd":0.0,"modelUsage":{}});
    let result = state.receipt(&zero, &BTreeMap::new(), SpendEvidence::Unseen);
    assert_eq!(result.cost_usd, None);
    assert_eq!(result.cost_usage, None);
    assert!(matches!(&result.events[..], [EngineEvent::ProtocolNote(_)]));
    assert_eq!(state.latest, latest);
    assert_eq!(state.before, Some(Usage::default()));
    assert_eq!(
        bill_after_spend(&mut next_run(&state), &receipt("s", 0.08)).0,
        Some(0.03)
    );
}

#[test]
fn a_reset_leaves_the_run_unknown_and_baselines_the_next_resume() {
    let mut state = SessionCost::default();
    let (_, events) = bill_after_spend(&mut state, &receipt("s", 0.05));
    assert!(matches!(&events[..], [EngineEvent::SessionUsage { .. }]));
    let mut lower = receipt("s", 0.04);
    let (cost, events) = bill_after_spend(&mut state, &lower);
    assert_eq!(cost, None);
    assert!(
        matches!(&events[..], [EngineEvent::ProtocolNote(_), EngineEvent::UsageDelta { .. },
        EngineEvent::SessionUsage { total, .. }] if total.cost_usd == 0.04)
    );
    lower["total_cost_usd"] = json!(0.06);
    lower["modelUsage"]["sonnet"]["outputTokens"] = json!(1);
    let (cost, events) = bill_after_spend(&mut state, &lower);
    assert_eq!(cost, None);
    assert!(
        matches!(&events[..], [EngineEvent::ProtocolNote(_), EngineEvent::UsageDelta { .. },
        EngineEvent::SessionUsage { total, .. }] if total.output_tokens == 1)
    );
    assert_eq!(state.latest.as_ref().unwrap().total.cost_usd, 0.06);
    let (mut resumed, mut covering) = (next_run(&state), next_run(&state));
    // The run's spend before the reset is lost, so its later receipts stay unknown.
    assert_eq!(bill_after_spend(&mut state, &receipt("s", 0.08)).0, None);
    assert!(
        (bill_after_spend(&mut resumed, &receipt("s", 0.08))
            .0
            .unwrap()
            - 0.02)
            .abs()
            < 1e-12
    );
    let covered = covered_usage(&mut covering, &receipt("s", 0.08)).unwrap();
    assert_eq!(covered.input_tokens, 0);
    assert_eq!(covered.output_tokens, 383);
}

#[test]
fn reset_dollars_are_adopted_across_resumes() {
    let mut state = resumed(
        Some("s".into()),
        Usage {
            cost_usd: 8.0,
            ..Usage::default()
        },
    );
    for (dollars, expected) in [(2.0, None), (5.0, Some(3.0)), (10.0, Some(5.0))] {
        let (cost, _) = bill_after_spend(&mut state, &receipt("s", dollars));
        assert_eq!(cost, expected);
        let saved = state
            .latest
            .as_ref()
            .expect("every nonzero reading is persisted");
        assert_eq!(saved.total.cost_usd, dollars);
        state = SessionCost::resumed(Some(saved.session.clone()), saved.total)
            .with_models(saved.models.clone());
    }
}

#[test]
fn a_missing_model_is_adopted_across_resumes() {
    let mut prior = receipt("s", 8.0);
    prior["modelUsage"]["opus"] = prior["modelUsage"]["sonnet"].clone();
    let mut state = SessionCost::default();
    bill_after_spend(&mut state, &prior);
    let saved = state.latest.as_ref().unwrap();
    state = SessionCost::resumed(Some(saved.session.clone()), saved.total)
        .with_models(saved.models.clone());
    for (dollars, expected) in [(2.0, None), (20.0, Some(18.0)), (200.0, Some(180.0))] {
        let (cost, _) = bill_after_spend(&mut state, &receipt("s", dollars));
        assert_eq!(cost, expected);
        let saved = state.latest.as_ref().unwrap();
        assert_eq!(saved.models.as_ref().unwrap().len(), 1);
        state = SessionCost::resumed(Some(saved.session.clone()), saved.total)
            .with_models(saved.models.clone());
    }
}

#[test]
fn a_missing_model_resets_even_when_remaining_counters_cover_the_prior_total() {
    let mut prior = receipt("s", 8.0);
    prior["modelUsage"]["opus"] = prior["modelUsage"]["sonnet"].clone();
    let mut state = SessionCost::default();
    bill_after_spend(&mut state, &prior);
    let mut next = receipt("s", 20.0);
    for field in [
        "inputTokens",
        "outputTokens",
        "cacheReadInputTokens",
        "cacheCreationInputTokens",
    ] {
        next["modelUsage"]["sonnet"][field] =
            json!(prior["modelUsage"]["sonnet"][field].as_u64().unwrap() * 3);
    }
    assert_eq!(bill_after_spend(&mut state, &next).0, None);
    assert_eq!(
        state
            .latest
            .as_ref()
            .unwrap()
            .models
            .as_ref()
            .unwrap()
            .len(),
        1
    );
    let mut resumed = next_run(&state);
    next["total_cost_usd"] = json!(200.0);
    assert_eq!(
        bill_after_spend(&mut state, &next).0,
        None,
        "the reset run stays unknown"
    );
    assert_eq!(bill_after_spend(&mut resumed, &next).0, Some(180.0));
}

#[test]
fn each_model_counter_reset_is_unknown_even_when_session_totals_grow() {
    for field in [
        "inputTokens",
        "outputTokens",
        "cacheReadInputTokens",
        "cacheCreationInputTokens",
        "thinkingTokens",
    ] {
        let mut prior = receipt("s", 8.0);
        prior["modelUsage"]["sonnet"]["thinkingTokens"] = json!(8);
        let mut state = SessionCost::default();
        bill_after_spend(&mut state, &prior);
        let mut value = prior.clone();
        value["total_cost_usd"] = json!(10.0);
        value["modelUsage"]["opus"] = prior["modelUsage"]["sonnet"].clone();
        value["modelUsage"]["sonnet"][field] = json!(1);
        let result = state.receipt(&value, &BTreeMap::new(), SpendEvidence::Seen);
        assert_eq!(result.cost_usd, None, "{field}");
        assert_eq!(result.cost_usage, None, "{field}");
        assert_eq!(
            state.latest.as_ref().unwrap().total.cost_usd,
            10.0,
            "{field}"
        );
        let mut resumed = next_run(&state);
        value["total_cost_usd"] = json!(12.0);
        assert_eq!(bill_after_spend(&mut state, &value).0, None, "{field}");
        assert_eq!(
            bill_after_spend(&mut resumed, &value).0,
            Some(2.0),
            "{field}"
        );
    }
}

#[test]
fn resumed_model_coverage_cannot_borrow_unstreamed_helper_counters() {
    let model_usage = |tokens| Usage {
        input_tokens: tokens,
        output_tokens: tokens,
        cache_read_tokens: tokens,
        cache_write_tokens: tokens,
        ..Usage::default()
    };
    let prior = BTreeMap::from([
        ("main".into(), model_usage(100)),
        ("helper".into(), model_usage(100)),
    ]);
    let mut baseline = sum_counters(&prior);
    baseline.set_cost(Some(0.05));
    let mut state = SessionCost::resumed(Some("s".into()), baseline).with_models(Some(prior));
    let value = json!({"session_id":"s","total_cost_usd":0.08,
        "modelUsage":{"main":{"inputTokens":200,"outputTokens":200,
            "cacheReadInputTokens":200,"cacheCreationInputTokens":200},
            "helper":{"inputTokens":1000,"outputTokens":1000,
                "cacheReadInputTokens":1000,"cacheCreationInputTokens":1000}}});
    let streamed = BTreeMap::from([("main".into(), model_usage(200))]);
    let receipt = state.receipt(&value, &streamed, SpendEvidence::Seen);
    assert!((receipt.cost_usd.unwrap() - 0.03).abs() < 1e-12);
    assert_eq!(receipt.cost_usage, Some(model_usage(100)));
}

#[test]
fn a_rejected_request_with_a_zeroed_receipt_costs_zero_and_keeps_the_baseline() {
    let mut state = SessionCost::default();
    bill_after_spend(&mut state, &receipt("s", 0.05));
    let (latest, before) = (state.latest.clone(), state.before);
    let zero = json!({"session_id":"s","total_cost_usd":0.0,"modelUsage":{}});
    let result = state.receipt(&zero, &BTreeMap::new(), SpendEvidence::Rejected);
    assert_eq!(result.cost_usd, Some(0.0));
    assert_eq!(result.cost_usage, None);
    assert!(result.events.is_empty());
    assert_eq!(state.latest, latest);
    assert_eq!(state.before, before);
}
