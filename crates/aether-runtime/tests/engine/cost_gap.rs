//! Cost coverage across cancellation and terminal usage receipts.
use super::*;
use aether_core::model::{CostGap, Usage};

struct CancelUsageEngine {
    submitted: bool,
    tokens: bool,
    started: Arc<tokio::sync::Notify>,
}

#[async_trait::async_trait]
impl Engine for CancelUsageEngine {
    fn kind(&self) -> EngineKind {
        EngineKind::CLAUDE_CODE
    }

    fn adapter(&self) -> String {
        "cancel-usage/1".into()
    }

    async fn run(
        &self,
        turn: EngineTurn,
        cancel: CancellationToken,
    ) -> Result<EngineRun, EngineError> {
        assert!(!turn.prompt.is_empty());
        assert!(turn.mcp.is_some());
        let (tx, rx) = mpsc::channel(4);
        let (submitted, tokens, started) = (self.submitted, self.tokens, self.started.clone());
        tokio::spawn(async move {
            if tokens {
                tx.send(EngineEvent::UsageDelta {
                    usage: Usage {
                        input_tokens: 100,
                        output_tokens: 10,
                        ..Default::default()
                    },
                })
                .await
                .expect("usage receiver");
            }
            started.notify_one();
            cancel.cancelled().await;
            if !submitted {
                tx.send(EngineEvent::Done(EngineOutcome {
                    is_error: true,
                    error: Some("cancelled before submission".into()),
                    prompt_not_submitted: true,
                    ..Default::default()
                }))
                .await
                .expect("closing receiver");
            }
        });
        Ok(EngineRun {
            events: rx,
            input: None,
            decisions: None,
            stop: None,
        })
    }
}

async fn cancelled_usage(submitted: bool, tokens: bool, priced: bool) -> Usage {
    assert!(
        submitted || !tokens,
        "unsubmitted work cannot report tokens"
    );
    let started = Arc::new(tokio::sync::Notify::new());
    let h = harness_with_engine_config(
        Arc::new(CancelUsageEngine {
            submitted,
            tokens,
            started: started.clone(),
        }),
        |config| {
            config.default_model = Some(ModelRef::new(
                "claude-code",
                if priced { "haiku" } else { "default" },
            ))
        },
    )
    .await;
    let id = chat_agent(&h);
    h.runtime
        .send_message(id, vec![ContentBlock::text("go")], Actor::user())
        .unwrap();
    tokio::time::timeout(Duration::from_secs(10), started.notified())
        .await
        .unwrap();
    let before = h.store.agent(id).unwrap().unwrap().stats.usage;
    h.runtime
        .cancel_agent(id, Some("stop".into()), Actor::user())
        .unwrap();
    let agent = wait_for(h.store.as_ref(), id, |a| a.status.is_terminal()).await;
    let usage = agent.stats.usage;
    assert!(usage.total_tokens() >= before.total_tokens());
    assert!(usage.cost_usd >= before.cost_usd);
    let events = h
        .store
        .events(0, 1000, &EventFilter::for_agent(id))
        .unwrap();
    let report = aether_core::report::build(agent.run, &events, aether_core::time::now());
    assert_eq!(report.body.usage.cost_gap, usage.cost_gap);
    assert_eq!(report.body.usage.cost_complete, usage.cost_complete);
    assert_eq!(report.body.usage.cost_usd, usage.cost_usd);
    assert_eq!(report.body.usage.total_tokens(), usage.total_tokens());
    h.runtime.shutdown().await;
    usage
}

#[tokio::test]
async fn cancellation_without_a_receipt_is_interrupted() {
    let usage = cancelled_usage(true, false, false).await;
    assert!(!usage.cost_complete);
    assert_eq!(usage.cost_gap, Some(CostGap::Interrupted));
    assert_eq!(usage.cost_usd, 0.0);
}

#[tokio::test]
async fn proven_unsubmitted_cancellation_keeps_complete_zero() {
    let usage = cancelled_usage(false, false, false).await;
    assert!(usage.cost_complete);
    assert_eq!(usage.cost_gap, None);
    assert_eq!(usage.total_tokens(), 0);
    assert_eq!(usage.cost_usd, 0.0);
}

#[tokio::test]
async fn unpriced_stream_then_cancel_combines_both_gaps() {
    let usage = cancelled_usage(true, true, false).await;
    assert!(!usage.cost_complete);
    assert_eq!(usage.cost_gap, Some(CostGap::Mixed));
    assert_eq!(usage.total_tokens(), 110);
    assert_eq!(usage.cost_usd, 0.0);
}

#[tokio::test]
async fn terminal_usage_without_a_model_price_is_unpriced() {
    let h = harness_with_engine_config(Arc::new(super::pricing::UsageEngine), |config| {
        config.default_model = Some(ModelRef::new("claude-code", "default"))
    })
    .await;
    let id = chat_agent(&h);
    h.runtime
        .send_message(id, vec![ContentBlock::text("go")], Actor::user())
        .unwrap();
    let agent = wait_for(h.store.as_ref(), id, |a| {
        a.stats.turns == 1 && a.status.kind() == AgentStatusKind::Waiting
    })
    .await;
    assert_eq!(agent.stats.usage.cost_gap, Some(CostGap::Unpriced));
    assert!(!agent.stats.usage.cost_complete);
    assert_eq!(agent.stats.usage.total_tokens(), 150_000);
    assert_eq!(agent.stats.usage.cost_usd, 0.0);
    h.runtime.shutdown().await;
}

#[tokio::test]
async fn cancellation_keeps_streamed_tokens_and_priced_dollars_as_a_floor() {
    let usage = cancelled_usage(true, true, true).await;
    assert_eq!(usage.cost_gap, Some(CostGap::Interrupted));
    assert!(!usage.cost_complete);
    assert_eq!(usage.total_tokens(), 110);
    assert!(usage.cost_usd > 0.0);
}
