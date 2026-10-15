//! Closing receipts, cancellation, and activity after a receipt.
use super::*;
use aether_core::model::{CostGap, Usage};

#[derive(Clone, Copy)]
enum Ending {
    Cancel { cost: Option<f64> },
    InexactCancel,
    Failed,
    Unsupported,
    Rejected,
    Background,
    Normal,
    Crash,
    AppClosed,
    NativeStop,
}

struct ReceiptEngine {
    adapter: &'static str,
    kind: EngineKind,
    ending: Ending,
    started: Arc<tokio::sync::Notify>,
}

#[async_trait::async_trait]
impl Engine for ReceiptEngine {
    fn kind(&self) -> EngineKind {
        self.kind
    }
    fn adapter(&self) -> String {
        self.adapter.into()
    }
    async fn run(
        &self,
        turn: EngineTurn,
        cancel: CancellationToken,
    ) -> Result<EngineRun, EngineError> {
        assert!(!turn.prompt.is_empty());
        assert!(turn.mcp.is_some());
        if matches!(self.ending, Ending::Unsupported) {
            return Err(EngineError::Unsupported(
                "fixture policy unavailable".into(),
            ));
        }
        let (tx, events) = mpsc::channel(8);
        let (input, _messages) = mpsc::channel(1);
        let ending = self.ending;
        let started = self.started.clone();
        tokio::spawn(receipt_script(tx, ending, started, cancel));
        Ok(EngineRun {
            events,
            input: Some(input),
            decisions: None,
            stop: None,
        })
    }
}

async fn receipt_usage(ending: Ending) -> Usage {
    receipt_usage_for(
        ending,
        EngineKind::CLAUDE_CODE,
        "claude-code/receipt-review",
    )
    .await
}

async fn receipt_usage_for(ending: Ending, kind: EngineKind, adapter: &'static str) -> Usage {
    let started = Arc::new(tokio::sync::Notify::new());
    let h = harness_with_engine_config(
        Arc::new(ReceiptEngine {
            kind,
            adapter,
            ending,
            started: started.clone(),
        }),
        |c| {
            c.default_model = Some(if kind == EngineKind::CODEX {
                ModelRef::new("codex", "gpt-6.1-sol")
            } else {
                ModelRef::new("claude-code", "haiku")
            });
        },
    )
    .await;
    let id = chat_agent(&h);
    h.runtime
        .send_message(id, vec![ContentBlock::text("go")], Actor::user())
        .unwrap();
    if matches!(ending, Ending::Cancel { .. } | Ending::InexactCancel) {
        tokio::time::timeout(Duration::from_secs(10), started.notified())
            .await
            .unwrap();
        h.runtime
            .cancel_agent(id, Some("stop".into()), Actor::user())
            .unwrap();
    }
    let agent = settled_agent(&h, id).await;
    assert_eq!(agent.stats.turns, 1);
    if matches!(ending, Ending::Normal) {
        let invocations = h.store.invocations_for_agent(id).unwrap();
        assert_eq!(invocations.len(), 1);
        assert_eq!(invocations[0].usage.total_tokens(), 0);
        assert!(!invocations[0].usage.cost_complete);
        assert_eq!(invocations[0].usage.cost_gap, Some(CostGap::Unreported));
    }
    let usage = agent.stats.usage;
    assert!(usage.cost_usd.is_finite());
    h.runtime.shutdown().await;
    usage
}

#[tokio::test]
async fn cancelled_non_error_receipt_is_interrupted() {
    let usage = receipt_usage(Ending::Cancel { cost: None }).await;
    assert_eq!(usage.total_tokens(), 110);
    assert_eq!(usage.cost_gap, Some(CostGap::Interrupted));
    assert!(!usage.cost_complete);
}

#[tokio::test]
async fn cancelled_dollar_receipt_remains_a_floor() {
    let usage = receipt_usage(Ending::Cancel { cost: Some(1.25) }).await;
    assert_eq!(usage.cost_usd, 1.25);
    assert_eq!(usage.cost_gap, Some(CostGap::Interrupted));
    assert!(!usage.cost_complete);
}

#[tokio::test]
async fn failed_closing_receipt_with_usage_stays_complete() {
    let usage = receipt_usage(Ending::Failed).await;
    assert_eq!(usage.total_tokens(), 110);
    assert!(usage.cost_complete);
    assert_eq!(usage.cost_gap, None);
}

#[tokio::test]
async fn unsupported_launch_stays_complete_zero() {
    let usage = receipt_usage(Ending::Unsupported).await;
    assert_eq!(usage.total_tokens(), 0);
    assert!(usage.cost_complete);
    assert_eq!(usage.cost_gap, None);
}

#[tokio::test]
async fn rejected_input_after_receipt_keeps_complete_cost() {
    let usage = receipt_usage(Ending::Rejected).await;
    assert_eq!(usage.total_tokens(), 110);
    assert!(usage.cost_complete);
    assert_eq!(usage.cost_gap, None);
}

#[tokio::test(start_paused = true)]
async fn background_result_after_receipt_without_answer_is_interrupted() {
    let usage = receipt_usage(Ending::Background).await;
    assert_eq!(usage.total_tokens(), 110);
    assert!(!usage.cost_complete);
    assert_eq!(usage.cost_gap, Some(CostGap::Interrupted));
}

#[tokio::test]
async fn normal_closing_receipt_stays_complete() {
    let usage = receipt_usage(Ending::Normal).await;
    assert_eq!(usage.total_tokens(), 110);
    assert!(usage.cost_complete);
    assert_eq!(usage.cost_gap, None);
}

#[tokio::test]
async fn inexact_usage_then_cancel_combines_unreported_and_interrupted() {
    let usage = receipt_usage(Ending::InexactCancel).await;
    assert_eq!(usage.total_tokens(), 110);
    assert_eq!(usage.cost_gap, Some(CostGap::Mixed));
    assert!(!usage.cost_complete);
}

async fn receipt_script(
    tx: mpsc::Sender<EngineEvent>,
    ending: Ending,
    started: Arc<tokio::sync::Notify>,
    cancel: CancellationToken,
) {
    assert!(!tx.is_closed());
    assert!(!cancel.is_cancelled());
    tx.send(EngineEvent::Assistant {
        content: vec![ContentBlock::text("done")],
        usage: None,
        model: None,
    })
    .await
    .expect("assistant message");
    tx.send(EngineEvent::UsageDelta {
        usage: streamed_usage(ending),
    })
    .await
    .expect("usage");
    if matches!(
        ending,
        Ending::Crash | Ending::AppClosed | Ending::NativeStop
    ) {
        tx.send(EngineEvent::Delta(
            aether_core::model::StreamEvent::MessageStart {
                provider_message_id: None,
                model: None,
            },
        ))
        .await
        .expect("next request started");
    }
    if matches!(ending, Ending::Background | Ending::Rejected) {
        tx.send(EngineEvent::BackgroundTask {
            id: "job".into(),
            running: true,
        })
        .await
        .expect("background start");
    }
    started.notify_one();
    if matches!(ending, Ending::Cancel { .. } | Ending::InexactCancel) {
        cancel.cancelled().await;
    }
    tx.send(EngineEvent::Done(receipt_outcome(ending)))
        .await
        .expect("receipt");
    after_receipt(&tx, ending, &cancel).await;
}

fn receipt_outcome(ending: Ending) -> EngineOutcome {
    if matches!(ending, Ending::Failed) {
        return failed_result();
    }
    if matches!(
        ending,
        Ending::Crash | Ending::AppClosed | Ending::NativeStop
    ) {
        return EngineOutcome {
            usage: if matches!(ending, Ending::AppClosed | Ending::NativeStop) {
                streamed_usage(ending)
            } else {
                Usage::default()
            },
            is_error: true,
            interrupted: matches!(ending, Ending::NativeStop),
            error: Some(
                match ending {
                    Ending::Crash => "claude exited without a result",
                    Ending::AppClosed => "codex app-server closed its input",
                    Ending::NativeStop => "Codex used native delegation tool spawnAgent",
                    _ => unreachable!(),
                }
                .into(),
            ),
            ..Default::default()
        };
    }
    EngineOutcome {
        closing_accounting: true,
        usage: streamed_usage(ending),
        text: Some("done".into()),
        stop_reason: Some(
            if matches!(ending, Ending::Cancel { .. } | Ending::InexactCancel) {
                "interrupted"
            } else {
                "end_turn"
            }
            .into(),
        ),
        cost_usd: match ending {
            Ending::Cancel { cost } => cost,
            _ => None,
        },
        ..Default::default()
    }
}

#[tokio::test]
async fn normal_codex_exec_receipt_is_estimated_not_billed() {
    let usage = receipt_usage_for(Ending::Normal, EngineKind::CODEX, "codex/exec").await;
    assert_eq!(usage.total_tokens(), 110);
    assert_unbilled_estimate(usage, CostGap::Unpriced);
}

#[tokio::test]
async fn normal_codex_app_server_receipt_is_estimated_not_billed() {
    let usage =
        receipt_usage_for(Ending::Normal, EngineKind::CODEX, "codex-app-server/review").await;
    assert_eq!(usage.total_tokens(), 110);
    assert_unbilled_estimate(usage, CostGap::Unpriced);
}

#[tokio::test]
async fn cancelled_codex_app_server_receipt_is_interrupted() {
    let usage = receipt_usage_for(
        Ending::Cancel { cost: None },
        EngineKind::CODEX,
        "codex-app-server/review",
    )
    .await;
    assert_eq!(usage.total_tokens(), 110);
    assert_unbilled_estimate(usage, CostGap::Mixed);
}

/// A subscription turn carries no billed dollars, only the API-equivalent
/// estimate; `gap` is why `cost_usd` is not a complete cost.
fn assert_unbilled_estimate(usage: Usage, gap: CostGap) {
    assert_eq!(usage.cost_usd, 0.0);
    assert!(!usage.cost_complete);
    assert_eq!(usage.cost_gap, Some(gap));
    assert!(usage.estimated_cost_usd.is_some_and(|cost| cost > 0.0));
}

fn streamed_usage(ending: Ending) -> Usage {
    Usage {
        input_tokens: 100,
        output_tokens: 10,
        cost_complete: !matches!(ending, Ending::InexactCancel),
        ..Default::default()
    }
}

#[tokio::test]
async fn claude_code_crash_without_result_is_interrupted() {
    let usage = receipt_usage(Ending::Crash).await;
    assert_eq!(usage.total_tokens(), 110);
    assert_eq!(usage.cost_gap, Some(CostGap::Interrupted));
    assert!(!usage.cost_complete);
}

#[tokio::test]
async fn app_server_closed_mid_turn_is_interrupted() {
    let usage = receipt_usage_for(
        Ending::AppClosed,
        EngineKind::CODEX,
        "codex-app-server/review",
    )
    .await;
    assert_eq!(usage.total_tokens(), 110);
    assert_unbilled_estimate(usage, CostGap::Mixed);
}

#[tokio::test]
async fn native_delegation_stop_is_interrupted() {
    let usage = receipt_usage_for(
        Ending::NativeStop,
        EngineKind::CODEX,
        "codex-app-server/review",
    )
    .await;
    assert_eq!(usage.total_tokens(), 110);
    assert_unbilled_estimate(usage, CostGap::Mixed);
}

fn failed_result() -> EngineOutcome {
    use aether_engine::claude_code::{ParseState, parse_line};
    let events = parse_line(
        &serde_json::json!({
            "type":"result", "subtype":"error_during_execution", "is_error":true,
            "errors":["fixture failure"], "usage":{"input_tokens":100,"output_tokens":10}
        })
        .to_string(),
        &mut ParseState::default(),
    );
    let outcome = events
        .into_iter()
        .find_map(|event| match event {
            EngineEvent::Done(outcome) => Some(outcome),
            _ => None,
        })
        .expect("native result");
    assert!(outcome.is_error);
    assert_eq!(outcome.usage.total_tokens(), 110);
    outcome
}

async fn settled_agent(h: &Harness, id: AgentId) -> aether_core::agent::Agent {
    let agent = tokio::time::timeout(Duration::from_secs(100), async {
        loop {
            let agent = h.store.agent(id).unwrap().unwrap();
            if agent.status.is_terminal()
                || (agent.stats.turns == 1 && agent.status.kind() == AgentStatusKind::Waiting)
            {
                break agent;
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    })
    .await
    .expect("receipt settles");
    assert_eq!(agent.id, id);
    assert_eq!(agent.stats.turns, 1);
    agent
}

async fn after_receipt(tx: &mpsc::Sender<EngineEvent>, ending: Ending, cancel: &CancellationToken) {
    match ending {
        Ending::Rejected => {
            tx.send(EngineEvent::InputRejected {
                reason: "not delivered".into(),
            })
            .await
            .expect("rejection");
        }
        Ending::Background => {
            tx.send(EngineEvent::BackgroundTask {
                id: "job".into(),
                running: false,
            })
            .await
            .expect("background result");
            cancel.cancelled().await;
        }
        _ => {}
    }
}
