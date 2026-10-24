use super::*;
use aether_core::model::StreamEvent;

#[test]
fn native_delegation_stops_the_turn_instead_of_disappearing() {
    for method in ["item/started", "item/completed"] {
        let mut state = TurnState::new(turn(None, false));
        state.thread_id = Some("parent".into());
        let events = state.notification(
            method,
            &json!({
                "threadId": "parent",
                "item": {"type": "collabAgentToolCall", "tool": "spawnAgent", "id": "call"}
            }),
        );
        assert!(state.done);
        assert!(matches!(&events[..], [EngineEvent::Done(outcome)]
            if outcome.is_error && outcome.interrupted && !outcome.closing_accounting && outcome.error.as_ref().unwrap().contains("native delegation")));
        assert!(
            state
                .notification("item/agentMessage/delta", &json!({"delta":"late"}))
                .is_empty()
        );
    }
}

#[test]
fn another_threads_output_never_becomes_the_parents_output() {
    let mut state = TurnState::new(turn(None, false));
    state.thread_id = Some("parent".into());
    for method in [
        "item/agentMessage/delta",
        "thread/tokenUsage/updated",
        "turn/completed",
    ] {
        assert!(
            state
                .notification(method, &json!({"threadId":"child", "delta":"child text"}))
                .is_empty()
        );
    }
    assert!(!state.done);
}

#[test]
fn notifications_become_events() {
    let mut s = TurnState::new(turn(None, false));
    s.thread_id = Some("th".into());
    assert_eq!(
        s.notification("item/agentMessage/delta", &json!({"delta": "hel"})),
        vec![EngineEvent::Delta(StreamEvent::TextDelta {
            index: 0,
            text: "hel".into()
        })]
    );
    let started = json!({"item": {"type": "commandExecution", "id": "c1", "command": "ls", "cwd": "/w", "status": "inProgress"}});
    match &s.notification("item/started", &started)[..] {
        [
            EngineEvent::BackgroundTask { id, running: true },
            EngineEvent::Assistant { content, .. },
        ] => {
            assert_eq!(id, "c1");
            assert!(matches!(&content[0], ContentBlock::ToolUse { name, .. } if name == "bash"));
        }
        other => panic!("a started command is a live job, then a tool use: {other:?}"),
    }
    let done = json!({"item": {"type": "commandExecution", "id": "c1", "command": "ls", "aggregatedOutput": "a\n", "exitCode": 0, "status": "completed"}});
    assert_eq!(
        s.notification("item/completed", &done),
        vec![
            EngineEvent::ToolResult {
                tool_use_id: "c1".into(),
                content: "a\n\n[exit code 0]".into(),
                is_error: false
            },
            EngineEvent::BackgroundTask {
                id: "c1".into(),
                running: false
            }
        ]
    );
    // A command closed while its process still runs stays a live job.
    let running = json!({"item": {"type": "commandExecution", "id": "c2", "command": "sleep 99", "status": "inProgress"}});
    s.notification("item/started", &running);
    let closed = s.notification("item/completed", &running);
    assert!(
        matches!(&closed[..], [EngineEvent::ToolResult { tool_use_id, .. }] if tool_use_id == "c2"),
        "{closed:?}"
    );
    // A completion never reported as started carries its own tool-use.
    let mcp = json!({"item": {"type": "mcpToolCall", "id": "m1", "server": "aether", "tool": "complete_task", "arguments": {"summary": "ok"}, "result": {"content": [{"type": "text", "text": "done"}]}, "status": "completed"}});
    let events = s.notification("item/completed", &mcp);
    assert_eq!(events.len(), 2);
    assert!(matches!(&events[0], EngineEvent::Assistant { content, .. }
        if matches!(&content[0], ContentBlock::ToolUse { name, .. } if name == "mcp__aether__complete_task")));
    assert_eq!(
        events[1],
        EngineEvent::ToolResult {
            tool_use_id: "m1".into(),
            content: "done".into(),
            is_error: false
        }
    );
    // A fresh thread starts from zero, so the whole reported total is this
    // run's - and the reading itself is published for the next turn.
    let accounted = s.notification(
        "thread/tokenUsage/updated",
        &json!({"tokenUsage": {"last": {"inputTokens": 120, "cachedInputTokens": 100, "outputTokens": 7, "reasoningOutputTokens": 2, "totalTokens": 127}, "total": {"inputTokens": 500, "cachedInputTokens": 400, "outputTokens": 30, "reasoningOutputTokens": 5, "totalTokens": 530}}}),
    );
    let total = Usage {
        input_tokens: 100,
        output_tokens: 30,
        cache_read_tokens: 400,
        reasoning_tokens: Some(5),
        ..Default::default()
    };
    assert_eq!(
        accounted,
        vec![
            EngineEvent::SessionUsage {
                models: None,
                session: "th".into(),
                total,
            },
            EngineEvent::UsageDelta { usage: total },
            EngineEvent::ContextUsage { tokens: 120 },
        ]
    );
    let msg = json!({"item": {"type": "agentMessage", "id": "a1", "text": "All done.", "phase": "final_answer"}});
    match &s.notification("item/completed", &msg)[0] {
        EngineEvent::Assistant { content, usage, .. } => {
            assert_eq!(
                content,
                &vec![ContentBlock::Text {
                    text: "All done.".into()
                }]
            );
            // Billing rides the usage stream alone.
            assert_eq!(usage, &None);
        }
        other => panic!("{other:?}"),
    }
    let completed =
        json!({"turn": {"id": "t1", "status": "completed", "durationMs": 900, "items": []}});
    match &s.notification("turn/completed", &completed)[0] {
        EngineEvent::Done(o) => {
            assert!(!o.is_error);
            assert_eq!(o.text.as_deref(), Some("All done."));
            assert_eq!(o.usage.input_tokens, 100, "this run's own total");
            assert_eq!(o.session.as_deref(), Some("th"));
            assert_eq!(o.duration_ms, Some(900));
        }
        other => panic!("{other:?}"),
    }
    assert!(s.done);
}

#[test]
fn failures_and_retries_are_reported() {
    let mut s = TurnState::new(turn(None, false));
    assert!(matches!(
        &s.notification("error", &json!({"error": {"message": "rate limited"}, "willRetry": true}))[0],
        EngineEvent::Note(n) if n.contains("rate limited")
    ));
    assert!(
        s.notification(
            "error",
            &json!({"error": {"message": "quota"}, "willRetry": false})
        )
        .is_empty()
    );
    let ended = s.notification(
        "turn/completed",
        &json!({"turn": {"id": "t", "status": "failed", "items": []}}),
    );
    // This turn reported no usage at all, which is said before the result.
    match ended.last() {
        Some(EngineEvent::Done(o)) => {
            assert!(o.is_error);
            assert!(o.closing_accounting);
            assert_eq!(o.error.as_deref(), Some("quota"));
        }
        other => panic!("{other:?}"),
    }
}

#[test]
fn unexpected_native_interrupt_is_an_error_without_a_harness_boundary_flag() {
    for cancelling in [false, true] {
        let mut state = TurnState::new(turn(None, false));
        state.cancelling = cancelling;
        let events =
            state.notification("turn/completed", &json!({"turn":{"status":"interrupted"}}));
        let outcome = events
            .iter()
            .find_map(|event| match event {
                EngineEvent::Done(outcome) => Some(outcome),
                _ => None,
            })
            .unwrap();
        assert_eq!(outcome.is_error, !cancelling);
        assert!(!outcome.closing_accounting);
        assert!(!outcome.interrupted);
        if !cancelling {
            assert!(outcome.error.as_ref().unwrap().contains("unexpectedly"));
        }
    }
}

#[test]
fn requested_interrupt_keeps_native_exhaustion_errors() {
    let mut state = TurnState::new(turn(None, false));
    state.cancelling = true;
    let events = state.notification(
        "turn/completed",
        &json!({"turn": {
            "status":"interrupted", "error":{"message":"Weekly limit reached"}
        }}),
    );
    let outcome = events
        .iter()
        .find_map(|event| match event {
            EngineEvent::Done(outcome) => Some(outcome),
            _ => None,
        })
        .unwrap();
    assert!(outcome.is_error);
    assert_eq!(outcome.error.as_deref(), Some("Weekly limit reached"));
    assert!(!outcome.interrupted);
}

#[test]
fn recorded_rate_limits_retain_resets_through_normalization() {
    let fixture: Value = serde_json::from_str(include_str!(
        "../../../tests/fixtures/codex_rate_limits.json"
    ))
    .unwrap();
    for with_reset in [true, false] {
        let mut params = fixture["params"].clone();
        if !with_reset {
            for name in ["primary", "secondary"] {
                params["rateLimits"][name]
                    .as_object_mut()
                    .unwrap()
                    .remove("resetsAt");
            }
        }
        let mut state = TurnState::new(turn(None, false));
        let events = state.notification(fixture["method"].as_str().unwrap(), &params);
        let [EngineEvent::RateLimit { payload }] = &events[..] else {
            panic!("rate limit event");
        };
        let usage = aether_core::provider::ProviderUsage::from_engine_payload(
            payload,
            aether_core::time::now(),
        )
        .unwrap();
        assert_eq!(usage.windows.len(), 2);
        for (window, seconds) in usage.windows.iter().zip([2000000000, 2000500000]) {
            assert_eq!(
                window.quota.resets_at.map(|at| at.timestamp()),
                with_reset.then_some(seconds)
            );
        }
        assert_eq!(usage.windows[0].quota.used_percent, 42);
    }
}
