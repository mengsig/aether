//! Stream delivery, errors, and native event decoding.

use super::*;
use crate::claude_code::user_message_line;
use serde_json::json;

#[test]
fn resumed_result_keeps_invocation_tokens_and_emits_baseline_before_done() {
    let mut state = ParseState::default().with_session_cost(
        Some("s".into()),
        Usage {
            cost_usd: 0.0595702,
            ..Usage::default()
        },
    );
    let result = json!({"type": "result", "subtype": "success", "session_id": "s",
        "total_cost_usd": 0.0712778,
        "usage": {"input_tokens": 6, "output_tokens": 89,
            "cache_read_input_tokens": 46168, "cache_creation_input_tokens": 393},
        "modelUsage": {"sonnet": {"inputTokens": 10, "outputTokens": 384,
            "cacheReadInputTokens": 78969, "cacheCreationInputTokens": 12906}}});
    let events = parse_line(&result.to_string(), &mut state);
    let [
        EngineEvent::SessionUsage { total, .. },
        EngineEvent::Done(outcome),
    ] = &events[..]
    else {
        panic!("unexpected result events: {events:?}")
    };
    assert_eq!(total.cost_usd, 0.0712778);
    assert_eq!(total.output_tokens, 384);
    assert!(outcome.closing_accounting);
    assert_eq!(outcome.usage.input_tokens, 6);
    assert_eq!(outcome.usage.output_tokens, 89);
    assert_eq!(outcome.usage.cache_read_tokens, 46168);
    assert_eq!(outcome.usage.cache_write_tokens, 393);
    assert!((outcome.cost_usd.unwrap() - 0.0117076).abs() < 1e-12);
}

#[test]
fn background_tasks_are_reported() {
    let mut st = ParseState::default();
    let started = r#"{"type":"system","subtype":"task_started","task_id":"t1","tool_use_id":"toolu_1","description":"sleep"}"#;
    assert_eq!(
        parse_line(started, &mut st),
        vec![EngineEvent::BackgroundTask {
            id: "t1".into(),
            running: true
        }]
    );
    let done =
        r#"{"type":"system","subtype":"task_notification","task_id":"t1","status":"completed"}"#;
    assert_eq!(
        parse_line(done, &mut st),
        vec![EngineEvent::BackgroundTask {
            id: "t1".into(),
            running: false
        }]
    );
}

#[test]
fn echoed_messages_acknowledge_steering_in_order() {
    let mut st = ParseState::default();
    st.expect_echo("first steer");
    st.expect_echo("second steer\n");
    // The prompt's own echo is not a steered message.
    let prompt = r#"{"type":"user","isReplay":true,"uuid":"u0","session_id":"s","message":{"role":"user","content":[{"type":"text","text":"do the task"}]}}"#;
    assert_eq!(parse_line(prompt, &mut st), vec![]);
    // Tool results are user messages too, never acknowledgements.
    let result = r#"{"type":"user","message":{"role":"user","content":[{"type":"tool_result","tool_use_id":"toolu_1","content":"ok"}]}}"#;
    assert!(matches!(
        parse_line(result, &mut st).as_slice(),
        [EngineEvent::ToolResult { .. }]
    ));
    // Out of order never matches: the oldest outstanding message is first.
    let second = r#"{"type":"user","isReplay":true,"message":{"role":"user","content":[{"type":"text","text":"second steer"}]}}"#;
    assert_eq!(parse_line(second, &mut st), vec![]);
    let first = r#"{"type":"user","isReplay":true,"message":{"role":"user","content":[{"type":"text","text":"first steer"}]}}"#;
    assert_eq!(parse_line(first, &mut st), vec![EngineEvent::InputAccepted]);
    assert_eq!(
        parse_line(second, &mut st),
        vec![EngineEvent::InputAccepted]
    );
    assert_eq!(parse_line(second, &mut st), vec![]);
}

fn replay_of(text: &str) -> String {
    serde_json::json!({"type":"user","isReplay":true,
        "message":{"role":"user","content":[{"type":"text","text":text}]}})
    .to_string()
}

fn done_texts(events: &[EngineEvent]) -> Vec<Option<String>> {
    events
        .iter()
        .filter_map(|e| match e {
            EngineEvent::Done(o) => Some(o.text.clone()),
            _ => None,
        })
        .collect()
}

/// Captured from claude 2.1.267 resuming a session whose background shell
/// died with the previous process (trimmed to the fields parsed here).
#[test]
fn a_resumed_session_answers_the_prompt_not_the_orphaned_shell_notice() {
    let prompt = "Reply with exactly: pong.";
    let lines = [
        r#"{"type":"system","subtype":"task_notification","task_id":"bigoplw09","status":"stopped","summary":"Background shell command didn't finish before the previous session ended","session_id":"s"}"#.to_owned(),
        r#"{"type":"system","subtype":"init","session_id":"s","model":"claude-haiku-4-5"}"#.to_owned(),
        r#"{"type":"result","subtype":"success","is_error":false,"duration_api_ms":0,"num_turns":0,"result":"","stop_reason":null,"session_id":"s","total_cost_usd":0,"usage":{"input_tokens":0,"output_tokens":0},"origin":{"kind":"task-notification"}}"#.to_owned(),
        r#"{"type":"system","subtype":"init","session_id":"s","model":"claude-haiku-4-5"}"#.to_owned(),
        replay_of(prompt),
        r#"{"type":"assistant","message":{"id":"msg_1","model":"claude-haiku-4-5","content":[{"type":"text","text":"pong."}]}}"#.to_owned(),
        r#"{"type":"result","subtype":"success","is_error":false,"duration_api_ms":1790,"num_turns":1,"result":"pong.","stop_reason":"end_turn","session_id":"s","total_cost_usd":0.0029}"#.to_owned(),
    ];
    let mut st = ParseState::awaiting_prompt(prompt);
    let events: Vec<EngineEvent> = lines.iter().flat_map(|l| parse_line(l, &mut st)).collect();
    assert_eq!(done_texts(&events), vec![Some("pong.".to_owned())]);
    assert!(
        !events.contains(&EngineEvent::InputAccepted),
        "the prompt's echo is not a steered message: {events:?}"
    );
    assert!(
        events
            .iter()
            .any(|e| matches!(e, EngineEvent::Assistant { .. }))
    );
}

/// Only a task notice that ran no model is skipped: an answer, an error, any
/// other empty result or anything after the echo still ends the turn, so it cannot hang.
#[test]
fn only_queued_work_before_the_prompt_echo_is_skipped() {
    let answered =
        r#"{"type":"result","subtype":"success","is_error":false,"num_turns":1,"result":"done"}"#;
    let notice_answered = r#"{"type":"result","subtype":"success","is_error":false,"num_turns":1,"result":"noted","origin":{"kind":"task-notification"}}"#;
    let failed =
        r#"{"type":"result","subtype":"error_during_execution","is_error":true,"num_turns":0}"#;
    let empty =
        r#"{"type":"result","subtype":"success","is_error":false,"num_turns":0,"result":""}"#;
    let notice = r#"{"type":"result","subtype":"success","is_error":false,"num_turns":0,"result":"","origin":{"kind":"task-notification"}}"#;

    let mut st = ParseState::awaiting_prompt("do it");
    assert_eq!(
        done_texts(&parse_line(answered, &mut st)),
        vec![Some("done".into())]
    );
    let mut st = ParseState::awaiting_prompt("do it");
    assert_eq!(
        done_texts(&parse_line(notice_answered, &mut st)),
        vec![Some("noted".into())]
    );
    let mut st = ParseState::awaiting_prompt("do it");
    assert_eq!(done_texts(&parse_line(failed, &mut st)).len(), 1);
    // Unknown queued work is not trusted to be followed by the prompt's result.
    let mut st = ParseState::awaiting_prompt("do it");
    assert_eq!(
        done_texts(&parse_line(empty, &mut st)),
        vec![Some(String::new())]
    );
    let mut st = ParseState::awaiting_prompt("do it");
    assert_eq!(
        done_texts(&parse_line(notice, &mut st)),
        Vec::<Option<String>>::new()
    );

    // Once read, the prompt's own empty result is the engine's to judge.
    st.expect_echo("do it");
    assert_eq!(parse_line(&replay_of("do it"), &mut st), vec![]);
    assert_eq!(
        done_texts(&parse_line(notice, &mut st)),
        vec![Some(String::new())]
    );
    assert_eq!(
        parse_line(&replay_of("do it"), &mut st),
        vec![EngineEvent::InputAccepted]
    );

    // A blank prompt is never echoed, so nothing waits on it.
    let mut st = ParseState::awaiting_prompt("  ");
    assert_eq!(done_texts(&parse_line(empty, &mut st)).len(), 1);
}

#[test]
fn user_message_lines_are_stream_json() {
    let line = user_message_line("hi\nthere", &[]);
    assert!(line.ends_with('\n'));
    let v: serde_json::Value = serde_json::from_str(line.trim()).expect("json");
    assert_eq!(v["type"], "user");
    assert_eq!(v["message"]["content"][0]["text"], "hi\nthere");
}

/// A message without images is, byte for byte, the line it was before a
/// message could carry any.
#[test]
fn a_text_only_user_message_line_is_unchanged() {
    assert_eq!(
        user_message_line("hi", &[]),
        "{\"message\":{\"content\":[{\"text\":\"hi\",\"type\":\"text\"}],\"role\":\"user\"},\"type\":\"user\"}\n"
    );
}

#[test]
fn a_user_message_line_carries_its_images_after_the_text() {
    use base64::Engine as _;
    let dir = tempfile::tempdir().expect("tempdir");
    let path = dir.path().join("shot.png");
    let bytes = b"\x89PNG\r\n\x1a\npixels";
    std::fs::write(&path, bytes).expect("write image");
    let stored = crate::EngineImage {
        path,
        media_type: "image/png".into(),
    };
    let gone = crate::EngineImage {
        path: dir.path().join("removed.png"),
        media_type: "image/png".into(),
    };

    let line = user_message_line("what is this?", &[stored, gone]);
    let v: serde_json::Value = serde_json::from_str(line.trim()).expect("json");
    assert_eq!(
        v["message"]["content"],
        json!([
            {"type": "text", "text": "what is this?"},
            {"type": "image", "source": {
                "type": "base64",
                "media_type": "image/png",
                "data": base64::engine::general_purpose::STANDARD.encode(bytes),
            }},
        ]),
        "the text, then the readable image; the removed one is left out"
    );
}

/// The CLI echoes every user line back (`--replay-user-messages`), and the
/// reader refuses a longer line: one message's worth of images must fit it.
#[test]
fn a_line_with_one_messages_worth_of_images_fits_the_echo_the_reader_accepts() {
    use aether_core::attachment::{MAX_IMAGE_BYTES, MAX_MESSAGE_IMAGE_BYTES};
    let dir = tempfile::tempdir().expect("tempdir");
    let image = |name: &str, len: usize| {
        let path = dir.path().join(name);
        std::fs::write(&path, vec![0u8; len]).expect("write image");
        crate::EngineImage {
            path,
            media_type: "image/png".into(),
        }
    };
    let full = image("full.png", MAX_IMAGE_BYTES);
    let rest = image("rest.png", MAX_MESSAGE_IMAGE_BYTES - 2 * MAX_IMAGE_BYTES);
    let one_message = [full.clone(), full.clone(), rest];
    let line = user_message_line("look", &one_message);
    assert!(
        line.len() < crate::output::MAX_LINE_BYTES,
        "{} bytes",
        line.len()
    );
    // Two legal messages folded into one line would not: the runtime splits them.
    let folded = [full.clone(), full.clone(), full.clone(), full];
    assert!(user_message_line("look", &folded).len() > crate::output::MAX_LINE_BYTES);
}

/// The CLI echoes a steered message with its images; that echo is still the
/// acknowledgement of its text.
#[test]
fn an_echo_with_images_acknowledges_its_message() {
    let mut st = ParseState::default();
    st.expect_echo("look at this");
    let echo = json!({"type": "user", "isReplay": true, "message": {"role": "user", "content": [
        {"type": "text", "text": "look at this"},
        {"type": "image", "source": {"type": "base64", "media_type": "image/png", "data": "AAAA"}},
    ]}})
    .to_string();
    assert_eq!(parse_line(&echo, &mut st), vec![EngineEvent::InputAccepted]);
}

#[test]
fn weekly_usage_error_preserves_provider_reset_wording() {
    let message = "You've hit your limit · resets Sep 18 at 2pm";
    let events = parse_line(
        &json!({"type": "result", "subtype": "error_during_execution",
        "is_error": true, "errors": [message]})
        .to_string(),
        &mut ParseState::default(),
    );
    let Some(EngineEvent::Done(outcome)) = events.last() else {
        panic!("expected result")
    };
    assert!(outcome.is_error);
    assert_eq!(outcome.error.as_deref(), Some(message));
    assert_eq!(
        aether_core::model::provider_exhaustion(outcome.error.as_deref().unwrap()),
        Some(aether_core::model::ModelErrorClass::UsageLimit)
    );
}

#[test]
fn result_errors_preserve_startup_diagnostics() {
    for result in [None, Some("Unable to start")] {
        let events = parse_line(
            &json!({
                "type": "result", "subtype": "error_during_execution",
                "is_error": true, "result": result,
                "errors": [null, "", "socat not installed"]
            })
            .to_string(),
            &mut ParseState::default(),
        );
        let Some(EngineEvent::Done(outcome)) = events.last() else {
            panic!("expected result")
        };
        let error = outcome.error.as_deref().expect("failure diagnostic");
        assert!(error.contains("socat not installed"), "{error}");
        assert_eq!(outcome.text, None, "failure prose is not assistant text");
        if let Some(result) = result {
            assert!(error.contains(result), "{error}");
        }
    }
}

#[test]
fn result_errors_keep_fallback_and_success_semantics() {
    for (subtype, expected) in [
        ("error_during_execution", Some("error_during_execution")),
        ("success", None),
    ] {
        let events = parse_line(
            &json!({"type": "result", "subtype": subtype,
                "errors": [null, {}, 17, "", "  "]})
            .to_string(),
            &mut ParseState::default(),
        );
        let Some(EngineEvent::Done(outcome)) = events.last() else {
            panic!("expected result")
        };
        assert_eq!(outcome.error.as_deref(), expected);
        assert_eq!(outcome.is_error, expected.is_some());
    }
}

#[test]
fn parses_observed_tool_stream() {
    let mut st = ParseState::default();
    let init = r#"{"type":"system","subtype":"init","cwd":"/x","session_id":"0b1a","tools":["Bash","mcp__aether__echo"],"mcp_servers":[{"name":"aether","status":"connected"}],"model":"claude-sonnet-5","permissionMode":"default"}"#;
    assert_eq!(
        parse_line(init, &mut st),
        vec![EngineEvent::Session {
            id: "0b1a".into(),
            model: Some("claude-sonnet-5".into()),
            tools: vec!["Bash".into(), "mcp__aether__echo".into()]
        }]
    );
    let start = r#"{"type":"stream_event","event":{"type":"content_block_start","index":1,"content_block":{"type":"tool_use","id":"toolu_1","name":"Bash","input":{}}}}"#;
    let ev = parse_line(start, &mut st);
    assert!(matches!(
        ev[0],
        EngineEvent::Delta(StreamEvent::BlockStart { index: 1, .. })
    ));
    let delta = r#"{"type":"stream_event","event":{"type":"content_block_delta","index":1,"delta":{"type":"input_json_delta","partial_json":"{\"command\""}}}"#;
    assert!(matches!(
        parse_line(delta, &mut st)[0],
        EngineEvent::Delta(StreamEvent::ToolInputDelta { index: 1, .. })
    ));
    let text = r#"{"type":"stream_event","event":{"type":"content_block_delta","index":0,"delta":{"type":"text_delta","text":"a"}}}"#;
    assert_eq!(
        parse_line(text, &mut st),
        vec![EngineEvent::Delta(StreamEvent::TextDelta {
            index: 0,
            text: "a".into()
        })]
    );
}

#[test]
fn parses_observed_assistant_and_tool_results() {
    let mut st = ParseState::default();
    let assistant = r#"{"type":"assistant","message":{"model":"claude-sonnet-5","content":[{"type":"text","text":"I'll run it."},{"type":"tool_use","id":"toolu_1","name":"Bash","input":{"command":"echo hi"}}],"usage":{"input_tokens":2,"cache_creation_input_tokens":12495,"cache_read_input_tokens":18534,"output_tokens":8}}}"#;
    let events = parse_line(assistant, &mut st);
    match &events[0] {
        EngineEvent::Assistant {
            content,
            usage,
            model,
        } => {
            assert_eq!(content.len(), 2);
            // Usage on the message would bill it once per content block.
            assert_eq!(usage, &None);
            assert_eq!(model.as_deref(), Some("claude-sonnet-5"));
        }
        other => panic!("{other:?}"),
    }
    assert!(
        events.iter().any(|e| matches!(e,
            EngineEvent::UsageRequest { request } if request.delta().cache_read_tokens == 18534)),
        "{events:?}"
    );
    let user = r#"{"type":"user","message":{"role":"user","content":[{"tool_use_id":"toolu_1","type":"tool_result","content":"hi\n","is_error":false}]}}"#;
    assert_eq!(
        parse_line(user, &mut st),
        vec![EngineEvent::ToolResult {
            tool_use_id: "toolu_1".into(),
            content: "hi\n".into(),
            is_error: false
        }]
    );
    let user_list = r#"{"type":"user","message":{"content":[{"type":"tool_result","tool_use_id":"t2","content":[{"type":"text","text":"echo: ping"}]}]}}"#;
    assert_eq!(
        parse_line(user_list, &mut st)[0],
        EngineEvent::ToolResult {
            tool_use_id: "t2".into(),
            content: "echo: ping".into(),
            is_error: false
        }
    );
}

#[test]
fn parses_observed_closing_result() {
    let mut st = ParseState::default();
    let result = r#"{"type":"result","subtype":"success","is_error":false,"duration_api_ms":3591,"stop_reason":"end_turn","session_id":"0b1a","total_cost_usd":0.0547,"usage":{"input_tokens":2,"cache_creation_input_tokens":12495,"cache_read_input_tokens":18534,"output_tokens":8},"result":"aether engine ok","num_turns":1}"#;
    let result_events = parse_line(result, &mut st);
    assert!(
        matches!(&result_events[0], EngineEvent::SessionUsage { session, total, .. }
        if session == "0b1a" && total.cost_usd == 0.0547)
    );
    assert_eq!(result_events.len(), 2);
    match &result_events[1] {
        EngineEvent::Done(o) => {
            assert!(!o.is_error);
            assert_eq!(o.cost_usd, Some(0.0547));
            assert_eq!(o.usage.output_tokens, 8);
            // The outcome carries the price; its usage must not introduce a gap.
            assert!(
                o.usage.cost_complete && o.usage.cost_usd == 0.0,
                "usage carries tokens only: {:?}",
                o.usage
            );
            assert_eq!(o.session.as_deref(), Some("0b1a"));
            assert_eq!(o.text.as_deref(), Some("aether engine ok"));
        }
        other => panic!("{other:?}"),
    }
}

#[test]
fn parses_errors_unknown_lines_and_rate_limits() {
    let mut st = ParseState::default();
    let failed = r#"{"type":"result","subtype":"error_max_turns","is_error":true,"session_id":"s","usage":{}}"#;
    let events = parse_line(failed, &mut st);
    let [EngineEvent::Done(outcome)] = &events[..] else {
        panic!("expected result without a gap: {events:?}");
    };
    assert!(outcome.is_error && outcome.error.as_deref() == Some("error_max_turns"));
    assert_eq!(outcome.cost_usd, None);
    assert!(matches!(
        parse_line("garbage", &mut st)[0],
        EngineEvent::Note(_)
    ));
    assert!(matches!(
        parse_line(
            r#"{"type":"rate_limit_event","rate_limit_info":{"status":"allowed"}}"#,
            &mut st
        )[0],
        EngineEvent::RateLimit { .. }
    ));
}
