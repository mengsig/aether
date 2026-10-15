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
