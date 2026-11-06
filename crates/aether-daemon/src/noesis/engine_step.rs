//! A Noesis step on a subscription engine CLI (Claude Code, Codex).
//!
//! The teaching loop runs every tool itself: it validates, seals and parks
//! `noesis.ask` until the learner answers, which may be hours. An engine
//! runs tools inside its own loop and cannot hand a call back unexecuted, so
//! a step is one fresh, toolless engine session instead: the agent's tools
//! and transcript go in as text, and the reply is one JSON object naming the
//! text and the tool calls, parsed back into the reply an API model gives.
//! The rest of the step (the budget, the sealed transcript, the tools) is
//! the direct path's.

use std::sync::Arc;

use aether_core::model::{
    BlockStart, ContentBlock, Message, MessageRole, ModelError, ModelErrorClass, ModelRequest,
    StreamEvent, SystemBlock, ToolResultBlock, ToolSpec, Usage,
};
use aether_model::{ModelClient, ModelStream};
use serde::Deserialize;
use serde_json::Value;
use tokio_util::sync::CancellationToken;

#[path = "engine_step_stream.rs"]
mod streaming;
#[path = "engine_step_text.rs"]
mod text;

/// A step tries the engine twice: one corrective retry when a reply is not
/// the JSON object asked for. Each attempt is gated and charged on its own.
pub(super) const ATTEMPTS: usize = 2;

const MALFORMED_REPLY: &str = "the engine's reply was not the JSON object asked for";

/// What a retry adds to the transcript. The refused reply is never shown
/// again: it may hold a value the transcript seals.
const RETRY_NOTE: &str = "Your previous reply was refused and discarded: it was not one valid \
    JSON object of the reply format. Reply again with only the JSON object.";

/// Renders a Noesis request for `inner`, a one-shot engine client, and
/// parses its reply into text and tool calls.
pub(super) struct EngineStepClient {
    inner: Arc<dyn ModelClient>,
}

impl EngineStepClient {
    pub(super) fn new(inner: Arc<dyn ModelClient>) -> Self {
        Self { inner }
    }
}

#[async_trait::async_trait]
impl ModelClient for EngineStepClient {
    fn adapter(&self) -> &str {
        "noesis-engine-step/1"
    }

    /// One engine attempt. The reply is parsed as the stream is read, so a
    /// caller that stops reading still holds what the attempt reported.
    async fn stream(
        &self,
        request: ModelRequest,
        cancel: CancellationToken,
    ) -> Result<ModelStream, ModelError> {
        debug_assert!(!request.messages.is_empty(), "a step has a transcript");
        let rendered = engine_request(&request);
        assert!(rendered.tools.is_empty(), "the engine gets no native tools");
        let stream = self.inner.stream(rendered, cancel).await?;
        Ok(streaming::parsed_events(stream))
    }
}

/// Whether a step's error is a reply that was not the object asked for.
pub(super) fn is_malformed(error: &ModelError) -> bool {
    error.class == ModelErrorClass::Server && error.message.starts_with(MALFORMED_REPLY)
}

/// The note a retried step's transcript ends with.
pub(super) fn retry_note() -> Message {
    Message::user_text(RETRY_NOTE)
}

/// The request the engine reads: the agent's prompt and the reply protocol
/// as its instructions, the transcript as one message, no tools.
fn engine_request(request: &ModelRequest) -> ModelRequest {
    let mut instructions: Vec<&str> = request.system.iter().map(|b| b.text.as_str()).collect();
    let protocol = protocol(&request.tools);
    instructions.push(&protocol);
    let mut rendered = ModelRequest::new(
        request.model.clone(),
        vec![Message::user_text(transcript(&request.messages))],
    );
    rendered.system = vec![SystemBlock {
        text: instructions.join("\n\n"),
        cache: false,
    }];
    rendered.effort = request.effort;
    debug_assert!(rendered.tools.is_empty(), "the engine gets no native tools");
    rendered
}

fn protocol(tools: &[ToolSpec]) -> String {
    let mut text = String::from(
        "# How you act\n\nYou have no native tools here. You act only through your reply, \
         and your whole reply is ONE JSON object, with nothing before or after it and no \
         code fence:\n\n{\"text\": \"<what the learner reads; may be empty>\", \
         \"tool_calls\": [{\"name\": \"<tool name>\", \"input\": {<its arguments>}}]}\n\n\
         - Aether runs the calls in `tool_calls` in order once you reply, and gives you \
         their results in the next message.\n\
         - An empty `tool_calls` ends your turn: the learner reads `text` and answers.\n\
         - Put `text` first, as a JSON string, so the learner can read it as you generate it.\n\
         - Each `input` follows its tool's input schema below.\n\
         - What the learner must not see yet (an answer key, an explanation, a solution) \
         goes only in a tool's input, never in `text`.\n\n## Tools\n",
    );
    for tool in tools {
        text.push_str(&format!(
            "\n### {}\n\n{}\n\nInput schema: {}\n",
            tool.name, tool.description, tool.input_schema
        ));
    }
    text
}

/// The transcript as text, oldest first, each entry under who wrote it.
fn transcript(messages: &[Message]) -> String {
    let mut text = String::from(
        "# The conversation so far\n\nReply as the next `You` entry, in the JSON format.\n",
    );
    for message in messages {
        let (texts, calls, results) = parts(message);
        let speaker = match message.role {
            MessageRole::Assistant => "You",
            _ => "User",
        };
        if !texts.is_empty() {
            text.push_str(&format!("\n## {speaker}\n\n{}\n", texts.join("\n\n")));
        }
        if !calls.is_empty() {
            text.push_str(&format!("\n## You called\n\n{}\n", Value::Array(calls)));
        }
        if !results.is_empty() {
            text.push_str(&format!("\n## Tool results\n\n{}\n", results.join("\n")));
        }
    }
    text
}

/// A message's text, its tool calls as JSON and its tool results as lines.
fn parts(message: &Message) -> (Vec<&str>, Vec<Value>, Vec<String>) {
    let (mut texts, mut calls, mut results) = (Vec::new(), Vec::new(), Vec::new());
    for block in &message.content {
        match block {
            ContentBlock::Text { text } if !text.trim().is_empty() => texts.push(text.as_str()),
            ContentBlock::ToolUse { id, name, input } => {
                calls.push(serde_json::json!({"id": id, "name": name, "input": input}));
            }
            ContentBlock::ToolResult {
                tool_use_id,
                content,
                is_error,
            } => {
                let body: Vec<&str> = content
                    .iter()
                    .filter_map(|block| match block {
                        ToolResultBlock::Text { text } => Some(text.as_str()),
                        _ => None,
                    })
                    .collect();
                let mark = if *is_error { " (error)" } else { "" };
                results.push(format!("- {tool_use_id}{mark}: {}", body.join("\n")));
            }
            _ => {}
        }
    }
    (texts, calls, results)
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct WireReply {
    #[serde(default)]
    text: String,
    #[serde(default)]
    tool_calls: Vec<WireCall>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct WireCall {
    name: String,
    #[serde(default)]
    input: Value,
}

/// A parsed reply: the learner-facing text and the calls, in order.
#[derive(Debug, PartialEq)]
struct Reply {
    text: Option<String>,
    /// `(id, name, input)`, in the order the engine gave them.
    calls: Vec<(String, String, Value)>,
}

/// The reply's text and calls, from the one reply object it must hold; text
/// outside that object is dropped. Refusals never quote the reply: it may
/// hold a sealed value, so bare prose is refused rather than shown.
fn parse_reply(raw: &str) -> Result<Reply, String> {
    let raw = unfenced(raw.trim());
    if raw.is_empty() {
        return Err("the reply was empty".into());
    }
    let Some(start) = raw.find('{') else {
        return Err("the reply holds no JSON object".into());
    };
    let object = match raw.rfind('}') {
        Some(end) if end > start => &raw[start..=end],
        _ => return Err("the reply has an unclosed JSON object".into()),
    };
    let wire: WireReply = serde_json::from_str(object).map_err(|error| {
        let what = match error.classify() {
            serde_json::error::Category::Data => "does not follow the reply format",
            _ => "is not valid JSON",
        };
        format!(
            "its object {what} (line {}, column {})",
            error.line(),
            error.column()
        )
    })?;
    reply_of(wire)
}

/// `raw` without a surrounding Markdown code fence.
fn unfenced(raw: &str) -> &str {
    let Some(body) = raw.strip_prefix("```") else {
        return raw;
    };
    let body = body.trim_start_matches(|c: char| c.is_ascii_alphanumeric());
    body.strip_suffix("```").unwrap_or(body).trim()
}

/// The reply's calls get ids unique across the agent's transcript, as a
/// provider's are, so a later step on an API model can replay them.
fn reply_of(wire: WireReply) -> Result<Reply, String> {
    let nonce = ulid::Ulid::generate().to_string().to_ascii_lowercase();
    let mut calls = Vec::with_capacity(wire.tool_calls.len());
    for (index, call) in wire.tool_calls.into_iter().enumerate() {
        if call.name.trim().is_empty() {
            return Err(format!("tool call {} has no name", index + 1));
        }
        if !call.input.is_object() && !call.input.is_null() {
            return Err(format!("tool call {} input is not an object", index + 1));
        }
        calls.push((format!("call_{nonce}_{}", index + 1), call.name, call.input));
    }
    let text = Some(wire.text).filter(|text| !text.trim().is_empty());
    if text.is_none() && calls.is_empty() {
        return Err("the reply has neither text nor a tool call".into());
    }
    debug_assert!(calls.iter().all(|(id, _, _)| id.starts_with("call_")));
    Ok(Reply { text, calls })
}

fn reply_events(
    usage: Usage,
    reply: Reply,
    streamed: bool,
) -> Vec<Result<StreamEvent, ModelError>> {
    debug_assert!(reply.text.is_some() || !reply.calls.is_empty());
    debug_assert!(!streamed || reply.text.is_some());
    let mut events = Vec::new();
    if !streamed {
        events.push(StreamEvent::MessageStart {
            provider_message_id: None,
            model: None,
        });
    }
    events.push(StreamEvent::Usage { usage });
    let offset = usize::from(reply.text.is_some());
    if let Some(text) = reply.text {
        if !streamed {
            events.extend([
                StreamEvent::BlockStart {
                    index: 0,
                    block: BlockStart::Text {
                        text: String::new(),
                    },
                },
                StreamEvent::TextDelta { index: 0, text },
            ]);
        }
        events.push(StreamEvent::BlockStop { index: 0 });
    }
    for (position, (id, name, input)) in reply.calls.into_iter().enumerate() {
        let index = offset + position;
        events.extend([
            StreamEvent::BlockStart {
                index,
                block: BlockStart::ToolUse {
                    id,
                    name,
                    input: Some(input),
                },
            },
            StreamEvent::BlockStop { index },
        ]);
    }
    events.push(StreamEvent::MessageStop);
    events.into_iter().map(Ok).collect()
}

/// What was spent, then the error: the step charges the usage it reports.
fn failed_events(usage: Usage, error: ModelError) -> Vec<Result<StreamEvent, ModelError>> {
    vec![Ok(StreamEvent::Usage { usage }), Err(error)]
}

#[cfg(test)]
#[path = "engine_step_tests.rs"]
mod tests;
