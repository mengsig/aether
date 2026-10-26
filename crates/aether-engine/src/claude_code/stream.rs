//! Decode native Claude stream events and account for usage.

use std::collections::VecDeque;
use std::sync::{Arc, Mutex};

use aether_core::model::{BlockStart, ContentBlock, StreamEvent, Usage};
use serde_json::Value;

use crate::{EngineEvent, EngineOutcome, UsageRequestId};

mod accounting;
mod refusal;

/// Per-process parser state: tool names by content-block index for the
/// message being streamed (so input deltas carry the name), and the steered
/// messages the CLI has not echoed back yet, oldest first.
#[derive(Debug, Default)]
pub struct ParseState {
    session_cost: super::session_cost::SessionCost,
    tool_names: std::collections::BTreeMap<usize, String>,
    pub(super) outstanding: Arc<Mutex<VecDeque<Steered>>>,
    message_usage: std::collections::BTreeMap<UsageRequestId, Usage>,
    message_models: std::collections::BTreeMap<UsageRequestId, String>,
    streamed_models: std::collections::BTreeMap<String, Usage>,
    streamed_usage_seen: bool,
    /// A closing receipt in this process counted usage, streamed or not.
    receipt_usage_seen: bool,
    /// An earlier receipt, model output, or a retry after an attempt the API did not
    /// refuse may have been billed without usage to show for it.
    refusal_unprovable: bool,
    /// This process proved a receipt unbilled; a later retry, request or output withdraws it.
    zero_proven: bool,
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

/// A message written to the CLI mid-turn and not yet echoed back.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct Steered {
    pub(super) uuid: String,
    pub(super) text: String,
}

/// Messages one run keeps high-water marks for. The ids are the engine's, so
/// this is bounded; evicting the oldest closed message can only bill one
/// twice, never drop one.
const MAX_TRACKED_MESSAGES: usize = 256;

impl ParseState {
    /// Initialize the prompt acknowledgement and the resumed session's cost baseline.
    pub fn for_turn(turn: &crate::EngineTurn) -> Self {
        let mut state = Self::awaiting_prompt(&turn.prompt)
            .with_session_cost(turn.session.clone(), turn.session_usage);
        state.session_cost = state
            .session_cost
            .with_models(turn.session_model_usage.clone());
        state
    }

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

    /// Remember a message handed to the CLI, so its echo is recognised.
    pub fn expect_echo(&self, uuid: &str, text: &str) {
        assert!(!uuid.is_empty(), "a steered message is echoed by its uuid");
        self.outstanding
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .push_back(Steered {
                uuid: uuid.to_owned(),
                text: text.to_owned(),
            });
    }

    /// How many messages an echo acknowledges; pops them. Matched by uuid,
    /// else by the oldest one's text, for a CLI that echoes uuids of its own.
    /// The CLI takes input in order, so an echo also covers older messages.
    fn take_echoes(&self, uuid: Option<&str>, text: Option<&str>) -> usize {
        let mut queue = self.outstanding.lock().unwrap_or_else(|e| e.into_inner());
        let by_uuid = uuid.and_then(|uuid| queue.iter().position(|m| m.uuid == uuid));
        let by_text = || {
            let oldest = queue.front()?;
            text.is_some_and(|t| t.trim() == oldest.text.trim())
                .then_some(0)
        };
        let Some(last) = by_uuid.or_else(by_text) else {
            return 0;
        };
        debug_assert!(last < queue.len(), "the echoed message is outstanding");
        queue.drain(..=last).count()
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
            Some("api_retry") => state.note_retry(&v),
            Some("status") => {
                let mut events = state.note_status(&v);
                events.extend(compaction_failure(&v));
                events
            }
            Some("compact_boundary") => vec![compacted(&v)],
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
        "stream_event" => {
            let mut events = state.note_output(&v);
            if let Some(event) = v.get("event") {
                events.extend(stream_deltas(event, state));
            }
            events
        }
        "assistant" => {
            let mut events = state.note_output(&v);
            let message = &v["message"];
            let content = parse_content(message.get("content"));
            // Usage rides the delta stream alone, keyed per message: attaching
            // it here would bill one message once per content block.
            let accounting = match message.get("usage") {
                Some(usage) => {
                    let key = state.assistant_key(message.get("id").and_then(Value::as_str));
                    state.account_usage(key, usage, message.get("model").and_then(Value::as_str))
                }
                None => Vec::new(),
            };
            state.tool_names.clear();
            events.push(EngineEvent::Assistant {
                content,
                usage: None,
                model: message
                    .get("model")
                    .and_then(Value::as_str)
                    .map(str::to_owned),
            });
            events.extend(accounting);
            events
        }
        "user" => {
            let mut out = Vec::new();
            if v.get("isReplay").and_then(Value::as_bool) == Some(true) {
                out.extend(echo_acknowledgements(&v, state));
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
            state.note_skipped_receipt();
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
            let usage = v.get("usage").map(parse_usage).unwrap_or_default();
            state.receipt_usage_seen |=
                usage.total_tokens() > 0 || usage.reasoning_tokens.is_some_and(|tokens| tokens > 0);
            let spend = state.spend_evidence(&v, is_error);
            let receipt = state
                .session_cost
                .receipt(&v, &state.streamed_models, spend);
            let cost_usd = receipt.cost_usd;
            let cost_usage = receipt.cost_usage;
            let mut events = receipt.events;
            events.push(EngineEvent::Done(EngineOutcome {
                closing_accounting: true,
                usage,
                cost_usd,
                cost_usage: cost_usage.map(Box::new),
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

/// `compact_boundary`: the session now continues from Claude Code's summary.
fn compacted(v: &Value) -> EngineEvent {
    debug_assert_eq!(
        v.get("subtype").and_then(Value::as_str),
        Some("compact_boundary")
    );
    let metadata = v.get("compact_metadata");
    let count = |key: &str| metadata.and_then(|m| m.get(key)).and_then(Value::as_u64);
    EngineEvent::Compacted {
        automatic: metadata
            .and_then(|m| m.get("trigger"))
            .and_then(Value::as_str)
            == Some("auto"),
        before_tokens: count("pre_tokens"),
        after_tokens: count("post_tokens"),
        duration_ms: count("duration_ms"),
    }
}

/// A `status` line that reports a failed compaction, with Claude Code's reason.
fn compaction_failure(v: &Value) -> Option<EngineEvent> {
    debug_assert_eq!(v.get("subtype").and_then(Value::as_str), Some("status"));
    if v.get("compact_result").and_then(Value::as_str) != Some("failed") {
        return None;
    }
    let error = v
        .get("compact_error")
        .and_then(Value::as_str)
        .filter(|e| !e.trim().is_empty())
        .unwrap_or("Claude Code could not compact the conversation");
    Some(EngineEvent::CompactionFailed {
        error: error.chars().take(500).collect(),
    })
}

/// Whether a pre-echo result closed a queued task notice without running the model, e.g. an
/// orphaned shell's on `--resume`. Any other result ends the turn, so an unmatched echo cannot hang it.
fn is_queued_work_result(v: &Value) -> bool {
    let succeeded = v.get("subtype").and_then(Value::as_str) == Some("success")
        && v.get("is_error").and_then(Value::as_bool) != Some(true);
    let notice = v.pointer("/origin/kind").and_then(Value::as_str) == Some("task-notification");
    succeeded && notice && v.get("num_turns").and_then(Value::as_u64) == Some(0)
}

/// One `InputAccepted` per steered message a replayed user message echoes.
/// The prompt's own echo acknowledges none.
fn echo_acknowledgements(v: &Value, state: &mut ParseState) -> Vec<EngineEvent> {
    debug_assert_eq!(v.get("isReplay").and_then(Value::as_bool), Some(true));
    let uuid = v.get("uuid").and_then(Value::as_str);
    let text = replayed_text(&v["message"]);
    if text.as_deref().is_some_and(|t| state.take_prompt_echo(t)) {
        return Vec::new();
    }
    let acknowledged = state.take_echoes(uuid, text.as_deref());
    vec![EngineEvent::InputAccepted; acknowledged]
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
                Some(usage) => {
                    state.account_usage(key, usage, message.get("model").and_then(Value::as_str))
                }
                None => Vec::new(),
            }
        }
        // The final output count. Keyed off the open message, or - when its
        // `message_start` never arrived - off a key of its own, so the count
        // is billed once rather than dropped.
        "message_delta" => match event.get("usage") {
            Some(usage) => {
                let key = state.open_key();
                state.account_usage(key, usage, None)
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

#[cfg(test)]
#[path = "tests/receipt_evidence.rs"]
mod receipt_evidence_tests;
