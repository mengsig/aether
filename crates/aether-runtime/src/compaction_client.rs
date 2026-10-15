//! Run checkpoint generation in a fresh engine session without the Aether bridge.
use std::sync::Arc;

use crate::turn_engine::TurnPricing;

use aether_core::model::{
    BlockStart, CostGap, ModelError, ModelErrorClass, ModelRequest, StreamEvent, Usage,
};
use aether_engine::{
    Engine, EngineEvent, EngineOutcome, EngineRun, EngineTurn, PermissionDecision,
};
use aether_model::{ModelClient, ModelStream};
use tokio_util::sync::CancellationToken;

pub(crate) struct CompactionClient {
    pub engine: Arc<dyn Engine>,
    pub turn: EngineTurn,
    pub pricing: TurnPricing,
}

#[derive(Default)]
struct Accounting {
    pricing: TurnPricing,
    usage: Option<Usage>,
    closing_received: bool,
    unsubmitted: bool,
}

impl Accounting {
    fn reported(&mut self, mut reported: Usage) {
        debug_assert!(reported.cost_usd.is_finite() && reported.cost_usd >= 0.0);
        debug_assert!(!self.unsubmitted, "unsubmitted work cannot report usage");
        let gap =
            (!reported.cost_complete).then_some(reported.cost_gap.unwrap_or(CostGap::Unreported));
        self.pricing.apply(&mut reported);
        if let Some(gap) = gap {
            reported.set_cost(None);
            reported.cost_gap = Some(gap);
        }
        *self.usage.get_or_insert_default() += reported;
    }

    fn close(&mut self, outcome: &EngineOutcome) {
        debug_assert!(
            outcome
                .cost_usd
                .is_none_or(|cost| cost.is_finite() && cost >= 0.0)
        );
        debug_assert!(!self.unsubmitted, "a launch failure cannot have a receipt");
        self.closing_received = outcome.closing_accounting && !outcome.interrupted;
        let mut total = if outcome.usage.total_tokens() > 0 {
            outcome.usage
        } else {
            self.usage.unwrap_or_default()
        };
        total.reasoning_tokens = total
            .reasoning_tokens
            .or(self.usage.and_then(|u| u.reasoning_tokens));
        self.unsubmitted = outcome.prompt_not_submitted
            && self.usage.is_none()
            && total == Usage::default()
            && outcome.cost_usd.is_none();
        let coverage = (!total.cost_complete)
            .then_some(total.cost_gap.unwrap_or(CostGap::Unreported))
            .or_else(|| {
                (self.usage.is_none() && total.total_tokens() == 0).then_some(CostGap::Unreported)
            });
        if let Some(cost) = outcome.cost_usd {
            total.set_cost(Some(cost));
        } else {
            self.pricing.apply(&mut total);
        }
        if outcome.cost_usd.is_none()
            && let Some(gap) = coverage
        {
            total.set_cost(None);
            total.cost_gap = Some(gap);
        }
        self.usage = Some(if self.unsubmitted {
            Usage::default()
        } else {
            total
        });
    }

    fn finish(&mut self, cancelled: bool) {
        debug_assert!(
            self.usage
                .is_none_or(|usage| usage.cost_usd.is_finite() && usage.cost_usd >= 0.0)
        );
        debug_assert!(
            !self.unsubmitted || self.usage.is_none_or(|usage| usage == Usage::default())
        );
        if self.unsubmitted {
            self.usage = Some(Usage::default());
        } else if cancelled || !self.closing_received {
            self.usage
                .get_or_insert_default()
                .mark_cost_gap(CostGap::Interrupted);
        }
    }
}

#[async_trait::async_trait]
impl ModelClient for CompactionClient {
    fn adapter(&self) -> &str {
        "engine-checkpoint/1"
    }

    async fn stream(
        &self,
        request: ModelRequest,
        cancel: CancellationToken,
    ) -> Result<ModelStream, ModelError> {
        let turn = checkpoint_turn(self.turn.clone(), &request);
        let child = cancel.child_token();
        let _cleanup = child.clone().drop_guard();
        let mut accounting = Accounting {
            pricing: self.pricing,
            ..Default::default()
        };
        let run = tokio::time::timeout(
            std::time::Duration::from_secs(180),
            collect_checkpoint(self.engine.as_ref(), turn, child.clone(), &mut accounting),
        );
        let result = tokio::select! {
            result = run => result.unwrap_or_else(|_| Err(failure("Checkpoint engine timed out"))),
            _ = cancel.cancelled() => Err(ModelError::new(ModelErrorClass::Cancelled, "Checkpoint generation cancelled")),
        };
        child.cancel();
        accounting.finish(cancel.is_cancelled());
        Ok(checkpoint_stream(result, accounting.usage))
    }
}

fn checkpoint_turn(mut turn: EngineTurn, request: &ModelRequest) -> EngineTurn {
    debug_assert!(turn.session.is_none(), "checkpoint sessions must be fresh");
    debug_assert!(
        turn.mcp.is_none(),
        "checkpoint generation has no harness tools"
    );
    turn.new_session_id = aether_engine::new_session_id();
    turn.system_prompt = request
        .system
        .iter()
        .map(|b| b.text.as_str())
        .collect::<Vec<_>>()
        .join("\n\n");
    turn.prompt = request
        .messages
        .iter()
        .map(|m| m.text())
        .collect::<Vec<_>>()
        .join("\n\n");
    turn.effort = request.effort;
    turn
}

async fn collect_checkpoint(
    engine: &dyn Engine,
    turn: EngineTurn,
    cancel: CancellationToken,
    accounting: &mut Accounting,
) -> Result<String, ModelError> {
    debug_assert!(turn.session.is_none());
    debug_assert!(accounting.usage.is_none() && !accounting.closing_received);
    let mut run = engine.run(turn, cancel.clone()).await.map_err(|error| {
        accounting.unsubmitted = error.proves_no_submission();
        failure(error.to_string())
    })?;
    let mut text = String::new();
    while let Some(event) = run.events.recv().await {
        match event {
            EngineEvent::Assistant { content, usage, .. } => {
                let message = aether_core::model::text_of(&content);
                if !message.trim().is_empty() {
                    text = message;
                }
                if let Some(usage) = usage {
                    accounting.reported(usage);
                }
            }
            EngineEvent::UsageDelta { usage } => accounting.reported(usage),
            EngineEvent::UsageRequest { request } => accounting.reported(request.delta()),
            EngineEvent::Permission { request_id, .. }
            | EngineEvent::Question { request_id, .. } => {
                refuse_checkpoint_input(&run, request_id).await?;
            }
            EngineEvent::Done(outcome) => {
                accounting.close(&outcome);
                return checkpoint_result(outcome, text);
            }
            _ => {}
        }
    }
    Err(if cancel.is_cancelled() {
        ModelError::new(
            ModelErrorClass::Cancelled,
            "Checkpoint generation cancelled",
        )
    } else {
        failure("Checkpoint engine ended without a result")
    })
}

async fn refuse_checkpoint_input(run: &EngineRun, request_id: String) -> Result<(), ModelError> {
    let Some(decisions) = &run.decisions else {
        return Err(failure("Checkpoint engine requested unavailable input"));
    };
    if request_id.is_empty() {
        return Err(failure("Checkpoint engine requested input without an id"));
    }
    decisions
        .send(PermissionDecision {
            request_id,
            allow: false,
            message: Some("Checkpoint generation cannot use tools or ask questions".into()),
            answer: None,
        })
        .await
        .map_err(|error| failure(error.to_string()))
}

fn checkpoint_result(outcome: EngineOutcome, text: String) -> Result<String, ModelError> {
    if outcome.is_error {
        return Err(failure(
            outcome
                .error
                .unwrap_or_else(|| "Checkpoint engine failed".into()),
        ));
    }
    let text = outcome.text.unwrap_or(text);
    if text.trim().is_empty() {
        return Err(failure("Checkpoint engine returned no text"));
    }
    Ok(text)
}

fn checkpoint_stream(result: Result<String, ModelError>, usage: Option<Usage>) -> ModelStream {
    debug_assert!(usage.is_none_or(|usage| usage.cost_usd.is_finite() && usage.cost_usd >= 0.0));
    debug_assert!(usage.is_none_or(|usage| usage.cost_complete == usage.cost_gap.is_none()));
    let mut events = vec![Ok(StreamEvent::MessageStart {
        provider_message_id: None,
        model: None,
    })];
    if let Some(usage) = usage {
        events.push(Ok(StreamEvent::Usage { usage }));
    }
    match result {
        Ok(text) => events.extend([
            Ok(StreamEvent::BlockStart {
                index: 0,
                block: BlockStart::Text {
                    text: String::new(),
                },
            }),
            Ok(StreamEvent::TextDelta { index: 0, text }),
            Ok(StreamEvent::BlockStop { index: 0 }),
            Ok(StreamEvent::MessageStop),
        ]),
        Err(error) => events.push(Err(error)),
    }
    Box::pin(futures::stream::iter(events))
}

fn failure(message: impl Into<String>) -> ModelError {
    let message = message.into();
    ModelError::new(crate::exhaustion::engine_error_class(&message), message)
}

#[cfg(test)]
#[allow(clippy::expect_used, clippy::unwrap_used)]
mod tests {
    use super::*;
    use aether_core::model::{Message, ModelRef};
    use aether_engine::{EngineError, EngineKind, EngineOutcome, EngineRun};
    use tokio::sync::mpsc;

    #[derive(Clone, Copy)]
    enum Ending {
        Failed,
        Exhausted,
        Cancelled,
        Timeout,
        Truncated,
        NotInstalled,
        Unsupported,
    }

    struct CheckpointEngine {
        ending: Ending,
        caller: CancellationToken,
    }

    #[async_trait::async_trait]
    impl Engine for CheckpointEngine {
        fn kind(&self) -> EngineKind {
            EngineKind::CLAUDE_CODE
        }
        fn adapter(&self) -> String {
            "checkpoint-fixture".into()
        }
        async fn run(
            &self,
            _turn: EngineTurn,
            cancel: CancellationToken,
        ) -> Result<EngineRun, EngineError> {
            match self.ending {
                Ending::NotInstalled => {
                    return Err(EngineError::NotInstalled {
                        program: "fixture".into(),
                    });
                }
                Ending::Unsupported => {
                    return Err(EngineError::Unsupported("fixture policy".into()));
                }
                _ => {}
            }
            let (tx, events) = mpsc::channel(4);
            let (decisions, replies) = mpsc::channel::<PermissionDecision>(1);
            let ending = self.ending;
            let caller = self.caller.clone();
            tokio::spawn(checkpoint_script(tx, replies, ending, caller, cancel));
            Ok(EngineRun {
                events,
                input: None,
                decisions: Some(decisions),
                stop: None,
            })
        }
    }

    async fn checkpoint_script(
        tx: mpsc::Sender<EngineEvent>,
        mut replies: mpsc::Receiver<PermissionDecision>,
        ending: Ending,
        caller: CancellationToken,
        cancel: CancellationToken,
    ) {
        tx.send(EngineEvent::UsageRequest {
            request: aether_engine::RequestUsage {
                id: aether_engine::UsageRequestId::Engine("checkpoint".into()),
                previous: Usage::default(),
                total: Usage {
                    input_tokens: 13,
                    output_tokens: 5,
                    reasoning_tokens: Some(2),
                    ..Default::default()
                },
            },
        })
        .await
        .expect("usage");
        // The refusal acknowledges that the preceding usage was read,
        // making cancellation independent of task scheduling order.
        tx.send(EngineEvent::Question {
            request_id: "q".into(),
            prompt: "input".into(),
        })
        .await
        .expect("question");
        assert!(!replies.recv().await.expect("refusal").allow);
        match ending {
            Ending::Failed | Ending::Exhausted => tx
                .send(EngineEvent::Done(failed_receipt(ending)))
                .await
                .expect("failure"),
            Ending::Cancelled => {
                caller.cancel();
                cancel.cancelled().await;
            }
            Ending::Timeout => cancel.cancelled().await,
            Ending::Truncated => {}
            Ending::NotInstalled | Ending::Unsupported => unreachable!(),
        }
    }

    fn failed_receipt(ending: Ending) -> EngineOutcome {
        EngineOutcome {
            closing_accounting: true,
            usage: Usage {
                input_tokens: 23,
                output_tokens: 7,
                ..Default::default()
            },
            cost_usd: Some(1.25),
            is_error: true,
            error: Some(if matches!(ending, Ending::Exhausted) {
                "Weekly limit reached".into()
            } else {
                "provider failed".into()
            }),
            ..Default::default()
        }
    }

    async fn failed_checkpoint(ending: Ending) -> aether_model::PartialFailure {
        let cancel = CancellationToken::new();
        let client = CompactionClient {
            pricing: TurnPricing::default(),
            engine: Arc::new(CheckpointEngine {
                ending,
                caller: cancel.clone(),
            }),
            turn: EngineTurn {
                prompt: String::new(),
                images: Vec::new(),
                system_prompt: String::new(),
                session: None,
                session_usage: aether_core::model::Usage::default(),
                new_session_id: String::new(),
                session_recorder: None,
                model: None,
                effort: None,
                cwd: "/unused-fixture".into(),
                extra_dirs: vec![],
                summary_only: true,
                lean_host_context: false,
                network_access: true,
                bypass_permissions: false,
                hidden_env: vec![],
                tool_hidden_env: vec![],
                mcp: None,
            },
        };
        let request = ModelRequest::new(
            ModelRef::new("claude-code", "sonnet"),
            vec![Message::user_text("summarize")],
        );
        let stream = client.stream(request, cancel).await.expect("stream");
        *aether_model::collect_partial(stream, |_| {})
            .await
            .expect_err("failed checkpoint")
    }

    #[tokio::test]
    async fn provider_failure_keeps_authoritative_cost_and_final_usage() {
        let failure = failed_checkpoint(Ending::Failed).await;
        assert!(failure.error.message.contains("provider failed"));
        assert_eq!(failure.partial.usage.input_tokens, 23);
        assert_eq!(failure.partial.usage.output_tokens, 7);
        assert_eq!(failure.partial.usage.reasoning_tokens, Some(2));
        assert_eq!(failure.partial.usage.cost_usd, 1.25);
        assert!(failure.partial.usage.cost_complete);
    }

    #[tokio::test(start_paused = true)]
    async fn cancellation_timeout_and_truncation_keep_partial_usage_unpriced() {
        for ending in [Ending::Cancelled, Ending::Timeout, Ending::Truncated] {
            let failure = failed_checkpoint(ending).await;
            match ending {
                Ending::Cancelled => assert_eq!(failure.error.class, ModelErrorClass::Cancelled),
                Ending::Timeout => assert!(failure.error.message.contains("timed out")),
                Ending::Truncated => assert!(failure.error.message.contains("without a result")),
                Ending::Failed | Ending::Exhausted | Ending::NotInstalled | Ending::Unsupported => {
                    unreachable!()
                }
            }
            assert_eq!(failure.partial.usage.input_tokens, 13);
            assert_eq!(failure.partial.usage.output_tokens, 5);
            assert!(!failure.partial.usage.cost_complete);
        }
    }
    #[tokio::test]
    async fn checkpoint_engine_preserves_exhaustion_class_and_usage() {
        let failure = failed_checkpoint(Ending::Exhausted).await;
        assert_eq!(failure.error.class, ModelErrorClass::UsageLimit);
        assert_eq!(failure.partial.usage.input_tokens, 23);
    }
    #[tokio::test]
    async fn prelaunch_checkpoint_errors_keep_complete_zero() {
        for ending in [Ending::NotInstalled, Ending::Unsupported] {
            let failure = failed_checkpoint(ending).await;
            assert!(failure.partial.usage.cost_complete);
            assert_eq!(failure.partial.usage, Usage::default());
        }
    }
    #[test]
    fn failed_checkpoint_receipt_without_dollars_is_unpriced_not_interrupted() {
        let mut accounting = Accounting::default();
        let mut outcome = failed_receipt(Ending::Failed);
        outcome.cost_usd = None;
        accounting.close(&outcome);
        accounting.finish(false);
        let usage = accounting.usage.unwrap();
        assert_eq!(usage.total_tokens(), 30);
        assert_eq!(usage.cost_gap, Some(CostGap::Unpriced));
        assert!(!usage.cost_complete);
    }

    #[test]
    fn cancelled_checkpoint_receipt_with_dollars_remains_interrupted() {
        let mut accounting = Accounting::default();
        accounting.close(&failed_receipt(Ending::Failed));
        accounting.finish(true);
        let usage = accounting.usage.unwrap();
        assert_eq!(usage.cost_usd, 1.25);
        assert_eq!(usage.cost_gap, Some(CostGap::Interrupted));
        assert!(!usage.cost_complete);
    }
    #[test]
    fn synthetic_checkpoint_done_keeps_known_cost_as_an_interrupted_floor() {
        let mut accounting = Accounting::default();
        let mut outcome = failed_receipt(Ending::Failed);
        outcome.closing_accounting = false;
        accounting.close(&outcome);
        accounting.finish(false);
        let usage = accounting.usage.unwrap();
        assert_eq!(usage.cost_usd, 1.25);
        assert_eq!(usage.total_tokens(), 30);
        assert!(!usage.cost_complete);
        assert_eq!(usage.cost_gap, Some(CostGap::Interrupted));
    }
    #[tokio::test]
    async fn known_price_zero_token_receipt_keeps_reported_coverage() {
        let (runtime, _dir) = crate::test_runtime(|_| {}).await;
        let resolved = runtime
            .models
            .resolve(&ModelRef::new("claude-code", "haiku"))
            .unwrap();
        let mut accounting = Accounting {
            pricing: TurnPricing::of(&runtime, &resolved),
            ..Default::default()
        };
        accounting.reported(Usage::default());
        accounting.close(&EngineOutcome {
            closing_accounting: true,
            ..Default::default()
        });
        accounting.finish(false);
        let usage = accounting.usage.unwrap();
        assert_eq!(usage.total_tokens(), 0);
        assert_eq!(usage.cost_usd, 0.0);
        assert!(usage.cost_complete);
        assert_eq!(usage.cost_gap, None);
        runtime.shutdown().await;
    }
}
