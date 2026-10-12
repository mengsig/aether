//! Run checkpoint generation in a fresh engine session without the Aether bridge.
use std::sync::Arc;

use aether_core::model::{
    BlockStart, ModelError, ModelErrorClass, ModelRequest, StreamEvent, Usage,
};
use aether_engine::{Engine, EngineEvent, EngineTurn, PermissionDecision};
use aether_model::{ModelClient, ModelStream};
use tokio_util::sync::CancellationToken;

pub(crate) struct CompactionClient {
    pub engine: Arc<dyn Engine>,
    pub turn: EngineTurn,
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
        let mut turn = self.turn.clone();
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
        let child = cancel.child_token();
        let _cleanup = child.clone().drop_guard();
        let mut usage: Option<Usage> = None;
        let run = tokio::time::timeout(std::time::Duration::from_secs(180), async {
            let mut run = self
                .engine
                .run(turn, child.clone())
                .await
                .map_err(|e| failure(e.to_string()))?;
            let mut text = String::new();
            while let Some(event) = run.events.recv().await {
                match event {
                    EngineEvent::Assistant {
                        content,
                        usage: reported,
                        ..
                    } => {
                        let message = aether_core::model::text_of(&content);
                        if !message.trim().is_empty() {
                            text = message;
                        }
                        if let Some(mut reported) = reported {
                            reported.set_cost(None);
                            *usage.get_or_insert_default() += reported;
                        }
                    }
                    EngineEvent::UsageDelta {
                        usage: mut reported,
                    } => {
                        reported.set_cost(None);
                        *usage.get_or_insert_default() += reported;
                    }
                    EngineEvent::UsageRequest { request } => {
                        let mut reported = request.delta();
                        reported.set_cost(None);
                        *usage.get_or_insert_default() += reported;
                    }
                    EngineEvent::Permission { request_id, .. }
                    | EngineEvent::Question { request_id, .. } => {
                        if let Some(decisions) = &run.decisions {
                            decisions
                                .send(PermissionDecision {
                                    request_id,
                                    allow: false,
                                    message: Some(
                                        "Checkpoint generation cannot use tools or ask questions"
                                            .into(),
                                    ),
                                    answer: None,
                                })
                                .await
                                .map_err(|e| failure(e.to_string()))?;
                        } else {
                            return Err(failure("Checkpoint engine requested unavailable input"));
                        }
                    }
                    EngineEvent::Done(outcome) => {
                        let mut total = if outcome.usage.total_tokens() > 0 {
                            outcome.usage
                        } else {
                            usage.unwrap_or_default()
                        };
                        total.reasoning_tokens = total
                            .reasoning_tokens
                            .or(usage.and_then(|u| u.reasoning_tokens));
                        total.set_cost(outcome.cost_usd);
                        usage = Some(total);
                        if outcome.is_error {
                            return Err(failure(
                                outcome
                                    .error
                                    .unwrap_or_else(|| "Checkpoint engine failed".into()),
                            ));
                        }
                        if let Some(final_text) = outcome.text {
                            text = final_text;
                        }
                        if text.trim().is_empty() {
                            return Err(failure("Checkpoint engine returned no text"));
                        }
                        return Ok(text);
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
        });
        let result = tokio::select! {
            result = run => result.unwrap_or_else(|_| Err(failure("Checkpoint engine timed out"))),
            _ = cancel.cancelled() => Err(ModelError::new(ModelErrorClass::Cancelled, "Checkpoint generation cancelled")),
        };
        child.cancel();
        let mut events = vec![Ok(StreamEvent::MessageStart {
            provider_message_id: None,
            model: None,
        })];
        // Report consumed usage before an error, so a failed or cancelled
        // checkpoint cannot disappear from the compactor's attempt ledger.
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
        Ok(Box::pin(futures::stream::iter(events)))
    }
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
            let (tx, events) = mpsc::channel(4);
            let (decisions, mut replies) = mpsc::channel::<PermissionDecision>(1);
            let ending = self.ending;
            let caller = self.caller.clone();
            tokio::spawn(async move {
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
                        .send(EngineEvent::Done(EngineOutcome {
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
                        }))
                        .await
                        .expect("failure"),
                    Ending::Cancelled => {
                        caller.cancel();
                        cancel.cancelled().await;
                    }
                    Ending::Timeout => cancel.cancelled().await,
                    Ending::Truncated => {}
                }
            });
            Ok(EngineRun {
                events,
                input: None,
                decisions: Some(decisions),
                stop: None,
            })
        }
    }

    async fn failed_checkpoint(ending: Ending) -> aether_model::PartialFailure {
        let cancel = CancellationToken::new();
        let client = CompactionClient {
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
                model: None,
                effort: None,
                cwd: "/unused-fixture".into(),
                extra_dirs: vec![],
                summary_only: true,
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
                Ending::Failed | Ending::Exhausted => unreachable!(),
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
}
