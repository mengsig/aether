use super::*;
use serde_json::json;

fn closing(dollars: Value, usage: Value) -> Value {
    json!({"type":"result","subtype":"success","session_id":"s",
        "total_cost_usd":dollars,"modelUsage":{},"usage":usage})
}

fn assert_gap(events: &[EngineEvent], expected: bool) {
    let deltas: Vec<_> = events
        .iter()
        .filter_map(|event| match event {
            EngineEvent::UsageDelta { usage } => Some(usage),
            _ => None,
        })
        .collect();
    assert_eq!(deltas.len(), usize::from(expected));
    for usage in deltas {
        assert_eq!(usage.total_tokens(), 0);
        assert_eq!(usage.cost_usd, 0.0);
        assert!(!usage.cost_complete);
        assert_eq!(
            usage.cost_gap,
            Some(aether_core::model::CostGap::Unreported)
        );
    }
    assert!(matches!(events.last(), Some(EngineEvent::Done(done))
        if done.cost_usd.is_none() && done.cost_usage.is_none()));
}

#[test]
fn unusable_receipts_without_spend_evidence_emit_no_gap() {
    for dollars in [Value::Null, json!(-1.0), json!(0.0), json!(0.25)] {
        for usage in [
            Value::Null,
            json!({}),
            json!({"input_tokens":"unknown"}),
            json!(0),
        ] {
            let mut state =
                ParseState::default().with_session_cost(Some("s".into()), Usage::default());
            let metadata = json!({"type":"assistant","message":{"model":"haiku","usage":{}}});
            let events = parse_line(&metadata.to_string(), &mut state);
            assert!(
                !events
                    .iter()
                    .any(|event| matches!(event, EngineEvent::UsageRequest { .. }))
            );
            assert!(!state.streamed_usage_seen);
            let value = closing(dollars.clone(), usage);
            assert_gap(&parse_line(&value.to_string(), &mut state), false);
        }
    }
}

#[test]
fn per_turn_usage_is_evidence_even_without_streamed_tokens() {
    for usage in [
        json!({"input_tokens":1}),
        json!({"output_tokens":1}),
        json!({"cache_read_input_tokens":1}),
        json!({"cache_creation_input_tokens":1}),
        json!({"output_tokens_details":{"thinking_tokens":1}}),
    ] {
        for dollars in [Value::Null, json!(-1.0), json!(0.0), json!(0.25)] {
            let mut state =
                ParseState::default().with_session_cost(Some("s".into()), Usage::default());
            let value = closing(dollars, usage.clone());
            assert_gap(&parse_line(&value.to_string(), &mut state), true);
        }
    }
}

#[test]
fn model_less_stream_evidence_survives_message_eviction() {
    for usage in [
        json!({"input_tokens":100,"output_tokens":20}),
        json!({"cache_read_input_tokens":1}),
        json!({"cache_creation_input_tokens":1}),
        json!({"output_tokens_details":{"thinking_tokens":1}}),
    ] {
        let mut state = ParseState::default();
        let delta = json!({"type":"stream_event","event":{"type":"message_delta","usage":usage}});
        let events = parse_line(&delta.to_string(), &mut state);
        assert!(
            events
                .iter()
                .any(|event| matches!(event, EngineEvent::UsageRequest { .. }))
        );
        for id in 0..=MAX_TRACKED_MESSAGES {
            let line = json!({"type":"stream_event","event":{"type":"message_start",
                "message":{"id":id.to_string(),"usage":{}}}});
            parse_line(&line.to_string(), &mut state);
        }
        assert!(state.streamed_models.is_empty());
        assert!(
            state
                .message_usage
                .values()
                .all(|usage| usage.total_tokens() == 0 && usage.reasoning_tokens.unwrap_or(0) == 0)
        );
        let value = closing(json!(0.0), json!({"input_tokens":0,"output_tokens":0}));
        assert_gap(&parse_line(&value.to_string(), &mut state), true);
    }
}

/// The closing receipt Claude Code 2.1.299 wrote when a local server refused its request.
fn rejected(status: Value) -> Value {
    json!({"type":"result","subtype":"success","is_error":true,"session_id":"s",
        "result":"API Error: refused","api_error_status":status,"terminal_reason":"api_error",
        "total_cost_usd":0,"modelUsage":{},"usage":{"output_tokens_details":{"thinking_tokens":0},
            "input_tokens":0,"cache_creation_input_tokens":0,"cache_read_input_tokens":0,
            "output_tokens":0,"server_tool_use":{"web_search_requests":0,"web_fetch_requests":0},
            "service_tier":"standard","cache_creation":{"ephemeral_1h_input_tokens":0,
                "ephemeral_5m_input_tokens":0},"inference_geo":"","iterations":[],
            "speed":"standard","fallback_credit":null}})
}

fn retry(status: Value) -> String {
    json!({"type":"system","subtype":"api_retry","attempt":1,"max_retries":10,
        "retry_delay_ms":600,"error_status":status,"error":"unknown","session_id":"s"})
    .to_string()
}

fn receipt_cost(events: &[EngineEvent]) -> Option<f64> {
    match events.last() {
        Some(EngineEvent::Done(done)) => done.cost_usd,
        other => panic!("a receipt must end in Done, got {other:?}"),
    }
}

#[test]
fn only_an_unbilled_rejection_proves_a_zeroed_receipt() {
    for (status, proven) in [
        (json!(400), true),
        (json!(401), true),
        (json!(403), true),
        (json!(404), true),
        (json!(413), true),
        (json!(429), false),
        (json!(500), false),
        (json!(529), false),
        (json!("400"), false),
        (Value::Null, false),
    ] {
        let events = parse_line(
            &rejected(status.clone()).to_string(),
            &mut ParseState::default(),
        );
        assert_eq!(receipt_cost(&events), proven.then_some(0.0), "{status}");
        let noted = events
            .iter()
            .any(|event| matches!(event, EngineEvent::ProtocolNote(_)));
        assert_eq!(noted, !proven, "{status}");
        assert!(
            !events
                .iter()
                .any(|event| matches!(event, EngineEvent::UsageDelta { .. }))
        );
    }
    for flag in [json!(false), Value::Null] {
        let mut unflagged = rejected(json!(400));
        unflagged["is_error"] = flag;
        unflagged["subtype"] = json!("error_during_execution");
        let events = parse_line(&unflagged.to_string(), &mut ParseState::default());
        assert_eq!(receipt_cost(&events), None);
    }
}

#[test]
fn earlier_usage_in_the_process_outweighs_a_later_rejection() {
    let mut state = ParseState::default();
    let counted = json!({"type":"result","subtype":"success","session_id":"s",
        "total_cost_usd":0.0,"modelUsage":{},"usage":{"input_tokens":100,"output_tokens":20}});
    assert_gap(&parse_line(&counted.to_string(), &mut state), true);
    assert!(!state.streamed_usage_seen);
    assert_gap(
        &parse_line(&rejected(json!(400)).to_string(), &mut state),
        true,
    );
    let mut state = ParseState::default();
    let delta = json!({"type":"stream_event","event":{"type":"message_delta",
        "usage":{"input_tokens":1}}});
    parse_line(&delta.to_string(), &mut state);
    assert!(state.streamed_usage_seen);
    assert_gap(
        &parse_line(&rejected(json!(400)).to_string(), &mut state),
        true,
    );
}

#[test]
fn a_retry_proves_nothing_unless_the_api_refused_the_attempt_before_it() {
    for status in [
        Value::Null,
        json!(429),
        json!(500),
        json!(529),
        json!("401"),
    ] {
        let mut state = ParseState::default();
        assert!(parse_line(&retry(status.clone()), &mut state).is_empty());
        let events = parse_line(&rejected(json!(400)).to_string(), &mut state);
        assert_eq!(receipt_cost(&events), None, "{status}");
        assert_gap(&events, false);
    }
    let mut state = ParseState::default();
    for _ in 0..3 {
        parse_line(&retry(json!(401)), &mut state);
    }
    let events = parse_line(&rejected(json!(401)).to_string(), &mut state);
    assert_eq!(receipt_cost(&events), Some(0.0));
}

#[test]
fn an_earlier_receipt_proves_nothing_for_a_later_rejection() {
    let dollars = json!({"type":"result","subtype":"success","session_id":"s",
        "total_cost_usd":0.05,"modelUsage":{},"usage":{}});
    let models = json!({"type":"result","subtype":"success","session_id":"s",
        "total_cost_usd":0.05,"modelUsage":{"haiku":{"inputTokens":100,"outputTokens":20,
            "cacheReadInputTokens":0,"cacheCreationInputTokens":0}},"usage":{}});
    for earlier in [dollars, models, rejected(json!(400))] {
        let mut state = ParseState::default();
        parse_line(&earlier.to_string(), &mut state);
        let events = parse_line(&rejected(json!(400)).to_string(), &mut state);
        assert_eq!(receipt_cost(&events), None, "{earlier}");
    }
}

#[test]
fn a_rejection_needs_every_counter_present_and_zero() {
    let mut cases: Vec<(&str, Value)> = vec![
        ("/usage", Value::Null),
        ("/usage", json!([])),
        ("/usage", json!({"input_tokens":0})),
        ("/usage/input_tokens", json!("unknown")),
        ("/usage/cache_read_input_tokens", json!("0")),
        ("/usage/cache_read_input_tokens", json!({})),
        ("/usage/cache_read_input_tokens", json!([])),
        ("/usage/server_tool_use/web_search_requests", Value::Null),
        (
            "/usage/output_tokens_details/thinking_tokens",
            json!("unknown"),
        ),
        ("/usage/output_tokens_details/thinking_tokens", json!(5)),
        ("/usage/server_tool_use/web_search_requests", json!(1)),
        ("/usage/iterations", json!([{"input_tokens":0}])),
        ("/usage/input_tokens", json!(-0.0)),
        ("/total_cost_usd", json!("0")),
        ("/modelUsage", Value::Null),
        ("/modelUsage", json!([])),
    ];
    cases.push((
        "/modelUsage",
        json!({"haiku":{"inputTokens":0,"outputTokens":0,"cacheReadInputTokens":0,
            "cacheCreationInputTokens":0,"thinkingTokens":"unknown"}}),
    ));
    for (pointer, replacement) in cases {
        let mut value = rejected(json!(400));
        *value.pointer_mut(pointer).expect("the field exists") = replacement.clone();
        let events = parse_line(&value.to_string(), &mut ParseState::default());
        assert_eq!(receipt_cost(&events), None, "{pointer} = {replacement}");
    }
}

#[test]
fn a_skipped_queued_receipt_proves_nothing_for_a_later_rejection() {
    let mut state = ParseState::awaiting_prompt("new prompt");
    let queued = json!({"type":"result","subtype":"success","is_error":false,"session_id":"s",
        "num_turns":0,"origin":{"kind":"task-notification"},"total_cost_usd":0.05,
        "modelUsage":{},"usage":{"input_tokens":0,"output_tokens":0}});
    let events = parse_line(&queued.to_string(), &mut state);
    assert!(matches!(&events[..], [EngineEvent::Note(_)]), "{events:?}");
    let events = parse_line(&rejected(json!(400)).to_string(), &mut state);
    assert_eq!(receipt_cost(&events), None);
}

#[test]
fn any_retry_after_a_proven_zero_withdraws_it() {
    for status in [json!(401), Value::Null] {
        let mut state = ParseState::default();
        let events = parse_line(&rejected(json!(400)).to_string(), &mut state);
        assert_eq!(receipt_cost(&events), Some(0.0));
        let events = parse_line(&retry(status.clone()), &mut state);
        assert!(
            matches!(&events[..], [EngineEvent::UsageDelta { usage }]
                if usage.cost_gap == Some(aether_core::model::CostGap::Unreported)
                    && usage.total_tokens() == 0),
            "{status}: {events:?}"
        );
        assert!(parse_line(&retry(status), &mut state).is_empty());
    }
}

fn output_records() -> [Value; 4] {
    let api_error = json!({"type":"assistant","session_id":"s","error":"unknown",
        "is_api_error_message":true,"api_error_status":400,"message":{"id":"e",
            "model":"<synthetic>","role":"assistant","usage":{"input_tokens":0,"output_tokens":0},
            "content":[{"type":"text","text":"API Error: 400 refused"}]}});
    let mut answer = api_error.clone();
    answer["message"]["model"] = json!("claude-haiku-5-5");
    answer["message"].as_object_mut().unwrap().remove("usage");
    let mut unflagged = api_error.clone();
    unflagged
        .as_object_mut()
        .unwrap()
        .remove("is_api_error_message");
    let started = json!({"type":"stream_event","session_id":"s","event":{"type":"message_start",
        "message":{"id":"m","model":"claude-haiku-5-5","usage":{"input_tokens":0,"output_tokens":0}}}});
    [api_error, answer, unflagged, started]
}

#[test]
fn model_output_before_a_rejection_rules_out_the_proof() {
    let [api_error, answer, unflagged, started] = output_records();
    for (before, cost) in [
        (&api_error, Some(0.0)),
        (&answer, None),
        (&unflagged, None),
        (&started, None),
    ] {
        let mut state = ParseState::default();
        parse_line(&before.to_string(), &mut state);
        let events = parse_line(&rejected(json!(400)).to_string(), &mut state);
        assert_eq!(receipt_cost(&events), cost, "{before}");
    }
}

#[test]
fn any_output_after_a_proven_zero_withdraws_it() {
    for after in output_records() {
        let mut state = ParseState::default();
        let events = parse_line(&rejected(json!(400)).to_string(), &mut state);
        assert_eq!(receipt_cost(&events), Some(0.0));
        let events = parse_line(&after.to_string(), &mut state);
        assert!(
            matches!(events.first(), Some(EngineEvent::UsageDelta { usage })
                if usage.cost_gap == Some(aether_core::model::CostGap::Unreported)
                    && usage.total_tokens() == 0),
            "{after}: {events:?}"
        );
        let again = parse_line(&after.to_string(), &mut state);
        assert!(
            !again
                .iter()
                .any(|event| matches!(event, EngineEvent::UsageDelta { .. })),
            "{after}: {again:?}"
        );
    }
}

#[test]
fn a_request_status_after_a_proven_zero_withdraws_it() {
    let requesting = json!({"type":"system","subtype":"status","status":"requesting",
        "session_id":"s"})
    .to_string();
    let idle =
        json!({"type":"system","subtype":"status","status":null,"session_id":"s"}).to_string();
    let mut state = ParseState::default();
    assert!(parse_line(&requesting, &mut state).is_empty());
    let events = parse_line(&rejected(json!(400)).to_string(), &mut state);
    assert_eq!(
        receipt_cost(&events),
        Some(0.0),
        "the first request is the refused one"
    );
    assert!(parse_line(&idle, &mut state).is_empty());
    let events = parse_line(&requesting, &mut state);
    assert!(
        matches!(&events[..], [EngineEvent::UsageDelta { usage }]
            if usage.cost_gap == Some(aether_core::model::CostGap::Unreported)
                && usage.total_tokens() == 0),
        "{events:?}"
    );
    assert!(parse_line(&requesting, &mut state).is_empty());
}
