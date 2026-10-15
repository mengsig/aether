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
        tokio::spawn(async move {
            for line in lines {
                for event in parse_line(&line.to_string(), &mut parser) {
                    tx.send(event).await.unwrap();
                }
            }
        });
        Ok(EngineRun {
            events,
            input: None,
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
