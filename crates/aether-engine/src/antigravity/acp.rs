//! The Agent Client Protocol as `agy acp` speaks it: the requests one turn
//! sends, and the translation of what the server sends back into engine events.
//!
//! One turn is a fixed sequence: `initialize`, then `session/new` (or
//! `session/load` to resume), `session/set_model` when a model is named, and
//! `session/prompt`, whose response closes the turn with its usage.

use std::collections::{HashMap, HashSet};
use std::sync::{Arc, Mutex};

use aether_core::model::{ContentBlock, StreamEvent, Usage};
use aether_core::tool::SideEffectClass;
use serde_json::{Value, json};
use tokio::sync::mpsc::UnboundedSender;

use crate::{EngineEvent, EngineOutcome, PermissionDecision};

const INITIALIZE: u64 = 1;
const SESSION: u64 = 2;
const SET_MODEL: u64 = 3;
const PROMPT: u64 = 4;

/// ACP's JSON-RPC code for a session id the server does not hold.
const SESSION_NOT_FOUND: i64 = -32002;

/// What one turn asks of the server, fixed before it starts.
#[derive(Debug, Clone)]
pub(super) struct TurnPlan {
    pub cwd: String,
    /// The session to resume; `None` starts a fresh one.
    pub resume: Option<String>,
    /// The `agy` model id, effort variant included.
    pub model: Option<String>,
    /// `session/prompt`'s content blocks.
    pub prompt: Vec<Value>,
    /// `session/new`'s `mcpServers`.
    pub mcp_servers: Vec<Value>,
    /// Refuse every permission request without asking: a read-only summary turn.
    pub deny_all: bool,
}

/// A permission request waiting for the runtime's answer, by its request id.
#[derive(Debug, Clone)]
pub(super) struct OpenRequest {
    id: Value,
    allow: Option<String>,
    reject: Option<String>,
}

/// State the stdin writers share with the parser: the live session id, for
/// `session/cancel`, and the permission requests still open.
#[derive(Debug, Default)]
pub(super) struct Shared {
    pub session: Option<String>,
    pub open: HashMap<String, OpenRequest>,
}

pub(super) type SharedState = Arc<Mutex<Shared>>;

/// Records a fresh session as this agent's; it runs before any request is
/// sent into the session, so an unowned session never receives the prompt.
pub(super) type SessionOwner = Box<dyn FnMut(&str) -> Result<(), String> + Send>;

pub(super) fn lock(shared: &SharedState) -> std::sync::MutexGuard<'_, Shared> {
    shared
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}

fn request(id: u64, method: &str, params: Value) -> String {
    format!(
        "{}\n",
        json!({"jsonrpc": "2.0", "id": id, "method": method, "params": params})
    )
}

/// The lines written before the server answers anything: the handshake and
/// the session request, which needs nothing from the handshake's answer.
pub(super) fn opening_lines(plan: &TurnPlan) -> String {
    assert!(!plan.cwd.is_empty(), "a session runs in a workspace");
    let initialize = request(
        INITIALIZE,
        "initialize",
        json!({
            "protocolVersion": 1,
            "clientCapabilities": {"fs": {"readTextFile": false, "writeTextFile": false}, "terminal": false}
        }),
    );
    let session = match &plan.resume {
        Some(id) => request(
            SESSION,
            "session/load",
            json!({"sessionId": id, "cwd": plan.cwd, "mcpServers": plan.mcp_servers}),
        ),
        None => request(
            SESSION,
            "session/new",
            json!({"cwd": plan.cwd, "mcpServers": plan.mcp_servers}),
        ),
    };
    format!("{initialize}{session}")
}

/// `session/cancel` for the live session, once there is one.
pub(super) fn cancel_line(shared: &SharedState) -> Option<String> {
    let session = lock(shared).session.clone()?;
    let note =
        json!({"jsonrpc": "2.0", "method": "session/cancel", "params": {"sessionId": session}});
    Some(format!("{note}\n"))
}

/// The JSON-RPC response that carries the runtime's permission decision, or
/// `None` for a request that is no longer open.
pub(super) fn decision_line(shared: &SharedState, decision: &PermissionDecision) -> Option<String> {
    let open = lock(shared).open.remove(&decision.request_id)?;
    let option = if decision.allow {
        open.allow
    } else {
        open.reject
    };
    let outcome = match option {
        Some(id) => json!({"outcome": "selected", "optionId": id}),
        None => json!({"outcome": "cancelled"}),
    };
    let reply = json!({"jsonrpc": "2.0", "id": open.id, "result": {"outcome": outcome}});
    Some(format!("{reply}\n"))
}

/// Parses the server's lines for one turn and writes the requests that
/// follow from its answers.
pub struct AcpState {
    plan: TurnPlan,
    shared: SharedState,
    own_session: SessionOwner,
    /// Requests that follow an answer; dropped when the turn is over, which
    /// closes the server's stdin.
    out: Option<UnboundedSender<String>>,
    /// Assistant text since the last tool call, flushed as one message.
    text: String,
    /// Every assistant text of the turn, the outcome's final text.
    reply: String,
    tools_done: HashSet<String>,
    tools_seen: HashSet<String>,
    /// The session answered; updates before that replay a resumed history.
    ready: bool,
    done: bool,
}

impl AcpState {
    pub(super) fn new(
        plan: TurnPlan,
        shared: SharedState,
        out: UnboundedSender<String>,
        own_session: SessionOwner,
    ) -> Self {
        Self {
            plan,
            shared,
            own_session,
            out: Some(out),
            text: String::new(),
            reply: String::new(),
            tools_done: HashSet::new(),
            tools_seen: HashSet::new(),
            ready: false,
            done: false,
        }
    }

    fn send(&self, line: String) {
        debug_assert!(line.ends_with('\n'), "one JSON-RPC message per line");
        if let Some(out) = &self.out {
            // A closed writer means the process is gone; its exit ends the turn.
            let _ = out.send(line);
        }
    }

    /// Translate one line from the server.
    pub fn parse_line(&mut self, line: &str) -> Vec<EngineEvent> {
        if self.done {
            return Vec::new();
        }
        let Ok(message) = serde_json::from_str::<Value>(line) else {
            return vec![EngineEvent::Note(format!(
                "agy acp: unparsed line: {}",
                bounded(line)
            ))];
        };
        let method = message["method"].as_str();
        let has_id = message.get("id").is_some_and(|id| !id.is_null());
        match (method, has_id) {
            (Some(method), true) => self.server_request(method, &message),
            (Some("session/update"), false) => self.update(&message["params"]["update"]),
            (Some(_), false) => Vec::new(),
            (None, true) => self.response(&message),
            (None, false) => vec![EngineEvent::Note(
                "agy acp: message without id or method".into(),
            )],
        }
    }

    fn response(&mut self, message: &Value) -> Vec<EngineEvent> {
        let id = message["id"].as_u64();
        let error = message.get("error").filter(|e| !e.is_null());
        match (id, error) {
            (Some(INITIALIZE), None) => Vec::new(),
            (Some(SESSION), None) => self.session_ready(&message["result"]),
            (Some(SET_MODEL), None) => {
                self.send_prompt();
                Vec::new()
            }
            (Some(PROMPT), None) => self.finish(&message["result"]),
            (Some(PROMPT), Some(error)) => self.fail(rpc_error("session/prompt", error), false),
            (Some(SESSION), Some(error)) if self.plan.resume.is_some() && lost(error) => {
                let mut events = self.fail(rpc_error("session/load", error), true);
                if let Some(EngineEvent::Done(outcome)) = events.last_mut() {
                    outcome.session_lost = true;
                }
                events
            }
            (Some(step), Some(error)) => {
                let method = match step {
                    INITIALIZE => "initialize",
                    SESSION => "session",
                    _ => "session/set_model",
                };
                self.fail(rpc_error(method, error), true)
            }
            (_, _) => vec![EngineEvent::Note(
                "agy acp: response to an unknown request".into(),
            )],
        }
    }

    fn session_ready(&mut self, result: &Value) -> Vec<EngineEvent> {
        let id = self
            .plan
            .resume
            .clone()
            .or_else(|| result["sessionId"].as_str().map(str::to_owned));
        let Some(id) = id.filter(|id| !id.trim().is_empty()) else {
            return self.fail("agy acp started a session without an id".into(), true);
        };
        if let Err(error) = (self.own_session)(&id) {
            return self.fail(error, true);
        }
        lock(&self.shared).session = Some(id.clone());
        self.ready = true;
        match self.plan.model.clone() {
            Some(model) => self.send(request(
                SET_MODEL,
                "session/set_model",
                json!({"sessionId": id, "modelId": model}),
            )),
            None => self.send_prompt(),
        }
        vec![EngineEvent::Session {
            id,
            model: self.plan.model.clone(),
            tools: Vec::new(),
        }]
    }

    fn send_prompt(&self) {
        let session = lock(&self.shared).session.clone();
        debug_assert!(session.is_some(), "a prompt goes to a started session");
        self.send(request(
            PROMPT,
            "session/prompt",
            json!({"sessionId": session, "prompt": self.plan.prompt}),
        ));
    }

    /// The `session/prompt` result: the turn's end and its token counts.
    fn finish(&mut self, result: &Value) -> Vec<EngineEvent> {
        let mut events = self.flush_text();
        let stop = result["stopReason"]
            .as_str()
            .unwrap_or("end_turn")
            .to_owned();
        let cancelled = stop == "cancelled";
        let refused = stop == "refusal";
        let usage = usage_of(&result["usage"]);
        if usage.is_none() {
            events.push(EngineEvent::ProtocolNote(
                "agy acp closed the turn without usage".into(),
            ));
        }
        let is_error = cancelled || refused;
        events.push(EngineEvent::Done(EngineOutcome {
            usage: usage.unwrap_or_default(),
            closing_accounting: usage.is_some(),
            stop_reason: Some(stop.clone()),
            is_error,
            error: is_error.then(|| format!("agy acp stopped the turn: {stop}")),
            session: lock(&self.shared).session.clone(),
            text: (!is_error && !self.reply.is_empty()).then(|| self.reply.clone()),
            interrupted: cancelled,
            ..Default::default()
        }));
        self.close();
        events
    }

    /// End the turn with `error`; `before_prompt` says the prompt never went out.
    fn fail(&mut self, error: String, before_prompt: bool) -> Vec<EngineEvent> {
        let mut events = self.flush_text();
        events.push(EngineEvent::Done(EngineOutcome {
            is_error: true,
            error: Some(error),
            session: lock(&self.shared).session.clone(),
            prompt_not_submitted: before_prompt,
            ..Default::default()
        }));
        self.close();
        events
    }

    fn close(&mut self) {
        self.done = true;
        self.out = None;
        lock(&self.shared).open.clear();
    }

    fn flush_text(&mut self) -> Vec<EngineEvent> {
        if self.text.trim().is_empty() {
            self.text.clear();
            return Vec::new();
        }
        let text = std::mem::take(&mut self.text);
        vec![EngineEvent::Assistant {
            content: vec![ContentBlock::Text { text }],
            usage: None,
            model: None,
        }]
    }
}

impl AcpState {
    fn update(&mut self, update: &Value) -> Vec<EngineEvent> {
        if !self.ready {
            return Vec::new();
        }
        match update["sessionUpdate"].as_str().unwrap_or_default() {
            "agent_message_chunk" => self.text_chunk(&update["content"]),
            "agent_thought_chunk" => match update["content"]["text"].as_str() {
                Some(text) => vec![EngineEvent::Delta(StreamEvent::ThinkingDelta {
                    index: 0,
                    text: text.to_owned(),
                })],
                None => Vec::new(),
            },
            "tool_call" | "tool_call_update" => self.tool_update(update),
            "usage_update" => match update["used"].as_u64() {
                Some(tokens) if tokens > 0 => vec![EngineEvent::ContextUsage { tokens }],
                _ => Vec::new(),
            },
            _ => Vec::new(),
        }
    }

    fn text_chunk(&mut self, content: &Value) -> Vec<EngineEvent> {
        let Some(text) = content["text"].as_str().filter(|t| !t.is_empty()) else {
            return Vec::new();
        };
        if !self.text.is_empty() || self.reply.is_empty() {
            self.reply.push_str(text);
        } else {
            self.reply.push_str("\n\n");
            self.reply.push_str(text);
        }
        self.text.push_str(text);
        vec![EngineEvent::Delta(StreamEvent::TextDelta {
            index: 0,
            text: text.to_owned(),
        })]
    }

    fn tool_update(&mut self, update: &Value) -> Vec<EngineEvent> {
        let Some(id) = update["toolCallId"].as_str().map(str::to_owned) else {
            return Vec::new();
        };
        let mut events = Vec::new();
        if self.tools_seen.insert(id.clone()) {
            events.extend(self.flush_text());
            events.push(EngineEvent::Assistant {
                content: vec![ContentBlock::ToolUse {
                    id: id.clone(),
                    name: tool_name(update),
                    input: update["rawInput"].clone(),
                }],
                usage: None,
                model: None,
            });
        }
        let status = update["status"].as_str().unwrap_or_default();
        let finished = status == "completed" || status == "failed";
        if finished && self.tools_done.insert(id.clone()) {
            events.push(EngineEvent::ToolResult {
                tool_use_id: id,
                content: tool_output(update),
                is_error: status == "failed",
            });
        }
        events
    }

    fn server_request(&mut self, method: &str, message: &Value) -> Vec<EngineEvent> {
        let id = message["id"].clone();
        if method != "session/request_permission" {
            // The client declared no fs or terminal capability; refuse the rest.
            let reply = json!({"jsonrpc": "2.0", "id": id, "error": {"code": -32601, "message": format!("{method} is not supported by Aether")}});
            self.send(format!("{reply}\n"));
            return Vec::new();
        }
        let params = &message["params"];
        let tool = &params["toolCall"];
        let option = |kind: &str| {
            params["options"]
                .as_array()
                .into_iter()
                .flatten()
                .find(|o| o["kind"] == kind)
                .and_then(|o| o["optionId"].as_str())
                .map(str::to_owned)
        };
        let key = id.to_string();
        let open = OpenRequest {
            id,
            allow: option("allow_once"),
            reject: option("reject_once"),
        };
        let name = tool_name(tool);
        if self.plan.deny_all {
            let reply = json!({"jsonrpc": "2.0", "id": open.id, "result": {"outcome": match open.reject {
                Some(id) => json!({"outcome": "selected", "optionId": id}),
                None => json!({"outcome": "cancelled"}),
            }}});
            self.send(format!("{reply}\n"));
            return vec![EngineEvent::Note(format!(
                "refused {name} in a read-only summary turn"
            ))];
        }
        lock(&self.shared).open.insert(key.clone(), open);
        vec![EngineEvent::Permission {
            request_id: key,
            side_effect: side_effect(&name, tool["kind"].as_str()),
            tool: name,
            input: tool["rawInput"].clone(),
        }]
    }
}

/// Aether's harness tools arrive as `mcp_aether_<tool>`; the runtime knows
/// them by Claude Code's `mcp__aether__<tool>`.
fn tool_name(tool: &Value) -> String {
    let name = tool["name"]
        .as_str()
        .or_else(|| tool["title"].as_str())
        .unwrap_or("tool");
    match name.strip_prefix("mcp_aether_") {
        Some(bare) => format!("mcp__aether__{bare}"),
        None => name.to_owned(),
    }
}

/// The class ACP's tool `kind` names; harness tools are left to the runtime.
fn side_effect(name: &str, kind: Option<&str>) -> Option<SideEffectClass> {
    if name.starts_with("mcp__aether__") {
        return None;
    }
    Some(match kind? {
        "read" | "search" | "think" => SideEffectClass::ReadOnly,
        "edit" | "move" => SideEffectClass::IdempotentMutation,
        "delete" => SideEffectClass::Mutation,
        "fetch" => SideEffectClass::External,
        _ => SideEffectClass::External,
    })
}

/// A finished tool's text: its content blocks, else its title.
fn tool_output(update: &Value) -> String {
    let texts: Vec<&str> = update["content"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|block| block["content"]["text"].as_str().or(block["text"].as_str()))
        .collect();
    if texts.is_empty() {
        return update["title"].as_str().unwrap_or_default().to_owned();
    }
    texts.join("\n")
}

/// The `session/prompt` result's counters. ACP's `inputTokens` leaves out
/// cache reads, as Aether's `input_tokens` does.
fn usage_of(usage: &Value) -> Option<Usage> {
    let count = |key: &str| usage[key].as_u64();
    let input = count("inputTokens")?;
    let output = count("outputTokens")?;
    let thoughts = count("thoughtTokens").unwrap_or(0);
    Some(Usage {
        input_tokens: input,
        output_tokens: output + thoughts,
        cache_read_tokens: count("cachedReadTokens").unwrap_or(0),
        cache_write_tokens: count("cachedWriteTokens").unwrap_or(0),
        reasoning_tokens: (thoughts > 0).then_some(thoughts),
        ..Usage::default()
    })
}

/// Only the protocol's own code: other load errors (a missing MCP server, say)
/// leave the session intact, so its context must not be dropped.
fn lost(error: &Value) -> bool {
    error["code"].as_i64() == Some(SESSION_NOT_FOUND)
}

fn rpc_error(method: &str, error: &Value) -> String {
    let message = error["message"].as_str().unwrap_or("unknown error");
    format!("agy acp {method} failed: {}", bounded(message))
}

fn bounded(text: &str) -> String {
    const MAX: usize = 500;
    let mut chars = text.chars();
    let head: String = chars.by_ref().take(MAX).collect();
    if chars.next().is_some() {
        format!("{head}…")
    } else {
        head
    }
}
