//! Dollar receipts and per-user-turn counters across a native park and resume.
use super::*;
use aether_core::model::{CostBasis, CostGap, Usage};
use aether_engine::claude_code::{ParseState, parse_line};
use serde_json::{Value, json};
use tokio::sync::oneshot;

struct ReceiptReplayEngine {
    runs: Mutex<std::collections::VecDeque<Vec<Value>>>,
    ready: mpsc::Sender<(EngineTurn, oneshot::Sender<()>)>,
}

#[async_trait::async_trait]
impl Engine for ReceiptReplayEngine {
    fn kind(&self) -> EngineKind {
        EngineKind::CLAUDE_CODE
    }

    fn adapter(&self) -> String {
        "receipt-replay/1".into()
    }

    async fn run(
        &self,
        turn: EngineTurn,
        cancel: CancellationToken,
    ) -> Result<EngineRun, EngineError> {
        assert!(turn.mcp.is_some());
        let lines = self.runs.lock().unwrap().pop_front().expect("scripted run");
        assert!(!lines.is_empty());
        let (tx, events) = mpsc::channel(64);
        let ready = self.ready.clone();
        tokio::spawn(async move {
            let mut parser = ParseState::for_turn(&turn);
            let parked = turn.session.is_none();
            let (finish, finished) = oneshot::channel();
            ready.send((turn, finish)).await.unwrap();
            finished.await.unwrap();
            if parked {
                cancel.cancelled().await;
            }
            tx.send(EngineEvent::Session {
                id: "receipt-session".into(),
                model: None,
                tools: Vec::new(),
            })
            .await
            .unwrap();
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

fn streamed_request(id: &str) -> Value {
    assert!(!id.is_empty());
    json!({"type":"assistant","message":{"id":id,"model":"claude-haiku-5-5",
        "content":[{"type":"text","text":"Working."}],
        "usage":{"input_tokens":100,"output_tokens":10,
            "cache_read_input_tokens":0,"cache_creation_input_tokens":0}}})
}

fn result(cost: f64, requests: u64, parked: bool, helper: bool) -> Value {
    assert!(cost > 0.0 && cost.is_finite());
    assert!(requests > 0);
    let mut value = json!({"type":"result","session_id":"receipt-session",
        "subtype":if parked {"error_during_execution"} else {"success"},
        "is_error":parked,"result":if parked {""} else {"Done."},
        "errors":if parked {vec!["[ede_diagnostic] result_type=user last_content_type=n/a stop_reason=tool_use"]} else {vec![]},
        "stop_reason":if parked {"tool_use"} else {"end_turn"},"total_cost_usd":cost,
        "usage":{"input_tokens":100,"output_tokens":10,
            "cache_read_input_tokens":0,"cache_creation_input_tokens":0},
        "modelUsage":{"claude-haiku-5-5":{"inputTokens":100 * requests,
            "outputTokens":10 * requests,"cacheReadInputTokens":0,"cacheCreationInputTokens":0}}});
    if helper {
        value["modelUsage"]["claude-helper"] = json!({"inputTokens":100,"outputTokens":10,
            "cacheReadInputTokens":0,"cacheCreationInputTokens":0});
    }
    value
}

async fn resume_turn(
    h: &Harness,
    id: AgentId,
    ready: &mut mpsc::Receiver<(EngineTurn, oneshot::Sender<()>)>,
    parked: bool,
) -> aether_core::agent::Agent {
    h.runtime
        .send_message(id, vec![ContentBlock::text("go")], Actor::user())
        .unwrap();
    let (turn, finish) = ready.recv().await.unwrap();
    assert_eq!(turn.session.is_some(), !parked);
    if parked {
        let args = &turn.mcp.as_ref().unwrap().args;
        let at = args.iter().position(|arg| arg == "--token").unwrap();
        let (reply, is_error) = h
            .runtime
            .engine_tool(
                id,
                &args[at + 1],
                "ask_user".into(),
                json!({"question":"Continue?"}),
            )
            .await
            .unwrap();
        assert!(!is_error, "{reply}");
    } else {
        assert!((turn.session_usage.cost_usd - 0.05).abs() < 1e-12);
    }
    finish.send(()).unwrap();
    wait_for(h.store.as_ref(), id, |agent| {
        agent.stats.turns == if parked { 1 } else { 2 }
            && agent.status.kind() == AgentStatusKind::Waiting
    })
    .await
}

async fn parked_receipts(
    receipt_covers_stream: bool,
    helper: bool,
) -> (Harness, AgentId, Usage, Usage) {
    let covered = if receipt_covers_stream { 2 } else { 1 };
    let runs = vec![
        vec![
            streamed_request("first"),
            streamed_request("second"),
            result(0.05, covered, true, helper),
        ],
        vec![
            streamed_request("third"),
            result(0.08, covered + 1, false, helper),
        ],
    ];
    let (tx, mut ready) = mpsc::channel(2);
    let h = harness_with_engine_config(
        Arc::new(ReceiptReplayEngine {
            runs: Mutex::new(runs.into()),
            ready: tx,
        }),
        |config| config.engine_park_grace = Duration::from_millis(5),
    )
    .await;
    let id = chat_agent(&h);
    let first = resume_turn(&h, id, &mut ready, true).await;
    assert_eq!(first.engine_session.as_deref(), Some("receipt-session"));
    let baseline = first.engine_usage.as_ref().unwrap();
    assert_eq!(baseline.total.cost_usd, 0.05);
    let models = baseline.models.as_ref().unwrap();
    assert_eq!(models["claude-haiku-5-5"].input_tokens, 100 * covered);
    assert_eq!(models.contains_key("claude-helper"), helper);
    let second = resume_turn(&h, id, &mut ready, false).await;
    assert_eq!(second.engine_session, first.engine_session);
    let events = h
        .store
        .events(0, 1000, &EventFilter::for_agent(id))
        .unwrap();
    assert_eq!(
        events
            .iter()
            .filter(|event| matches!(&event.kind,
        EventKind::Custom {name, ..} if name == "engine.parked"))
            .count(),
        1
    );
    (h, id, first.stats.usage, second.stats.usage)
}

#[tokio::test]
async fn cumulative_dollars_are_not_rebilled_after_park_and_resume() {
    let (h, id, first, second) = parked_receipts(true, true).await;
    assert!((first.cost_usd - 0.05).abs() < 1e-12, "{first:?}");
    assert!((second.cost_usd - 0.08).abs() < 1e-12, "{second:?}");
    assert_eq!(first.total_tokens(), 220);
    assert_eq!(second.total_tokens(), 330);
    assert_eq!(second.cost_basis, CostBasis::ProviderReported);
    for usage in [first, second] {
        assert!(!usage.cost_complete);
        assert_eq!(usage.cost_gap, Some(CostGap::Interrupted));
        assert_eq!(usage.estimated_cost_usd, None);
    }
    assert!(super::usage_replay::noted(
        &h,
        id,
        &["cumulative dollar receipt covers"]
    ));
    h.runtime.shutdown().await;
}

#[tokio::test]
async fn dollars_missing_from_model_usage_stay_billed_after_resume() {
    assert_missing_receipt(false).await;
}

#[tokio::test]
async fn dollars_missing_from_model_usage_stay_billed_despite_unstreamed_helper_after_resume() {
    assert_missing_receipt(true).await;
}

#[tokio::test]
async fn a_reset_receipt_persists_the_new_baseline_and_keeps_a_stream_floor_with_a_gap() {
    let gap = reset_receipt_floor(vec![streamed_request("second")]).await;
    assert_eq!(gap, Some(CostGap::Mixed));
}

#[tokio::test]
async fn a_reset_receipt_without_streamed_usage_prices_its_closing_tokens_as_a_floor() {
    let gap = reset_receipt_floor(Vec::new()).await;
    assert!(gap.is_some());
}

/// The cost gap of a resumed run whose receipt resets after `streamed`.
async fn reset_receipt_floor(mut streamed: Vec<Value>) -> Option<CostGap> {
    assert!(streamed.len() <= 1);
    streamed.push(result(0.02, 1, false, false));
    let runs = vec![
        vec![streamed_request("first"), result(0.05, 1, true, false)],
        streamed,
    ];
    let (tx, mut ready) = mpsc::channel(2);
    let h = harness_with_engine_config(
        Arc::new(ReceiptReplayEngine {
            runs: Mutex::new(runs.into()),
            ready: tx,
        }),
        |config| config.engine_park_grace = Duration::from_millis(5),
    )
    .await;
    let id = chat_agent(&h);
    let first = resume_turn(&h, id, &mut ready, true).await;
    let second = resume_turn(&h, id, &mut ready, false).await;
    assert_eq!(second.engine_usage.as_ref().unwrap().total.cost_usd, 0.02);
    let rate = aether_model::Registry::builtin()
        .resolve(&ModelRef::new("claude-code", "haiku"))
        .unwrap()
        .model
        .capabilities
        .cost
        .unwrap();
    let floor = rate.cost_of(&Usage {
        input_tokens: 100,
        output_tokens: 10,
        ..Usage::default()
    });
    let added = second.stats.usage.cost_usd - first.stats.usage.cost_usd;
    assert!((added - floor).abs() < 1e-12, "{:?}", second.stats.usage);
    assert_eq!(
        second.stats.usage.total_tokens() - first.stats.usage.total_tokens(),
        110
    );
    assert!(!second.stats.usage.cost_complete);
    h.runtime.shutdown().await;
    second.stats.usage.cost_gap
}

async fn assert_missing_receipt(helper: bool) {
    let (h, id, first, second) = parked_receipts(false, helper).await;
    let rate = aether_model::Registry::builtin()
        .resolve(&ModelRef::new("claude-code", "haiku"))
        .unwrap()
        .model
        .capabilities
        .cost
        .unwrap();
    let missing = rate.cost_of(&Usage {
        input_tokens: 100,
        output_tokens: 10,
        ..Usage::default()
    });
    assert!(
        (first.cost_usd - (0.05 + missing)).abs() < 1e-12,
        "{first:?}"
    );
    assert!(
        (second.cost_usd - (0.08 + missing)).abs() < 1e-12,
        "{second:?}"
    );
    assert!(second.cost_usd > 0.08);
    assert!(!second.cost_complete);
    assert_eq!(second.cost_gap, Some(CostGap::Interrupted));
    assert!(super::usage_replay::noted(
        &h,
        id,
        &["the missing part is priced from the stream"]
    ));
    assert!(!super::usage_replay::noted(
        &h,
        id,
        &["left out of the billed cost"]
    ));
    h.runtime.shutdown().await;
}
