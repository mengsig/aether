use super::*;
use serde_json::json;

fn receipt(session: &str, cost: f64) -> Value {
    json!({"session_id": session, "total_cost_usd": cost,
        "modelUsage": {"sonnet": {"inputTokens": 10, "outputTokens": 384,
            "cacheReadInputTokens": 78969, "cacheCreationInputTokens": 12906}}})
}

#[test]
fn fresh_receipt_preserves_cumulative_baseline() {
    let (cost, events) = bill(&mut SessionCost::default(), &receipt("s", 0.0712778));
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
    let (cost, _) = bill(&mut state, &receipt("s", 0.0712778));
    assert!((cost.unwrap() - 0.0117076).abs() < 1e-12);
    // A later result in the same invocation still covers the entire invocation.
    let (cost, _) = bill(&mut state, &receipt("s", 0.08));
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
        let (cost, events) = bill(&mut state, &receipt("s", 0.5));
        assert_eq!(cost, None);
        if prior.cost_complete {
            assert!(matches!(&events[..], [EngineEvent::ProtocolNote(_)]));
        } else {
            assert!(matches!(&events[..], [EngineEvent::ProtocolNote(_),
                EngineEvent::SessionUsage { total, .. }] if total.cost_usd == 0.5));
        }
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
    assert_eq!(bill(&mut state, &receipt("new", 0.2)).0, Some(0.2));
}

#[test]
fn missing_or_invalid_receipt_never_invents_a_price() {
    let mut state = SessionCost::default();
    assert_eq!(bill(&mut state, &json!({})), (None, vec![]));
    let (cost, events) = bill(&mut state, &receipt("s", -1.0));
    assert_eq!(cost, None);
    assert!(matches!(&events[..], [EngineEvent::ProtocolNote(_)]));
}

fn covered_usage(state: &mut SessionCost, value: &Value) -> Option<Usage> {
    let streamed = model_counters(value).unwrap_or_default();
    let receipt = state.receipt(value, &streamed);
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
    assert!((bill(&mut state, &value).0.unwrap() - 0.00020473).abs() < 1e-12);
}

#[test]
fn an_unread_counter_cannot_become_a_known_zero_baseline_on_resume() {
    let mut value = receipt("s", 0.05);
    value["modelUsage"]["sonnet"]["outputTokens"] = Value::Null;
    let (_, events) = bill(&mut SessionCost::default(), &value);
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
    let (cost, events) = bill(&mut state, &zeroed);
    assert_eq!(cost, None);
    for event in events {
        if let EngineEvent::SessionUsage { total, .. } = event {
            baseline = total;
        }
    }
    let mut state = resumed(Some("s".into()), baseline);
    let mut next = receipt("s", 0.08);
    next["modelUsage"]["sonnet"]["inputTokens"] = json!(200);
    let (cost, _) = bill(&mut state, &next);
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

fn bill(state: &mut SessionCost, value: &Value) -> (Option<f64>, Vec<EngineEvent>) {
    let receipt = state.receipt(value, &BTreeMap::new());
    (receipt.cost_usd, receipt.events)
}

#[test]
fn an_all_zero_resumed_baseline_cannot_price_session_history() {
    let mut state = SessionCost::resumed(Some("s".into()), Usage::default());
    let receipt = state.receipt(&receipt("s", 0.08), &BTreeMap::new());
    assert_eq!(receipt.cost_usd, None);
    assert_eq!(receipt.cost_usage, None);
    assert!(matches!(receipt.events[0], EngineEvent::ProtocolNote(_)));
}

#[test]
fn later_receipts_in_one_process_cannot_replace_a_higher_baseline() {
    let mut state = SessionCost::default();
    let (_, events) = bill(&mut state, &receipt("s", 0.05));
    assert!(matches!(&events[..], [EngineEvent::SessionUsage { .. }]));
    let mut lower = receipt("s", 0.04);
    let (cost, events) = bill(&mut state, &lower);
    assert_eq!(cost, None);
    assert!(matches!(&events[..], [EngineEvent::ProtocolNote(_)]));
    lower["total_cost_usd"] = json!(0.06);
    lower["modelUsage"]["sonnet"]["outputTokens"] = json!(1);
    let (cost, events) = bill(&mut state, &lower);
    assert_eq!(cost, None);
    assert!(matches!(&events[..], [EngineEvent::ProtocolNote(_)]));
    assert_eq!(state.latest.as_ref().unwrap().total.cost_usd, 0.05);
    assert_eq!(bill(&mut state, &receipt("s", 0.08)).0, Some(0.08));
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
    let receipt = state.receipt(&value, &streamed);
    assert!((receipt.cost_usd.unwrap() - 0.03).abs() < 1e-12);
    assert_eq!(receipt.cost_usage, Some(model_usage(100)));
}
