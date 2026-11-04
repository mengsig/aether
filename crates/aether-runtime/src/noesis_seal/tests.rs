use super::*;
use serde_json::json;

#[test]
fn every_spelling_of_a_sealed_tool_is_one_name() {
    assert_eq!(canonical_tool_name("noesis_ask"), "noesis_ask");
    assert_eq!(canonical_tool_name("noesis.ask"), "noesis_ask");
    assert_eq!(canonical_tool_name("mcp__noesis__noesis_ask"), "noesis_ask");
    assert_eq!(
        canonical_tool_name("mcp__srv__noesis.problem"),
        "noesis_problem"
    );
    assert_eq!(canonical_tool_name("aether__noesis_ask"), "noesis_ask");
    assert_eq!(canonical_tool_name("aether.noesis.ask"), "noesis_ask");
    assert_eq!(canonical_tool_name("mcp__aether.noesis_ask"), "noesis_ask");
    assert_eq!(canonical_tool_name("read_file"), "read_file");
    assert!(sealed_fields("mcp__x__noesis_ask").is_some());
    assert!(sealed_fields("noesis_asked").is_none());
}

#[test]
fn only_the_sealed_fields_are_replaced() {
    let mut input = json!({"answer_key": "B", "explanation": "why", "question": "q?"});
    assert!(seal_input("noesis_ask", &mut input));
    assert_eq!(
        input,
        json!({"answer_key": SEALED, "explanation": SEALED, "question": "q?"})
    );
    assert!(
        !seal_input("noesis_ask", &mut input),
        "sealing twice changes nothing"
    );
    let mut other = json!({"answer_key": "B"});
    assert!(!seal_input("write_file", &mut other));
    assert_eq!(other, json!({"answer_key": "B"}));
}

#[test]
fn a_sealed_view_borrows_an_unsealed_tools_input() {
    let input = json!({"solution": "42"});
    assert!(matches!(sealed_view("read_file", &input), Cow::Borrowed(_)));
    assert_eq!(
        sealed_view("noesis_problem", &input).as_ref(),
        &json!({"solution": SEALED})
    );
    assert_eq!(
        input,
        json!({"solution": "42"}),
        "the real input is untouched"
    );
}

#[test]
fn a_stream_withholds_sealed_input_and_noesis_thinking_only() {
    let sealed_input = DeltaKind::ToolInput {
        name: "mcp__n__noesis_ask".into(),
        partial_json: "{".into(),
        tool_use_id: None,
    };
    let plain_input = DeltaKind::ToolInput {
        name: "read_file".into(),
        partial_json: "{".into(),
        tool_use_id: None,
    };
    let thinking = DeltaKind::Thinking { text: "hm".into() };
    for seal in [StreamSeal::Ordinary, StreamSeal::Noesis] {
        assert!(!seal.admits(&sealed_input));
        assert!(seal.admits(&plain_input));
        assert!(seal.admits(&DeltaKind::Text { text: "t".into() }));
    }
    assert!(StreamSeal::Ordinary.admits(&thinking));
    assert!(!StreamSeal::Noesis.admits(&thinking));
}

#[test]
fn a_text_encoded_input_of_a_sealed_tool_is_sealed_whole() {
    let mut input = json!("{\"answer_key\":\"B\",\"question\":\"q?\"}");
    assert!(seal_input("noesis_ask", &mut input));
    assert_eq!(input, json!(SEALED));
    assert!(!seal_input("noesis_ask", &mut input));
    let raw = json!("{\"solution\":\"x\"}");
    let view = sealed_view("noesis_problem", &raw);
    assert_eq!(*view, json!(SEALED));
}

fn keyed_call(name: &str, input: Value) -> ToolCall {
    ToolCall {
        id: aether_core::id::ToolCallId::new(),
        provider_call_id: "p1".into(),
        name: name.into(),
        idempotency_key: Some(IdempotencyKey::derive(name, &input)),
        input,
        agent_id: AgentId::new(),
        task_id: None,
        invocation_id: None,
        resources: Vec::new(),
        requested_at: aether_core::time::now(),
    }
}

#[test]
fn a_sealed_calls_key_ignores_the_sealed_values_and_is_stable() {
    let mut first = keyed_call("noesis_ask", json!({"question": "q?", "answer_key": "B"}));
    let mut second = keyed_call("noesis_ask", json!({"question": "q?", "answer_key": "C"}));
    seal_call_key(&mut first);
    seal_call_key(&mut second);
    assert_eq!(first.idempotency_key, second.idempotency_key);
    let keyed = first.idempotency_key.clone();
    seal_input(&first.name, &mut first.input);
    seal_call_key(&mut first);
    assert_eq!(
        first.idempotency_key, keyed,
        "re-keying sealed input changes nothing"
    );

    let mut other = keyed_call(
        "noesis_ask",
        json!({"question": "other?", "answer_key": "B"}),
    );
    seal_call_key(&mut other);
    assert_ne!(
        other.idempotency_key, keyed,
        "visible fields still tell calls apart"
    );

    let mut plain = keyed_call("write_file", json!({"answer_key": "B"}));
    let before = plain.idempotency_key.clone();
    seal_call_key(&mut plain);
    assert_eq!(
        plain.idempotency_key, before,
        "an unsealed tool keeps its key"
    );
    let mut unkeyed = keyed_call("noesis_ask", json!({"answer_key": "B"}));
    unkeyed.idempotency_key = None;
    seal_call_key(&mut unkeyed);
    assert_eq!(
        unkeyed.idempotency_key, None,
        "a read-only call stays unkeyed"
    );
}
