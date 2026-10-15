//! Codex CLI as an engine: `codex exec --json`.
//!
//! Event shapes follow the Codex CLI's JSONL protocol (`thread.started`,
//! `turn.started`, `item.*`, `turn.completed`, `turn.failed`, `error`).
//!
//! `aether-runtime`'s `detect_engines` (`crates/aether-runtime/src/lib.rs`)
//! only reaches for this engine when `CodexAppEngine::detect_verbose()`
//! fails. Current CLIs normally speak app-server; exec remains the fallback
//! when that capability is unavailable.

use std::collections::HashSet;
use std::path::PathBuf;
use std::sync::Arc;

use aether_core::model::{ContentBlock, StreamEvent, Usage};
use serde_json::{Value, json};
use tokio::sync::{Mutex, mpsc};
use tokio_util::sync::CancellationToken;

use crate::codex_app::{RawTotals, advanced, consumed_since};
use crate::detect::{self, DetectError, EngineVersion};
use crate::{
    Engine, EngineError, EngineEvent, EngineKind, EngineOutcome, EngineRun, EngineTurn, run_process,
};

#[derive(Debug, Clone)]
pub struct CodexEngine {
    program: String,
    version: String,
    provider_notices: Arc<Mutex<HashSet<String>>>,
}

impl CodexEngine {
    pub fn new(program: impl Into<String>, version: impl Into<String>) -> Self {
        Self {
            program: program.into(),
            version: version.into(),
            provider_notices: Arc::new(Mutex::new(HashSet::new())),
        }
    }

    /// Find `codex` on PATH and read its version. `Err` names why the CLI is
    /// unusable; see [`DetectError`]. No numeric floor is enforced here: `exec`'s
    /// argv (`-c` overlays, `--skip-git-repo-check`, `--dangerously-bypass-approvals-and-sandbox`,
    /// `--add-dir`, `resume`) has no known minimum, same reasoning as
    /// `codex_app`'s support-matrix note.
    pub async fn detect_verbose() -> Result<Self, DetectError> {
        Self::detect_verbose_on(None).await
    }

    /// `detect_verbose`, optionally searching `path` instead of the process's
    /// own `PATH` - the seam a test drives with a fake `codex` on a temp dir.
    async fn detect_verbose_on(path: Option<&std::path::Path>) -> Result<Self, DetectError> {
        let version = detect::probe_version("codex", path).await?;
        Ok(Self::new("codex", version.to_string()))
    }

    pub fn args(&self, turn: &EngineTurn) -> Vec<String> {
        let mut args: Vec<String> = vec!["exec".into()];
        // `--image` takes every value up to the next flag, so on a fresh run
        // it sits before one; a resumed run takes it on `resume` instead.
        if turn.session.is_none() {
            args.extend(image_args(turn));
        }
        args.extend([
            "--json".into(),
            "-c".into(),
            "features.multi_agent=false".into(),
            "-c".into(),
            "agents.enabled=false".into(),
            "--skip-git-repo-check".into(),
            "-C".into(),
            turn.cwd.to_string_lossy().into_owned(),
        ]);
        if let Some(model) = turn.model.as_deref().filter(|m| *m != "default") {
            args.push("-m".into());
            args.push(model.to_owned());
        }
        // Hooks may execute outside the native sandbox, so isolated turns
        // cannot inherit them.
        if turn.summary_only || !turn.network_access {
            args.extend(["-c".into(), "features.hooks=false".into()]);
        }
        if let Some(effort) = turn.effort {
            args.extend([
                "-c".into(),
                format!(
                    "model_reasoning_effort=\"{}\"",
                    crate::codex_effort_arg(effort)
                ),
            ]);
        }
        // Codex exec cannot ask anyone, and it only lets MCP tool calls
        // through with approvals and the sandbox off (openai/codex#24135).
        // Aether's tools are essential, so every Codex turn runs full-access;
        // the runtime keeps such agents' approval policy empty to match.
        let _ = turn.bypass_permissions;
        if !turn.network_access {
            args.extend(["-c".into(), "web_search=\"disabled\"".into()]);
        }
        if turn.summary_only {
            args.extend(["--sandbox".into(), "read-only".into()]);
            for setting in [
                "mcp_servers={}",
                "features.shell_tool=false",
                "features.unified_exec=false",
                "features.apps=false",
                "features.plugins=false",
                "web_search=\"disabled\"",
            ] {
                args.extend(["-c".into(), setting.into()]);
            }
        } else {
            args.push("--dangerously-bypass-approvals-and-sandbox".into());
        }
        if let Some(mcp) = &turn.mcp {
            // The authenticated bridge enforces Aether's scope and approval
            // policy. A second native MCP gate can reject completion outright
            // when Codex runs with approvalPolicy=never.
            args.extend([
                "-c".into(),
                "mcp_servers.aether.default_tools_approval_mode=\"approve\"".into(),
            ]);
            args.push("-c".into());
            args.push(format!(
                "mcp_servers.aether.command={}",
                json!(mcp.command.to_string_lossy())
            ));
            args.push("-c".into());
            args.push(format!("mcp_servers.aether.args={}", json!(mcp.args)));
        }
        for dir in &turn.extra_dirs {
            args.push("--add-dir".into());
            args.push(dir.to_string_lossy().into_owned());
        }
        if let Some(session) = &turn.session {
            args.push("resume".into());
            args.extend(image_args(turn));
            args.push(session.clone());
        }
        // Prompt arrives on stdin.
        args.push("-".into());
        args
    }

    fn with_provider_notice(&self, mut run: EngineRun, session: Option<String>) -> EngineRun {
        const NOTICE: &str = "Provider credentials may remain visible to Codex commands: the codex exec fallback cannot safely inspect shell_environment_policy layers; provider environment filters were not applied.";
        let notices = Arc::clone(&self.provider_notices);
        let (events, receiver) = mpsc::channel(256);
        let mut incoming = std::mem::replace(&mut run.events, receiver);
        tokio::spawn(async move {
            let notify = match session {
                Some(id) => notices.lock().await.insert(id),
                None => true,
            };
            if notify
                && events
                    .send(EngineEvent::ProtocolNote(NOTICE.into()))
                    .await
                    .is_err()
            {
                return;
            }
            while let Some(event) = incoming.recv().await {
                if let EngineEvent::Session { id, .. } = &event
                    && !id.is_empty()
                {
                    // Fresh exec sessions choose their own id; a failed resume may replace it.
                    let new_session = notices.lock().await.insert(id.clone());
                    if new_session
                        && !notify
                        && events
                            .send(EngineEvent::ProtocolNote(NOTICE.into()))
                            .await
                            .is_err()
                    {
                        return;
                    }
                }
                if events.send(event).await.is_err() {
                    return;
                }
            }
        });
        run
    }
}

/// One `--image <file>` pair per image of the turn's message.
fn image_args(turn: &EngineTurn) -> impl Iterator<Item = String> + '_ {
    turn.images.iter().flat_map(|image| {
        [
            "--image".to_owned(),
            image.path.to_string_lossy().into_owned(),
        ]
    })
}

#[async_trait::async_trait]
impl Engine for CodexEngine {
    fn kind(&self) -> EngineKind {
        EngineKind::CODEX
    }

    fn adapter(&self) -> String {
        format!("codex/{}", self.version)
    }

    /// Exec mode cannot prompt, and only lets MCP tool calls through with
    /// approvals and the sandbox off (openai/codex#24135).
    fn gates_permissions(&self) -> bool {
        false
    }

    fn validate_network_policy(&self, network_access: bool) -> Result<(), EngineError> {
        if !network_access {
            return Err(EngineError::Unsupported("Network-denied Codex turns require an app-server capable CLI; legacy codex exec runs without a sandbox".into()));
        }
        Ok(())
    }

    async fn run(
        &self,
        turn: EngineTurn,
        cancel: CancellationToken,
    ) -> Result<EngineRun, EngineError> {
        use std::io::Write as _;
        if !turn.summary_only {
            self.validate_network_policy(turn.network_access)?;
        }
        let mut args = self.args(&turn);
        let instructions = if turn.summary_only && !turn.system_prompt.trim().is_empty() {
            let mut file = tempfile::NamedTempFile::new().map_err(|source| EngineError::Spawn {
                program: self.program.clone(),
                source,
            })?;
            file.write_all(turn.system_prompt.as_bytes())
                .map_err(|source| EngineError::Spawn {
                    program: self.program.clone(),
                    source,
                })?;
            args.splice(
                1..1,
                [
                    "-c".into(),
                    format!(
                        "model_instructions_file={}",
                        json!(file.path().to_string_lossy())
                    ),
                ],
            );
            Some(file)
        } else {
            if !turn.system_prompt.trim().is_empty() {
                args.splice(
                    1..1,
                    [
                        "-c".into(),
                        format!("developer_instructions={}", json!(turn.system_prompt)),
                    ],
                );
            }
            None
        };
        let cwd: PathBuf = turn.cwd.clone();
        let mut state = ParseState::for_turn(&turn, EngineVersion::parse(&self.version));
        let ownership_turn = turn.clone();
        let ownership_cancel = cancel.clone();
        let mut ownership_recorded = false;
        let run = run_process(
            &self.program,
            args,
            cwd,
            &turn.hidden_env,
            turn.prompt.clone(),
            false,
            None,
            cancel,
            move |line| {
                // Keep the replacement prompt file alive until the process driver exits.
                let _instructions = &instructions;
                let events = parse_line(line, &mut state);
                if !ownership_recorded
                    && let Some(id) = events.iter().find_map(|event| match event {
                        EngineEvent::Session { id, .. } => Some(id.as_str()),
                        _ => None,
                    })
                {
                    ownership_recorded = true;
                    if let Err(error) =
                        ownership_turn.record_new_session(EngineKind::CODEX, id, Vec::new())
                    {
                        ownership_cancel.cancel();
                        return vec![EngineEvent::Done(EngineOutcome {
                            is_error: true,
                            error: Some(error.to_string()),
                            ..Default::default()
                        })];
                    }
                }
                events
            },
        )
        .await?
        .into_run();
        // Adding filters without raw layers can discard an operator's legacy exclusions.
        if !turn.summary_only
            && turn
                .tool_hidden_env
                .iter()
                .any(|name| !turn.hidden_env.contains(name))
        {
            Ok(self.with_provider_notice(run, turn.session))
        } else {
            Ok(run)
        }
    }
}

/// What one `codex exec` run carries from line to line.
#[derive(Debug, Default)]
pub struct ParseState {
    /// The thread this run is on: the resumed one, or what `thread.started` names.
    thread_id: Option<String>,
    /// The thread's cumulative counters as last read, seeded with what a
    /// resumed thread had already counted so its history is not billed again.
    counted: RawTotals,
    /// What this run has billed of each of those counters.
    billed: RawTotals,
    /// This run's usage so far. Monotone: a counter that dips never lowers it.
    usage: Usage,
    /// A reading this run carried a counter it could not read. The carried
    /// value settles on a later reading, so no delta after it is exact.
    inexact: bool,
    /// Which newer counters the CLI sends on every thread, by its version.
    cli_sends: NewerCounters,
    /// Which newer counters this thread's CLI sends: `cli_sends`, plus any it
    /// has been seen to send.
    sends: NewerCounters,
    /// The current turn's last non-blank agent message: its answer.
    final_text: Option<String>,
}

impl ParseState {
    /// The state for `turn` on a CLI at `version` (`None`: unknown, so only
    /// what it sends says what it must). A resumed thread starts from the
    /// counters the runtime persisted for it, a fresh one from nothing.
    pub fn for_turn(turn: &EngineTurn, version: Option<EngineVersion>) -> Self {
        let cli_sends = version.map(NewerCounters::sent_by).unwrap_or_default();
        let Some(thread) = &turn.session else {
            return Self {
                cli_sends,
                sends: cli_sends,
                ..Self::default()
            };
        };
        let counted = RawTotals::of(turn.session_usage);
        Self {
            thread_id: Some(thread.clone()),
            counted,
            cli_sends,
            // A counter the thread has counted was sent by an earlier run.
            sends: NewerCounters {
                cache_write: cli_sends.cache_write || counted.cache_write > 0,
                reasoning: cli_sends.reasoning || counted.reasoning > 0,
            },
            ..Self::default()
        }
    }
}

/// `turn.completed` counters newer than the event: openai/codex#19308 and
/// #33454 added them, so the older CLIs this adapter serves leave them out.
/// Once a thread's CLI sends one, leaving it out is an unread counter.
#[derive(Clone, Copy, Debug, Default)]
struct NewerCounters {
    cache_write: bool,
    reasoning: bool,
}

impl NewerCounters {
    /// What a CLI at `version` always sends: exec_events.rs serializes every
    /// `Usage` field, reasoning from rust-v0.125.0, cache writes from 0.145.0.
    fn sent_by(version: EngineVersion) -> Self {
        Self {
            cache_write: version >= EngineVersion::new(0, 145, 0),
            reasoning: version >= EngineVersion::new(0, 125, 0),
        }
    }
}

/// Map one JSONL line to engine events.
pub fn parse_line(line: &str, state: &mut ParseState) -> Vec<EngineEvent> {
    let v: Value = match serde_json::from_str(line) {
        Ok(v) => v,
        Err(_) => return vec![EngineEvent::Note(line.chars().take(300).collect())],
    };
    let kind = v.get("type").and_then(Value::as_str).unwrap_or("");
    match kind {
        "thread.started" => {
            let id = v
                .get("thread_id")
                .and_then(Value::as_str)
                .unwrap_or("")
                .to_owned();
            if !id.is_empty() && state.thread_id.as_deref() != Some(id.as_str()) {
                // Not the thread the run resumed: it has counted nothing yet.
                state.counted = RawTotals::default();
                state.sends = state.cli_sends;
                state.thread_id = Some(id.clone());
            }
            vec![EngineEvent::Session {
                id,
                model: None,
                tools: Vec::new(),
            }]
        }
        "item.started" | "item.updated" | "item.completed" => {
            let item = &v["item"];
            if kind == "item.completed"
                && item["type"].as_str() == Some("agent_message")
                && let Some(text) = item["text"].as_str().filter(|t| !t.trim().is_empty())
            {
                state.final_text = Some(text.to_owned());
            }
            item_events(kind, item)
        }
        "turn.completed" => {
            // Without a usage object every counter is unread, and said so.
            let mut events = account(v.get("usage").unwrap_or(&Value::Null), state);
            events.push(EngineEvent::Done(EngineOutcome {
                closing_accounting: true,
                usage: state.usage,
                text: state.final_text.take(),
                ..Default::default()
            }));
            events
        }
        "turn.failed" | "error" => {
            let message = v
                .get("error")
                .and_then(|e| e.get("message"))
                .or_else(|| v.get("message"))
                .and_then(Value::as_str)
                .unwrap_or("unknown error")
                .to_owned();
            let session_lost = message.contains("session")
                && (message.contains("not found") || message.contains("No such"));
            state.final_text = None;
            let closing_accounting = kind == "turn.failed" && v["usage"].is_object();
            let mut events = if closing_accounting {
                account(&v["usage"], state)
            } else {
                Vec::new()
            };
            events.push(EngineEvent::Done(EngineOutcome {
                closing_accounting,
                usage: state.usage,
                is_error: true,
                error: Some(message),
                session_lost,
                ..Default::default()
            }));
            events
        }
        _ => Vec::new(),
    }
}

/// Bill one `turn.completed` usage. Like the app-server's, it is the
/// *thread's* cumulative count, so the run is billed each counter's increase
/// over the last reading, exactly as [`crate::codex_app::TurnState`] bills it.
/// Emitted as a [`EngineEvent::UsageDelta`] so the runtime prices it; one
/// built from a counter that could not be read is marked `cost_complete:
/// false`, which the runtime leaves unpriced.
fn account(usage: &Value, state: &mut ParseState) -> Vec<EngineEvent> {
    // Input, cached input and output have been in every `turn.completed`
    // since the event exists; a newer counter carries whether it was sent.
    let counters = [
        (
            "input_tokens",
            &mut state.counted.input,
            &mut state.billed.input,
            None,
        ),
        (
            "cached_input_tokens",
            &mut state.counted.cached,
            &mut state.billed.cached,
            None,
        ),
        (
            "output_tokens",
            &mut state.counted.output,
            &mut state.billed.output,
            None,
        ),
        (
            "cache_write_input_tokens",
            &mut state.counted.cache_write,
            &mut state.billed.cache_write,
            Some(&mut state.sends.cache_write),
        ),
        (
            "reasoning_output_tokens",
            &mut state.counted.reasoning,
            &mut state.billed.reasoning,
            Some(&mut state.sends.reasoning),
        ),
    ];
    let mut unreadable = Vec::new();
    let mut backwards = Vec::new();
    for (key, counted, billed, sent) in counters {
        // An unread counter keeps its last reading: read as zero, it would
        // look like a restarted counter and re-bill the thread.
        let now = match (usage.get(key), sent) {
            (Some(value), sent) => {
                if let Some(sent) = sent {
                    *sent = true;
                }
                value.as_u64().unwrap_or_else(|| {
                    unreadable.push(key);
                    *counted
                })
            }
            // A CLI that predates this counter: nothing to read, nothing lost.
            (None, Some(sent)) if !*sent => *counted,
            (None, _) => {
                unreadable.push(key);
                *counted
            }
        };
        let moved = advanced(now, *counted);
        *counted = moved.carried;
        *billed = billed.saturating_add(moved.billed);
        if moved.dipped > 0 {
            backwards.push(format!("{key} by {}", moved.dipped));
        }
    }
    let mut events = Vec::new();
    if !unreadable.is_empty() {
        state.inexact = true;
        events.push(EngineEvent::ProtocolNote(format!(
            "codex ended the turn with no readable {}; the previous reading stands \
             for {}, so this run's cost is not exact",
            unreadable.join(", "),
            if unreadable.len() == 1 { "it" } else { "them" }
        )));
    }
    if !backwards.is_empty() {
        events.push(EngineEvent::ProtocolNote(format!(
            "codex's thread counter came back below what it had reported: {}",
            backwards.join(", ")
        )));
    }
    // Never lowered: the cached-inside-input split can dip, and a bill that
    // dips is billed twice when it climbs back.
    let mut delta = consumed_since(state.billed.into_usage(), state.usage);
    if state.inexact {
        delta.mark_cost_gap(aether_core::model::CostGap::Unreported);
    }
    state.usage += delta;
    if let Some(session) = state.thread_id.clone() {
        // Ahead of the delta: whatever persists the spend persists the
        // baseline with it, so a resumed thread is never billed twice.
        events.push(EngineEvent::SessionUsage {
            models: None,
            session,
            total: state.counted.into_usage(),
        });
    }
    if delta.total_tokens() > 0 || delta.reasoning_tokens.is_some_and(|n| n > 0) {
        events.push(EngineEvent::UsageDelta { usage: delta });
    }
    events
}

fn item_events(kind: &str, item: &Value) -> Vec<EngineEvent> {
    let id = item
        .get("id")
        .and_then(Value::as_str)
        .unwrap_or("")
        .to_owned();
    let completed = kind == "item.completed";
    match item.get("type").and_then(Value::as_str).unwrap_or("") {
        "agent_message" if completed => {
            let text = item
                .get("text")
                .and_then(Value::as_str)
                .unwrap_or("")
                .to_owned();
            vec![
                EngineEvent::Delta(StreamEvent::TextDelta {
                    index: 0,
                    text: text.clone(),
                }),
                EngineEvent::Assistant {
                    content: vec![ContentBlock::Text { text }],
                    usage: None,
                    model: None,
                },
            ]
        }
        "reasoning" if completed => {
            let text = item
                .get("text")
                .and_then(Value::as_str)
                .unwrap_or("")
                .to_owned();
            vec![EngineEvent::Delta(StreamEvent::ThinkingDelta {
                index: 0,
                text,
            })]
        }
        "command_execution" => {
            let command = item
                .get("command")
                .and_then(Value::as_str)
                .unwrap_or("")
                .to_owned();
            if kind == "item.started" {
                return vec![EngineEvent::Assistant {
                    content: vec![ContentBlock::ToolUse {
                        id: id.clone(),
                        name: "bash".into(),
                        input: json!({ "command": command }),
                    }],
                    usage: None,
                    model: None,
                }];
            }
            if completed {
                let output = item
                    .get("aggregated_output")
                    .and_then(Value::as_str)
                    .unwrap_or("")
                    .to_owned();
                let exit = item.get("exit_code").and_then(Value::as_i64);
                return vec![EngineEvent::ToolResult {
                    tool_use_id: id,
                    content: format!(
                        "{output}\n[exit code {}]",
                        exit.map(|c| c.to_string()).unwrap_or_else(|| "?".into())
                    ),
                    is_error: exit.is_some_and(|c| c != 0)
                        || item.get("status").and_then(Value::as_str) == Some("failed"),
                }];
            }
            Vec::new()
        }
        "file_change" if completed => {
            let changes: Vec<String> = item
                .get("changes")
                .and_then(Value::as_array)
                .map(|a| {
                    a.iter()
                        .map(|c| {
                            format!(
                                "{} {}",
                                c.get("kind").and_then(Value::as_str).unwrap_or("change"),
                                c.get("path").and_then(Value::as_str).unwrap_or("?")
                            )
                        })
                        .collect()
                })
                .unwrap_or_default();
            vec![
                EngineEvent::Assistant {
                    content: vec![ContentBlock::ToolUse {
                        id: id.clone(),
                        name: "apply_patch".into(),
                        input: json!({ "changes": changes }),
                    }],
                    usage: None,
                    model: None,
                },
                EngineEvent::ToolResult {
                    tool_use_id: id,
                    content: changes.join("\n"),
                    is_error: item.get("status").and_then(Value::as_str) == Some("failed"),
                },
            ]
        }
        "mcp_tool_call" if completed => {
            let server = item.get("server").and_then(Value::as_str).unwrap_or("mcp");
            let tool = item.get("tool").and_then(Value::as_str).unwrap_or("tool");
            let name = format!("mcp__{server}__{tool}");
            let result = item
                .get("result")
                .map(|r| r.to_string())
                .unwrap_or_default();
            vec![
                EngineEvent::Assistant {
                    content: vec![ContentBlock::ToolUse {
                        id: id.clone(),
                        name,
                        input: item
                            .get("arguments")
                            .cloned()
                            .unwrap_or(Value::Object(Default::default())),
                    }],
                    usage: None,
                    model: None,
                },
                EngineEvent::ToolResult {
                    tool_use_id: id,
                    content: result,
                    is_error: item.get("status").and_then(Value::as_str) == Some("failed"),
                },
            ]
        }
        "error" => vec![EngineEvent::Note(
            item.get("message")
                .and_then(Value::as_str)
                .unwrap_or("error")
                .to_owned(),
        )],
        _ => Vec::new(),
    }
}

#[cfg(test)]
mod tests;
