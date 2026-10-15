//! Native token accounting and bounded deduplication.

use super::*;
use serde_json::json;

#[test]
fn dated_model_names_share_receipt_coverage_on_fresh_and_resumed_sessions() {
    let short = "claude-haiku-4-5";
    let dated = "claude-haiku-4-5-20251001";
    for (streamed, reported) in [(dated, short), (short, dated)] {
        for resumed in [false, true] {
            let mut state = ParseState::default();
            if resumed {
                let before = Usage {
                    input_tokens: 100,
                    output_tokens: 10,
                    cost_usd: 8.0,
                    ..Usage::default()
                };
                state = state.with_session_cost(Some("s".into()), before);
                state.session_cost =
                    state
                        .session_cost
                        .with_models(Some(std::collections::BTreeMap::from([(
                            dated.into(),
                            before,
                        )])));
            }
            let message = json!({"type":"assistant","message":{"id":"m", "model":streamed,
                "content":[{"type":"text","text":"Done."}],
                "usage":{"input_tokens":100,"output_tokens":10}}});
            parse_line(&message.to_string(), &mut state);
            let factor = if resumed { 2 } else { 1 };
            let result = json!({"type":"result","subtype":"success","session_id":"s",
                "total_cost_usd":10.0,"modelUsage":{reported:{"inputTokens":100 * factor,
                    "outputTokens":10 * factor,"cacheReadInputTokens":0,"cacheCreationInputTokens":0}}});
            let events = parse_line(&result.to_string(), &mut state);
            let Some(EngineEvent::Done(outcome)) = events.last() else {
                panic!("missing result: {events:?}");
            };
            assert_eq!(outcome.cost_usd, Some(if resumed { 2.0 } else { 10.0 }));
            let covered = outcome.cost_usage.as_ref().expect("same canonical model");
            assert_eq!((covered.input_tokens, covered.output_tokens), (100, 10));
            assert!(
                matches!(events.first(), Some(EngineEvent::SessionUsage { models: Some(models), .. })
                if models.contains_key(short) && models.len() == 1)
            );
        }
    }
}

#[test]
fn content_blocks_share_usage_and_final_stream_output_is_counted() {
    let mut state = ParseState::default();
    let mut total = Usage::default();
    let mut content = Vec::new();
    let mut contexts = Vec::new();
    let start = json!({"type":"stream_event","event":{"type":"message_start",
        "message":{"id":"msg_a","usage":{"input_tokens":2,
            "cache_creation_input_tokens":100,"cache_read_input_tokens":200,"output_tokens":1}}}});
    let block = |id: &str, content: Value| {
        json!({"type":"assistant","message":{
        "id":id,"model":"claude-sonnet-5","content":[content],
        "usage":{"input_tokens":2,"cache_creation_input_tokens":100,
            "cache_read_input_tokens":200,"output_tokens":1}}})
    };
    let lines = [
        start,
        block(
            "msg_a",
            json!({"type":"thinking","thinking":"Consider the task."}),
        ),
        block("msg_a", json!({"type":"text","text":"Reading now."})),
        block(
            "msg_a",
            json!({"type":"tool_use","id":"tool_1","name":"Read","input":{}}),
        ),
        json!({"type":"stream_event","event":{"type":"message_delta",
            "usage":{"output_tokens":350,"output_tokens_details":{"thinking_tokens":80}}}}),
        // A late block repeats provisional usage after the final count.
        block(
            "msg_a",
            json!({"type":"tool_use","id":"tool_2","name":"Read","input":{}}),
        ),
        json!({"type":"stream_event","event":{"type":"message_stop"}}),
        // Equal usage on a different request must still be charged.
        block("msg_b", json!({"type":"text","text":"Done."})),
    ];
    for line in lines {
        for event in parse_line(&line.to_string(), &mut state) {
            match event {
                EngineEvent::UsageRequest { request } => total += request.delta(),
                EngineEvent::ContextUsage { tokens } => contexts.push(tokens),
                EngineEvent::Assistant {
                    content: blocks,
                    usage,
                    ..
                } => {
                    assert!(usage.is_none());
                    content.extend(blocks);
                }
                _ => {}
            }
        }
    }
    assert_eq!(total.input_tokens, 4);
    assert_eq!(total.cache_read_tokens, 400);
    assert_eq!(total.cache_write_tokens, 200);
    assert_eq!(total.output_tokens, 351);
    assert_eq!(total.reasoning_tokens, Some(80));
    assert_eq!(content.len(), 5);
    assert!(contexts.iter().all(|tokens| *tokens == 302));
}

/// The request an operator reported: one `claude-code/haiku` message whose
/// thinking, text and tool_use blocks each repeated input=10 /
/// cache_write=16,973, i.e. 16,984 counted three times, tripping a 40,000
/// grant at 50,952. One request bills once. The `message_delta` below is
/// this fixture's own addition (the operator's excerpt carried none), so
/// the once-billed total here is 16,984 + 42 = 17,025.
#[test]
fn a_repeated_multi_block_message_bills_one_request_not_three() {
    let mut state = ParseState::default();
    let usage = json!({"input_tokens":10,"cache_creation_input_tokens":16_973,
        "cache_read_input_tokens":0,"output_tokens":1});
    let block = |content: Value| {
        json!({"type":"assistant","message":{
            "id":"msg_haiku","model":"claude-haiku-4-5","content":[content],
            "usage":usage.clone()}})
    };
    let lines = [
        json!({"type":"stream_event","event":{"type":"message_start",
            "message":{"id":"msg_haiku","usage":usage.clone()}}}),
        block(json!({"type":"thinking","thinking":"Read the file."})),
        block(json!({"type":"text","text":"Reading the README."})),
        block(json!({"type":"tool_use","id":"toolu_1","name":"Read","input":{}})),
        json!({"type":"stream_event","event":{"type":"message_delta",
            "usage":{"output_tokens":42}}}),
        json!({"type":"stream_event","event":{"type":"message_stop"}}),
    ];
    let mut billed = Usage::default();
    let mut accountings = 0;
    let mut blocks = 0;
    for line in lines {
        for event in parse_line(&line.to_string(), &mut state) {
            match event {
                EngineEvent::UsageRequest { request } => {
                    billed += request.delta();
                    accountings += 1;
                }
                EngineEvent::Assistant { content, usage, .. } => {
                    assert!(usage.is_none(), "usage rides the delta stream alone");
                    blocks += content.len();
                }
                _ => {}
            }
        }
    }
    assert_eq!(blocks, 3, "every block is still recorded");
    assert_eq!(
        accountings, 2,
        "the request, then its final output count - never once per block"
    );
    assert_eq!(billed.input_tokens, 10);
    assert_eq!(billed.cache_write_tokens, 16_973);
    assert_eq!(billed.output_tokens, 42);
    // What the budget gate sees. The operator's own request billed 16,984
    // once; counted per block it was 50,952, which failed an otherwise
    // affordable request against a 40,000 grant.
    assert_eq!(billed.total_tokens(), 17_025);
    assert_eq!(16_984 * 3, 50_952, "the reported figure, for the record");
    assert!(billed.total_tokens() < 40_000);
}

/// The dedup key is `message.id`, and the CLI does not owe Aether that
/// field. Without it the blocks of one message must still share a key, or
/// the triple count returns verbatim.
#[test]
fn a_message_without_an_id_still_bills_once_per_message() {
    let mut state = ParseState::default();
    let usage = json!({"input_tokens":10,"cache_creation_input_tokens":16_973,
        "cache_read_input_tokens":0,"output_tokens":1});
    let block = |content: Value| {
        json!({"type":"assistant","message":{
            "model":"claude-haiku-4-5","content":[content],"usage":usage.clone()}})
    };
    let message = |text: &str| {
        [
            json!({"type":"stream_event","event":{"type":"message_start",
                "message":{"usage":usage.clone()}}}),
            block(json!({"type":"thinking","thinking":text})),
            block(json!({"type":"text","text":text})),
            block(json!({"type":"tool_use","id":"toolu_1","name":"Read","input":{}})),
            json!({"type":"stream_event","event":{"type":"message_stop"}}),
        ]
    };
    let mut billed = Usage::default();
    for line in message("one").into_iter().chain(message("two")) {
        for event in parse_line(&line.to_string(), &mut state) {
            if let EngineEvent::UsageRequest { request } = event {
                billed += request.delta();
            }
        }
    }
    assert_eq!(
        billed.total_tokens(),
        2 * 16_984,
        "two messages, four blocks each, billed once per message"
    );
}

/// Everything a batch of lines bills.
fn bill(lines: impl IntoIterator<Item = Value>, state: &mut ParseState) -> Usage {
    let mut billed = Usage::default();
    for line in lines {
        for event in parse_line(&line.to_string(), state) {
            if let EngineEvent::UsageRequest { request } = event {
                billed += request.delta();
            }
        }
    }
    billed
}

/// An `assistant` line with no id, arriving while a message the stream
/// opened *under an id* is still open, is a different message. Inheriting
/// the open key billed it nothing: 500,005 spent, 100,001 billed.
#[test]
fn an_id_less_message_inside_an_open_stream_is_not_absorbed_by_it() {
    let usage = |n: u64| json!({"input_tokens": n, "output_tokens": 1});
    let mut lines = vec![
        json!({"type":"stream_event","event":{"type":"message_start",
        "message":{"id":"msg_A","usage":usage(100_000)}}}),
    ];
    lines.extend((0..4).map(|_| {
        json!({"type":"assistant","message":{
            "model":"claude-sonnet-5","content":[{"type":"text","text":"x"}],
            "usage": usage(100_000)}})
    }));
    let billed = bill(lines, &mut ParseState::default());
    assert_eq!(billed.total_tokens(), 500_005, "five messages, five bills");
}

/// The same with no `stream_event` lines at all: nothing is open, so no
/// message can inherit another's counters (50,050 spent, 10,010 billed).
#[test]
fn id_less_messages_without_any_stream_events_are_billed_one_by_one() {
    let lines = (0..5).map(|_| {
        json!({"type":"assistant","message":{
            "model":"claude-sonnet-5","content":[{"type":"text","text":"x"}],
            "usage":{"input_tokens":10_000,"output_tokens":10}}})
    });
    let billed = bill(lines, &mut ParseState::default());
    assert_eq!(billed.total_tokens(), 50_050);
}

/// The keys Aether mints are their own namespace: an engine that names a
/// message after one cannot inherit what that one was billed.
#[test]
fn an_engine_id_cannot_be_forged_onto_a_minted_key() {
    let mut state = ParseState::default();
    // One id-less message, so a key is minted and charged.
    let first = bill(
        [json!({"type":"assistant","message":{
            "model":"claude-sonnet-5","content":[{"type":"text","text":"x"}],
            "usage":{"input_tokens":40_000,"output_tokens":2}}})],
        &mut state,
    );
    assert_eq!(first.total_tokens(), 40_002);
    // Then a message naming every shape the minted key has ever taken.
    for forged in ["\u{0}message-0", "\u{0}message-1", "Minted(1)", "1"] {
        let billed = bill(
            [json!({"type":"assistant","message":{
                "id": forged, "model":"claude-sonnet-5",
                "content":[{"type":"text","text":"x"}],
                "usage":{"input_tokens":500_000,"output_tokens":1}}})],
            &mut state,
        );
        assert_eq!(
            billed.total_tokens(),
            500_001,
            "{forged:?} must be billed in full"
        );
    }
}

/// The high-water marks are the engine's ids, so the map is bounded. An
/// evicted message can only be billed twice, never dropped.
#[test]
fn a_run_minting_endless_message_ids_cannot_grow_the_map_without_bound() {
    let mut state = ParseState::default();
    let lines = (0..MAX_TRACKED_MESSAGES * 3).map(|n| {
        json!({"type":"assistant","message":{
            "id": format!("msg_{n}"), "model":"claude-sonnet-5",
            "content":[{"type":"text","text":"x"}],
            "usage":{"input_tokens":10,"output_tokens":1}}})
    });
    let billed = bill(lines, &mut state);
    assert_eq!(
        billed.total_tokens(),
        (MAX_TRACKED_MESSAGES * 3) as u64 * 11,
        "every message billed once"
    );
    assert!(
        state.message_usage.len() <= MAX_TRACKED_MESSAGES,
        "bounded: {}",
        state.message_usage.len()
    );
}

/// The final output count rides `message_delta`, which is keyed off the
/// `message_start` that opened the message. A start the stream never
/// carried must not make that count disappear - an undercount walks
/// straight past a budget.
#[test]
fn a_missed_message_start_does_not_drop_the_final_output_count() {
    let mut state = ParseState::default();
    let lines = [
        json!({"type":"stream_event","event":{"type":"message_delta",
            "usage":{"output_tokens":137}}}),
        json!({"type":"stream_event","event":{"type":"message_stop"}}),
    ];
    let mut billed = Usage::default();
    for line in lines {
        for event in parse_line(&line.to_string(), &mut state) {
            if let EngineEvent::UsageRequest { request } = event {
                billed += request.delta();
            }
        }
    }
    assert_eq!(
        billed.output_tokens, 137,
        "the count is billed, not dropped"
    );
}

#[test]
fn interrupted_stream_preserves_usage_already_consumed() {
    let mut state = ParseState::default();
    let events = parse_line(
        &json!({"type":"stream_event","event":{
        "type":"message_start","message":{"id":"interrupted", "usage":{
            "input_tokens":2,"cache_read_input_tokens":1000,"output_tokens":1}}}})
        .to_string(),
        &mut state,
    );
    assert!(events.iter().any(|event| matches!(event,
        EngineEvent::UsageRequest { request } if request.delta().cache_read_tokens == 1000)));
    // No message_stop/result is needed to preserve partial usage on exit.
    let events = parse_line(
        &json!({"type":"stream_event","event":{
        "type":"message_delta","usage":{"output_tokens":17}}})
        .to_string(),
        &mut state,
    );
    assert!(
        events.iter().any(|event| matches!(event,
        EngineEvent::UsageRequest { request } if request.delta().output_tokens == 16 && request.delta().input_tokens == 0))
    );
}

#[test]
fn native_request_totals_retain_the_prompt_for_late_output() {
    let mut state = ParseState::default();
    let lines = [
        json!({"type":"stream_event","event":{"type":"message_start",
            "message":{"id":"long","usage":{"input_tokens":10,
                "cache_read_input_tokens":100_000}}}}),
        json!({"type":"stream_event","event":{"type":"message_delta",
            "usage":{"output_tokens":2_000}}}),
        json!({"type":"assistant","message":{"id":"long",
            "content":[{"type":"text","text":"done"}],
            "usage":{"input_tokens":10,"cache_read_input_tokens":100_000,
                "output_tokens":1}}}),
    ];
    let requests: Vec<_> = lines
        .into_iter()
        .flat_map(|line| parse_line(&line.to_string(), &mut state))
        .filter_map(|event| match event {
            EngineEvent::UsageRequest { request } => Some(request),
            _ => None,
        })
        .collect();
    assert_eq!(requests.len(), 2, "late repeated blocks add no usage");
    assert_eq!(requests[0].id, UsageRequestId::Engine("long".into()));
    assert_eq!(requests[0].previous, Usage::default());
    assert_eq!(requests[1].id, requests[0].id);
    assert_eq!(requests[1].previous, requests[0].total);
    assert_eq!(requests[1].total.context_tokens(), 100_010);
    assert_eq!(requests[1].total.output_tokens, 2_000);
    assert_eq!(requests[1].delta().context_tokens(), 0);
}

fn streamed_total(lines: &[Value]) -> Usage {
    let mut state = ParseState::default();
    let mut total = Usage::default();
    for line in lines {
        for event in parse_line(&line.to_string(), &mut state) {
            if let EngineEvent::UsageRequest { request } = event {
                total += request.delta();
            }
        }
    }
    total
}

fn split_usage(writes: u64, one_hour: Option<u64>) -> Value {
    let mut usage = json!({"input_tokens":2,"cache_creation_input_tokens":writes,
        "cache_read_input_tokens":10,"output_tokens":1});
    if let Some(one_hour) = one_hour {
        usage["cache_creation"] = json!({"ephemeral_5m_input_tokens": writes.saturating_sub(one_hour),
            "ephemeral_1h_input_tokens": one_hour});
    }
    usage
}

#[test]
fn one_hour_cache_writes_are_read_once_per_request() {
    let block = |id: &str, usage: Value| {
        json!({"type":"assistant","message":{"id":id,
            "content":[{"type":"text","text":"x"}],"usage":usage}})
    };
    let total = streamed_total(&[
        json!({"type":"stream_event","event":{"type":"message_start",
            "message":{"id":"msg_a","usage":split_usage(100, Some(60))}}}),
        block("msg_a", split_usage(100, Some(60))),
        block("msg_a", split_usage(100, Some(60))),
        block("msg_b", split_usage(40, Some(40))),
    ]);
    assert_eq!(total.cache_write_tokens, 140);
    assert_eq!(total.cache_write_1h_tokens, 100);
}

#[test]
fn writes_without_a_split_are_not_guessed_one_hour() {
    let total = streamed_total(&[json!({"type":"assistant","message":{"id":"msg_a",
        "content":[],"usage":split_usage(100, None)}})]);
    assert_eq!(total.cache_write_tokens, 100);
    assert_eq!(total.cache_write_1h_tokens, 0);
}

#[test]
fn a_split_beyond_its_total_is_bounded_by_the_total() {
    let mut usage = split_usage(100, Some(60));
    usage["cache_creation"]["ephemeral_1h_input_tokens"] = json!(500);
    let parsed = parse_usage(&usage);
    assert_eq!(parsed.cache_write_tokens, 100);
    assert_eq!(parsed.cache_write_1h_tokens, 100);
}

/// Writes first reported without a split, then split on an otherwise identical
/// line, must still reach pricing: the request is repriced at the 1-hour rate.
#[test]
fn a_split_reported_after_its_writes_still_reprices_the_request() {
    let block = |usage: Value| {
        json!({"type":"assistant","message":{"id":"msg_a",
            "content":[{"type":"text","text":"x"}],"usage":usage}})
    };
    let mut state = ParseState::default();
    let mut requests = Vec::new();
    for line in [
        block(split_usage(100, None)),
        block(split_usage(100, Some(100))),
    ] {
        for event in parse_line(&line.to_string(), &mut state) {
            if let EngineEvent::UsageRequest { request } = event {
                requests.push(request);
            }
        }
    }
    assert_eq!(
        requests.len(),
        2,
        "the late split was never sent for pricing"
    );
    assert_eq!(requests[1].previous.cache_write_1h_tokens, 0);
    assert_eq!(requests[1].total.cache_write_1h_tokens, 100);
    assert_eq!(
        requests[1].delta().total_tokens(),
        0,
        "no token is counted twice"
    );
}

#[test]
fn model_totals_retain_late_names_final_output_and_evicted_messages() {
    let mut state = ParseState::default();
    let start = json!({"type":"stream_event","event":{"type":"message_start",
        "message":{"id":"late","usage":{"input_tokens":100,"output_tokens":1}}}});
    parse_line(&start.to_string(), &mut state);
    let named = json!({"type":"assistant","message":{"id":"late","model":"main",
        "usage":{"input_tokens":100,"output_tokens":1}}});
    parse_line(&named.to_string(), &mut state);
    let delta = json!({"type":"stream_event","event":{"type":"message_delta",
        "usage":{"output_tokens":10}}});
    parse_line(&delta.to_string(), &mut state);
    parse_line(&named.to_string(), &mut state);
    assert_eq!(state.streamed_models["main"].input_tokens, 100);
    assert_eq!(state.streamed_models["main"].output_tokens, 10);
    let extra = bill(
        (0..MAX_TRACKED_MESSAGES).map(|n| {
            json!({"type":"assistant",
        "message":{"id":format!("msg-{n}"),"model":"main",
            "usage":{"input_tokens":1,"output_tokens":1}}})
        }),
        &mut state,
    );
    assert_eq!(extra.total_tokens(), MAX_TRACKED_MESSAGES as u64 * 2);
    assert_eq!(
        state.streamed_models["main"].total_tokens(),
        110 + extra.total_tokens()
    );
    assert!(state.message_models.len() <= MAX_TRACKED_MESSAGES);
}

#[test]
fn model_totals_keep_a_cache_ttl_split_reported_after_its_writes() {
    let mut state = ParseState::default();
    let mut message = json!({"type":"assistant","message":{"id":"ttl","model":"main",
        "usage":{"cache_creation_input_tokens":100}}});
    let before = bill([message.clone()], &mut state);
    assert_eq!(before.cache_write_tokens, 100);
    message["message"]["usage"]["cache_creation"] = json!({"ephemeral_1h_input_tokens":100});
    let after = bill([message], &mut state);
    assert_eq!(after.total_tokens(), 0);
    assert_eq!(state.streamed_models["main"].cache_write_tokens, 100);
    assert_eq!(state.streamed_models["main"].cache_write_1h_tokens, 100);
}
