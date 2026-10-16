//! JSON-RPC transport, startup handshake, and process lifetime.

use std::collections::{HashMap, HashSet};
use std::time::Duration;

use serde_json::{Value, json};
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;

use super::loopback::{self, Environment};
use super::{
    ServerReply, TurnState, command_hidden_env, loopback_note_in_effect, thread_params, turn_params,
};
use crate::detect::EngineVersion;
use crate::{EngineEvent, EngineOutcome, EngineTurn, PermissionDecision, kill_group};

/// How long the server may take to answer `initialize`, start the thread
/// and start the turn before the turn is given up.
const STARTUP_TIMEOUT: Duration = Duration::from_secs(60);
/// After `turn/interrupt`, how long to wait for the server to wind down
/// before the process is killed.
const INTERRUPT_GRACE: Duration = Duration::from_secs(3);
// Nested shell filters and preserved model auth verified against codex-cli 0.162.0.
const COMMAND_FILTER_MIN_VERSION: EngineVersion = EngineVersion::new(0, 162, 0);

/// What one of our JSON-RPC requests was for.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Sent {
    Initialize,
    EnvironmentAdd,
    ConfigRead,
    ThreadStart,
    ThreadResume,
    McpTools,
    TurnStart,
    EnvironmentCheck,
    Steer,
    Interrupt,
}

pub(super) struct Driver {
    stdin: tokio::process::ChildStdin,
    events: mpsc::Sender<EngineEvent>,
    state: TurnState,
    version: Option<EngineVersion>,
    /// The loopback sandbox commands must run in, when the turn is network-denied.
    environment: Option<Environment>,
    /// Whether Codex reported the loopback sandbox as the thread's only environment.
    environment_confirmed: bool,
    /// The resumed thread's latest network note is the offline one.
    loopback_note_in_effect: bool,
    /// Our outstanding requests by id.
    pending: HashMap<i64, Sent>,
    next_id: i64,
    /// The runtime cancelled the run; honoured once, and never by extending
    /// a wind-down already under way.
    cancelled: bool,
    /// The adapter's own grace expired and it ended the run itself.
    forced: bool,
    turn_submitted: bool,
    mcp_poll: bool,
    mcp_cursors: HashSet<String>,
}

#[derive(Debug, PartialEq, Eq)]
pub(super) enum McpInventory {
    Ready(Vec<String>),
    Next(String),
    Starting,
    Unavailable(String),
}

pub(super) fn mcp_inventory(result: &Value) -> McpInventory {
    let Some(servers) = result["data"].as_array() else {
        return McpInventory::Unavailable("invalid MCP status response".into());
    };
    let Some(server) = servers.iter().find(|server| server["name"] == "aether") else {
        return match result["nextCursor"]
            .as_str()
            .filter(|cursor| !cursor.is_empty())
        {
            Some(cursor) => McpInventory::Next(cursor.into()),
            None => McpInventory::Unavailable("Aether MCP server is absent".into()),
        };
    };
    match server["runtimeStatus"].as_str() {
        Some("notStarted" | "starting") => return McpInventory::Starting,
        Some(status) if status != "connected" => {
            return McpInventory::Unavailable(format!("Aether MCP server status: {status}"));
        }
        _ => {}
    }
    match server["tools"]
        .as_object()
        .filter(|tools| !tools.is_empty())
    {
        Some(tools) => McpInventory::Ready(tools.keys().cloned().collect()),
        None => McpInventory::Unavailable("Aether MCP server loaded no tools".into()),
    }
}

fn command_env_config(turn: &EngineTurn, result: &Value) -> Result<Value, &'static str> {
    let invalid = "config/read did not return inspectable shell_environment_policy layers";
    let layers = result["layers"].as_array().ok_or(invalid)?;
    for layer in layers {
        let config = layer["config"].as_object().ok_or(invalid)?;
        if let Some(policy) = config.get("shell_environment_policy") {
            let policy = policy.as_object().ok_or(invalid)?;
            // Mixing legacy arrays with filters makes Codex discard the entire policy.
            if policy.contains_key("exclude") || policy.contains_key("include_only") {
                return Err(
                    "the operator's legacy shell_environment_policy cannot safely be combined with provider filters",
                );
            }
        }
    }
    let filters: serde_json::Map<String, Value> = command_hidden_env(turn)
        .into_iter()
        .map(|name| (name.to_owned(), json!("exclude")))
        .collect();
    Ok(json!({"shell_environment_policy": {"filters": filters}}))
}

impl Driver {
    pub(super) fn new(
        stdin: tokio::process::ChildStdin,
        events: mpsc::Sender<EngineEvent>,
        turn: EngineTurn,
        environment: Option<Environment>,
        version: Option<EngineVersion>,
    ) -> Self {
        Self {
            stdin,
            events,
            state: TurnState::new(turn),
            version,
            environment,
            environment_confirmed: false,
            loopback_note_in_effect: false,
            pending: HashMap::new(),
            next_id: 1,
            cancelled: false,
            forced: false,
            turn_submitted: false,
            mcp_poll: false,
            mcp_cursors: HashSet::new(),
        }
    }

    // The run's four channels, the child's two handles and the sandbox's exit;
    // bundling them would only move the same list one type away.
    #[allow(clippy::too_many_arguments)]
    pub(super) async fn drive(
        &mut self,
        mut child: tokio::process::Child,
        stdout: tokio::process::ChildStdout,
        mut input_rx: mpsc::Receiver<crate::EngineInput>,
        mut decision_rx: mpsc::Receiver<PermissionDecision>,
        mut stop_rx: mpsc::Receiver<Duration>,
        cancel: CancellationToken,
        stderr_task: tokio::task::JoinHandle<crate::output::StderrCapture>,
        sandbox_exit: impl std::future::Future<Output = String>,
    ) {
        let pid = child.id();
        tokio::pin!(sandbox_exit);
        let mut lines = crate::output::BoundedLines::new(stdout);
        if !self
            .request(Sent::Initialize, "initialize", init_params())
            .await
        {
            return;
        }
        let startup = tokio::time::sleep(STARTUP_TIMEOUT);
        tokio::pin!(startup);
        let grace = tokio::time::sleep(INTERRUPT_GRACE);
        tokio::pin!(grace);
        let mut mcp_retry = tokio::time::interval(Duration::from_millis(250));
        mcp_retry.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
        loop {
            tokio::select! {
                line = lines.next_line() => match line {
                    Ok(Some(line)) => {
                        if !self.line(&line).await {
                            break;
                        }
                        if self.state.done {
                            break;
                        }
                    }
                    Ok(None) => {
                        let stderr = stderr_task.await.map(|capture| capture.text)
                            .unwrap_or_else(|error| format!("stderr capture task failed: {error}"));
                        let tail: String = stderr.lines().rev().take(5).collect::<Vec<_>>().into_iter().rev().collect::<Vec<_>>().join("\n");
                        self.finish_error(format!("codex app-server exited{}", if tail.is_empty() { String::new() } else { format!(": {tail}") })).await;
                        break;
                    }
                    Err(e) => {
                        self.finish_error(format!("codex app-server read error: {e}")).await;
                        break;
                    }
                },
                Some(message) = input_rx.recv() => {
                    if self.state.turn_id.is_none() {
                        // Sent before the turn started: steer as soon as it has.
                        self.state.queued_input.push(message);
                    } else if !self.steer(message).await {
                        break;
                    }
                }
                Some(decision) = decision_rx.recv() => {
                    if let Some((id, result)) = self.state.decision_result(
                        &decision.request_id,
                        decision.allow,
                        decision.answer.as_deref(),
                    ) && !self.respond(&id, result).await
                    {
                        break;
                    }
                }
                // The hard stop. It can arrive while the adapter is already
                // winding down from a park, and must shorten that deadline,
                // never postpone it.
                () = cancel.cancelled(), if !self.cancelled => {
                    self.cancelled = true;
                    let at = tokio::time::Instant::now() + INTERRUPT_GRACE;
                    let at = if self.state.cancelling { at.min(grace.as_ref().deadline()) } else { at };
                    grace.as_mut().reset(at);
                    self.interrupt().await;
                }
                // A harness tool ended the turn. Codex ignores a closed
                // stdin, so this is the only stop it hears. It carries the
                // runtime's own grace: capping the wind-down at the adapter's
                // 3s would make `engine_park_grace` inert on Codex.
                Some(runtime_grace) = stop_rx.recv(), if !self.state.cancelling => {
                    grace.as_mut().reset(tokio::time::Instant::now() + runtime_grace);
                    self.interrupt().await;
                }
                () = &mut grace, if self.state.cancelling => {
                    self.forced = true;
                    self.finish_error("cancelled".into()).await;
                    break;
                }
                // Codex reports each later command's transport error to the model, not as a turn failure.
                status = &mut sandbox_exit => {
                    self.finish_error(format!("the loopback-only sandbox exited mid-turn ({status}); its commands could not run")).await;
                    break;
                }
                () = &mut startup, if self.state.turn_id.is_none() => {
                    self.finish_error("codex app-server did not start the turn in time".into()).await;
                    break;
                }
                _ = mcp_retry.tick(), if self.mcp_poll && !self.state.cancelling => {
                    self.mcp_poll = false;
                    self.mcp_cursors.clear();
                    if !self.request(Sent::McpTools, "mcpServerStatus/list",
                        json!({"threadId": self.state.thread_id})).await {
                        break;
                    }
                }
            }
        }
        kill_group(pid).await;
        let _ = child.kill().await;
    }

    /// Ask the app-server to end the current turn. The caller starts the
    /// grace after which the process is killed anyway.
    async fn interrupt(&mut self) {
        self.state.cancelling = true;
        if let (Some(thread), Some(turn)) =
            (self.state.thread_id.clone(), self.state.turn_id.clone())
        {
            let params = json!({ "threadId": thread, "turnId": turn });
            let _ = self
                .request(Sent::Interrupt, "turn/interrupt", params)
                .await;
        }
    }

    /// Handle one line from the server. False when the events channel is gone.
    async fn line(&mut self, line: &str) -> bool {
        let line = line.trim();
        if line.is_empty() {
            return true;
        }
        let v: Value = match serde_json::from_str(line) {
            Ok(v) => v,
            Err(_) => {
                return self
                    .emit(EngineEvent::Note(line.chars().take(300).collect()))
                    .await;
            }
        };
        let id = v.get("id").cloned();
        let method = v.get("method").and_then(Value::as_str).map(str::to_owned);
        match (id, method) {
            (Some(id), Some(method)) => self.server_request(&id, &method, &v["params"]).await,
            (Some(id), None) => self.response(&id, &v).await,
            (None, Some(method)) => {
                for event in self.state.notification(&method, &v["params"]) {
                    if !self.emit(event).await {
                        return false;
                    }
                }
                true
            }
            (None, None) => true,
        }
    }

    async fn response(&mut self, id: &Value, v: &Value) -> bool {
        let Some(sent) = id.as_i64().and_then(|i| self.pending.remove(&i)) else {
            return true;
        };
        if let Some(error) = v.get("error") {
            let message = error["message"]
                .as_str()
                .unwrap_or("unknown error")
                .to_owned();
            return match sent {
                Sent::ConfigRead => {
                    let code = error["code"]
                        .as_i64()
                        .map_or_else(|| "unknown".into(), |code| code.to_string());
                    self.start_without_command_filters(&format!(
                        "config/read failed (RPC error {code})"
                    ))
                    .await
                }
                Sent::ThreadResume => {
                    self.state.done = true;
                    self.emit(EngineEvent::Done(EngineOutcome {
                        is_error: true,
                        error: Some(format!("could not resume thread: {message}")),
                        session_lost: true,
                        prompt_not_submitted: !self.turn_submitted,
                        ..Default::default()
                    }))
                    .await
                }
                Sent::Initialize
                | Sent::EnvironmentAdd
                | Sent::ThreadStart
                | Sent::McpTools
                | Sent::TurnStart
                | Sent::EnvironmentCheck => {
                    self.finish_error(format!("codex app-server refused {sent:?}: {message}"))
                        .await;
                    false
                }
                Sent::Steer => {
                    self.emit(EngineEvent::InputRejected { reason: message })
                        .await
                }
                Sent::Interrupt => true,
            };
        }
        let result = &v["result"];
        match sent {
            Sent::Initialize => match self.environment.as_ref().map(Environment::add_params) {
                Some(params) => {
                    self.request(Sent::EnvironmentAdd, "environment/add", params)
                        .await
                }
                None => self.prepare_thread().await,
            },
            Sent::EnvironmentAdd => self.prepare_thread().await,
            Sent::ConfigRead => match command_env_config(&self.state.turn, result) {
                Ok(config) => self.start_thread(Some(config)).await,
                Err(reason) => self.start_without_command_filters(reason).await,
            },
            Sent::ThreadStart | Sent::ThreadResume => {
                self.environment_confirmed = loopback::confirms(result);
                self.loopback_note_in_effect =
                    sent == Sent::ThreadResume && loopback_note_in_effect(&result["thread"]);
                let thread_id = result["thread"]["id"]
                    .as_str()
                    .unwrap_or_default()
                    .to_owned();
                if sent == Sent::ThreadStart {
                    let paths = result["thread"]["path"]
                        .as_str()
                        .filter(|path| !path.is_empty())
                        .map(std::path::PathBuf::from)
                        .filter(|path| {
                            path.is_absolute()
                                && !path
                                    .components()
                                    .any(|part| matches!(part, std::path::Component::ParentDir))
                        })
                        .into_iter()
                        .collect();
                    if let Err(error) = self.state.turn.record_new_session(
                        crate::EngineKind::CODEX,
                        &thread_id,
                        paths,
                    ) {
                        self.finish_error(error.to_string()).await;
                        return false;
                    }
                }
                // A fresh thread reports its sticky sandbox now; a resumed one only after turn/start.
                if sent == Sent::ThreadStart
                    && self.environment.is_some()
                    && !self.environment_confirmed
                {
                    self.finish_error(
                        "codex app-server did not start the thread in the loopback-only sandbox; stopped before the turn"
                            .into(),
                    )
                    .await;
                    return false;
                }
                self.state.thread_id = Some(thread_id.clone());
                let model = result["model"].as_str().map(str::to_owned);
                if !self
                    .emit(EngineEvent::Session {
                        id: thread_id.clone(),
                        model,
                        tools: Vec::new(),
                    })
                    .await
                {
                    return false;
                }
                if self.state.turn.mcp.is_some() {
                    self.request(
                        Sent::McpTools,
                        "mcpServerStatus/list",
                        json!({"threadId": thread_id}),
                    )
                    .await
                } else {
                    let params =
                        turn_params(&self.state.turn, &thread_id, self.loopback_note_in_effect);
                    self.request(Sent::TurnStart, "turn/start", params).await
                }
            }
            Sent::McpTools => {
                let tools = match mcp_inventory(result) {
                    McpInventory::Ready(tools) => tools,
                    McpInventory::Starting => {
                        self.mcp_poll = true;
                        return true;
                    }
                    McpInventory::Next(cursor) => {
                        if !self.mcp_cursors.insert(cursor.clone()) {
                            self.finish_error("Codex repeated an MCP pagination cursor".into())
                                .await;
                            return false;
                        }
                        return self
                            .request(
                                Sent::McpTools,
                                "mcpServerStatus/list",
                                json!({"threadId": self.state.thread_id, "cursor": cursor}),
                            )
                            .await;
                    }
                    McpInventory::Unavailable(reason) => {
                        self.finish_error(format!("{reason}; refusing to start without Aether coordination. Check the Aether bridge connection and retry.")).await;
                        return false;
                    }
                };
                if !self
                    .emit(EngineEvent::Note(format!(
                        "Aether MCP tools ready: {}",
                        tools.join(", ")
                    )))
                    .await
                {
                    return false;
                }
                let thread_id = self.state.thread_id.clone().unwrap_or_default();
                let params =
                    turn_params(&self.state.turn, &thread_id, self.loopback_note_in_effect);
                self.request(Sent::TurnStart, "turn/start", params).await
            }
            Sent::TurnStart => {
                self.state.turn_id = result["turn"]["id"].as_str().map(str::to_owned);
                // Codex ignores unknown turn fields, so confirm the sandbox took effect.
                if self.environment.is_some()
                    && !self.environment_confirmed
                    && !self
                        .request(
                            Sent::EnvironmentCheck,
                            "thread/read",
                            json!({"threadId": self.state.thread_id}),
                        )
                        .await
                {
                    return false;
                }
                for message in std::mem::take(&mut self.state.queued_input) {
                    if !self.steer(message).await {
                        return false;
                    }
                }
                true
            }
            Sent::EnvironmentCheck => {
                if loopback::confirms(result) {
                    self.environment_confirmed = true;
                    return true;
                }
                self.finish_error(
                    "codex app-server did not run the turn in the loopback-only sandbox; stopped it"
                        .into(),
                )
                .await;
                false
            }
            Sent::Steer => self.emit(EngineEvent::InputAccepted).await,
            Sent::Interrupt => true,
        }
    }

    async fn prepare_thread(&mut self) -> bool {
        let dropped: Vec<_> = self
            .state
            .turn
            .tool_hidden_env
            .iter()
            .filter(|name| {
                !self.state.turn.hidden_env.contains(name) && !super::valid_env_filter_name(name)
            })
            .cloned()
            .collect();
        if !dropped.is_empty()
            && !self
                .emit(EngineEvent::ProtocolNote(format!(
                    "Codex command environment filters cannot represent these variable names: {}",
                    dropped
                        .iter()
                        .map(|name| format!("{name:?}"))
                        .collect::<Vec<_>>()
                        .join(", ")
                )))
                .await
        {
            return false;
        }
        if command_hidden_env(&self.state.turn).is_empty() {
            return self.start_thread(None).await;
        }
        if !self
            .version
            .is_some_and(|version| version >= COMMAND_FILTER_MIN_VERSION)
        {
            return self
                .start_without_command_filters(
                    "the Codex version is unknown or older than the verified minimum 0.162.0",
                )
                .await;
        }
        self.request(
            Sent::ConfigRead,
            "config/read",
            json!({"includeLayers": true, "cwd": self.state.turn.cwd.to_string_lossy()}),
        )
        .await
    }

    async fn start_without_command_filters(&mut self, reason: &str) -> bool {
        // Note is debug-only; skipped safeguards must reach the event ledger.
        if !self
            .emit(EngineEvent::ProtocolNote(format!(
                "Provider credentials may remain visible to Codex commands: {reason}; provider environment filters were not applied."
            )))
            .await
        {
            return false;
        }
        self.start_thread(None).await
    }

    async fn start_thread(&mut self, command_config: Option<Value>) -> bool {
        let (method, mut params) = thread_params(&self.state.turn);
        if let Some(config) = command_config {
            params["config"] = config;
        }
        let kind = if method == "thread/resume" {
            Sent::ThreadResume
        } else {
            Sent::ThreadStart
        };
        self.request(kind, method, params).await
    }

    /// Hand a user message to the running turn; the response acknowledges it.
    async fn steer(&mut self, message: crate::EngineInput) -> bool {
        let (Some(thread), Some(turn)) = (self.state.thread_id.clone(), self.state.turn_id.clone())
        else {
            return true;
        };
        let params = super::steer_params(&thread, &turn, &message);
        self.request(Sent::Steer, "turn/steer", params).await
    }

    async fn server_request(&mut self, id: &Value, method: &str, params: &Value) -> bool {
        match self.state.server_request(id, method, params) {
            ServerReply::Ask(event) => self.emit(event).await,
            ServerReply::Now(result, note) => {
                if let Some(note) = note
                    && !self.emit(EngineEvent::Note(note)).await
                {
                    return false;
                }
                self.respond(id, result).await
            }
            ServerReply::Unsupported => {
                let msg = json!({
                    "jsonrpc": "2.0",
                    "id": id,
                    "error": { "code": -32601, "message": format!("{method} is not supported by aether") },
                });
                self.write(&msg).await
            }
        }
    }

    async fn request(&mut self, sent: Sent, method: &str, params: Value) -> bool {
        let id = self.next_id;
        self.next_id += 1;
        self.pending.insert(id, sent);
        // A partial write can dispatch the prompt even without an acknowledgement.
        self.turn_submitted |= sent == Sent::TurnStart;
        let msg = json!({ "jsonrpc": "2.0", "id": id, "method": method, "params": params });
        self.write(&msg).await
    }

    async fn respond(&mut self, id: &Value, result: Value) -> bool {
        let msg = json!({ "jsonrpc": "2.0", "id": id, "result": result });
        self.write(&msg).await
    }

    async fn write(&mut self, msg: &Value) -> bool {
        use tokio::io::AsyncWriteExt as _;
        let mut line = msg.to_string();
        line.push('\n');
        if self.stdin.write_all(line.as_bytes()).await.is_err() {
            self.finish_error("codex app-server closed its input".into())
                .await;
            return false;
        }
        self.stdin.flush().await.is_ok()
    }

    async fn emit(&mut self, event: EngineEvent) -> bool {
        self.events.send(event).await.is_ok()
    }

    async fn finish_error(&mut self, error: String) {
        if self.state.done {
            return;
        }
        self.state.done = true;
        let _ = self
            .events
            .send(EngineEvent::Done(EngineOutcome {
                usage: self.state.usage_total,
                is_error: true,
                error: Some(error),
                session: self.state.thread_id.clone(),
                text: self.state.final_text.clone(),
                forced: self.forced,
                prompt_not_submitted: !self.turn_submitted,
                ..Default::default()
            }))
            .await;
    }
}
pub(super) fn init_params() -> Value {
    json!({
        "clientInfo": {
            "name": "aether",
            "title": "Aether",
            "version": env!("CARGO_PKG_VERSION"),
        },
        "capabilities": { "experimentalApi": true },
    })
}

#[cfg(test)]
#[path = "tests/startup.rs"]
mod tests;
