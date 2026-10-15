use super::*;
use aether_core::model::{CostBasis, CostGap};
use aether_engine::claude_code::{ParseState, parse_line};
use serde_json::{Value, json};

struct ClaudeReceiptEngine {
    runs: Mutex<std::collections::VecDeque<Vec<Value>>>,
    turns: Mutex<Vec<EngineTurn>>,
}

#[async_trait::async_trait]
impl Engine for ClaudeReceiptEngine {
    fn kind(&self) -> EngineKind {
        EngineKind::CLAUDE_CODE
    }

    fn adapter(&self) -> String {
        "claude-receipt-retry/1".into()
    }

    async fn run(
        &self,
        turn: EngineTurn,
        _cancel: CancellationToken,
    ) -> Result<EngineRun, EngineError> {
        assert!(turn.mcp.is_some());
        let lines = self.runs.lock().unwrap().pop_front().expect("scripted run");
        assert!(!lines.is_empty());
        let mut parser = ParseState::for_turn(&turn);
        self.turns.lock().unwrap().push(turn);
        let (tx, events) = mpsc::channel(64);
        // An input channel keeps the run open past a receipt while background work runs.
        let (input, messages) = mpsc::channel(1);
        tokio::spawn(async move {
            let _messages = messages;
            for line in lines {
                for event in parse_line(&line.to_string(), &mut parser) {
                    tx.send(event).await.unwrap();
                }
            }
        });
        Ok(EngineRun {
            events,
            input: Some(input),
            decisions: None,
            stop: None,
        })
    }
}

fn overload_retry_engine() -> Arc<ClaudeReceiptEngine> {
    let init = json!({"type":"system","subtype":"init","session_id":"same",
        "model":"claude-haiku-5-5"});
    let overloaded = json!({"type":"result", "subtype":"error_during_execution",
        "is_error":true, "result":"API Error: 529 Model is overloaded",
        "session_id":"same", "total_cost_usd":0.0, "modelUsage":{},
        "usage":{"input_tokens":0,"output_tokens":0}});
    let recovered = json!({"type":"result", "subtype":"success", "result":"recovered",
        "session_id":"same", "total_cost_usd":0.25,
        "modelUsage":{"claude-haiku-5-5":{"inputTokens":10,"outputTokens":2,
            "cacheReadInputTokens":0,"cacheCreationInputTokens":0}},
        "usage":{"input_tokens":10,"output_tokens":2}});
    Arc::new(ClaudeReceiptEngine {
        runs: Mutex::new(vec![vec![init.clone(), overloaded], vec![init, recovered]].into()),
        turns: Mutex::new(Vec::new()),
    })
}

#[tokio::test]
async fn a_claude_zero_cost_overload_receipt_is_metadata_and_can_retry() {
    let engine = overload_retry_engine();
    let h = harness_with_engine(engine.clone(), Arc::default(), Arc::default()).await;
    let id = chat_agent(&h);
    engine_turn(&h, id, "go").await;
    {
        let turns = engine.turns.lock().unwrap();
        assert_eq!(turns.len(), 2);
        assert_eq!(turns[0].session, None);
        assert_eq!(turns[1].session.as_deref(), Some("same"));
        assert_eq!(turns[1].session_usage.total_tokens(), 0);
        assert_eq!(turns[1].session_usage.cost_usd, 0.0);
        assert!(!turns[1].session_usage.cost_complete);
        assert_eq!(turns[1].session_usage.cost_gap, Some(CostGap::Unknown));
        assert_eq!(turns[0].prompt, turns[1].prompt);
    }
    assert_eq!(
        failures(&h, id),
        [(aether_core::model::ModelErrorClass::Overloaded, true)]
    );
    let agent = h.store.agent(id).unwrap().unwrap();
    let usage = agent.stats.usage;
    assert_eq!((usage.input_tokens, usage.output_tokens), (10, 2));
    let resolved = h
        .runtime
        .models()
        .resolve(&ModelRef::new("claude-code", "haiku"))
        .unwrap();
    let expected = resolved.model.capabilities.cost.unwrap().cost_of(&usage);
    assert!(expected > 0.0 && expected < 0.25);
    assert_eq!(usage.cost_usd, expected);
    assert!(!usage.cost_complete);
    assert_eq!(usage.cost_gap, Some(CostGap::Unreported));
    assert_eq!(usage.cost_basis, CostBasis::ListPrice);
    assert_eq!(agent.engine_usage.unwrap().total.cost_usd, 0.25);
    h.runtime.shutdown().await;
}

/// Claude Code's stream when its first request fails: a synthetic zero-usage
/// message, then an all-zero receipt carrying the HTTP status if there was one.
fn failed_first_request(status: Option<u64>, error: &str) -> Arc<ClaudeReceiptEngine> {
    let zero = json!({"input_tokens":0,"output_tokens":0,
        "cache_creation_input_tokens":0,"cache_read_input_tokens":0});
    let init = json!({"type":"system","subtype":"init","session_id":"fresh",
        "model":"claude-haiku-5-5"});
    let synthetic = json!({"type":"assistant","session_id":"fresh","is_api_error_message":true,
        "api_error_status":status,"message":{"id":"err","model":"<synthetic>",
            "role":"assistant","usage":zero,"content":[{"type":"text","text":error}]}});
    let failed = json!({"type":"result","subtype":"success","is_error":true,"result":error,
        "api_error_status":status,"terminal_reason":"api_error","session_id":"fresh",
        "total_cost_usd":0,"modelUsage":{},"usage":zero});
    Arc::new(ClaudeReceiptEngine {
        runs: Mutex::new(vec![vec![init, synthetic, failed]].into()),
        turns: Mutex::new(Vec::new()),
    })
}

async fn failed_worker_usage(engine: Arc<ClaudeReceiptEngine>) -> aether_core::model::Usage {
    let h = harness_with_engine(engine.clone(), Arc::default(), Arc::default()).await;
    let (id, _) = engine_worker(&h, "reply with one word");
    let agent = wait_for(h.store.as_ref(), id, |a| a.status.is_terminal()).await;
    assert!(
        matches!(&agent.status, AgentStatus::Failed { .. }),
        "expected a failure, got {:?}",
        agent.status
    );
    assert_eq!(engine.turns.lock().unwrap().len(), 1);
    h.runtime.shutdown().await;
    assert_eq!(agent.stats.usage.total_tokens(), 0);
    assert_eq!(agent.stats.usage.cost_usd, 0.0);
    agent.stats.usage
}

#[tokio::test]
async fn a_claude_request_rejected_before_inference_costs_a_complete_zero() {
    let error = "API Error: 400 rejected before inference";
    let usage = failed_worker_usage(failed_first_request(Some(400), error)).await;
    assert!(usage.cost_complete, "{usage:?}");
    assert_eq!(usage.cost_gap, None);
}

#[tokio::test]
async fn a_dropped_claude_connection_keeps_its_cost_gap() {
    let error = "API Error: Connection error.";
    let usage = failed_worker_usage(failed_first_request(None, error)).await;
    assert!(!usage.cost_complete, "{usage:?}");
    assert_eq!(usage.cost_gap, Some(CostGap::Unreported));
}

#[tokio::test]
async fn per_turn_usage_from_an_earlier_receipt_keeps_a_later_zero_receipt_incomplete() {
    let init = json!({"type":"system","subtype":"init","session_id":"same",
        "model":"claude-haiku-5-5"});
    let counted = json!({"type":"result","subtype":"error_during_execution","is_error":true,
        "result":"API Error: 529 Model is overloaded","api_error_status":529,
        "session_id":"same","total_cost_usd":0,"modelUsage":{},
        "usage":{"input_tokens":100,"output_tokens":20}});
    let mut zeroed = counted.clone();
    zeroed["result"] = json!("API Error: 400 refused");
    zeroed["api_error_status"] = json!(400);
    zeroed["usage"] = json!({"input_tokens":0,"output_tokens":0});
    let engine = Arc::new(ClaudeReceiptEngine {
        runs: Mutex::new(vec![vec![init, counted, zeroed]].into()),
        turns: Mutex::new(Vec::new()),
    });
    let h = harness_with_engine_config(engine.clone(), |config| {
        config.engine_transient_retry_window = Duration::ZERO;
    })
    .await;
    let (id, _) = engine_worker(&h, "reply with one word");
    let agent = wait_for(h.store.as_ref(), id, |a| a.status.is_terminal()).await;
    assert_eq!(engine.turns.lock().unwrap().len(), 1);
    let usage = agent.stats.usage;
    assert_eq!(usage.total_tokens(), 120);
    assert!(!usage.cost_complete, "{usage:?}");
    assert_eq!(usage.cost_gap, Some(CostGap::Unreported));
    h.runtime.shutdown().await;
}

/// `failed_first_request` with `lines` inserted after its init line.
fn failed_after(lines: Vec<Value>) -> Arc<ClaudeReceiptEngine> {
    let engine = failed_first_request(Some(400), "API Error: 400 refused");
    let mut runs = engine.runs.lock().unwrap();
    let run = runs.front_mut().expect("one scripted run");
    for (offset, line) in lines.into_iter().enumerate() {
        run.insert(1 + offset, line);
    }
    drop(runs);
    engine
}

#[tokio::test]
async fn a_retry_after_a_dropped_connection_keeps_a_later_refusal_incomplete() {
    let retry = json!({"type":"system","subtype":"api_retry","attempt":1,"max_retries":10,
        "retry_delay_ms":600,"error_status":null,"error":"unknown","session_id":"fresh"});
    let usage = failed_worker_usage(failed_after(vec![retry])).await;
    assert!(!usage.cost_complete, "{usage:?}");
    assert_eq!(usage.cost_gap, Some(CostGap::Unreported));
}

#[tokio::test]
async fn an_earlier_receipt_keeps_a_later_refusal_incomplete() {
    let background = json!({"type":"system","subtype":"task_started","task_id":"background"});
    let billed = json!({"type":"result","subtype":"success","session_id":"fresh",
        "total_cost_usd":0.05,"modelUsage":{},"usage":{}});
    let usage = failed_worker_usage(failed_after(vec![background, billed])).await;
    assert!(!usage.cost_complete, "{usage:?}");
    assert_eq!(usage.cost_gap, Some(CostGap::Unreported));
}

#[tokio::test]
async fn a_refusal_with_unreadable_counters_keeps_its_cost_gap() {
    let engine = failed_first_request(Some(400), "API Error: 400 refused");
    engine
        .runs
        .lock()
        .unwrap()
        .front_mut()
        .unwrap()
        .last_mut()
        .unwrap()["usage"] = Value::Null;
    let usage = failed_worker_usage(engine).await;
    assert!(!usage.cost_complete, "{usage:?}");
    assert_eq!(usage.cost_gap, Some(CostGap::Unreported));
}

#[tokio::test]
async fn a_skipped_queued_receipt_keeps_a_later_refusal_incomplete() {
    let queued = json!({"type":"result","subtype":"success","is_error":false,
        "num_turns":0,"origin":{"kind":"task-notification"},"session_id":"fresh",
        "total_cost_usd":0.05,"modelUsage":{},"usage":{"input_tokens":0,"output_tokens":0}});
    let usage = failed_worker_usage(failed_after(vec![queued])).await;
    assert!(!usage.cost_complete, "{usage:?}");
    assert_eq!(usage.cost_gap, Some(CostGap::Unreported));
}

#[tokio::test]
async fn any_retry_after_a_proven_zero_reopens_the_gap() {
    for status in [json!(401), Value::Null] {
        let background = json!({"type":"system","subtype":"task_started","task_id":"background"});
        let engine = failed_after(vec![background]);
        let retry = json!({"type":"system","subtype":"api_retry","attempt":1,"max_retries":10,
            "retry_delay_ms":600,"error_status":status,"error":"unknown","session_id":"fresh"});
        engine.runs.lock().unwrap().front_mut().unwrap().push(retry);
        let usage = failed_worker_usage(engine).await;
        assert!(!usage.cost_complete, "{status}: {usage:?}");
        assert_eq!(usage.cost_gap, Some(CostGap::Interrupted));
    }
}

#[tokio::test]
async fn an_answer_without_counters_keeps_a_later_refusal_incomplete() {
    let answer = json!({"type":"assistant","session_id":"fresh","message":{"id":"answer",
        "model":"claude-haiku-5-5","role":"assistant",
        "content":[{"type":"text","text":"Here is my answer."}]}});
    let usage = failed_worker_usage(failed_after(vec![answer])).await;
    assert!(!usage.cost_complete, "{usage:?}");
    assert_eq!(usage.cost_gap, Some(CostGap::Unreported));
}

#[tokio::test]
async fn a_message_start_after_a_proven_zero_reopens_the_gap() {
    let background = json!({"type":"system","subtype":"task_started","task_id":"background"});
    let engine = failed_after(vec![background]);
    let start = json!({"type":"stream_event","session_id":"fresh","event":{"type":"message_start",
        "message":{"id":"new","model":"claude-haiku-5-5","usage":{"input_tokens":0,"output_tokens":0}}}});
    engine.runs.lock().unwrap().front_mut().unwrap().push(start);
    let usage = failed_worker_usage(engine).await;
    assert!(!usage.cost_complete, "{usage:?}");
    assert_eq!(usage.cost_gap, Some(CostGap::Interrupted));
}

#[tokio::test]
async fn a_request_after_a_proven_zero_reopens_the_gap() {
    let background = json!({"type":"system","subtype":"task_started","task_id":"background"});
    let engine = failed_after(vec![background]);
    let requesting = json!({"type":"system","subtype":"status","status":"requesting",
        "session_id":"fresh"});
    engine
        .runs
        .lock()
        .unwrap()
        .front_mut()
        .unwrap()
        .push(requesting);
    let usage = failed_worker_usage(engine).await;
    assert!(!usage.cost_complete, "{usage:?}");
    assert_eq!(usage.cost_gap, Some(CostGap::Interrupted));
}
