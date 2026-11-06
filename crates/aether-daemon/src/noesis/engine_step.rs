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
use aether_model::{ModelClient, ModelStream, collect_partial};
use serde::Deserialize;
use serde_json::Value;
use tokio_util::sync::CancellationToken;

/// One corrective retry when a reply is not the JSON object asked for.
const ATTEMPTS: usize = 2;

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

    async fn stream(
        &self,
        request: ModelRequest,
        cancel: CancellationToken,
    ) -> Result<ModelStream, ModelError> {
        debug_assert!(!request.messages.is_empty(), "a step has a transcript");
        let mut prompt = engine_request(&request);
        let mut spent = Usage::default();
        let mut last_error = String::new();
        for attempt in 0..ATTEMPTS {
            let stream = self.inner.stream(prompt.clone(), cancel.clone()).await;
            // Nothing was sent on the first attempt's refusal; a retry's is
            // reported with what the first attempt spent.
            let stream = match stream {
                Ok(stream) => stream,
                Err(error) if attempt == 0 => return Err(error),
                Err(error) => return Ok(failed_stream(spent, error)),
            };
            let reply = match collect_partial(stream, |_| {}).await {
                Ok(reply) => reply,
                Err(failure) => {
                    spent += failure.partial.usage;
                    return Ok(failed_stream(spent, failure.error));
                }
            };
            spent += reply.usage;
            match parse_reply(&reply.text()) {
                Ok(parsed) => return Ok(reply_stream(spent, parsed)),
                Err(error) => last_error = error,
            }
            prompt = corrected(prompt, &reply.text(), &last_error);
        }
        let error = ModelError::new(
            ModelErrorClass::Server,
            format!("the engine's reply was not the JSON object asked for: {last_error}"),
        );
        Ok(failed_stream(spent, error))
    }
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

/// The prompt again, with the reply that failed and why.
fn corrected(mut request: ModelRequest, reply: &str, error: &str) -> ModelRequest {
    debug_assert_eq!(request.messages.len(), 1, "the rendered prompt is one message");
    let note = format!(
        "\n\n## Your previous reply was refused\n\nIt was not one valid JSON object of the \
         reply format ({error}). It read:\n\n{reply}\n\nReply again with only the JSON object."
    );
    if let Some(ContentBlock::Text { text }) = request
        .messages
        .first_mut()
        .and_then(|message| message.content.first_mut())
    {
        text.push_str(&note);
    }
    request
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
            text.push_str(&format!(
                "\n## You called\n\n{}\n",
                Value::Array(calls)
            ));
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

/// The reply's text and calls. A reply that opens with `{` (after any code
/// fence) must be the object; other prose is plain text that ends the turn,
/// unless it embeds a reply object, which is then taken.
fn parse_reply(raw: &str) -> Result<Reply, String> {
    let raw = unfenced(raw.trim());
    if raw.is_empty() {
        return Err("the reply was empty".into());
    }
    if raw.starts_with('{') {
        let wire: WireReply = serde_json::from_str(raw).map_err(|e| e.to_string())?;
        return reply_of(wire);
    }
    let embedded = match (raw.find('{'), raw.rfind('}')) {
        (Some(start), Some(end)) if start < end => {
            serde_json::from_str::<WireReply>(&raw[start..=end]).ok()
        }
        _ => None,
    };
    match embedded {
        Some(wire) => reply_of(wire),
        None => Ok(Reply {
            text: Some(raw.into()),
            calls: Vec::new(),
        }),
    }
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

fn reply_stream(usage: Usage, reply: Reply) -> ModelStream {
    debug_assert!(reply.text.is_some() || !reply.calls.is_empty());
    let mut events = vec![
        StreamEvent::MessageStart {
            provider_message_id: None,
            model: None,
        },
        StreamEvent::Usage { usage },
    ];
    let offset = usize::from(reply.text.is_some());
    if let Some(text) = reply.text {
        events.extend([
            StreamEvent::BlockStart {
                index: 0,
                block: BlockStart::Text {
                    text: String::new(),
                },
            },
            StreamEvent::TextDelta { index: 0, text },
            StreamEvent::BlockStop { index: 0 },
        ]);
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
    Box::pin(futures::stream::iter(events.into_iter().map(Ok)))
}

/// What was spent, then the error: the step charges the usage it reports.
fn failed_stream(usage: Usage, error: ModelError) -> ModelStream {
    let events = vec![Ok(StreamEvent::Usage { usage }), Err(error)];
    Box::pin(futures::stream::iter(events))
}

#[cfg(test)]
#[path = "engine_step_tests.rs"]
mod tests;
