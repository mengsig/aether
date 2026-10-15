use super::*;
use serde_json::json;

fn receipt(session: &str, cost: f64) -> Value {
    json!({"session_id": session, "total_cost_usd": cost,
        "modelUsage": {"sonnet": {"inputTokens": 10, "outputTokens": 384,
            "cacheReadInputTokens": 78969, "cacheCreationInputTokens": 12906}}})
}

#[test]
fn fresh_receipt_preserves_cumulative_baseline() {
    let (cost, events) = SessionCost::default().receipt(&receipt("s", 0.0712778));
    assert_eq!(cost, Some(0.0712778));
    assert!(
        matches!(&events[..], [EngineEvent::SessionUsage { session, total }]
        if session == "s" && total.cost_usd == 0.0712778 && total.cost_complete
        && total.input_tokens == 10 && total.output_tokens == 384
        && total.cache_read_tokens == 78969 && total.cache_write_tokens == 12906)
    );
}

#[test]
fn resumed_receipts_subtract_initial_baseline_exactly_once() {
    let state = SessionCost::resumed(
        Some("s".into()),
        Usage {
            cost_usd: 0.0595702,
            ..Usage::default()
        },
    );
    let (cost, _) = state.receipt(&receipt("s", 0.0712778));
    assert!((cost.unwrap() - 0.0117076).abs() < 1e-12);
    // A later result in the same invocation still covers the entire invocation.
    let (cost, _) = state.receipt(&receipt("s", 0.08));
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
        let state = SessionCost::resumed(Some("s".into()), prior);
        let (cost, events) = state.receipt(&receipt("s", 0.5));
        assert_eq!(cost, None);
        assert!(matches!(&events[..], [EngineEvent::ProtocolNote(_),
            EngineEvent::SessionUsage { total, .. }] if total.cost_usd == 0.5));
    }
}

#[test]
fn replacement_session_starts_a_new_cost_history() {
    let state = SessionCost::resumed(
        Some("old".into()),
        Usage {
            cost_usd: 5.0,
            ..Usage::default()
        },
    );
    assert_eq!(state.receipt(&receipt("new", 0.2)).0, Some(0.2));
}

#[test]
fn missing_or_invalid_receipt_never_invents_a_price() {
    let state = SessionCost::default();
    assert_eq!(state.receipt(&json!({})), (None, vec![]));
    let (cost, events) = state.receipt(&receipt("s", -1.0));
    assert_eq!(cost, None);
    assert!(matches!(&events[..], [EngineEvent::ProtocolNote(_)]));
}

fn covered_usage(state: &SessionCost, value: &Value) -> Option<Usage> {
    let (cost, events) = state.receipt(value);
    assert!(cost.is_some());
    let [EngineEvent::SessionUsage { total, .. }] = &events[..] else {
        panic!("expected session counters: {events:?}");
    };
    state.cost_usage(value, *total)
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
    let state = SessionCost::resumed(Some("s".into()), before);
    for cost in [0.0712778, 0.08] {
        let usage = covered_usage(&state, &receipt("s", cost)).unwrap();
        assert_eq!((usage.input_tokens, usage.output_tokens), (6, 304));
        assert_eq!(
            (usage.cache_read_tokens, usage.cache_write_tokens),
            (77969, 10906)
        );
    }
}

#[test]
fn absent_malformed_or_regressed_model_usage_cannot_prove_coverage() {
    let state = SessionCost::resumed(
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
        assert_eq!(covered_usage(&state, &value), None);
    }
    let mut value = receipt("s", 0.08);
    value["modelUsage"]["sonnet"]["inputTokens"] = json!(4);
    assert_eq!(covered_usage(&state, &value), None);
    value.as_object_mut().unwrap().remove("modelUsage");
    assert_eq!(covered_usage(&state, &value), None);
}

#[test]
fn a_dollar_only_baseline_cannot_prove_a_resumed_tokens_scope() {
    let state = SessionCost::resumed(
        Some("s".into()),
        Usage {
            cost_usd: 0.02,
            ..Usage::default()
        },
    );
    assert_eq!(covered_usage(&state, &receipt("s", 0.08)), None);
    let usage = covered_usage(&state, &receipt("new", 0.08)).unwrap();
    assert_eq!(usage.input_tokens, 10);
    assert_eq!(usage.output_tokens, 384);
}

#[test]
fn native_interrupt_receipt_includes_restored_session_spend() {
    // Claude Code 2.1.299, three Haiku 5.5 processes: reply, interrupt at tool_use, resume.
    // The native receipts were $0.000491, $0.00069573 and $0.00077314.
    let state = SessionCost::resumed(
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
    let usage = covered_usage(&state, &value).unwrap();
    assert_eq!((usage.input_tokens, usage.output_tokens), (2, 82));
    assert_eq!(
        (usage.cache_read_tokens, usage.cache_write_tokens),
        (1793, 728)
    );
    assert!((state.receipt(&value).0.unwrap() - 0.00020473).abs() < 1e-12);
}

#[test]
fn an_unread_counter_cannot_become_a_known_zero_baseline_on_resume() {
    let mut value = receipt("s", 0.05);
    value["modelUsage"]["sonnet"]["outputTokens"] = Value::Null;
    let (_, events) = SessionCost::default().receipt(&value);
    let [EngineEvent::SessionUsage { total, .. }] = &events[..] else {
        panic!("expected the dollar baseline: {events:?}");
    };
    assert_eq!(total.cost_usd, 0.05);
    assert_eq!(total.total_tokens(), 0);
    let state = SessionCost::resumed(Some("s".into()), *total);
    assert_eq!(covered_usage(&state, &receipt("s", 0.08)), None);
}
