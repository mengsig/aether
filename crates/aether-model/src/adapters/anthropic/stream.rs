//! Anthropic SSE events → normalized stream events.

use std::collections::HashSet;

use aether_core::model::{BlockStart, ModelError, ModelErrorClass, StopReason, StreamEvent, Usage};
use serde_json::Value;

use crate::adapters::streaming::EventMapper;
use crate::http::{interrupted_stream, is_context_limit_message, safe_provider_text};
use crate::sse::SseEvent;

#[derive(Debug, Default)]
pub struct AnthropicMapper {
    usage: Usage,
    open_blocks: HashSet<usize>,
    /// A `message_delta` carried a stop reason: the model said it finished.
    stopped: bool,
    message_stopped: bool,
    started: bool,
}

impl AnthropicMapper {
    pub fn new() -> Self {
        Self::default()
    }

    fn absorb_usage(&mut self, usage: &Value) {
        let get = |k: &str| usage.get(k).and_then(Value::as_u64);
        if let Some(v) = get("input_tokens") {
            self.usage.input_tokens = v;
        }
        if let Some(v) = get("output_tokens") {
            self.usage.output_tokens = v;
        }
        if let Some(v) = get("cache_read_input_tokens") {
            self.usage.cache_read_tokens = v;
        }
        if let Some(v) = get("cache_creation_input_tokens") {
            self.usage.cache_write_tokens = v;
        }
        if let Some(v) = usage
            .get("cache_creation")
            .and_then(|split| split.get("ephemeral_1h_input_tokens"))
            .and_then(Value::as_u64)
        {
            self.usage.cache_write_1h_tokens = v;
        }
        // An untrusted split beyond its total cannot be priced; bill the total at the 1-hour rate.
        if self.usage.cache_write_1h_tokens > self.usage.cache_write_tokens {
            tracing::warn!(
                one_hour = self.usage.cache_write_1h_tokens,
                cache_write_tokens = self.usage.cache_write_tokens,
                "Anthropic reported more 1-hour cache writes than cache writes"
            );
            self.usage.cache_write_1h_tokens = self.usage.cache_write_tokens;
        }
    }
}

impl EventMapper for AnthropicMapper {
    fn map(&mut self, event: &SseEvent) -> Result<Vec<StreamEvent>, ModelError> {
        let data: Value = serde_json::from_str(&event.data).map_err(|e| {
            ModelError::new(
                ModelErrorClass::Unknown,
                format!(
                    "malformed SSE data: {e}: {}",
                    safe_provider_text(&event.data, 512)
                ),
            )
        })?;
        let kind = event
            .event
            .as_deref()
            .or_else(|| data.get("type").and_then(Value::as_str))
            .unwrap_or("");
        let mut out = Vec::new();
        match kind {
            "message_start" => {
                self.started = true;
                let message = &data["message"];
                out.push(StreamEvent::MessageStart {
                    provider_message_id: message
                        .get("id")
                        .and_then(Value::as_str)
                        .map(str::to_owned),
                    model: message
                        .get("model")
                        .and_then(Value::as_str)
                        .map(str::to_owned),
                });
                if let Some(usage) = message.get("usage") {
                    self.absorb_usage(usage);
                    out.push(StreamEvent::Usage { usage: self.usage });
                }
            }
            "content_block_start" => {
                let index = index_of(&data)?;
                let block = &data["content_block"];
                let start = match block.get("type").and_then(Value::as_str).unwrap_or("") {
                    "text" => BlockStart::Text {
                        text: str_of(block, "text"),
                    },
                    "thinking" => BlockStart::Thinking {
                        text: str_of(block, "thinking"),
                        redacted: false,
                        signature: block
                            .get("signature")
                            .and_then(Value::as_str)
                            .map(str::to_owned),
                    },
                    "redacted_thinking" => BlockStart::Thinking {
                        text: String::new(),
                        redacted: true,
                        signature: block.get("data").and_then(Value::as_str).map(str::to_owned),
                    },
                    "tool_use" => BlockStart::ToolUse {
                        id: str_of(block, "id"),
                        name: str_of(block, "name"),
                        input: None,
                    },
                    // Server-side tool blocks and future types are surfaced
                    // verbatim rather than dropped, so nothing is hidden.
                    other => {
                        tracing::debug!(
                            block_type = other,
                            "surfacing unknown content block as text"
                        );
                        BlockStart::Text {
                            text: block.to_string(),
                        }
                    }
                };
                self.open_blocks.insert(index);
                out.push(StreamEvent::BlockStart {
                    index,
                    block: start,
                });
            }
            "content_block_delta" => {
                let index = index_of(&data)?;
                let delta = &data["delta"];
                match delta.get("type").and_then(Value::as_str).unwrap_or("") {
                    "text_delta" => out.push(StreamEvent::TextDelta {
                        index,
                        text: str_of(delta, "text"),
                    }),
                    "input_json_delta" => out.push(StreamEvent::ToolInputDelta {
                        index,
                        partial_json: str_of(delta, "partial_json"),
                    }),
                    "thinking_delta" => out.push(StreamEvent::ThinkingDelta {
                        index,
                        text: str_of(delta, "thinking"),
                    }),
                    "signature_delta" => out.push(StreamEvent::SignatureDelta {
                        index,
                        signature: str_of(delta, "signature"),
                    }),
                    // Citations annotate text already delivered.
                    "citations_delta" => {}
                    other => tracing::debug!(delta_type = other, "ignoring unknown delta"),
                }
            }
            "content_block_stop" => {
                let index = index_of(&data)?;
                self.open_blocks.remove(&index);
                out.push(StreamEvent::BlockStop { index });
            }
            "message_delta" => {
                if let Some(usage) = data.get("usage") {
                    self.absorb_usage(usage);
                    out.push(StreamEvent::Usage { usage: self.usage });
                }
                let delta = &data["delta"];
                if let Some(reason) = delta.get("stop_reason").and_then(Value::as_str) {
                    let details = delta
                        .get("stop_details")
                        .or_else(|| data.get("stop_details"));
                    out.push(StreamEvent::Stop {
                        reason: stop_reason(reason, details),
                    });
                    self.stopped = true;
                }
            }
            "message_stop" => {
                self.message_stopped = true;
                out.push(StreamEvent::MessageStop);
            }
            "error" => return Err(stream_error(&data["error"])),
            "ping" => {}
            other => tracing::debug!(event = other, "ignoring unknown anthropic event"),
        }
        Ok(out)
    }

    fn finish(&mut self) -> Result<Vec<StreamEvent>, ModelError> {
        // A missing `message_stop` after a stop reason is only the envelope;
        // no stop reason at all means the answer itself was cut short.
        if self.started && !self.stopped && !self.message_stopped {
            return Err(interrupted_stream());
        }
        let mut out = Vec::new();
        let mut open: Vec<usize> = self.open_blocks.drain().collect();
        open.sort_unstable();
        for index in open {
            out.push(StreamEvent::BlockStop { index });
        }
        if !self.message_stopped && self.stopped {
            out.push(StreamEvent::MessageStop);
        }
        Ok(out)
    }
}

fn index_of(data: &Value) -> Result<usize, ModelError> {
    data.get("index")
        .and_then(Value::as_u64)
        .map(|i| i as usize)
        .ok_or_else(|| ModelError::new(ModelErrorClass::Unknown, "anthropic event without index"))
}

fn str_of(v: &Value, key: &str) -> String {
    v.get(key).and_then(Value::as_str).unwrap_or("").to_owned()
}

pub fn stop_reason(reason: &str, details: Option<&Value>) -> StopReason {
    match reason {
        "end_turn" => StopReason::EndTurn,
        "tool_use" => StopReason::ToolUse,
        "max_tokens" => StopReason::MaxTokens,
        "stop_sequence" => StopReason::StopSequence,
        "pause_turn" => StopReason::PauseTurn,
        "refusal" => StopReason::Refusal {
            category: details
                .and_then(|d| d.get("category"))
                .and_then(Value::as_str)
                .map(str::to_owned),
        },
        other => StopReason::Other {
            value: other.to_owned(),
        },
    }
}

/// Map an in-stream `error` event.
fn stream_error(error: &Value) -> ModelError {
    let kind = error.get("type").and_then(Value::as_str).unwrap_or("");
    let message = safe_provider_text(
        error
            .get("message")
            .and_then(Value::as_str)
            .unwrap_or("unknown error"),
        2000,
    );
    let exhaustion = aether_core::model::provider_exhaustion(&message);
    let class = if matches!(kind, "invalid_request_error" | "rate_limit_error")
        && let Some(class) = exhaustion
    {
        class
    } else {
        match kind {
            "overloaded_error" => ModelErrorClass::Overloaded,
            "rate_limit_error" => ModelErrorClass::RateLimit,
            "authentication_error" => ModelErrorClass::Auth,
            "permission_error" => ModelErrorClass::Permission,
            "not_found_error" => ModelErrorClass::NotFound,
            "request_too_large" => ModelErrorClass::ContextLimit,
            "invalid_request_error" => {
                if is_context_limit_message(&message.to_lowercase()) {
                    ModelErrorClass::ContextLimit
                } else {
                    ModelErrorClass::InvalidRequest
                }
            }
            "api_error" => ModelErrorClass::Server,
            _ => ModelErrorClass::Unknown,
        }
    };
    ModelError::new(class, message).with_provider_code(safe_provider_text(kind, 200))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::adapters::streaming::mapped_stream;
    use crate::client::collect;
    use serde_json::json;

    fn sse(event: &str, data: Value) -> SseEvent {
        SseEvent {
            event: Some(event.into()),
            data: data.to_string(),
            id: None,
        }
    }

    fn fixture() -> Vec<SseEvent> {
        vec![
            sse(
                "message_start",
                json!({"type":"message_start","message":{"id":"msg_1","type":"message","role":"assistant","model":"claude-opus-5","content":[],"stop_reason":null,"usage":{"input_tokens":25,"cache_creation_input_tokens":10,"cache_read_input_tokens":5,"output_tokens":1,"cache_creation":{"ephemeral_5m_input_tokens":6,"ephemeral_1h_input_tokens":4}}}}),
            ),
            sse(
                "content_block_start",
                json!({"type":"content_block_start","index":0,"content_block":{"type":"thinking","thinking":""}}),
            ),
            sse(
                "content_block_delta",
                json!({"type":"content_block_delta","index":0,"delta":{"type":"thinking_delta","thinking":"Let me think"}}),
            ),
            sse(
                "content_block_delta",
                json!({"type":"content_block_delta","index":0,"delta":{"type":"signature_delta","signature":"abc"}}),
            ),
            sse(
                "content_block_stop",
                json!({"type":"content_block_stop","index":0}),
            ),
            sse(
                "content_block_start",
                json!({"type":"content_block_start","index":1,"content_block":{"type":"text","text":""}}),
            ),
            sse("ping", json!({"type":"ping"})),
            sse(
                "content_block_delta",
                json!({"type":"content_block_delta","index":1,"delta":{"type":"text_delta","text":"I'll check."}}),
            ),
            sse(
                "content_block_stop",
                json!({"type":"content_block_stop","index":1}),
            ),
            sse(
                "content_block_start",
                json!({"type":"content_block_start","index":2,"content_block":{"type":"tool_use","id":"toolu_1","name":"get_weather","input":{}}}),
            ),
            sse(
                "content_block_delta",
                json!({"type":"content_block_delta","index":2,"delta":{"type":"input_json_delta","partial_json":"{\"location\":"}}),
            ),
            sse(
                "content_block_delta",
                json!({"type":"content_block_delta","index":2,"delta":{"type":"input_json_delta","partial_json":" \"Paris\"}"}}),
            ),
            sse(
                "content_block_stop",
                json!({"type":"content_block_stop","index":2}),
            ),
            sse(
                "message_delta",
                json!({"type":"message_delta","delta":{"stop_reason":"tool_use","stop_sequence":null},"usage":{"output_tokens":42}}),
            ),
            sse("message_stop", json!({"type":"message_stop"})),
        ]
    }

    #[tokio::test]
    async fn fixture_maps_to_complete_response() {
        let stream = mapped_stream(fixture(), AnthropicMapper::new());
        let mut seen = 0;
        let resp = collect(stream, |_| seen += 1).await.expect("collect");
        assert!(seen > 10);
        assert_eq!(resp.provider_message_id.as_deref(), Some("msg_1"));
        assert_eq!(resp.model.as_deref(), Some("claude-opus-5"));
        assert_eq!(resp.text(), "I'll check.");
        assert_eq!(resp.stop_reason, Some(StopReason::ToolUse));
        assert_eq!(resp.usage.input_tokens, 25);
        assert_eq!(resp.usage.output_tokens, 42);
        assert_eq!(resp.usage.cache_read_tokens, 5);
        assert_eq!(resp.usage.cache_write_tokens, 10);
        assert_eq!(resp.usage.cache_write_1h_tokens, 4);
        let calls: Vec<_> = resp.tool_uses().collect();
        assert_eq!(
            calls,
            vec![("toolu_1", "get_weather", &json!({"location": "Paris"}))]
        );
        match &resp.content[0] {
            aether_core::model::ContentBlock::Thinking {
                text,
                signature,
                redacted,
            } => {
                assert_eq!(text, "Let me think");
                assert_eq!(signature.as_deref(), Some("abc"));
                assert!(!redacted);
            }
            other => panic!("unexpected first block {other:?}"),
        }
    }

    #[tokio::test]
    async fn refusal_with_category_and_stream_error() {
        let events = vec![
            sse(
                "message_start",
                json!({"type":"message_start","message":{"id":"m","usage":{"input_tokens":1,"output_tokens":0}}}),
            ),
            sse(
                "message_delta",
                json!({"type":"message_delta","delta":{"stop_reason":"refusal","stop_details":{"type":"refusal","category":"cyber"}},"usage":{"output_tokens":0}}),
            ),
            sse("message_stop", json!({"type":"message_stop"})),
        ];
        let resp = collect(mapped_stream(events, AnthropicMapper::new()), |_| {})
            .await
            .expect("collect");
        assert_eq!(
            resp.stop_reason,
            Some(StopReason::Refusal {
                category: Some("cyber".into())
            })
        );

        let events = vec![
            sse(
                "message_start",
                json!({"type":"message_start","message":{"id":"m","usage":{"input_tokens":1,"output_tokens":0}}}),
            ),
            sse(
                "error",
                json!({"type":"error","error":{"type":"overloaded_error","message":"Overloaded"}}),
            ),
        ];
        let err = collect(mapped_stream(events, AnthropicMapper::new()), |_| {})
            .await
            .unwrap_err();
        assert_eq!(err.class, ModelErrorClass::Overloaded);
        assert!(err.is_retryable());
    }

    /// Same rule as the other adapters: no stop reason arrived, so the answer
    /// was cut short and the turn is interrupted, not finished.
    #[tokio::test]
    async fn truncated_stream_is_interrupted_and_keeps_what_arrived() {
        let events = vec![
            sse(
                "message_start",
                json!({"type":"message_start","message":{"id":"m","usage":{"input_tokens":1,"output_tokens":0}}}),
            ),
            sse(
                "content_block_start",
                json!({"type":"content_block_start","index":0,"content_block":{"type":"text","text":""}}),
            ),
            sse(
                "content_block_delta",
                json!({"type":"content_block_delta","index":0,"delta":{"type":"text_delta","text":"partial"}}),
            ),
        ];
        let failure =
            crate::client::collect_partial(mapped_stream(events, AnthropicMapper::new()), |_| {})
                .await
                .expect_err("a cut stream is not a finished turn");
        assert_eq!(failure.error.class, ModelErrorClass::Network);
        assert!(failure.error.is_retryable());
        assert_eq!(failure.partial.text(), "partial");
        assert_eq!(failure.partial.stop_reason, None);
    }

    #[test]
    fn malformed_json_is_visible() {
        let mut m = AnthropicMapper::new();
        let err = m
            .map(&SseEvent {
                event: Some("message_start".into()),
                data: "{nope".into(),
                id: None,
            })
            .unwrap_err();
        assert!(err.message.contains("malformed SSE data"));
    }
}
