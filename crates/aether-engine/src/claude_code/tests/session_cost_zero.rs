use super::*;

fn zero_receipt(session: &str) -> Value {
    json!({"session_id":session,"total_cost_usd":0.0,"modelUsage":{},
        "usage":{"input_tokens":0,"output_tokens":0}})
}

#[test]
fn a_known_zero_baseline_and_no_activity_prove_zero_cost() {
    for mut state in [
        SessionCost::default(),
        resumed(
            Some("old".into()),
            Usage {
                cost_usd: 1.0,
                ..Usage::default()
            },
        ),
    ] {
        let before = state.before;
        let result = state.receipt(&zero_receipt("new"), &BTreeMap::new());
        assert_eq!(result.cost_usd, Some(0.0));
        assert!(
            result.events.is_empty(),
            "zero spend needs no gap or persist"
        );
        assert!(state.latest.is_none());
        assert_eq!(state.before, before);
        assert_eq!(bill(&mut state, &receipt("new", 0.25)).0, Some(0.25));
    }
}

#[test]
fn zero_receipts_with_unknown_nonzero_or_reset_baselines_keep_the_gap() {
    let mut reset = SessionCost::default();
    bill(&mut reset, &receipt("s", 1.0));
    bill(&mut reset, &receipt("s", 0.5));
    assert!(reset.reset);
    for (mut state, session) in [
        (resumed(Some("s".into()), Usage::default()), "s"),
        (
            resumed(
                Some("s".into()),
                Usage {
                    cost_usd: 1.0,
                    ..Usage::default()
                },
            ),
            "s",
        ),
        (reset, "replacement"),
    ] {
        let before = state.before;
        let latest = state.latest.clone();
        let (cost, events) = bill(&mut state, &zero_receipt(session));
        assert_eq!(cost, None);
        assert_unreported_gap(&events);
        assert_eq!(state.before, before);
        assert_eq!(state.latest, latest);
        assert!(
            !events
                .iter()
                .any(|event| matches!(event, EngineEvent::SessionUsage { .. }))
        );
    }
}

#[test]
fn zero_receipts_after_streamed_tokens_keep_the_gap() {
    for usage in [
        Usage {
            input_tokens: 1,
            ..Usage::default()
        },
        Usage {
            reasoning_tokens: Some(1),
            ..Usage::default()
        },
    ] {
        let mut state = SessionCost::default();
        let streamed = BTreeMap::from([("sonnet".into(), usage)]);
        let result = state.receipt(&zero_receipt("s"), &streamed);
        assert_eq!(result.cost_usd, None);
        assert_unreported_gap(&result.events);
        assert_eq!(state.before, Some(Usage::default()));
        assert!(state.latest.is_none());
    }
}

#[test]
fn zero_receipts_with_per_turn_tokens_keep_the_gap() {
    for usage in [
        json!({"input_tokens":1}),
        json!({"output_tokens":1}),
        json!({"cache_read_input_tokens":1}),
        json!({"cache_creation_input_tokens":1}),
        json!({"output_tokens_details":{"thinking_tokens":1}}),
        json!({"cache_creation":{"ephemeral_5m_input_tokens":1}}),
    ] {
        let mut value = zero_receipt("s");
        value["usage"] = usage;
        let mut state = SessionCost::default();
        let (cost, events) = bill(&mut state, &value);
        assert_eq!(cost, None);
        assert_unreported_gap(&events);
        assert!(state.latest.is_none());
    }
}

#[test]
fn zero_receipts_with_unreadable_counters_cannot_prove_zero_cost() {
    for (field, value) in [
        ("usage", Value::Null),
        ("usage", json!({"input_tokens":"unknown"})),
        ("usage", json!({"output_tokens_details":null})),
        (
            "usage",
            json!({"cache_creation":{"ephemeral_1h_input_tokens":1}}),
        ),
        ("modelUsage", Value::Null),
        ("modelUsage", json!({"sonnet":{"inputTokens":null}})),
        (
            "modelUsage",
            json!({"sonnet":{"inputTokens":0,"outputTokens":0,
            "cacheReadInputTokens":0,"cacheCreationInputTokens":0,"thinkingTokens":"unknown"}}),
        ),
    ] {
        let mut receipt = zero_receipt("s");
        receipt[field] = value;
        let mut state = SessionCost::default();
        let (cost, events) = bill(&mut state, &receipt);
        assert_eq!(cost, None);
        assert_unreported_gap(&events);
        assert!(state.latest.is_none());
    }
}

#[test]
fn explicit_zero_model_counters_and_stream_metadata_stay_complete() {
    let mut value = zero_receipt("s");
    value["modelUsage"] = json!({"sonnet":{"inputTokens":0,"outputTokens":0,
        "cacheReadInputTokens":0,"cacheCreationInputTokens":0,"thinkingTokens":0}});
    value["usage"] = json!({"input_tokens":0,"output_tokens":0,
        "output_tokens_details":{"thinking_tokens":0},
        "cache_creation":{"ephemeral_5m_input_tokens":0,"ephemeral_1h_input_tokens":0}});
    let streamed = BTreeMap::from([("sonnet".into(), Usage::default())]);
    let mut state = SessionCost::default();
    let result = state.receipt(&value, &streamed);
    assert_eq!(result.cost_usd, Some(0.0));
    assert!(result.events.is_empty());
    assert!(state.latest.is_none());
}
