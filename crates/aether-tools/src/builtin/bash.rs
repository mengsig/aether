//! Shell execution in the agent's workspace with process-group cleanup.

use std::sync::Arc;
use std::time::Duration;

use aether_core::agent::NetworkPolicy;
use aether_core::tool::{ResourceKey, SideEffectClass, ToolDescriptor, ToolOutcome};
use async_trait::async_trait;
use nix::sys::signal::{Signal, killpg};
use nix::unistd::Pid;
use serde::Deserialize;
use serde_json::{Value, json};
use tokio_util::sync::CancellationToken;

use crate::background::BackgroundProcesses;
use crate::capture::{Capture, FOREGROUND_CAPTURE_BYTES, pump};
use crate::exec::{Teardown, prepare};
use crate::output::OutputPolicy;
use crate::tool::{Tool, ToolContext, ToolError, ToolExecution, parse_input};

/// How long to wait for pipes to close after the shell exits before killing strays.
const PIPE_GRACE: Duration = Duration::from_millis(1000);

/// What a shell call touches: the tree it runs in, and the network when the
/// scope allows it. Shared with the engine gate so both derive the same keys.
pub fn shell_resources(ctx: &ToolContext) -> Vec<ResourceKey> {
    // Empty repo paths contain every tree; saved rules may name the raw cwd.
    let mut keys: Vec<_> = crate::fs::resource_paths(ctx, ".")
        .into_iter()
        .filter(|path| !path.as_os_str().is_empty())
        .map(ResourceKey::repo)
        .collect();
    let declared = ResourceKey::repo(&ctx.cwd);
    if !keys.contains(&declared) {
        keys.insert(0, declared);
    }
    if ctx.scope.network == NetworkPolicy::Allowed {
        keys.push(ResourceKey::network());
    }
    keys
}

#[derive(Debug)]
pub struct BashTool {
    pub output: OutputPolicy,
    /// Most bytes of one foreground command's output kept in its log and artifact.
    pub capture_limit: u64,
    processes: Arc<BackgroundProcesses>,
}

impl BashTool {
    pub fn new(processes: Arc<BackgroundProcesses>) -> Self {
        Self {
            output: OutputPolicy::default(),
            capture_limit: FOREGROUND_CAPTURE_BYTES,
            processes,
        }
    }
}

#[derive(Debug, Deserialize)]
struct Input {
    command: String,
    /// Start the command and return a handle instead of waiting for it.
    #[serde(default)]
    background: bool,
    #[serde(default)]
    tty: bool,
}

/// Whether the call may run alongside another agent's work in the same
/// workspace: the caller declared `read_only`, or the command is plainly a
/// reader. The declaration relaxes mutual exclusion only; approval and retry
/// policy keep using the tool's own side-effect class.
fn is_read_only(input: &Value) -> bool {
    let declared = input.get("read_only").and_then(Value::as_bool) == Some(true);
    let command = input.get("command").and_then(Value::as_str).unwrap_or("");
    declared || super::read_only::is_read_only_command(command)
}

/// Kills the whole process group when dropped, so a timeout or cancellation
/// that drops the execution future never leaks children. A container is not
/// in that group, so it is torn down by name as well.
struct GroupGuard {
    pgid: Option<Pid>,
    teardown: Option<Teardown>,
}

impl GroupGuard {
    /// The command finished on its own, so there is no container left to
    /// kill; forget it rather than spawning a `docker kill` for a container
    /// that `--rm` has already removed.
    fn disarm_teardown(&mut self) {
        self.teardown = None;
    }

    /// Kills once: the teardown is taken, so a `Drop` after an explicit kill
    /// does not spawn a second `docker kill` for a container that is already
    /// gone. Signalling the group again is harmless and stays unconditional.
    fn kill(&mut self) {
        if let Some(pid) = self.pgid.take() {
            // ESRCH means the group is already gone; nothing else is actionable here.
            let _ = killpg(pid, Signal::SIGKILL);
        }
        if let Some(teardown) = self.teardown.take() {
            teardown.run();
        }
    }
}

impl Drop for GroupGuard {
    fn drop(&mut self) {
        self.kill();
    }
}

#[async_trait]
impl Tool for BashTool {
    fn descriptor(&self) -> ToolDescriptor {
        super::descriptor(
            "bash",
            "Run a shell command in the workspace with bash. Returns combined stdout and stderr \
             and the exit code. A foreground command runs in a fresh process group that is \
             killed when the call ends, so stray children never outlive it. With background: \
             true you get a handle; descendants are stopped when the command leader exits: poll it with `shell_output`, \
             end it with `shell_stop`. tty: true starts an interactive terminal session; send input with write_stdin. Output is saved in the returned log path, readable with read_file or shell commands. Background logs retain up to 8 MiB; stopped logs remain readable until eviction or agent end. Background processes are killed when the agent ends. \
             Where the command runs - this machine or a container - is the agent's scope, \
             not something the command can change.",
            json!({
                "type": "object",
                "properties": {
                    "command": {"type": "string", "description": "The command to run."},
                    "read_only": {
                        "type": "boolean",
                        "description": "True if the command only reads. A read-only call runs \
                                        alongside other agents in the same workspace instead of \
                                        waiting for exclusive access to it."
                    },
                    "tty": {"type": "boolean", "default": false, "description": "Allocate an interactive terminal and return a process handle. Implies background; use write_stdin for input."},
                    "background": {
                        "type": "boolean",
                        "description": "Keep it running and return a handle instead of waiting."
                    }
                },
                "required": ["command"],
                "additionalProperties": false
            }),
            SideEffectClass::Mutation,
            false,
        )
    }

    fn leases_resources(&self, input: &Value) -> bool {
        !is_read_only(input)
    }

    /// The command, and - when it is detached - that it is. A rule
    /// remembered from a foreground `npm test` must not also allow the same
    /// command as a process that outlives the turn.
    fn rule_argument(&self, input: &Value) -> Option<String> {
        let command = input["command"].as_str()?;
        if input["tty"].as_bool() == Some(true) {
            return Some(format!("{command} (interactive terminal)"));
        }
        Some(match input["background"].as_bool() {
            Some(true) => format!("{command} (background)"),
            _ => command.to_owned(),
        })
    }

    fn resources(&self, _input: &Value, ctx: &ToolContext) -> Vec<ResourceKey> {
        shell_resources(ctx)
    }

    async fn execute(
        &self,
        input: Value,
        ctx: &ToolContext,
        cancel: CancellationToken,
    ) -> Result<ToolExecution, ToolError> {
        let Input {
            command,
            background,
            tty,
        } = parse_input(input)?;
        if command.trim().is_empty() {
            return Err(ToolError::InvalidInput("command is empty".into()));
        }
        if background || tty {
            let status = self
                .processes
                .start_with_terminal(ctx.agent_id, &command, ctx, tty)?;
            return Ok(ToolExecution::Outcome(ToolOutcome {
                content: format!(
                    "started {} in the background · read it with shell_output, end it with \
                     shell_stop\n{}",
                    status.id,
                    crate::builtin::shell_bg::render(std::slice::from_ref(&status))
                ),
                structured: Some(
                    json!({ "process_id": status.id.to_string(), "output_path": status.output_path, "tty": status.tty }),
                ),
                side_effect_occurred: Some(true),
                ..Default::default()
            }));
        }
        let prepared = prepare(&command, ctx)?;
        let mut cmd = prepared.command;
        let capture = Capture::new(ctx, self.capture_limit)?;
        let mut child = cmd.spawn()?;
        let mut guard = GroupGuard {
            pgid: child.id().map(|p| Pid::from_raw(p as i32)),
            teardown: prepared.teardown,
        };

        let mut readers = Vec::with_capacity(2);
        if let Some(out) = child.stdout.take() {
            readers.push(tokio::spawn(pump(out, capture.clone(), false)));
        }
        if let Some(err) = child.stderr.take() {
            readers.push(tokio::spawn(pump(err, capture.clone(), false)));
        }

        let status = tokio::select! {
            status = child.wait() => {
                let status = status?;
                guard.disarm_teardown();
                status
            }
            _ = cancel.cancelled() => {
                guard.kill();
                let _ = child.wait().await;
                for reader in &readers { reader.abort(); }
                return Err(ToolError::Cancelled);
            }
            _ = capture.failed.cancelled() => {
                guard.kill();
                child.wait().await?
            }
        };
        // Grandchildren holding the pipes open would block readers forever; kill them.
        let drained = tokio::time::timeout(PIPE_GRACE, async {
            for r in &mut readers {
                if let Err(e) = r.await {
                    capture.fail(format!("output reader failed: {e}"));
                }
            }
        })
        .await;
        if drained.is_err() {
            guard.kill();
            for reader in &readers {
                reader.abort();
            }
            capture.fail("output pipes did not close after process exit; log may be incomplete");
        }
        // Finish process-group cleanup before potentially slow artifact storage.
        guard.kill();
        let (mut text, end) = capture.page(0, true)?;
        let output_error = capture.error()?;
        if let Some(error) = &output_error {
            text.push_str(&format!("\n[output capture failed: {error}]"));
        }
        if text.trim().is_empty() {
            text = "(no output)".into();
        }
        let exit_code = status.code();
        match exit_code {
            Some(0) => {}
            Some(code) => text.push_str(&format!("\n[exit code: {code}]")),
            None => text.push_str("\n[terminated by signal]"),
        }
        text.push_str(&format!(
            "\n[output log: {}; {} bytes; read ranges with read_file or bash]",
            capture.path.display(),
            capture.size()
        ));
        let mut artifacts = Vec::new();
        if end < capture.size() {
            let artifact = ctx
                .artifacts
                .store_file(
                    aether_core::artifact::ArtifactKind::CommandOutput,
                    "bash output",
                    "Full shell output",
                    Some("text/plain"),
                    &capture.path,
                    ctx.agent_id,
                    ctx.task_id,
                )
                .await?;
            text.push_str(&format!(
                "\n[full output stored as artifact {}; use read_artifact for pages]",
                artifact.id
            ));
            artifacts.push(artifact.id);
        }
        let bounded = self
            .output
            .bound_plain(&text, "read the output log for more");
        Ok(ToolExecution::Outcome(ToolOutcome {
            content: bounded.text,
            is_error: !status.success() || output_error.is_some(),
            artifacts,
            structured: Some(
                json!({ "exit_code": exit_code, "output_path": capture.path, "output_bytes": capture.size(), "output_error": output_error }),
            ),
            duration_ms: 0,
            truncated: bounded.truncated || end < capture.size(),
            side_effect_occurred: Some(true),
        }))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing::{Observed, ctx_in, wait_for};
    use aether_core::id::ToolCallId;
    use aether_core::tool::ToolCall;

    async fn run(
        ctx: &ToolContext,
        command: &str,
        cancel: CancellationToken,
    ) -> Result<ToolOutcome, ToolError> {
        match BashTool::new(Arc::new(BackgroundProcesses::new()))
            .execute(json!({"command": command}), ctx, cancel)
            .await?
        {
            ToolExecution::Outcome(o) => Ok(o),
            ToolExecution::Action(_) => Err(ToolError::Internal("unexpected action".into())),
        }
    }

    #[tokio::test]
    async fn captures_output_and_exit_code() {
        let dir = tempfile::tempdir().unwrap();
        let ctx = ctx_in(dir.path());
        let out = run(
            &ctx,
            "echo hello; echo err 1>&2; exit 3",
            CancellationToken::new(),
        )
        .await
        .unwrap();
        assert!(out.content.contains("hello"));
        assert!(out.content.contains("err"));
        assert!(out.content.contains("[exit code: 3]"));
        assert!(out.is_error);
        assert_eq!(out.structured.as_ref().unwrap()["exit_code"], 3);
        assert_eq!(out.side_effect_occurred, Some(true));
    }

    #[tokio::test]
    async fn runs_in_workspace_with_env() {
        let dir = tempfile::tempdir().unwrap();
        let mut ctx = ctx_in(dir.path());
        ctx.env.insert("AETHER_TEST_VAR".into(), "42".into());
        let out = run(&ctx, "pwd; echo $AETHER_TEST_VAR", CancellationToken::new())
            .await
            .unwrap();
        assert!(
            out.content
                .contains(&dir.path().canonicalize().unwrap().display().to_string())
        );
        assert!(out.content.contains("42"));
        assert!(!out.is_error);
    }

    #[tokio::test]
    async fn cancellation_kills_the_child() {
        let dir = tempfile::tempdir().unwrap();
        let ctx = ctx_in(dir.path());
        let marker = dir.path().join("pid");
        let cancel = CancellationToken::new();
        let marker_tmp = marker.with_extension("tmp");
        let cmd = format!(
            "echo $$ > {} && mv {} {}; sleep 30",
            marker_tmp.display(),
            marker_tmp.display(),
            marker.display()
        );
        let running = {
            let ctx = ctx.clone();
            let cancel = cancel.clone();
            tokio::spawn(async move { run(&ctx, &cmd, cancel).await })
        };
        // A login shell can take a while to start; cancel only once the shell is up.
        wait_for("the shell to report its pid", || {
            if marker.exists() {
                Observed::Held
            } else {
                Observed::Pending("no pid file yet".into())
            }
        })
        .await;
        cancel.cancel();
        let err = running.await.unwrap().unwrap_err();
        assert!(matches!(err, ToolError::Cancelled));
        let pid: i32 = std::fs::read_to_string(&marker)
            .unwrap()
            .trim()
            .parse()
            .unwrap();
        wait_for("the cancelled shell to die", || {
            if process_alive(pid) {
                Observed::Pending(format!("shell {pid} is still alive"))
            } else {
                Observed::Held
            }
        })
        .await;
    }

    #[tokio::test]
    async fn background_children_die_with_the_command() {
        let dir = tempfile::tempdir().unwrap();
        let ctx = ctx_in(dir.path());
        let marker = dir.path().join("child_pid");
        let cmd = format!(
            "(sleep 30 & echo $! > {}; wait) & sleep 0.1",
            marker.display()
        );
        let _ = run(&ctx, &cmd, CancellationToken::new()).await.unwrap();
        tokio::time::sleep(Duration::from_millis(1200)).await;
        let pid: i32 = std::fs::read_to_string(&marker)
            .unwrap()
            .trim()
            .parse()
            .unwrap();
        assert!(!process_alive(pid), "background sleep {pid} should be dead");
    }

    #[tokio::test]
    async fn empty_command_is_invalid() {
        let dir = tempfile::tempdir().unwrap();
        let ctx = ctx_in(dir.path());
        assert!(matches!(
            run(&ctx, "   ", CancellationToken::new()).await,
            Err(ToolError::InvalidInput(_))
        ));
    }

    fn process_alive(pid: i32) -> bool {
        std::path::Path::new(&format!("/proc/{pid}")).exists()
            && std::fs::read_to_string(format!("/proc/{pid}/stat"))
                .map(|s| !s.contains(") Z "))
                .unwrap_or(false)
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn a_command_cannot_read_a_variable_the_daemon_hides() {
        // The search credential lives in the daemon's environment, and bash
        // inherits it unless it is stripped. HOME stands in for it: the test
        // process has it and a login shell does not put it back, so seeing
        // it absent in the child is a real result. (Setting a variable is
        // `unsafe`, which the workspace forbids.)
        let dir = tempfile::tempdir().unwrap();
        let mut ctx = crate::testing::ctx_in(dir.path());
        assert!(std::env::var("HOME").is_ok(), "the test needs HOME set");
        ctx.hidden_env = vec!["HOME".to_owned()].into();
        let o = run(
            &ctx,
            "printenv HOME || echo HIDDEN",
            CancellationToken::new(),
        )
        .await
        .expect("run");
        assert!(
            o.content.contains("HIDDEN"),
            "the hidden variable must not reach the child: {}",
            o.content
        );

        let background = Arc::new(crate::background::BackgroundProcesses::new());
        let started = background
            .start(ctx.agent_id, "printenv HOME || echo HIDDEN", &ctx)
            .expect("start");
        let deadline = std::time::Instant::now() + Duration::from_secs(10);
        let mut seen = String::new();
        while std::time::Instant::now() < deadline {
            seen.push_str(&background.read(ctx.agent_id, started.id).unwrap().text);
            if seen.contains("HIDDEN") {
                break;
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
        assert!(
            seen.contains("HIDDEN"),
            "a background command must not carry it either: {seen}"
        );
        background.stop_all().await;
    }

    /// Where phase 1's read-only declaration meets phase 2's keying: the
    /// declaration relaxes mutual exclusion and nothing else. `bash` is a
    /// mutating tool whatever a given call does, so every shell call is
    /// still keyed by its class.
    #[test]
    fn a_read_only_shell_call_skips_the_lease_and_is_still_keyed_by_its_class() {
        let dir = tempfile::tempdir().unwrap();
        let ctx = ctx_in(dir.path());
        let tool = BashTool::new(Arc::new(BackgroundProcesses::new()));
        let keyed = |input: &Value| {
            let mut call = ToolCall {
                id: ToolCallId::new(),
                provider_call_id: "p".into(),
                name: "bash".into(),
                input: input.clone(),
                agent_id: ctx.agent_id,
                task_id: None,
                invocation_id: None,
                resources: Vec::new(),
                idempotency_key: None,
                requested_at: aether_core::time::now(),
            };
            crate::ToolRuntime::prepare(&mut call, &tool, &ctx);
            call.idempotency_key.is_some()
        };

        let reader = json!({"command": "ls -la"});
        assert!(!tool.leases_resources(&reader), "a reader takes no key");
        assert!(
            keyed(&reader),
            "skipping the lease must not skip the keying"
        );

        let writer = json!({"command": "cargo build"});
        assert!(tool.leases_resources(&writer));
        assert!(keyed(&writer));

        // A detached command takes the workspace key for its start only: the
        // lease ends with the call and the process does not (ARCHITECTURE
        // §12.3), so it is queued behind a holder but never fences one out.
        assert!(tool.leases_resources(&json!({"command": "cargo build", "background": true})));
    }
    #[tokio::test]
    async fn foreground_output_past_the_cap_is_neither_kept_nor_stored_beyond_it() {
        let dir = tempfile::tempdir().unwrap();
        let sink = Arc::new(crate::testing::MemorySink::default());
        let mut ctx = ctx_in(dir.path());
        ctx.artifacts = sink.clone();
        let mut tool = BashTool::new(Arc::new(BackgroundProcesses::new()));
        tool.capture_limit = 100_000;
        let ToolExecution::Outcome(out) = tool
            .execute(
                json!({"command": "head -c 400000 /dev/zero | tr '\\0' x"}),
                &ctx,
                CancellationToken::new(),
            )
            .await
            .unwrap()
        else {
            panic!("expected an outcome");
        };
        let path = out.structured.as_ref().unwrap()["output_path"]
            .as_str()
            .unwrap();
        assert_eq!(std::fs::metadata(path).unwrap().len(), 100_000);
        let stored = sink.stored();
        assert_eq!(stored.len(), 1);
        assert_eq!(stored[0].1.len(), 100_000);
        assert!(stored[0].1.ends_with(b"capture byte limit reached]\n"));
    }

    #[tokio::test]
    async fn large_output_is_retrievable_after_the_foreground_call() {
        let dir = tempfile::tempdir().unwrap();
        let ctx = ctx_in(dir.path());
        let size = 2 << 20;
        let out = run(
            &ctx,
            &format!("head -c {size} /dev/zero | tr '\\0' x; printf TAIL"),
            CancellationToken::new(),
        )
        .await
        .unwrap();
        assert!(!out.is_error, "{}", out.content);
        assert!(out.truncated);
        assert_eq!(out.artifacts.len(), 1);
        assert!(out.content.len() < 30_000);
        let path = out.structured.as_ref().unwrap()["output_path"]
            .as_str()
            .unwrap();
        let log = std::fs::read(path).unwrap();
        assert_eq!(log.len(), size + 4);
        assert!(log[..size].iter().all(|b| *b == b'x'));
        assert_eq!(&log[size..], b"TAIL");
        use std::os::unix::fs::PermissionsExt;
        assert_eq!(
            std::fs::metadata(path).unwrap().permissions().mode() & 0o777,
            0o600
        );
    }
    #[tokio::test]
    async fn shell_logging_does_not_dirty_the_repository_it_inspects() {
        let dir = tempfile::tempdir().unwrap();
        assert!(
            std::process::Command::new("git")
                .args(["init", "--quiet"])
                .current_dir(dir.path())
                .status()
                .unwrap()
                .success()
        );
        let ctx = ctx_in(dir.path());
        let out = run(&ctx, "git status --porcelain", CancellationToken::new())
            .await
            .unwrap();
        assert!(!out.is_error, "{}", out.content);
        let status = std::process::Command::new("git")
            .args([
                "-c",
                "core.excludesFile=/dev/null",
                "status",
                "--porcelain",
                "--untracked-files=all",
            ])
            .current_dir(dir.path())
            .output()
            .unwrap();
        assert!(status.status.success());
        assert!(
            status.stdout.is_empty(),
            "capturing git status must not add untracked files: {}",
            String::from_utf8_lossy(&status.stdout)
        );
    }
}
