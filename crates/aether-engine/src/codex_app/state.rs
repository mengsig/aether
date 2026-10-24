//! Pure turn-state mapping from native notifications and approval requests.

use std::collections::{HashMap, HashSet};

use aether_core::model::{ContentBlock, StreamEvent, Usage};
use aether_core::tool::SideEffectClass;
use serde_json::{Value, json};

use super::items::{str_of, texts_of, tool_result_of, tool_use_of};
use super::usage::{RawTotals, advanced, consumed_since, read_totals, usage_of};
use crate::{EngineEvent, EngineOutcome, EngineTurn};

/// A server request that waits for an approval decision from the runtime.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ServerRequest {
    Command,
    FileChange,
    /// Carries the requested permission profile so it can be granted as asked.
    Permissions(Value),
    /// Question ids, in the order asked; the person's reply answers the first.
    Question(Vec<String>),
}

/// Everything learned about the turn so far. Pure: mapping is testable
/// without a process.
#[derive(Debug)]
pub struct TurnState {
    pub(super) turn: EngineTurn,
    pub thread_id: Option<String>,
    pub turn_id: Option<String>,
    /// Tool items already reported as started, so completion only adds the result.
    started: HashSet<String>,
    /// Reasoning items whose text arrived as deltas.
    reasoning_streamed: HashSet<String>,
    /// The thread's counters as last read, field by field. Seeded with what
    /// the thread had already counted when this run resumed it; a field the
    /// engine leaves out or sends unparseably keeps the value it had.
    usage_last: RawTotals,
    /// What this run has billed of each of those counters.
    usage_billed: RawTotals,
    /// This run's usage so far. Monotone: a counter that dips never lowers it.
    pub(super) usage_total: Usage,
    /// A `thread/tokenUsage/updated` was seen; a turn without one is a
    /// protocol change that would silently bill nothing.
    usage_seen: bool,
    /// Protocol notes emitted this turn, so a flapping engine cannot drive
    /// the log or the ledger.
    protocol_notes: u32,
    pub(super) final_text: Option<String>,
    error: Option<String>,
    /// Open server requests by JSON-RPC id (as text), awaiting decisions.
    pub requests: HashMap<String, (Value, ServerRequest)>,
    /// Messages to steer with once the turn has an id.
    pub queued_input: Vec<crate::EngineInput>,
    pub done: bool,
    pub(super) cancelling: bool,
}

/// Protocol notes one turn may raise. Past it the engine is flapping and the
/// notes say nothing new, so they stop reaching the log and the ledger.
const MAX_PROTOCOL_NOTES: u32 = 8;

impl TurnState {
    pub fn new(turn: EngineTurn) -> Self {
        // A fresh thread has counted nothing, whatever the caller passed.
        debug_assert!(
            turn.session.is_some() || turn.session_usage == Usage::default(),
            "a fresh thread carries no prior spend"
        );
        let baseline = if turn.session.is_some() {
            turn.session_usage
        } else {
            Usage::default()
        };
        Self {
            turn,
            thread_id: None,
            turn_id: None,
            started: HashSet::new(),
            reasoning_streamed: HashSet::new(),
            usage_last: RawTotals::of(baseline),
            usage_billed: RawTotals::default(),
            usage_total: Usage::default(),
            usage_seen: false,
            protocol_notes: 0,
            final_text: None,
            error: None,
            requests: HashMap::new(),
            queued_input: Vec::new(),
            done: false,
            cancelling: false,
        }
    }

    /// A surprise in what the app-server sent that billing depends on: said
    /// out loud rather than swallowed, because silently mis-billing an engine
    /// is what this exists to prevent. Capped per turn so an engine flapping its
    /// counters cannot drive the log or the ledger.
    fn protocol_note(&mut self, text: String) -> EngineEvent {
        self.protocol_notes = self.protocol_notes.saturating_add(1);
        if self.protocol_notes > MAX_PROTOCOL_NOTES {
            tracing::debug!("{text}");
            return EngineEvent::Note(text);
        }
        tracing::warn!("{text}");
        EngineEvent::ProtocolNote(text)
    }

    /// Map one server notification to engine events.
    pub fn notification(&mut self, method: &str, p: &Value) -> Vec<EngineEvent> {
        if self.done {
            // Billing is closed once the turn reported its result; usage
            // arriving after it would be lost, so say so rather than drop it.
            if method == "thread/tokenUsage/updated" {
                // The payload is the child's: truncated, and at debug, so an
                // arbitrarily large value cannot become an arbitrarily large
                // warn line. The note itself carries no engine JSON.
                let usage: String = p["tokenUsage"].to_string().chars().take(200).collect();
                tracing::debug!(%usage, "unbilled usage after the turn ended");
                return vec![self.protocol_note(
                    "codex reported token usage after the turn ended; it is not billed".into(),
                )];
            }
            return Vec::new();
        }
        // A server can broadcast native child events on this connection. They
        // must never be attributed to the Aether agent owning this thread.
        if let (Some(expected), Some(actual)) = (&self.thread_id, p["threadId"].as_str())
            && expected != actual
        {
            return Vec::new();
        }
        if matches!(method, "item/started" | "item/completed")
            && p["item"]["type"] == "collabAgentToolCall"
        {
            self.done = true;
            return vec![EngineEvent::Done(EngineOutcome {
                is_error: true,
                interrupted: true,
                error: Some(format!(
                    "Codex used native delegation tool {:?} despite agents.enabled=false; \
                     stopped the engine because these agents are not tracked by Aether. \
                     Update Codex and retry using Aether delegate.",
                    str_of(&p["item"], "tool")
                )),
                usage: self.usage_total,
                ..Default::default()
            })];
        }
        match method {
            "item/agentMessage/delta" => vec![EngineEvent::Delta(StreamEvent::TextDelta {
                index: 0,
                text: str_of(p, "delta"),
            })],
            "item/reasoning/textDelta" | "item/reasoning/summaryTextDelta" => {
                self.reasoning_streamed.insert(str_of(p, "itemId"));
                vec![EngineEvent::Delta(StreamEvent::ThinkingDelta {
                    index: 0,
                    text: str_of(p, "delta"),
                })]
            }
            "item/started" => self.item_started(&p["item"]),
            "item/completed" => self.item_completed(&p["item"]),
            "thread/tokenUsage/updated" => self.account(&p["tokenUsage"]),
            "account/rateLimits/updated" => vec![EngineEvent::RateLimit {
                payload: p["rateLimits"].clone(),
            }],
            "error" => {
                let message = p["error"]["message"]
                    .as_str()
                    .unwrap_or("unknown error")
                    .to_owned();
                if p["willRetry"].as_bool().unwrap_or(false) {
                    vec![EngineEvent::Note(format!("retrying: {message}"))]
                } else {
                    self.error = Some(message);
                    Vec::new()
                }
            }
            "turn/completed" => {
                let turn = &p["turn"];
                let status = turn["status"].as_str().unwrap_or("completed");
                let unexpected_interrupt = status == "interrupted" && !self.cancelling;
                let error = turn["error"]["message"]
                    .as_str()
                    .map(str::to_owned)
                    .or_else(|| self.error.clone())
                    .or_else(|| {
                        unexpected_interrupt
                            .then(|| "codex interrupted the turn unexpectedly".into())
                    });
                let failed = status == "failed"
                    || unexpected_interrupt
                    || (status == "interrupted" && error.is_some());
                self.done = true;
                let mut events = Vec::new();
                // Billing rides these updates alone, so a turn that saw none
                // is billed zero - loudly, never silently.
                if !self.usage_seen {
                    events.push(
                        self.protocol_note(
                            "codex ended the turn without reporting any token usage; \
                         this turn is billed zero"
                                .into(),
                        ),
                    );
                }
                events.push(EngineEvent::Done(EngineOutcome {
                    closing_accounting: status != "interrupted",
                    usage: self.usage_total,
                    stop_reason: Some(status.to_owned()),
                    duration_ms: turn["durationMs"].as_u64(),
                    is_error: failed,
                    error: if failed { error } else { None },
                    session: self.thread_id.clone(),
                    text: self.final_text.clone(),
                    ..Default::default()
                }));
                events
            }
            _ => Vec::new(),
        }
    }

    /// Account one `thread/tokenUsage/updated`. Codex reports the *thread's*
    /// cumulative totals, so this run's share is the increase over what the
    /// thread had counted when this run resumed it - a figure the runtime
    /// persists per session, never one derived from the first update, which
    /// on many Codex builds merely echoes the previous turn's counters.
    /// Every native request lands exactly once that way: tool-only requests
    /// and usage arriving after an item completed included. Each counter is
    /// tracked on its own, so one field the engine drops or garbles cannot
    /// re-bill the rest.
    fn account(&mut self, usage: &Value) -> Vec<EngineEvent> {
        if usage["total"].is_null() {
            return vec![self.protocol_note(
                "codex sent a token usage update with no `total`; nothing can be billed from it"
                    .into(),
            )];
        }
        self.usage_seen = true;
        let (reading, unusable) = read_totals(&usage["total"], self.usage_last);
        let mut events = Vec::new();
        if !unusable.is_empty() {
            events.push(self.protocol_note(format!(
                "codex sent a token usage update whose {} could not be read; \
                 the previous reading stands for {}",
                unusable.join(", "),
                if unusable.len() == 1 { "it" } else { "them" }
            )));
        }
        // Per field, because the fields do not move together: on a compaction
        // the cached counter restarts while the input counter climbs, and one
        // restarted counter must not re-bill the others from zero.
        let mut restarted = Vec::new();
        let mut dipped = Vec::new();
        macro_rules! step {
            ($field:ident, $reported:literal) => {{
                let moved = advanced(reading.$field, self.usage_last.$field);
                self.usage_last.$field = moved.carried;
                self.usage_billed.$field = self.usage_billed.$field.saturating_add(moved.billed);
                if moved.restarted {
                    restarted.push(format!("{} by {}", $reported, moved.dipped));
                } else if moved.dipped > 0 {
                    dipped.push(format!("{} by {}", $reported, moved.dipped));
                }
            }};
        }
        step!(input, "inputTokens");
        step!(cached, "cachedInputTokens");
        step!(output, "outputTokens");
        step!(cache_write, "cacheWriteInputTokens");
        step!(reasoning, "reasoningOutputTokens");
        // Every dip that is not a restart is said out loud, the shallow ones
        // included: a correction bills nothing, and a bill nobody can see is
        // a bill nobody can check. A restart is the same counter contradicting
        // itself, and its own note says so - both would spend two of the
        // eight notes one turn has on one event.
        if !dipped.is_empty() {
            events.push(self.protocol_note(format!(
                "codex's thread counter came back below what it had reported: {}",
                dipped.join(", ")
            )));
        }
        if !restarted.is_empty() {
            events.push(self.protocol_note(format!(
                "codex restarted its thread counter: {} ({} billed so far this run)",
                restarted.join(", "),
                self.usage_total.total_tokens()
            )));
        }
        // Never lowered: a derived split can dip when the cached counter grows
        // faster than the input one, and a bill that dips is billed twice.
        let delta = consumed_since(self.usage_billed.into_usage(), self.usage_total);
        self.usage_total += delta;
        if let Some(session) = self.thread_id.clone() {
            // Ahead of the delta: whatever writes the delta persists this with
            // it, so a turn cut short cannot bill a run twice on resume.
            events.push(EngineEvent::SessionUsage {
                models: None,
                session,
                total: self.usage_last.into_usage(),
            });
        }
        if delta.total_tokens() > 0 || delta.reasoning_tokens.is_some_and(|n| n > 0) {
            events.push(EngineEvent::UsageDelta { usage: delta });
        }
        if usage["last"].is_null() {
            events.push(self.protocol_note(
                "codex sent a token usage update with no `last`; context size is unknown".into(),
            ));
            return events;
        }
        let last = usage_of(&usage["last"]);
        events.push(EngineEvent::ContextUsage {
            tokens: last
                .input_tokens
                .saturating_add(last.cache_read_tokens)
                .saturating_add(last.cache_write_tokens),
        });
        events
    }

    fn item_started(&mut self, item: &Value) -> Vec<EngineEvent> {
        let Some(block) = tool_use_of(item) else {
            return Vec::new();
        };
        let id = str_of(item, "id");
        self.started.insert(id.clone());
        let mut out = Vec::new();
        // A command outlives its tool call once it yields; reported live, a
        // `wait_for` keeps the session, and with it the process, attached.
        // Ahead of the tool use, so a recorded command is already live.
        if item["type"] == "commandExecution" {
            out.push(EngineEvent::BackgroundTask { id, running: true });
        }
        out.push(EngineEvent::Assistant {
            content: vec![block],
            usage: None,
            model: None,
        });
        out
    }

    fn item_completed(&mut self, item: &Value) -> Vec<EngineEvent> {
        let id = str_of(item, "id");
        match item["type"].as_str().unwrap_or("") {
            "agentMessage" => {
                let text = str_of(item, "text");
                if text.trim().is_empty() {
                    return Vec::new();
                }
                self.final_text = Some(text.clone());
                vec![EngineEvent::Assistant {
                    content: vec![ContentBlock::Text { text }],
                    // Usage rides `thread/tokenUsage/updated` alone: attaching
                    // it here too would count the request twice and would miss
                    // every tool-only request.
                    usage: None,
                    model: None,
                }]
            }
            "reasoning" if !self.reasoning_streamed.contains(&id) => {
                let text = texts_of(&item["summary"]).join("\n");
                if text.trim().is_empty() {
                    return Vec::new();
                }
                vec![EngineEvent::Delta(StreamEvent::ThinkingDelta {
                    index: 0,
                    text,
                })]
            }
            _ => {
                let Some((content, is_error)) = tool_result_of(item) else {
                    return Vec::new();
                };
                let mut out = Vec::new();
                if !self.started.remove(&id)
                    && let Some(block) = tool_use_of(item)
                {
                    out.push(EngineEvent::Assistant {
                        content: vec![block],
                        usage: None,
                        model: None,
                    });
                }
                let exited = item["type"] == "commandExecution" && item["status"] != "inProgress";
                out.push(EngineEvent::ToolResult {
                    tool_use_id: id.clone(),
                    content,
                    is_error,
                });
                if exited {
                    out.push(EngineEvent::BackgroundTask { id, running: false });
                }
                out
            }
        }
    }

    /// A request from the server. Approvals become permission events for
    /// the runtime to decide; anything else is answered on the spot.
    pub fn server_request(&mut self, id: &Value, method: &str, p: &Value) -> ServerReply {
        let key = id.to_string();
        match method {
            "item/commandExecution/requestApproval" => {
                self.requests
                    .insert(key.clone(), (id.clone(), ServerRequest::Command));
                ServerReply::Ask(EngineEvent::Permission {
                    request_id: key,
                    tool: "bash".into(),
                    input: json!({
                        "command": p["command"],
                        "tool_use_id": p["itemId"],
                        "cwd": p["cwd"],
                        "reason": p["reason"],
                    }),
                    side_effect: Some(SideEffectClass::External),
                })
            }
            "item/fileChange/requestApproval" => {
                self.requests
                    .insert(key.clone(), (id.clone(), ServerRequest::FileChange));
                ServerReply::Ask(EngineEvent::Permission {
                    request_id: key,
                    tool: "apply_patch".into(),
                    input: json!({ "reason": p["reason"], "grant_root": p["grantRoot"], "tool_use_id": p["itemId"] }),
                    side_effect: Some(SideEffectClass::External),
                })
            }
            "item/permissions/requestApproval" => {
                self.requests.insert(
                    key.clone(),
                    (
                        id.clone(),
                        ServerRequest::Permissions(p["permissions"].clone()),
                    ),
                );
                ServerReply::Ask(EngineEvent::Permission {
                    request_id: key,
                    tool: "permissions".into(),
                    input: json!({ "permissions": p["permissions"], "reason": p["reason"] }),
                    side_effect: Some(SideEffectClass::External),
                })
            }
            "item/tool/requestUserInput" => {
                let questions: Vec<&Value> =
                    p["questions"].as_array().into_iter().flatten().collect();
                let ids: Vec<String> = questions
                    .iter()
                    .filter_map(|q| q["id"].as_str().map(str::to_owned))
                    .collect();
                let prompt = question_prompt(&questions);
                self.requests
                    .insert(key.clone(), (id.clone(), ServerRequest::Question(ids)));
                ServerReply::Ask(EngineEvent::Question {
                    request_id: key,
                    prompt,
                })
            }
            "mcpServer/elicitation/request" => {
                ServerReply::Now(json!({ "action": "decline" }), None)
            }
            _ => ServerReply::Unsupported,
        }
    }

    /// The JSON-RPC result answering a stored request with a decision or,
    /// for a question, the person's reply.
    pub fn decision_result(
        &mut self,
        request_id: &str,
        allow: bool,
        answer: Option<&str>,
    ) -> Option<(Value, Value)> {
        let (id, kind) = self.requests.remove(request_id)?;
        let result = match (kind, allow) {
            (ServerRequest::Question(ids), _) => {
                let mut answers = serde_json::Map::new();
                for (i, qid) in ids.iter().enumerate() {
                    let given: Vec<&str> = match answer {
                        Some(text) if i == 0 && !text.trim().is_empty() => vec![text.trim()],
                        _ => Vec::new(),
                    };
                    answers.insert(qid.clone(), json!({ "answers": given }));
                }
                json!({ "answers": answers })
            }
            (ServerRequest::Command | ServerRequest::FileChange, true) => {
                json!({ "decision": "accept" })
            }
            (ServerRequest::Command | ServerRequest::FileChange, false) => {
                json!({ "decision": "decline" })
            }
            (ServerRequest::Permissions(asked), true) => {
                json!({ "permissions": asked, "scope": "turn" })
            }
            (ServerRequest::Permissions(_), false) => json!({ "permissions": {} }),
        };
        Some((id, result))
    }
}

/// What to do with a server request.
#[derive(Debug)]
pub enum ServerReply {
    /// Emit this event; the runtime answers later through a decision.
    Ask(EngineEvent),
    /// Answer immediately with this result (and optionally note why).
    Now(Value, Option<String>),
    Unsupported,
}
/// The questions as one message from the agent, options included.
fn question_prompt(questions: &[&Value]) -> String {
    let mut out = String::new();
    for q in questions {
        let header = q["header"].as_str().unwrap_or("").trim();
        let text = q["question"].as_str().unwrap_or("").trim();
        if !out.is_empty() {
            out.push_str("\n\n");
        }
        if !header.is_empty() && header != text {
            out.push_str(header);
            out.push_str(": ");
        }
        out.push_str(text);
        for option in q["options"].as_array().into_iter().flatten() {
            let label = option["label"].as_str().unwrap_or("");
            let description = option["description"].as_str().unwrap_or("");
            if description.is_empty() {
                out.push_str(&format!("\n- {label}"));
            } else {
                out.push_str(&format!("\n- {label}: {description}"));
            }
        }
    }
    if out.is_empty() {
        out.push_str("(codex asked a question without text)");
    }
    out
}
