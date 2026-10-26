//! Claude Code as an engine: `claude -p --output-format stream-json`.
//!
//! Messages sent while a turn runs go to the CLI's stdin. It folds them in at
//! its next step (after the current tool, or as a new turn after its result)
//! and, with `--replay-user-messages`, echoes each one when it does; that
//! echo is the acknowledgement the runtime waits for before recording the
//! message as consumed.

use std::path::PathBuf;

use serde_json::json;
use tokio_util::sync::CancellationToken;

use crate::detect::{self, DetectError, EngineVersion};
use crate::offline::{self, executable_on_path};
use crate::{
    Engine, EngineError, EngineImage, EngineKind, EngineLaunchContext, EngineRun, EngineTurn,
    run_process_with_env,
};

// Native coordination would create state outside Aether's tracked tree and
// durable scheduler. Keep native workspace/research tools, including background
// Bash's TaskOutput/TaskStop and the code-review renderer ReportFindings.
const NATIVE_COORDINATION_TOOLS: &str = "Task,Agent,ListAgents,SendMessage,ScheduleWakeup,CronCreate,CronDelete,CronList,Workflow,RemoteTrigger";

/// Support matrix: the oldest `claude` this adapter's offline sandbox path is
/// verified against. `args()` unconditionally passes `--replay-user-messages`
/// and `--include-partial-messages`, passes `--effort` whenever a turn's
/// effort is set (agent config or the protocol default, which is most
/// turns), and (offline turns only) depends on the native sandbox's
/// `network` settings block; 2.1.219 is the one version boundary already
/// established in this codebase (see `validate_offline_support`, and
/// `docs/engines.md`'s sandbox section) as supporting all of them together.
/// Earlier releases may well support the non-sandbox flags too, but that is
/// not independently verified here, so registration does not gate on this
/// floor - only `validate_offline_support`, the one path it
/// is actually verified for, does. A `claude` below it still registers, with
/// [`ClaudeCodeEngine::limitation`] naming the gap.
pub const MIN_VERSION: EngineVersion = EngineVersion::new(2, 1, 219);

#[derive(Debug, Clone)]
pub struct ClaudeCodeEngine {
    program: String,
    version: String,
    prompt_cache_ttl: Option<aether_core::model::PromptCacheTtl>,
}

impl ClaudeCodeEngine {
    pub fn new(program: impl Into<String>, version: impl Into<String>) -> Self {
        Self {
            program: program.into(),
            version: version.into(),
            prompt_cache_ttl: None,
        }
    }

    /// Override the cache-write tier every launched turn requests.
    /// Without an override, Claude Code keeps its native cache settings.
    pub fn with_prompt_cache_ttl(mut self, ttl: aether_core::model::PromptCacheTtl) -> Self {
        self.prompt_cache_ttl = Some(ttl);
        self
    }

    /// Find `claude` on PATH and read its version. `Err` names why the CLI
    /// could not be started or read at all; see [`DetectError`]. A version
    /// below [`MIN_VERSION`] still registers - see [`Self::limitation`].
    pub async fn detect_verbose() -> Result<Self, DetectError> {
        Self::detect_verbose_on(None).await
    }

    /// `detect_verbose`, optionally searching `path` instead of the process's
    /// own `PATH` - the seam a test drives with a fake `claude` on a temp dir.
    async fn detect_verbose_on(path: Option<&std::path::Path>) -> Result<Self, DetectError> {
        let version = detect::probe_version("claude", path).await?;
        Ok(Self::new("claude", version.to_string()))
    }

    /// Known offline-sandbox gaps; online turns remain available.
    /// This only checks the version and executable presence on PATH.
    pub fn limitation(&self) -> Option<String> {
        self.limitation_on(std::env::var_os("PATH").as_deref())
    }

    fn limitation_on(&self, path: Option<&std::ffi::OsStr>) -> Option<String> {
        let mut limitations = Vec::new();
        if let Some(version) = EngineVersion::parse(&self.version)
            && version < MIN_VERSION
        {
            limitations.push(format!(
                "network-denied turns require claude >= {MIN_VERSION} (found {version})"
            ));
        }
        if let Some(reason) =
            offline::dependency_limitation("Claude", |program| executable_on_path(program, path))
        {
            limitations.push(reason);
        }
        (!limitations.is_empty()).then(|| limitations.join("; "))
    }

    fn validate_offline_support(
        &self,
        has_executable: impl Fn(&str) -> bool,
    ) -> Result<(), EngineError> {
        let supported = EngineVersion::parse(&self.version).is_some_and(|v| v >= MIN_VERSION);
        if !supported {
            return Err(EngineError::Unsupported("Network-denied Claude turns require Claude Code 2.1.219 or newer for strict sandbox network enforcement".into()));
        }
        if let Some(reason) = offline::dependency_limitation("Claude", has_executable) {
            return Err(EngineError::Unsupported(reason));
        }
        Ok(())
    }

    pub fn args(&self, turn: &EngineTurn) -> Vec<String> {
        let mut args: Vec<String> = vec![
            "-p".into(),
            "--input-format".into(),
            "stream-json".into(),
            "--output-format".into(),
            "stream-json".into(),
            "--verbose".into(),
            "--include-partial-messages".into(),
            "--replay-user-messages".into(),
            "--disallowedTools".into(),
            if turn.network_access {
                NATIVE_COORDINATION_TOOLS.to_owned()
            } else {
                format!("{NATIVE_COORDINATION_TOOLS},WebSearch,WebFetch")
            },
        ];
        if turn.summary_only || turn.compact_session {
            args.extend(["--tools".into(), "".into(), "--strict-mcp-config".into()]);
        }
        // `/compact` is itself a slash command.
        if turn.summary_only {
            args.push("--disable-slash-commands".into());
        }
        // Summary turns already pass both flags, offline turns the MCP one.
        if turn.lean_host_context && !turn.summary_only && !turn.compact_session {
            args.push("--disable-slash-commands".into());
            if turn.network_access {
                args.push("--strict-mcp-config".into());
            }
        }
        let mut settings = json!({"env": {}});
        if let Some(ttl) = self.prompt_cache_ttl {
            settings["env"]["CLAUDE_CODE_PROMPT_CACHE_TTL"] = json!(ttl.as_str());
        }
        if let Some(window) = turn.auto_compact_tokens.map(auto_compact_window) {
            settings["env"]["CLAUDE_CODE_AUTO_COMPACT_WINDOW"] = json!(window.to_string());
        }
        // Hooks can perform their own I/O outside the requested sandbox.
        let toolless = turn.summary_only || turn.compact_session;
        if toolless || !turn.network_access {
            settings["disableAllHooks"] = json!(true);
        }
        if !toolless && !turn.network_access {
            // Inherited exclusions and MCP servers can escape the sandbox.
            args.extend([
                "--setting-sources".into(),
                "".into(),
                "--strict-mcp-config".into(),
            ]);
            settings["sandbox"] = json!({
                "enabled": true,
                "failIfUnavailable": true,
                "allowUnsandboxedCommands": false,
                "network": {"strictAllowlist": true, "allowedDomains": [], "deniedDomains": ["*"]}
            });
            let writable: Vec<_> = turn
                .temp_dir
                .iter()
                .chain(turn.build_env.iter().map(|(_, dir)| dir))
                .map(|dir| dir.to_string_lossy())
                .collect();
            if !writable.is_empty() {
                settings["sandbox"]["filesystem"] = json!({"allowWrite": writable});
            }
            settings["permissions"] = json!({"deny": ["WebFetch(domain:*)", "WebSearch"]});
        } else if !toolless && !turn.build_env.is_empty() {
            // An operator-enabled sandbox must still let builds reach the cache outside the workspace.
            let writable: Vec<_> = turn
                .build_env
                .iter()
                .map(|(_, dir)| dir.to_string_lossy())
                .collect();
            settings["sandbox"] = json!({"filesystem": {"allowWrite": writable}});
        }
        args.extend(["--settings".into(), settings.to_string()]);
        if !turn.system_prompt.trim().is_empty() {
            args.push(if turn.summary_only {
                "--system-prompt".into()
            } else {
                "--append-system-prompt".into()
            });
            args.push(turn.system_prompt.clone());
        }
        match &turn.session {
            Some(id) => {
                args.push("--resume".into());
                args.push(id.clone());
            }
            None => {
                args.push("--session-id".into());
                args.push(turn.new_session_id.clone());
            }
        }
        if let Some(model) = turn.model.as_deref().filter(|m| *m != "default") {
            args.push("--model".into());
            args.push(model.to_owned());
        }
        if let Some(effort) = turn.effort {
            args.push("--effort".into());
            args.push(effort.as_str().into());
        }
        if turn.bypass_permissions {
            args.push("--permission-mode".into());
            args.push("bypassPermissions".into());
        } else {
            args.push("--permission-mode".into());
            args.push("default".into());
            if turn.mcp.is_some() {
                args.push("--permission-prompt-tool".into());
                args.push("mcp__aether__permission".into());
                args.push("--permission-prompts".into());
                args.push("host".into());
            }
        }
        if let Some(mcp) = &turn.mcp {
            // Coordination must be available when the first prompt is built.
            // Other native MCP servers keep their own tool-search settings.
            let config = json!({
                "mcpServers": {
                    "aether": {
                        "command": mcp.command.to_string_lossy(),
                        "args": mcp.args,
                        "alwaysLoad": true
                    }
                }
            });
            args.push("--mcp-config".into());
            args.push(config.to_string());
        }
        for dir in &turn.extra_dirs {
            args.push("--add-dir".into());
            args.push(dir.to_string_lossy().into_owned());
        }
        args
    }
}

/// Claude Code compacts this many tokens below `CLAUDE_CODE_AUTO_COMPACT_WINDOW`
/// (its summary buffer) and accepts a window from 100K to 1M.
/// A focused brief roughly halves how long Claude Code's own summary takes.
const COMPACT_COMMAND: &str = "/compact Keep the summary concise, under 1,500 words: the goal and current task, decisions made and why, files, functions and commands involved, errors still open, and the next step.";
const AUTO_COMPACT_BUFFER: u32 = 13_000;
const AUTO_COMPACT_WINDOW: std::ops::RangeInclusive<u32> = 100_000..=1_000_000;

/// The window that makes Claude Code compact at `threshold` tokens, within its bounds.
fn auto_compact_window(threshold: u32) -> u32 {
    let window = threshold.saturating_add(AUTO_COMPACT_BUFFER);
    window.clamp(*AUTO_COMPACT_WINDOW.start(), *AUTO_COMPACT_WINDOW.end())
}

/// A user message in Claude Code's stream-json input format: its text, then
/// one image block per image. Without images it is the line it always was.
pub fn user_message_line(text: &str, images: &[EngineImage]) -> String {
    format!("{}\n", user_message(text, images))
}

/// A steered message's line, under the `uuid` the CLI echoes it back with.
/// Messages sent close together run as one turn, and the CLI echoes each
/// member of that batch only by the uuid it came with.
pub fn steered_message_line(uuid: &str, text: &str, images: &[EngineImage]) -> String {
    assert!(!uuid.is_empty(), "a steered message is echoed by its uuid");
    let mut message = user_message(text, images);
    message["uuid"] = serde_json::Value::from(uuid);
    format!("{message}\n")
}

fn user_message(text: &str, images: &[EngineImage]) -> serde_json::Value {
    let mut content = vec![serde_json::json!({"type": "text", "text": text})];
    content.extend(images.iter().filter_map(image_block));
    serde_json::json!({
        "type": "user",
        "message": {"role": "user", "content": content}
    })
}

/// An image as an Anthropic content block. One whose file cannot be read is
/// left out: the message text names the stored file either way.
fn image_block(image: &EngineImage) -> Option<serde_json::Value> {
    use base64::Engine as _;
    match std::fs::read(&image.path) {
        Ok(bytes) => Some(serde_json::json!({
            "type": "image",
            "source": {
                "type": "base64",
                "media_type": image.media_type,
                "data": base64::engine::general_purpose::STANDARD.encode(bytes),
            }
        })),
        Err(error) => {
            tracing::warn!(path = %image.path.display(), %error, "image left out of a Claude Code message");
            None
        }
    }
}

#[async_trait::async_trait]
impl Engine for ClaudeCodeEngine {
    fn kind(&self) -> EngineKind {
        EngineKind::CLAUDE_CODE
    }

    fn adapter(&self) -> String {
        format!("claude-code/{}", self.version)
    }

    fn validate_network_policy(&self, network_access: bool) -> Result<(), EngineError> {
        if network_access {
            return Ok(());
        }
        let path = std::env::var_os("PATH");
        self.validate_offline_support(|program| executable_on_path(program, path.as_deref()))
    }

    fn compacts_natively(&self) -> bool {
        true
    }

    async fn run(
        &self,
        turn: EngineTurn,
        cancel: CancellationToken,
    ) -> Result<EngineRun, EngineError> {
        self.launch(turn, None, cancel).await
    }

    async fn run_with_context(
        &self,
        turn: EngineTurn,
        context: EngineLaunchContext,
        cancel: CancellationToken,
    ) -> Result<EngineRun, EngineError> {
        self.launch(turn, Some(context), cancel).await
    }
}

impl ClaudeCodeEngine {
    async fn launch(
        &self,
        mut turn: EngineTurn,
        context: Option<EngineLaunchContext>,
        cancel: CancellationToken,
    ) -> Result<EngineRun, EngineError> {
        if turn.compact_session {
            if turn.session.is_none() || turn.mcp.is_some() || turn.summary_only {
                return Err(EngineError::Unsupported(
                    "Compacting needs an existing Claude Code session and no tools".into(),
                ));
            }
            turn.prompt = COMPACT_COMMAND.into();
            turn.images.clear();
        } else if !turn.summary_only {
            self.validate_network_policy(turn.network_access)?;
        }
        let args = self.args(&turn);
        let cwd: PathBuf = turn.cwd.clone();
        let mut state = ParseState::for_turn(&turn);
        let outstanding = state.outstanding.clone();
        let environment = telemetry::environment(context, &turn.hidden_env);
        let mut process = run_process_with_env(
            &self.program,
            args,
            cwd,
            &turn.hidden_env,
            turn.temp_dir.as_deref(),
            &turn.build_env,
            &environment,
            context.map(|context| context.agent_id),
            &turn.redactor,
            user_message_line(&turn.prompt, &turn.images),
            true,
            Some(input::interrupt_line()),
            cancel.clone(),
            move |line| parse_line(line, &mut state),
        )
        .await?;
        if let Err(error) =
            turn.record_new_session(self.kind(), &turn.new_session_id, session_log_paths(&turn))
        {
            cancel.cancel();
            // A failed launch is safe to retry only once its process cleanup finished.
            while process.events.recv().await.is_some() {}
            return Err(error);
        }
        let Some(stdin) = process.stdin else {
            return Err(EngineError::Exited("stdin was not kept open".into()));
        };
        // Messages become stream-json lines; each is remembered until the
        // CLI echoes it back, which is when it reached the model.
        let (input_tx, stop_tx) = input::spawn(stdin, outstanding);
        Ok(EngineRun {
            events: process.events,
            input: Some(input_tx),
            decisions: None,
            stop: Some(stop_tx),
        })
    }
}

mod input;
mod ownership;
mod telemetry;
use ownership::session_log_paths;
mod session_cost;
mod stream;
pub use stream::{ParseState, parse_line};

#[cfg(test)]
#[path = "claude_code/tests/launch.rs"]
mod tests;
