//! External agent engines: official CLIs that run inference under the user's
//! own subscription login (Claude Code, Codex).
//!
//! An engine owns the model loop and its built-in tools. Aether stays the
//! authority on tasks, delegation, waits, events, approvals and cost: it
//! hands the engine one prompt per turn, exposes its harness tools to the
//! engine over MCP through [`bridge`], records everything the engine streams
//! back, and resumes the engine's session on the next turn.

pub mod bridge;
pub mod claude_code;
pub mod codex;
pub mod codex_app;
pub mod contract;
pub mod detect;
mod offline;
mod output;
mod process_interrupt;
mod usage;

use std::path::PathBuf;

use aether_core::model::{ContentBlock, Effort, StreamEvent, Usage};
use aether_core::tool::SideEffectClass;
use nix::errno::Errno;
use nix::sys::signal::{Signal, killpg};
use nix::unistd::Pid;
use serde_json::Value;
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;

pub use claude_code::ClaudeCodeEngine;
pub use codex::CodexEngine;
pub use codex_app::CodexAppEngine;
pub use detect::{DetectError, EngineVersion};
pub use usage::{RequestUsage, UsageRequestId};

/// The `model_reasoning_effort` value a Codex launch (`codex_app.rs`,
/// `codex.rs`) passes for one of Codex's reasoning-effort levels - today a
/// direct 1:1 mapping onto Aether's own five, kept as its own function
/// so a future Codex level that stops matching Aether's own name has one
/// place to translate instead of every call site. Codex does not validate
/// this at launch: a level a given model does not support is accepted and
/// echoed back, not refused, so per-model support is a fact about the
/// model, never this function's concern.
pub fn codex_effort_arg(effort: Effort) -> &'static str {
    effort.as_str()
}

/// Which engine a turn runs on, as an opaque name.
///
/// Open on purpose: a third-party engine (see `docs/engines.md`) names itself
/// from its own crate without editing this one. Closed variants would make
/// [`Engine`] an extension point only for code that lives here.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct EngineKind(&'static str);

impl EngineKind {
    pub const CLAUDE_CODE: Self = Self("claude-code");
    pub const CODEX: Self = Self("codex");

    /// Name an engine. Static because the name identifies the engine for the
    /// life of the process: it keys the runtime's engine table and lands in
    /// snapshots and events.
    pub const fn new(name: &'static str) -> Self {
        Self(name)
    }

    pub fn as_str(self) -> &'static str {
        self.0
    }
}

impl std::fmt::Display for EngineKind {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.0)
    }
}

#[derive(Debug, thiserror::Error)]
pub enum EngineError {
    #[error("cannot record engine session ownership: {0}")]
    SessionOwnership(String),
    #[error("unsupported engine configuration: {0}")]
    Unsupported(String),
    #[error("{program} is not installed or not on PATH")]
    NotInstalled { program: String },
    #[error("could not start {program}: {source}")]
    Spawn {
        program: String,
        source: std::io::Error,
    },
    #[error("engine exited before finishing: {0}")]
    Exited(String),
}

impl EngineError {
    /// These failures occur before any model request can be submitted.
    pub fn proves_no_submission(&self) -> bool {
        match self {
            Self::Unsupported(_) | Self::NotInstalled { .. } | Self::Spawn { .. } => true,
            Self::Exited(_) | Self::SessionOwnership(_) => false,
        }
    }
}

/// How the engine reaches Aether's harness tools.
#[derive(Debug, Clone)]
pub struct McpBridge {
    /// Program to run (normally the `aether` binary itself).
    pub command: PathBuf,
    pub args: Vec<String>,
}

/// Persists evidence only for sessions explicitly created by this launch.
pub trait SessionRecorder: Send + Sync + std::fmt::Debug {
    fn record(&self, engine: EngineKind, id: &str, log_paths: Vec<PathBuf>) -> Result<(), String>;
}

/// One turn handed to an engine.
#[derive(Debug, Clone)]
pub struct EngineTurn {
    /// The user message for this turn.
    pub prompt: String,
    /// Images sent with that message. `prompt` names each stored file too,
    /// so an engine that cannot take an image leaves the agent a path.
    pub images: Vec<EngineImage>,
    /// Explicit additional instructions. Ordinary runtime turns carry Aether
    /// state in `prompt` and leave this empty to preserve native instructions.
    /// Isolated summary turns may use this as a replacement prompt.
    pub system_prompt: String,
    /// Resume this engine session when set.
    pub session: Option<String>,
    /// What the resumed session has already counted for itself, as the engine
    /// last reported it. Zero for a fresh session, which has spent nothing.
    /// An engine reporting session-cumulative totals (Codex counts a thread)
    /// subtracts this so a resumed session is not billed twice.
    pub session_usage: Usage,
    /// Session id to use when starting fresh.
    pub new_session_id: String,
    /// Durable creation evidence; resumed sessions never invoke this recorder.
    pub session_recorder: Option<std::sync::Arc<dyn SessionRecorder>>,
    /// Engine model alias; `None` or `default` leaves it to the engine.
    pub model: Option<String>,
    pub effort: Option<Effort>,
    pub cwd: PathBuf,
    pub extra_dirs: Vec<PathBuf>,
    /// Isolated checkpoint generation: restrict tools and enforce read-only execution.
    pub summary_only: bool,
    /// Leave out the engine's own skills and non-Aether MCP servers
    /// (`HostContext::Lean`). Engines without such a switch ignore it.
    pub lean_host_context: bool,
    /// Whether this agent scope permits native web and sandbox network access.
    pub network_access: bool,
    /// Let the engine run its tools without asking anyone.
    pub bypass_permissions: bool,
    /// Variables hidden from the engine process, leaving its provider auth available.
    pub hidden_env: Vec<String>,
    /// Variables hidden from separately spawned command runners, including provider auth.
    pub tool_hidden_env: Vec<String>,
    pub mcp: Option<McpBridge>,
}

impl EngineTurn {
    pub(crate) fn record_new_session(
        &self,
        engine: EngineKind,
        id: &str,
        log_paths: Vec<PathBuf>,
    ) -> Result<(), EngineError> {
        // Resuming an old or user-supplied id never establishes ownership.
        if self.session.is_some() {
            return Ok(());
        }
        if id.trim().is_empty() {
            return Err(EngineError::SessionOwnership(
                "engine returned an empty session id".into(),
            ));
        }
        if let Some(recorder) = &self.session_recorder {
            recorder
                .record(engine, id, log_paths)
                .map_err(EngineError::SessionOwnership)?;
        }
        Ok(())
    }
}

/// One image a person sent, as the daemon stores it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EngineImage {
    pub path: PathBuf,
    /// `image/png`, `image/jpeg`, `image/gif` or `image/webp`.
    pub media_type: String,
}

/// A user message handed to a running engine: its text and its images.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct EngineInput {
    pub text: String,
    pub images: Vec<EngineImage>,
}

impl From<String> for EngineInput {
    fn from(text: String) -> Self {
        Self {
            text,
            images: Vec::new(),
        }
    }
}

impl From<&str> for EngineInput {
    fn from(text: &str) -> Self {
        Self::from(text.to_owned())
    }
}

/// What an engine reports while it runs, already normalized.
#[derive(Debug, Clone, PartialEq)]
pub enum EngineEvent {
    /// Latest model context size, independent of cumulative billing usage.
    ContextUsage { tokens: u64 },
    /// Newly consumed tokens, independent of content blocks and current context.
    /// `cost_complete: false` says a counter behind them could not be read;
    /// the runtime counts such tokens but leaves them unpriced.
    UsageDelta { usage: Usage },
    /// Per-request high-water marks, retained so pricing can select the prompt tier.
    UsageRequest { request: RequestUsage },
    /// The engine session's own cumulative counters, as last reported.
    /// Persisted against the session so the next turn on it bills only what
    /// it adds; engines that report per-turn usage never emit this.
    SessionUsage { session: String, total: Usage },
    /// The engine's session id and what it loaded.
    Session {
        id: String,
        model: Option<String>,
        tools: Vec<String>,
    },
    /// Live streaming delta, same shape as the model adapters produce.
    Delta(StreamEvent),
    /// One complete assistant message (one model call inside the engine).
    Assistant {
        content: Vec<ContentBlock>,
        usage: Option<Usage>,
        model: Option<String>,
    },
    /// The result of a tool the engine ran itself.
    ToolResult {
        tool_use_id: String,
        content: String,
        is_error: bool,
    },
    /// Subscription or rate-limit information, kept verbatim for the UI.
    RateLimit { payload: Value },
    /// The engine moved a tool to the background (`running`) or that task
    /// finished. A turn is not over while such tasks are still running.
    BackgroundTask { id: String, running: bool },
    /// The engine asks before acting; the runtime answers through
    /// [`EngineRun::decisions`] with the same `request_id`.
    Permission {
        request_id: String,
        tool: String,
        input: Value,
        /// Class the engine already knows, else the runtime infers it from `tool`.
        side_effect: Option<SideEffectClass>,
    },
    /// The engine asks the person something and waits; the runtime answers
    /// through [`EngineRun::decisions`] with the person's next message.
    Question { request_id: String, prompt: String },
    /// The oldest message handed over through [`EngineRun::input`] is now
    /// part of the engine's conversation. Acknowledgements arrive in order.
    InputAccepted,
    /// The oldest message handed over could not be delivered; the runtime
    /// keeps it for the next turn.
    InputRejected { reason: String },
    /// Something worth logging that is not a turn event.
    Note(String),
    /// The engine departed from the protocol billing depends on - a counter
    /// it restarted, a field it stopped sending, a turn it never accounted.
    /// Unlike [`EngineEvent::Note`] this reaches the ledger: an invoice a
    /// person cannot audit is what this exists to prevent.
    ProtocolNote(String),
    /// The turn is over. Always the last event.
    Done(EngineOutcome),
}

#[derive(Debug, Clone, PartialEq, Default)]
pub struct EngineOutcome {
    pub usage: Usage,
    /// Parsed from the engine's own final usage message, never synthesized.
    pub closing_accounting: bool,
    pub cost_usd: Option<f64>,
    pub stop_reason: Option<String>,
    pub duration_ms: Option<u64>,
    pub is_error: bool,
    pub error: Option<String>,
    pub session: Option<String>,
    /// Final assistant text as the engine reported it.
    pub text: Option<String>,
    /// The engine could not resume the requested session.
    pub session_lost: bool,
    /// The adapter proves the turn prompt was never submitted to the engine.
    /// A missing start acknowledgement alone does not establish this.
    pub prompt_not_submitted: bool,
    /// The adapter stopped the turn to enforce a harness boundary. Preserve
    /// the agent/task for resumption; `error` explains the interruption.
    pub interrupted: bool,
    /// The adapter itself ended the run - its own grace expired and it killed
    /// the process. The engine did not stop of its own accord, so a wind-down
    /// must not record it as one.
    pub forced: bool,
}

/// The runtime's answer to an [`EngineEvent::Permission`] or
/// [`EngineEvent::Question`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PermissionDecision {
    pub request_id: String,
    pub allow: bool,
    /// Why a permission was refused.
    pub message: Option<String>,
    /// The person's reply to a question.
    pub answer: Option<String>,
}

/// A running turn: events arrive on the channel; the last one is `Done`.
#[derive(Debug)]
pub struct EngineRun {
    /// The sender must remain alive until native process cleanup completes.
    /// Stream closure fences retries after the final outcome.
    pub events: mpsc::Receiver<EngineEvent>,
    /// User messages to hand the engine while it runs, for engines that
    /// accept further input; each is answered by [`EngineEvent::InputAccepted`]
    /// or [`EngineEvent::InputRejected`]. Dropping it closes the channel.
    pub input: Option<mpsc::Sender<EngineInput>>,
    /// Answers to the engine's permission prompts, for engines that ask
    /// through their own protocol rather than the MCP bridge.
    pub decisions: Option<mpsc::Sender<PermissionDecision>>,
    /// Asks the engine to end the turn now, for engines a closed stdin does
    /// not stop (Codex: `turn/interrupt`). `None` means closing `input` is
    /// the whole stop verb - Claude Code's CLI exits on it. The `Duration` is
    /// how long the runtime will wait before it stops the process itself: an
    /// adapter with its own wind-down grace must not outlast it.
    pub stop: Option<mpsc::Sender<std::time::Duration>>,
}

#[async_trait::async_trait]
pub trait Engine: Send + Sync {
    fn kind(&self) -> EngineKind;
    /// Adapter label for snapshots, e.g. `claude-code/2.1.263`.
    fn adapter(&self) -> String;
    /// Check known local prerequisites before creating a delegated agent.
    /// Engines still enforce the requested policy when a turn starts.
    fn validate_network_policy(&self, _network_access: bool) -> Result<(), EngineError> {
        Ok(())
    }
    async fn run(
        &self,
        turn: EngineTurn,
        cancel: CancellationToken,
    ) -> Result<EngineRun, EngineError>;
    /// Whether the engine asks before acting, so an approval policy can be
    /// honoured. False means its agents must run without one.
    fn gates_permissions(&self) -> bool {
        true
    }
}

/// Take the daemon's secrets and its own reach out of a child's environment.
pub(crate) fn strip_hidden(command: &mut tokio::process::Command, hidden_env: &[String]) {
    for name in hidden_env {
        command.env_remove(name);
    }
}

/// A spawned CLI: its normalized events and, when kept open, its raw stdin.
#[derive(Debug)]
pub(crate) struct ProcessRun {
    pub events: mpsc::Receiver<EngineEvent>,
    /// Raw stdin lines; dropping it closes the CLI's stdin.
    pub stdin: Option<mpsc::Sender<String>>,
}

impl ProcessRun {
    /// The run of a CLI that takes nothing more once it has its first input.
    pub(crate) fn into_run(self) -> EngineRun {
        EngineRun {
            events: self.events,
            input: None,
            decisions: None,
            stop: None,
        }
    }
}

/// Spawn a CLI, feed `first_input` on stdin, and stream stdout lines through
/// a parser. With `keep_stdin_open`, further raw stdin lines can be sent
/// through the returned `stdin` until it is dropped. Shared by both engines.
#[allow(clippy::too_many_arguments)]
pub(crate) async fn run_process<P>(
    program: &str,
    args: Vec<String>,
    cwd: PathBuf,
    hidden_env: &[String],
    first_input: String,
    keep_stdin_open: bool,
    cancel_input: Option<String>,
    cancel: CancellationToken,
    mut parser: P,
) -> Result<ProcessRun, EngineError>
where
    P: FnMut(&str) -> Vec<EngineEvent> + Send + 'static,
{
    use std::os::unix::process::CommandExt as _;
    use tokio::io::AsyncWriteExt as _;

    let mut command = tokio::process::Command::new(program);
    command
        .args(&args)
        .current_dir(&cwd)
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .kill_on_drop(true);
    // Own process group so cancellation can take the CLI's children with it.
    command.as_std_mut().process_group(0);
    strip_hidden(&mut command, hidden_env);
    let mut child = command.spawn().map_err(|e| match e.kind() {
        std::io::ErrorKind::NotFound => EngineError::NotInstalled {
            program: program.to_owned(),
        },
        _ => EngineError::Spawn {
            program: program.to_owned(),
            source: e,
        },
    })?;
    let program = program.to_owned();
    let pid = child.id();
    let mut stdin = child
        .stdin
        .take()
        .ok_or_else(|| EngineError::Exited("no stdin".into()))?;
    let stdout = child
        .stdout
        .take()
        .ok_or_else(|| EngineError::Exited("no stdout".into()))?;
    let stderr = child
        .stderr
        .take()
        .ok_or_else(|| EngineError::Exited("no stderr".into()))?;
    let (tx, rx) = mpsc::channel(256);
    let (input_tx, mut input_rx) = mpsc::channel::<String>(16);
    // Do not hold stdin open after the runtime drops its final input sender.
    let interrupt_input = input_tx.downgrade();

    tokio::spawn(async move {
        let _ = stdin.write_all(first_input.as_bytes()).await;
        let _ = stdin.flush().await;
        // Ends as soon as every sender is gone; immediately when none is kept.
        while let Some(line) = input_rx.recv().await {
            if stdin.write_all(line.as_bytes()).await.is_err() {
                break;
            }
            let _ = stdin.flush().await;
        }
        drop(stdin);
    });
    let stderr_stop = CancellationToken::new();
    let mut stderr_task = tokio::spawn(output::drain_stderr_until(stderr, stderr_stop.clone()));
    tokio::spawn(async move {
        let mut lines = output::BoundedLines::new(stdout);
        let mut response_done = false;
        let mut stdout_error = None;
        loop {
            let line = tokio::select! {
                l = lines.next_line() => match l {
                    Ok(Some(l)) => l,
                    Ok(None) => break,
                    Err(e) => {
                        stdout_error = Some(format!("stdout read error: {e}"));
                        kill_group(pid).await;
                        let _ = child.kill().await;
                        break;
                    }
                },
                _ = cancel.cancelled() => {
                    let receipt = if !response_done {
                        let mut receipt = match cancel_input {
                            Some(request) => process_interrupt::drain(
                                &interrupt_input, request, &mut lines, &mut parser, &tx,
                            ).await,
                            None => None,
                        }.unwrap_or_default();
                        receipt.is_error = true;
                        receipt.error.get_or_insert_with(|| "cancelled".into());
                        Some(receipt)
                    } else {
                        None
                    };
                    // Publish first when capacity permits; a blocked terminal
                    // send must not postpone process-group cleanup.
                    tokio::join!(
                        async {
                            if let Some(receipt) = receipt {
                                let _ = tx.send(EngineEvent::Done(receipt)).await;
                            }
                        },
                        async {
                            kill_group(pid).await;
                            let _ = child.kill().await;
                        },
                    );
                    stderr_task.abort();
                    return;
                }
            };
            if line.trim().is_empty() {
                continue;
            }
            for mut event in parser(&line) {
                let Ok(permit) = tx.reserve().await else {
                    kill_group(pid).await;
                    let _ = child.kill().await;
                    stderr_task.abort();
                    return;
                };
                match &mut event {
                    EngineEvent::Done(outcome) => {
                        // Check after reserving capacity: a blocked send must
                        // not turn an already-cancelled run into success.
                        if cancel.is_cancelled() {
                            outcome.is_error = true;
                            outcome.error.get_or_insert_with(|| "cancelled".into());
                        }
                        response_done = true;
                    }
                    // Accepted input or resumed activity starts another response.
                    EngineEvent::InputAccepted
                    | EngineEvent::UsageDelta { .. }
                    | EngineEvent::UsageRequest { .. }
                    | EngineEvent::Delta(_)
                    | EngineEvent::Assistant { .. }
                    | EngineEvent::ToolResult { .. }
                    | EngineEvent::BackgroundTask { .. }
                    | EngineEvent::Permission { .. }
                    | EngineEvent::Question { .. } => response_done = false,
                    _ => {}
                }
                permit.send(event);
            }
        }
        let status = child.wait().await.ok();
        let stderr_result = if stdout_error.is_some() {
            match tokio::time::timeout(std::time::Duration::from_secs(1), &mut stderr_task).await {
                Ok(result) => result,
                Err(_) => {
                    stderr_stop.cancel();
                    stderr_task.await
                }
            }
        } else {
            stderr_task.await
        };
        let stderr = stderr_result.unwrap_or_else(|error| output::StderrCapture {
            text: format!("stderr capture task failed: {error}"),
            session_lost: false,
        });
        let stderr_text = stderr.text;
        if !response_done {
            let tail: String = stderr_text
                .lines()
                .rev()
                .take(6)
                .collect::<Vec<_>>()
                .into_iter()
                .rev()
                .collect::<Vec<_>>()
                .join("\n");
            let status_text = status
                .map(|s| s.to_string())
                .unwrap_or_else(|| "unknown".into());
            let session_lost = stderr.session_lost;
            let _ = tx
                .send(EngineEvent::Done(EngineOutcome {
                    is_error: true,
                    error: Some(format!(
                        "{program} exited ({status_text}) without a result{}: {tail}",
                        stdout_error
                            .as_ref()
                            .map(|error| format!("; {error}"))
                            .unwrap_or_default()
                    )),
                    session_lost,
                    ..Default::default()
                }))
                .await;
        } else if let Some(error) = stdout_error {
            let _ = tx
                .send(EngineEvent::Note(format!("{error}: {stderr_text}")))
                .await;
        } else if !stderr_text.trim().is_empty() {
            let _ = tx
                .send(EngineEvent::Note(stderr_text.chars().take(2_000).collect()))
                .await;
        }
    });
    Ok(ProcessRun {
        events: rx,
        stdin: keep_stdin_open.then_some(input_tx),
    })
}

/// How long a group member gets to honour TERM before it is killed.
const GROUP_TERM_GRACE: std::time::Duration = std::time::Duration::from_secs(2);

/// Stop the CLI's whole process group: TERM now, KILL to whatever is left
/// after the grace. Without the escalation a member that traps or ignores
/// TERM (a test runner, a container shim) outlives the run.
///
/// Awaited so the caller does not reap the leader before the KILL lands: an
/// unreaped leader holds its pid, and with it the group id, so the KILL
/// cannot land on a recycled group. It buys nothing against a daemon
/// shutdown inside the grace - this already runs in a detached task.
///
/// It reaches only the group the CLI was spawned into: a grandchild that
/// `setsid`s into its own group survives both signals.
pub(crate) async fn kill_group(pid: Option<u32>) {
    let Some(pid) = pid else {
        return;
    };
    // 0 would target the daemon's own group and 1 everything it may signal.
    assert!(
        (2..=i32::MAX as u32).contains(&pid),
        "not an engine child pid: {pid}"
    );
    if !signal_group(pid, Signal::SIGTERM) {
        return;
    }
    tokio::time::sleep(GROUP_TERM_GRACE).await;
    signal_group(pid, Signal::SIGKILL);
}

/// Send one signal to the whole group; false when the group is already gone.
/// Any other failure means a process may have been leaked, so it is logged
/// with its errno rather than discarded.
fn signal_group(pid: u32, signal: Signal) -> bool {
    // Negative pid targets the process group created with process_group(0).
    match killpg(Pid::from_raw(pid as i32), signal) {
        Ok(()) => true,
        Err(Errno::ESRCH) => false,
        Err(errno) => {
            tracing::warn!(pid, ?signal, %errno, "could not signal the engine process group");
            false
        }
    }
}

/// A fresh session id in the UUID form both CLIs accept.
pub fn new_session_id() -> String {
    let u = ulid::Ulid::generate().to_bytes();
    format!(
        "{:02x}{:02x}{:02x}{:02x}-{:02x}{:02x}-4{:01x}{:02x}-8{:01x}{:02x}-{:02x}{:02x}{:02x}{:02x}{:02x}{:02x}",
        u[0],
        u[1],
        u[2],
        u[3],
        u[4],
        u[5],
        u[6] & 0x0f,
        u[7],
        u[8] & 0x0f,
        u[9],
        u[10],
        u[11],
        u[12],
        u[13],
        u[14],
        u[15]
    )
}

/// Best-effort side-effect class for an engine's own tool names, used when
/// recording tool calls the engine ran itself.
pub fn side_effect_of(tool_name: &str) -> aether_core::tool::SideEffectClass {
    use aether_core::tool::SideEffectClass::*;
    let lower = tool_name.to_ascii_lowercase();
    if lower.starts_with("mcp__aether__") || lower.starts_with("aether__") {
        return ReadOnly;
    }
    match lower.as_str() {
        "read" | "grep" | "glob" | "ls" | "list_dir" | "read_file" | "toolsearch" | "websearch"
        | "webfetch" | "todowrite" => ReadOnly,
        "edit" | "write" | "multiedit" | "notebookedit" | "write_file" | "edit_file"
        | "apply_patch" | "file_change" => IdempotentMutation,
        _ => Mutation,
    }
}

#[cfg(test)]
mod test_script;
#[cfg(test)]
mod tests;

#[cfg(test)]
mod ownership_tests;
