use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};
use serde_json::Value;

use super::{ContentBlock, Message, Usage, text_of};

/// Why the model stopped generating.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(tag = "reason", rename_all = "snake_case")]
pub enum StopReason {
    EndTurn,
    ToolUse,
    MaxTokens,
    StopSequence,
    Refusal {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        category: Option<String>,
    },
    PauseTurn,
    Other {
        value: String,
    },
}

/// Start of a streamed content block.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum BlockStart {
    Text {
        #[serde(default)]
        text: String,
    },
    Thinking {
        #[serde(default)]
        text: String,
        #[serde(default)]
        redacted: bool,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        signature: Option<String>,
    },
    ToolUse {
        id: String,
        name: String,
        /// Some providers deliver complete input at block start.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        input: Option<Value>,
    },
    Compaction {
        #[serde(default)]
        content: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        signature: Option<String>,
    },
}

/// Normalized streaming event emitted by every adapter.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(tag = "event", rename_all = "snake_case")]
pub enum StreamEvent {
    MessageStart {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        provider_message_id: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        model: Option<String>,
    },
    BlockStart {
        index: usize,
        block: BlockStart,
    },
    TextDelta {
        index: usize,
        text: String,
    },
    ThinkingDelta {
        index: usize,
        text: String,
    },
    SignatureDelta {
        index: usize,
        signature: String,
    },
    ToolInputDelta {
        index: usize,
        partial_json: String,
    },
    CompactionDelta {
        index: usize,
        content: String,
    },
    BlockStop {
        index: usize,
    },
    /// Cumulative usage so far; the last one wins.
    Usage {
        usage: Usage,
    },
    Stop {
        reason: StopReason,
    },
    MessageStop,
}

/// A complete model response, assembled from the stream.
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize, schemars::JsonSchema)]
pub struct ModelResponse {
    pub content: Vec<ContentBlock>,
    pub usage: Usage,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub stop_reason: Option<StopReason>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub provider_message_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
}

impl ModelResponse {
    pub fn text(&self) -> String {
        text_of(&self.content)
    }

    /// Tool calls requested by the model, in order.
    pub fn tool_uses(&self) -> impl Iterator<Item = (&str, &str, &Value)> {
        self.content.iter().filter_map(|b| match b {
            ContentBlock::ToolUse { id, name, input } => Some((id.as_str(), name.as_str(), input)),
            _ => None,
        })
    }

    pub fn has_tool_use(&self) -> bool {
        self.tool_uses().next().is_some()
    }

    /// The response as an assistant message ready to append to a transcript.
    pub fn as_message(&self) -> Message {
        Message::assistant_blocks(self.content.clone())
    }
}

/// Error from assembling a stream into a response.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum AccumulateError {
    #[error("delta for unknown block index {0}")]
    UnknownBlock(usize),
    #[error("block {0} started twice")]
    DuplicateBlock(usize),
    #[error("delta type does not match block {0}")]
    KindMismatch(usize),
    #[error("tool call `{name}` produced invalid JSON input: {reason}")]
    InvalidToolInput { name: String, reason: String },
}

#[derive(Debug, Clone)]
enum PendingBlock {
    Text(String),
    Thinking {
        text: String,
        signature: Option<String>,
        redacted: bool,
    },
    ToolUse {
        id: String,
        name: String,
        json: String,
        input: Option<Value>,
    },
    Compaction {
        content: String,
        signature: Option<String>,
    },
}

/// Folds [`StreamEvent`]s into a [`ModelResponse`]. Shared by all adapters so
/// block ordering and tool-input parsing behave identically everywhere.
#[derive(Debug, Default)]
pub struct StreamAccumulator {
    blocks: BTreeMap<usize, PendingBlock>,
    usage: Usage,
    stop_reason: Option<StopReason>,
    provider_message_id: Option<String>,
    model: Option<String>,
}

impl StreamAccumulator {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn apply(&mut self, event: &StreamEvent) -> Result<(), AccumulateError> {
        match event {
            StreamEvent::MessageStart {
                provider_message_id,
                model,
            } => {
                self.provider_message_id.clone_from(provider_message_id);
                self.model.clone_from(model);
            }
            StreamEvent::BlockStart { index, block } => {
                if self.blocks.contains_key(index) {
                    return Err(AccumulateError::DuplicateBlock(*index));
                }
                let pending = match block {
                    BlockStart::Text { text } => PendingBlock::Text(text.clone()),
                    BlockStart::Thinking {
                        text,
                        redacted,
                        signature,
                    } => PendingBlock::Thinking {
                        text: text.clone(),
                        signature: signature.clone(),
                        redacted: *redacted,
                    },
                    BlockStart::Compaction { content, signature } => PendingBlock::Compaction {
                        content: content.clone(),
                        signature: signature.clone(),
                    },
                    BlockStart::ToolUse { id, name, input } => PendingBlock::ToolUse {
                        id: id.clone(),
                        name: name.clone(),
                        json: String::new(),
                        input: input.clone(),
                    },
                };
                self.blocks.insert(*index, pending);
            }
            StreamEvent::TextDelta { index, text } => match self.block_mut(*index)? {
                PendingBlock::Text(buf) => buf.push_str(text),
                _ => return Err(AccumulateError::KindMismatch(*index)),
            },
            StreamEvent::CompactionDelta { index, content } => match self.block_mut(*index)? {
                PendingBlock::Compaction { content: buf, .. } => buf.push_str(content),
                _ => return Err(AccumulateError::KindMismatch(*index)),
            },
            StreamEvent::ThinkingDelta { index, text } => match self.block_mut(*index)? {
                PendingBlock::Thinking { text: buf, .. } => buf.push_str(text),
                _ => return Err(AccumulateError::KindMismatch(*index)),
            },
            StreamEvent::SignatureDelta { index, signature } => match self.block_mut(*index)? {
                PendingBlock::Thinking { signature: sig, .. } => {
                    sig.get_or_insert_with(String::new).push_str(signature);
                }
                _ => return Err(AccumulateError::KindMismatch(*index)),
            },
            StreamEvent::ToolInputDelta {
                index,
                partial_json,
            } => match self.block_mut(*index)? {
                PendingBlock::ToolUse { json, .. } => json.push_str(partial_json),
                _ => return Err(AccumulateError::KindMismatch(*index)),
            },
            StreamEvent::BlockStop { index } => {
                self.block_mut(*index)?;
            }
            StreamEvent::Usage { usage } => self.usage = *usage,
            StreamEvent::Stop { reason } => self.stop_reason = Some(reason.clone()),
            StreamEvent::MessageStop => {}
        }
        Ok(())
    }

    fn block_mut(&mut self, index: usize) -> Result<&mut PendingBlock, AccumulateError> {
        self.blocks
            .get_mut(&index)
            .ok_or(AccumulateError::UnknownBlock(index))
    }

    pub fn usage(&self) -> Usage {
        self.usage
    }

    /// Text accumulated so far across all text blocks.
    pub fn text_so_far(&self) -> String {
        let mut out = String::new();
        for block in self.blocks.values() {
            if let PendingBlock::Text(t) = block {
                out.push_str(t);
            }
        }
        out
    }

    pub fn finish(self) -> Result<ModelResponse, AccumulateError> {
        let mut content = Vec::with_capacity(self.blocks.len());
        for (_, block) in self.blocks {
            content.push(match block {
                PendingBlock::Text(text) => ContentBlock::Text { text },
                PendingBlock::Thinking {
                    text,
                    signature,
                    redacted,
                } => ContentBlock::Thinking {
                    text,
                    signature,
                    redacted,
                },
                PendingBlock::Compaction { content, signature } => {
                    ContentBlock::Compaction { content, signature }
                }
                PendingBlock::ToolUse {
                    id,
                    name,
                    json,
                    input,
                } => {
                    let input = match input {
                        Some(v) => v,
                        None if json.trim().is_empty() => Value::Object(Default::default()),
                        None => serde_json::from_str(&json).map_err(|e| {
                            AccumulateError::InvalidToolInput {
                                name: name.clone(),
                                reason: e.to_string(),
                            }
                        })?,
                    };
                    ContentBlock::ToolUse { id, name, input }
                }
            });
        }
        Ok(ModelResponse {
            content,
            usage: self.usage,
            stop_reason: self.stop_reason,
            provider_message_id: self.provider_message_id,
            model: self.model,
        })
    }
}
