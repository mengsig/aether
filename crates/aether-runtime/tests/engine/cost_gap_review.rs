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
    assert_eq!(agent.stats.turns, 1);
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
    tx.send(EngineEvent::UsageDelta {
        usage: Usage {
            input_tokens: 100,
            output_tokens: 10,
            cost_complete: !matches!(ending, Ending::InexactCancel),
            ..Default::default()
        },
    })
    .await
    .expect("usage");
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

fn receipt_outcome(ending: Ending) -> EngineOutcome {
    EngineOutcome {
        text: Some("done".into()),
        is_error: matches!(ending, Ending::Failed),
        error: matches!(ending, Ending::Failed).then(|| "fixture failure".into()),
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
async fn normal_codex_exec_receipt_stays_complete() {
    let usage = receipt_usage_for(Ending::Normal, EngineKind::CODEX, "codex/exec").await;
    assert_eq!(usage.total_tokens(), 110);
    assert!(usage.cost_complete);
    assert_eq!(usage.cost_gap, None);
}

#[tokio::test]
async fn normal_codex_app_server_receipt_stays_complete() {
    let usage =
        receipt_usage_for(Ending::Normal, EngineKind::CODEX, "codex-app-server/review").await;
    assert_eq!(usage.total_tokens(), 110);
    assert!(usage.cost_complete);
    assert_eq!(usage.cost_gap, None);
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
    assert!(!usage.cost_complete);
    assert_eq!(usage.cost_gap, Some(CostGap::Interrupted));
}
