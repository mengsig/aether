//! Decode native Claude stream events and account for usage.

use std::collections::VecDeque;
use std::sync::{Arc, Mutex};

use aether_core::model::{BlockStart, ContentBlock, StreamEvent, Usage};
use serde_json::Value;

use crate::{EngineEvent, EngineOutcome, RequestUsage, UsageRequestId};

/// Per-process parser state: tool names by content-block index for the
/// message being streamed (so input deltas carry the name), and the steered
/// messages the CLI has not echoed back yet, oldest first.
#[derive(Debug, Default)]
pub struct ParseState {
    session_cost: super::session_cost::SessionCost,
    tool_names: std::collections::BTreeMap<usize, String>,
    pub(super) outstanding: Arc<Mutex<VecDeque<String>>>,
    message_usage: std::collections::BTreeMap<UsageRequestId, Usage>,
    /// The keys above in the order they were first seen, so the map can be
    /// bounded without evicting the message still being streamed.
    usage_order: VecDeque<UsageRequestId>,
    streaming_message: Option<UsageRequestId>,
    /// Keys minted here so far.
    minted: u64,
    /// The turn's prompt until the CLI echoes it. Before that echo, a result
    /// that ran no model closes work the CLI queued ahead of the prompt.
    prompt: Option<String>,
}

/// Messages one run keeps high-water marks for. The ids are the engine's, so
/// this is bounded; evicting the oldest closed message can only bill one
/// twice, never drop one.
const MAX_TRACKED_MESSAGES: usize = 256;

impl ParseState {
    pub(super) fn with_session_cost(mut self, session: Option<String>, usage: Usage) -> Self {
        self.session_cost = super::session_cost::SessionCost::resumed(session, usage);
        self
    }

    /// State for a run that writes `prompt` to the CLI first. Its echo marks
    /// where the CLI starts answering it; see [`is_queued_work_result`].
    pub fn awaiting_prompt(prompt: &str) -> Self {
        Self {
            prompt: Some(prompt.to_owned()).filter(|p| !p.trim().is_empty()),
            ..Self::default()
        }
    }

    /// Whether `text` is the prompt's echo; the prompt is then read.
    fn take_prompt_echo(&mut self, text: &str) -> bool {
        let echoed = self
            .prompt
            .as_deref()
            .is_some_and(|p| p.trim() == text.trim());
        if echoed {
            self.prompt = None;
        }
        echoed
    }

    /// A key distinct from every key handed out before it.
    fn mint(&mut self) -> UsageRequestId {
        self.minted = self.minted.saturating_add(1);
        UsageRequestId::Minted(self.minted)
    }

    /// The key an `assistant` line's counters dedup under. With an id, its
    /// own - never per block, which is the triple count. Without one it
    /// joins the open message only when that message had no id either, which
    /// is the CLI's per-block shape; a message the stream opened under an id
    /// is a different message, and inheriting its already-charged key bills
    /// this one nothing (measured: 4/5 of a run's spend free).
    fn assistant_key(&mut self, id: Option<&str>) -> UsageRequestId {
        match (id, &self.streaming_message) {
            (Some(id), _) => UsageRequestId::Engine(id.to_owned()),
            (None, Some(open @ UsageRequestId::Minted(_))) => open.clone(),
            (None, _) => self.mint(),
        }
    }

    /// The key the open message's own events belong to. A stream whose
    /// `message_start` never arrived opens one here, so its final output
    /// count is billed rather than dropped.
    fn open_key(&mut self) -> UsageRequestId {
        if let Some(open) = &self.streaming_message {
            return open.clone();
        }
        let key = self.mint();
        self.streaming_message = Some(key.clone());
        key
    }

    // Claude repeats message usage on each content block, then supplies the
    // final output count in message_delta. Retain high-water marks per key so
    // late blocks cannot count the same input again or undo the final count.
    fn account_usage(&mut self, key: UsageRequestId, value: &Value) -> Vec<EngineEvent> {
        let reported = parse_usage(value);
        if !self.message_usage.contains_key(&key) {
            self.usage_order.push_back(key.clone());
        }
        if self.usage_order.len() > MAX_TRACKED_MESSAGES
            && let Some(pos) = self
                .usage_order
                .iter()
                .position(|k| Some(k) != self.streaming_message.as_ref())
            && let Some(evicted) = self.usage_order.remove(pos)
        {
            self.message_usage.remove(&evicted);
        }
        let total = self.message_usage.entry(key.clone()).or_default();
        let previous = *total;
        macro_rules! advance {
            ($field:ident) => {
                total.$field = total.$field.max(reported.$field);
            };
        }
        advance!(input_tokens);
        advance!(output_tokens);
        advance!(cache_read_tokens);
        advance!(cache_write_tokens);
        advance!(cache_write_1h_tokens);
        debug_assert!(total.cache_write_1h_tokens <= total.cache_write_tokens);
        if let Some(reasoning) = reported.reasoning_tokens {
            total.reasoning_tokens = Some(total.reasoning_tokens.unwrap_or(0).max(reasoning));
        }
        let request = RequestUsage {
            id: key,
            previous,
            total: *total,
        };
        let delta = request.delta();
        // A split reported after its writes adds no tokens, but reprices the request.
        let reclassified = total.cache_write_1h_tokens > previous.cache_write_1h_tokens;
        let mut events = Vec::new();
        if delta.total_tokens() > 0 || delta.reasoning_tokens.is_some_and(|n| n > 0) || reclassified
        {
            events.push(EngineEvent::UsageRequest { request });
        }
        events.push(EngineEvent::ContextUsage {
            tokens: total
                .input_tokens
                .saturating_add(total.cache_read_tokens)
                .saturating_add(total.cache_write_tokens),
        });
        events
    }

    /// Remember a message handed to the CLI, so its echo is recognised.
    pub fn expect_echo(&self, text: &str) {
        self.outstanding
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .push_back(text.to_owned());
    }

    /// Whether `text` is the oldest message still awaiting its echo; pops it.
    fn take_echo(&self, text: &str) -> bool {
        let mut queue = self.outstanding.lock().unwrap_or_else(|e| e.into_inner());
        if queue.front().is_some_and(|t| t.trim() == text.trim()) {
            queue.pop_front();
            true
        } else {
            false
        }
    }
}

/// Map one stream-json line to engine events. Unknown lines become notes.
pub fn parse_line(line: &str, state: &mut ParseState) -> Vec<EngineEvent> {
    let v: Value = match serde_json::from_str(line) {
        Ok(v) => v,
        Err(_) => return vec![EngineEvent::Note(line.chars().take(300).collect())],
    };
    match v.get("type").and_then(Value::as_str).unwrap_or("") {
        "system" => match v.get("subtype").and_then(Value::as_str) {
            Some("task_started") => vec![EngineEvent::BackgroundTask {
                id: v
                    .get("task_id")
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                    .to_owned(),
                running: true,
            }],
            Some("task_notification") => vec![EngineEvent::BackgroundTask {
                id: v
                    .get("task_id")
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                    .to_owned(),
                running: false,
            }],
            Some("init") => {
                vec![EngineEvent::Session {
                    id: v
                        .get("session_id")
                        .and_then(Value::as_str)
                        .unwrap_or("")
                        .to_owned(),
                    model: v.get("model").and_then(Value::as_str).map(str::to_owned),
                    tools: v
                        .get("tools")
                        .and_then(Value::as_array)
                        .map(|a| {
                            a.iter()
                                .filter_map(Value::as_str)
                                .map(str::to_owned)
                                .collect()
                        })
                        .unwrap_or_default(),
                }]
            }
            _ => Vec::new(),
        },
        "rate_limit_event" => vec![EngineEvent::RateLimit {
            payload: v.get("rate_limit_info").cloned().unwrap_or(Value::Null),
        }],
        "stream_event" => v
            .get("event")
            .map(|e| stream_deltas(e, state))
            .unwrap_or_default(),
        "assistant" => {
            let message = &v["message"];
            let content = parse_content(message.get("content"));
            // Usage rides the delta stream alone, keyed per message: attaching
            // it here would bill one message once per content block.
            let accounting = match message.get("usage") {
                Some(usage) => {
                    let key = state.assistant_key(message.get("id").and_then(Value::as_str));
                    state.account_usage(key, usage)
                }
                None => Vec::new(),
            };
            state.tool_names.clear();
            let mut events = vec![EngineEvent::Assistant {
                content,
                usage: None,
                model: message
                    .get("model")
                    .and_then(Value::as_str)
                    .map(str::to_owned),
            }];
            events.extend(accounting);
            events
        }
        "user" => {
            let mut out = Vec::new();
            if v.get("isReplay").and_then(Value::as_bool) == Some(true)
                && let Some(text) = replayed_text(&v["message"])
                && !state.take_prompt_echo(&text)
                && state.take_echo(&text)
            {
                out.push(EngineEvent::InputAccepted);
            }
            if let Some(items) = v["message"].get("content").and_then(Value::as_array) {
                for item in items {
                    if item.get("type").and_then(Value::as_str) != Some("tool_result") {
                        continue;
                    }
                    let content = match item.get("content") {
                        Some(Value::String(s)) => s.clone(),
                        Some(Value::Array(parts)) => parts
                            .iter()
                            .filter_map(|p| match p.get("type").and_then(Value::as_str) {
                                Some("text") => {
                                    p.get("text").and_then(Value::as_str).map(str::to_owned)
                                }
                                Some(other) => Some(format!("[{other}]")),
                                None => None,
                            })
                            .collect::<Vec<_>>()
                            .join("\n"),
                        _ => String::new(),
                    };
                    out.push(EngineEvent::ToolResult {
                        tool_use_id: item
                            .get("tool_use_id")
                            .and_then(Value::as_str)
                            .unwrap_or("")
                            .to_owned(),
                        content,
                        is_error: item
                            .get("is_error")
                            .and_then(Value::as_bool)
                            .unwrap_or(false),
                    });
                }
            }
            out
        }
        "result" if state.prompt.is_some() && is_queued_work_result(&v) => {
            vec![EngineEvent::Note(
                "claude-code ended a turn it queued ahead of the prompt; awaiting the prompt's own result"
                    .into(),
            )]
        }
        "result" => {
            let subtype = v.get("subtype").and_then(Value::as_str).unwrap_or("");
            let is_error =
                v.get("is_error").and_then(Value::as_bool).unwrap_or(false) || subtype != "success";
            let text = v.get("result").and_then(Value::as_str).map(str::to_owned);
            let (cost_usd, mut events) = state.session_cost.receipt(&v);
            events.push(EngineEvent::Done(EngineOutcome {
                closing_accounting: true,
                usage: v.get("usage").map(parse_usage).unwrap_or_default(),
                cost_usd,
                stop_reason: v
                    .get("stop_reason")
                    .and_then(Value::as_str)
                    .map(str::to_owned),
                duration_ms: v
                    .get("duration_api_ms")
                    .or_else(|| v.get("duration_ms"))
                    .and_then(Value::as_u64),
                is_error,
                error: is_error.then(|| {
                    let mut details: Vec<&str> = text
                        .as_deref()
                        .filter(|text| !text.trim().is_empty())
                        .into_iter()
                        .collect();
                    if let Some(errors) = v.get("errors").and_then(Value::as_array) {
                        details.extend(
                            errors
                                .iter()
                                .filter_map(Value::as_str)
                                .filter(|error| !error.trim().is_empty()),
                        );
                    }
                    if details.is_empty() {
                        subtype.to_owned()
                    } else {
                        details.join("\n")
                    }
                }),
                session: v
                    .get("session_id")
                    .and_then(Value::as_str)
                    .map(str::to_owned),
                text: if is_error { None } else { text },
                session_lost: false,
                prompt_not_submitted: false,
                interrupted: false,
                forced: false,
            }));
            events
        }
        _ => Vec::new(),
    }
}

/// Whether a pre-echo result closed a queued task notice without running the model, e.g. an
/// orphaned shell's on `--resume`. Any other result ends the turn, so an unmatched echo cannot hang it.
fn is_queued_work_result(v: &Value) -> bool {
    let succeeded = v.get("subtype").and_then(Value::as_str) == Some("success")
        && v.get("is_error").and_then(Value::as_bool) != Some(true);
    let notice = v.pointer("/origin/kind").and_then(Value::as_str) == Some("task-notification");
    succeeded && notice && v.get("num_turns").and_then(Value::as_u64) == Some(0)
}

/// The text of an echoed user message; `None` for tool-result messages.
/// The images a message was sent with are echoed too, and are not its text.
fn replayed_text(message: &Value) -> Option<String> {
    let items = message.get("content").and_then(Value::as_array)?;
    let mut text = String::new();
    for item in items {
        match item.get("type").and_then(Value::as_str) {
            Some("text") => text.push_str(item.get("text").and_then(Value::as_str).unwrap_or("")),
            Some("image") => {}
            _ => return None,
        }
    }
    Some(text)
}

fn stream_deltas(event: &Value, state: &mut ParseState) -> Vec<EngineEvent> {
    let index = event.get("index").and_then(Value::as_u64).unwrap_or(0) as usize;
    match event.get("type").and_then(Value::as_str).unwrap_or("") {
        "message_start" => {
            let message = &event["message"];
            let key = match message.get("id").and_then(Value::as_str) {
                Some(id) => UsageRequestId::Engine(id.to_owned()),
                None => state.mint(),
            };
            state.streaming_message = Some(key.clone());
            match message.get("usage") {
                Some(usage) => state.account_usage(key, usage),
                None => Vec::new(),
            }
        }
        // The final output count. Keyed off the open message, or - when its
        // `message_start` never arrived - off a key of its own, so the count
        // is billed once rather than dropped.
        "message_delta" => match event.get("usage") {
            Some(usage) => {
                let key = state.open_key();
                state.account_usage(key, usage)
            }
            None => Vec::new(),
        },
        "message_stop" => {
            state.streaming_message = None;
            Vec::new()
        }
        "content_block_start" => {
            let block = &event["content_block"];
            let start = match block.get("type").and_then(Value::as_str).unwrap_or("") {
                "text" => BlockStart::Text {
                    text: String::new(),
                },
                "thinking" => BlockStart::Thinking {
                    text: String::new(),
                    redacted: false,
                    signature: None,
                },
                "tool_use" => {
                    let name = block
                        .get("name")
                        .and_then(Value::as_str)
                        .unwrap_or("")
                        .to_owned();
                    state.tool_names.insert(index, name.clone());
                    BlockStart::ToolUse {
                        id: block
                            .get("id")
                            .and_then(Value::as_str)
                            .unwrap_or("")
                            .to_owned(),
                        name,
                        input: None,
                    }
                }
                _ => return Vec::new(),
            };
            vec![EngineEvent::Delta(StreamEvent::BlockStart {
                index,
                block: start,
            })]
        }
        "content_block_delta" => {
            let delta = &event["delta"];
            match delta.get("type").and_then(Value::as_str).unwrap_or("") {
                "text_delta" => vec![EngineEvent::Delta(StreamEvent::TextDelta {
                    index,
                    text: delta
                        .get("text")
                        .and_then(Value::as_str)
                        .unwrap_or("")
                        .to_owned(),
                })],
                "thinking_delta" => vec![EngineEvent::Delta(StreamEvent::ThinkingDelta {
                    index,
                    text: delta
                        .get("thinking")
                        .and_then(Value::as_str)
                        .unwrap_or("")
                        .to_owned(),
                })],
                "input_json_delta" => vec![EngineEvent::Delta(StreamEvent::ToolInputDelta {
                    index,
                    partial_json: delta
                        .get("partial_json")
                        .and_then(Value::as_str)
                        .unwrap_or("")
                        .to_owned(),
                })],
                _ => Vec::new(),
            }
        }
        "content_block_stop" => vec![EngineEvent::Delta(StreamEvent::BlockStop { index })],
        _ => Vec::new(),
    }
}

fn parse_content(content: Option<&Value>) -> Vec<ContentBlock> {
    let Some(items) = content.and_then(Value::as_array) else {
        return Vec::new();
    };
    items
        .iter()
        .filter_map(|b| match b.get("type").and_then(Value::as_str)? {
            "text" => Some(ContentBlock::Text {
                text: b
                    .get("text")
                    .and_then(Value::as_str)
                    .unwrap_or("")
                    .to_owned(),
            }),
            "thinking" => Some(ContentBlock::Thinking {
                text: b
                    .get("thinking")
                    .and_then(Value::as_str)
                    .unwrap_or("")
                    .to_owned(),
                signature: b
                    .get("signature")
                    .and_then(Value::as_str)
                    .map(str::to_owned),
                redacted: false,
            }),
            "redacted_thinking" => Some(ContentBlock::Thinking {
                text: String::new(),
                signature: None,
                redacted: true,
            }),
            "tool_use" => Some(ContentBlock::ToolUse {
                id: b.get("id").and_then(Value::as_str).unwrap_or("").to_owned(),
                name: b
                    .get("name")
                    .and_then(Value::as_str)
                    .unwrap_or("")
                    .to_owned(),
                input: b
                    .get("input")
                    .cloned()
                    .unwrap_or(Value::Object(Default::default())),
            }),
            _ => None,
        })
        .collect()
}

fn parse_usage(u: &Value) -> Usage {
    let get = |k: &str| u.get(k).and_then(Value::as_u64).unwrap_or(0);
    let cache_write_tokens = get("cache_creation_input_tokens");
    Usage {
        input_tokens: get("input_tokens"),
        output_tokens: get("output_tokens"),
        cache_read_tokens: get("cache_read_input_tokens"),
        cache_write_tokens,
        cache_write_1h_tokens: one_hour_writes(u, cache_write_tokens),
        reasoning_tokens: u
            .get("output_tokens_details")
            .and_then(|d| d.get("thinking_tokens"))
            .and_then(Value::as_u64),
        // Tokens only. The price is on the outcome (`total_cost_usd`) and is
        // folded once there; calling this unpriced would mark a total the
        // engine did report a price for as a floor.
        cost_usd: 0.0,
        cost_complete: true,
        cost_gap: None,
        cost_basis: Default::default(),
        // Any estimate for these tokens comes from the registry, in the runtime.
        estimated_cost_usd: None,
    }
}

/// The 1-hour share of `cache_creation_input_tokens`, from `cache_creation`. Zero when
/// unreported, and never more than the total: a split beyond it cannot be trusted.
fn one_hour_writes(u: &Value, cache_write_tokens: u64) -> u64 {
    let one_hour = u
        .get("cache_creation")
        .and_then(|split| split.get("ephemeral_1h_input_tokens"))
        .and_then(Value::as_u64)
        .unwrap_or(0);
    if one_hour > cache_write_tokens {
        tracing::warn!(
            one_hour,
            cache_write_tokens,
            "Claude reported more 1-hour cache writes than cache writes; pricing the total as 1-hour"
        );
        return cache_write_tokens;
    }
    one_hour
}

#[cfg(test)]
#[path = "tests/stream.rs"]
mod tests;

#[cfg(test)]
#[path = "tests/usage.rs"]
mod usage_tests;
