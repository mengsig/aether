//! Codex CLI as an engine over its app-server protocol (JSON-RPC lines on
//! stdio). One `codex app-server` process per turn: `thread/start` or
//! `thread/resume`, then `turn/start`; the thread persists on disk, so the
//! next turn resumes it.
//!
//! Compared with `codex exec`, the server asks the client before it leaves
//! its sandbox (`item/*/requestApproval`), takes more input while a turn
//! runs (`turn/steer`), and stops on request (`turn/interrupt`). That is
//! what lets Aether gate Codex with approvals like any other agent.

use std::path::PathBuf;
use std::time::Duration;

use serde_json::{Value, json};
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;

use crate::detect::{self, DetectError, EngineVersion};
use crate::{Engine, EngineError, EngineKind, EngineRun, EngineTurn, PermissionDecision};

mod driver;
mod items;
mod loopback;
mod read_usage;
mod state;
mod usage;

use driver::Driver;
pub use state::{ServerReply, ServerRequest, TurnState};
// `codex exec` reports the same thread-cumulative counters.
pub(crate) use usage::{RawTotals, advanced, consumed_since};

#[cfg(test)]
mod tests;

/// Support matrix: this adapter has no established minimum *version* -
/// `args()`'s `-c key=value` overrides (`agents.enabled`, `features.*`,
/// `model_reasoning_effort`, `mcp_servers.*`) are TOML overlays Codex accepts
/// for any dotted path, so an older build that does not read a given key
/// simply does not act on it rather than rejecting the flag. What genuinely
/// gates this adapter is the `app-server` subcommand itself, plus the
/// `initialize`, `thread/start`/`thread/resume`, `turn/start`, `turn/steer`,
/// `turn/interrupt` and `item/*/requestApproval` JSON-RPC methods it drives -
/// none of which a `--version`/`--help` probe can confirm beyond `app-server
/// --help` answering at all. That is [`CAPABILITY_APP_SERVER`]'s check; a
/// renamed or removed `app-server` on an old or dev build fails it.
const CAPABILITY_APP_SERVER: &str = "app-server";

/// The oldest Codex whose experimental `environment/add` and per-turn
/// `environments` the loopback sandbox is verified against (0.159.3).
const LOOPBACK_MIN_VERSION: EngineVersion = EngineVersion::new(0, 159, 0);

/// Codex's own permissions text says network is enabled in the loopback sandbox.
/// Turn input, not developer instructions: those outlive the turn on resume.
const LOOPBACK_INSTRUCTIONS: &str = "Network for this turn: offline sandbox. Only loopback (127.0.0.1, ::1) is reachable, so local servers work; external hosts and DNS are not, and approval or escalation does not grant network. Any \"network access is enabled\" note means loopback only, until a later turn's network note says otherwise.";

/// A network-allowed turn under approvals: hosts are reached only through an
/// escalation, which the runtime gates as an external effect.
const GATED_NETWORK_INSTRUCTIONS: &str = "Network for this turn: commands run in a sandbox without network, loopback included. To reach a host or a local server, request escalation for that command; it runs once approved.";

/// Supersedes [`LOOPBACK_INSTRUCTIONS`] on a later network-allowed turn of the same thread.
const NETWORK_RESTORED_INSTRUCTIONS: &str = "Network for this turn: external hosts and DNS are reachable; any earlier offline-sandbox or loopback-only note no longer applies.";

#[derive(Debug, Clone)]
pub struct CodexAppEngine {
    program: String,
    version: String,
}

impl CodexAppEngine {
    pub fn new(program: impl Into<String>, version: impl Into<String>) -> Self {
        Self {
            program: program.into(),
            version: version.into(),
        }
    }

    /// Find `codex` on PATH, read its version, and confirm it knows
    /// `app-server`. `Err` names why the CLI is unusable; see [`DetectError`].
    pub async fn detect_verbose() -> Result<Self, DetectError> {
        Self::detect_verbose_on(None).await
    }

    /// `detect_verbose`, optionally searching `path` instead of the process's
    /// own `PATH` - the seam a test drives with a fake `codex` on a temp dir.
    async fn detect_verbose_on(path: Option<&std::path::Path>) -> Result<Self, DetectError> {
        let version = detect::probe_version("codex", path).await?;
        detect::probe_capability(
            "codex",
            &["app-server", "--help"],
            CAPABILITY_APP_SERVER,
            path,
        )
        .await?;
        Ok(Self::new("codex", version.to_string()))
    }

    /// Whether network-denied Linux turns can run in the loopback sandbox:
    /// a verified Codex version plus `bwrap` and `socat`.
    fn validate_loopback_support(
        &self,
        has_executable: impl Fn(&str) -> bool,
    ) -> Result<(), EngineError> {
        if !EngineVersion::parse(&self.version).is_some_and(|v| v >= LOOPBACK_MIN_VERSION) {
            return Err(EngineError::Unsupported(format!(
                "Network-denied Codex turns require codex >= {LOOPBACK_MIN_VERSION} for the loopback-only sandbox (found {})",
                self.version
            )));
        }
        match crate::offline::dependency_limitation("Codex", has_executable) {
            Some(reason) => Err(EngineError::Unsupported(reason)),
            None => Ok(()),
        }
    }

    /// Process arguments: the server plus Aether's MCP bridge as an override.
    pub fn args(&self, turn: &EngineTurn) -> Vec<String> {
        let mut args: Vec<String> = vec![
            "app-server".into(),
            // Model metadata can enable V2 delegation independently of the
            // legacy feature flag. Disable the tool surface itself as well.
            "-c".into(),
            "agents.enabled=false".into(),
            "-c".into(),
            "features.multi_agent=false".into(),
        ];
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
        if !turn.network_access {
            args.extend(["-c".into(), "web_search=\"disabled\"".into()]);
        }
        if turn.summary_only {
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
        args.extend(crate::codex_compaction_args(turn));
        args.extend(crate::codex_env_args(turn));
        args
    }
}

/// `thread/start` or `thread/resume` parameters for a turn.
///
/// Native approval policy is Aether's, not the operator's: `never` makes
/// Codex refuse outright any command an operator `decision="prompt"` rule
/// covers ("approval required by policy, but AskForApproval is set to
/// Never"), with no request Aether could answer. So a bypass turn with
/// network runs `on-request`: those prompts become `requestApproval`
/// requests that `server_request` turns into permission events and the
/// runtime decides under the agent's own policy - for a bypass agent, that
/// policy gates nothing, so every native prompt is granted. That is a
/// deliberate broadening the operator accepted: any prompt
/// rule, not only `gh pr merge`, is answered by Aether policy there.
/// `decision="forbidden"` rules stay native refusals under either policy.
///
/// A bypass turn without network keeps `never`: granting a sandbox
/// escalation there runs the command outside the sandbox, with network, so
/// an ungated agent would silently escape its offline scope.
pub fn thread_params(turn: &EngineTurn) -> (&'static str, Value) {
    let (approval, sandbox) = if turn.bypass_permissions && turn.network_access {
        ("on-request", "danger-full-access")
    } else if turn.bypass_permissions {
        ("never", "workspace-write")
    } else {
        ("on-request", "workspace-write")
    };
    let mut params = json!({
        "cwd": turn.cwd.to_string_lossy(),
        "approvalPolicy": approval,
        "sandbox": sandbox,
    });
    if !turn.system_prompt.trim().is_empty() {
        let key = if turn.summary_only {
            "baseInstructions"
        } else {
            "developerInstructions"
        };
        params[key] = json!(turn.system_prompt);
    }
    if turn.summary_only {
        params["sandbox"] = json!("read-only");
        params["approvalPolicy"] = json!("never");
    }
    if let Some(model) = turn.model.as_deref().filter(|m| *m != "default") {
        params["model"] = json!(model);
    }
    match &turn.session {
        Some(id) => {
            params["threadId"] = json!(id);
            ("thread/resume", params)
        }
        None => {
            // Sticky, so the response confirms the sandbox before any turn runs.
            if let Some(environments) = loopback_environments(turn) {
                params["environments"] = environments;
            }
            ("thread/start", params)
        }
    }
}

fn command_hidden_env(turn: &EngineTurn) -> Vec<&str> {
    turn.tool_hidden_env
        .iter()
        .filter(|name| !turn.hidden_env.contains(name))
        .map(String::as_str)
        // Codex filters are patterns; credential names must not introduce wildcards.
        .filter(|name| valid_env_filter_name(name))
        .collect()
}

/// An image of a user message as the app server reads it: a file on disk.
fn local_image_item(image: &crate::EngineImage) -> Value {
    json!({ "type": "localImage", "path": image.path })
}

/// `turn/steer` parameters: a user message, with its images, for the turn
/// that is running.
pub fn steer_params(thread_id: &str, turn_id: &str, message: &crate::EngineInput) -> Value {
    let mut input = vec![json!({ "type": "text", "text": message.text.trim_end() })];
    input.extend(message.images.iter().map(local_image_item));
    json!({
        "threadId": thread_id,
        "expectedTurnId": turn_id,
        "input": input,
    })
}

/// `turn/start` parameters: the prompt, model, effort, and the sandbox with
/// every extra directory writable and network access matching the agent scope.
///
/// An omitted model or effort leaves the native thread default in effect.
/// `loopback_note_in_effect`: the thread's latest network note is an offline or gated one.
pub fn turn_params(turn: &EngineTurn, thread_id: &str, loopback_note_in_effect: bool) -> Value {
    let mut input = Vec::new();
    if loopback_sandboxed(turn) {
        input.push(json!({ "type": "text", "text": LOOPBACK_INSTRUCTIONS }));
    } else if network_gated(turn) {
        input.push(json!({ "type": "text", "text": GATED_NETWORK_INSTRUCTIONS }));
    } else if loopback_note_in_effect && turn.network_access && !turn.summary_only {
        input.push(json!({ "type": "text", "text": NETWORK_RESTORED_INSTRUCTIONS }));
    }
    input.push(json!({ "type": "text", "text": turn.prompt }));
    input.extend(turn.images.iter().map(local_image_item));
    let mut params = json!({
        "threadId": thread_id,
        "input": input,
    });
    // A resumed thread may have started on another model. The next turn must
    // use this agent's current selection even when the thread is retained.
    if let Some(model) = turn.model.as_deref().filter(|m| *m != "default") {
        params["model"] = json!(model);
    }
    if let Some(effort) = turn.effort {
        params["effort"] = json!(crate::codex_effort_arg(effort));
    }
    if turn.summary_only {
        params["sandboxPolicy"] = json!({ "type": "readOnly" });
    } else if turn.bypass_permissions && turn.network_access {
        params["sandboxPolicy"] = json!({ "type": "dangerFullAccess" });
    } else {
        let roots: Vec<String> = crate::writable_dirs(turn)
            .map(|d| d.to_string_lossy().into_owned())
            .collect();
        let environments = loopback_environments(turn);
        // Codex's seccomp would also block loopback; bwrap's namespace and socket
        // filter deny egress instead. A gated turn reaches hosts only by escalation.
        params["sandboxPolicy"] = json!({
            "type": "workspaceWrite",
            "writableRoots": roots,
            "networkAccess": (turn.network_access && !network_gated(turn)) || environments.is_some(),
        });
        // Resumed threads forget sticky environments, so every turn selects it. This
        // experimental field is checked by `thread/read` after turn/start; see LOOPBACK_MIN_VERSION.
        if let Some(environments) = environments {
            params["environments"] = environments;
        }
    }
    params
}

/// Whether the turn's commands run in the loopback-only sandbox. Elsewhere,
/// network-denied turns keep Codex's own sandbox, which also denies loopback.
fn loopback_sandboxed(turn: &EngineTurn) -> bool {
    cfg!(target_os = "linux") && !turn.network_access && !turn.summary_only
}

/// Whether the agent's network is behind approvals: Codex runs commands
/// without network, so reaching a host takes an escalation the runtime gates.
fn network_gated(turn: &EngineTurn) -> bool {
    turn.network_access && !turn.bypass_permissions && !turn.summary_only
}

/// Whether the latest network note among `thread/resume`'s chronological turns is
/// an offline or gated one. Missing history counts as yes: the correction is true on any allowed turn.
fn loopback_note_in_effect(thread: &Value) -> bool {
    let Some(turns) = thread["turns"].as_array() else {
        return true;
    };
    let mut notes = turns
        .iter()
        .filter_map(|turn| turn["items"].as_array())
        .flatten()
        .filter(|item| item["type"] == "userMessage")
        .filter_map(|item| item["content"].as_array())
        .flatten()
        .filter_map(|input| input["text"].as_str())
        .filter(|text| {
            [
                LOOPBACK_INSTRUCTIONS,
                GATED_NETWORK_INSTRUCTIONS,
                NETWORK_RESTORED_INSTRUCTIONS,
            ]
            .contains(text)
        });
    notes
        .next_back()
        .is_some_and(|note| note != NETWORK_RESTORED_INSTRUCTIONS)
}

/// The sole environment a loopback-sandboxed turn may run commands in.
fn loopback_environments(turn: &EngineTurn) -> Option<Value> {
    loopback_sandboxed(turn).then(|| {
        json!([{
            "environmentId": loopback::ENVIRONMENT_ID,
            "cwd": turn.cwd.to_string_lossy(),
        }])
    })
}

#[async_trait::async_trait]
impl Engine for CodexAppEngine {
    fn kind(&self) -> EngineKind {
        EngineKind::CODEX
    }

    fn adapter(&self) -> String {
        format!("codex-app/{}", self.version)
    }

    fn validate_network_policy(&self, network_access: bool) -> Result<(), EngineError> {
        if network_access || !cfg!(target_os = "linux") {
            return Ok(());
        }
        let path = std::env::var_os("PATH");
        self.validate_loopback_support(|program| {
            crate::offline::executable_on_path(program, path.as_deref())
        })
    }

    async fn read_usage(
        &self,
        hidden_env: &[String],
        cancel: CancellationToken,
    ) -> Result<Option<Value>, EngineError> {
        read_usage::read(&self.program, hidden_env, cancel)
            .await
            .map(Some)
    }

    async fn run(
        &self,
        turn: EngineTurn,
        cancel: CancellationToken,
    ) -> Result<EngineRun, EngineError> {
        use std::os::unix::process::CommandExt as _;

        let sandbox = if loopback_sandboxed(&turn) {
            self.validate_network_policy(false)?;
            Some(
                loopback::start(
                    &self.program,
                    &turn.cwd,
                    &turn.tool_hidden_env,
                    turn.temp_dir.as_deref(),
                    &turn.build_env,
                )
                .await?,
            )
        } else {
            None
        };
        let environment = sandbox.as_ref().map(|s| s.environment.clone());
        let args = self.args(&turn);
        let cwd: PathBuf = turn.cwd.clone();
        let mut command = tokio::process::Command::new(&self.program);
        command
            .args(&args)
            .current_dir(&cwd)
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped())
            .kill_on_drop(true);
        command.as_std_mut().process_group(0);
        aether_spawn::reset_signals(command.as_std_mut());
        crate::strip_hidden(&mut command, &turn.hidden_env);
        crate::export_temp_dir(&mut command, turn.temp_dir.as_deref());
        crate::export_build_env(&mut command, &turn.build_env);
        let run_token = crate::orphans::tag(&mut command);
        let mut child = command.spawn().map_err(|e| match e.kind() {
            std::io::ErrorKind::NotFound => EngineError::NotInstalled {
                program: self.program.clone(),
            },
            _ => EngineError::Spawn {
                program: self.program.clone(),
                source: e,
            },
        })?;
        let engine_record =
            crate::orphans::EngineRecord::register(child.id(), run_token.as_deref());
        let stdin = child
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
        let (events_tx, events_rx) = mpsc::channel(256);
        let (input_tx, input_rx) = mpsc::channel::<crate::EngineInput>(16);
        let (decision_tx, decision_rx) = mpsc::channel::<PermissionDecision>(16);
        let (stop_tx, stop_rx) = mpsc::channel::<Duration>(1);
        let stderr_task = tokio::spawn(crate::output::drain_stderr_until(
            stderr,
            CancellationToken::new(),
            turn.redactor.clone(),
        ));
        let version = EngineVersion::parse(&self.version);
        tokio::spawn(async move {
            let _engine_record = engine_record;
            let mut sandbox = sandbox;
            let sandbox_exit = async {
                match sandbox.as_mut() {
                    Some(sandbox) => sandbox.exited().await,
                    None => std::future::pending().await,
                }
            };
            let mut driver = Driver::new(stdin, events_tx, turn, environment, version);
            driver
                .drive(
                    child,
                    stdout,
                    input_rx,
                    decision_rx,
                    stop_rx,
                    cancel,
                    stderr_task,
                    sandbox_exit,
                )
                .await;
            drop(sandbox);
        });
        Ok(EngineRun {
            events: events_rx,
            input: Some(input_tx),
            decisions: Some(decision_tx),
            stop: Some(stop_tx),
        })
    }
}

fn valid_env_filter_name(name: &str) -> bool {
    !name.is_empty()
        && name
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || b"_-".contains(&byte))
}
