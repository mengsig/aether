use std::time::Duration;

use super::*;
use crate::testing::{Observed, WAIT_CEILING, ctx_in, wait_for};

fn outcome(exec: ToolExecution) -> ToolOutcome {
    match exec {
        ToolExecution::Outcome(o) => o,
        other => panic!("expected an outcome, got {other:?}"),
    }
}

fn status(reg: &BackgroundProcesses, ctx: &ToolContext, id: ProcessId) -> ProcessStatus {
    reg.list(ctx.agent_id)
        .expect("list")
        .into_iter()
        .find(|s| s.id == id)
        .expect("the process is still registered")
}

/// Wait for `marker` in the process's log, reading the file directly: a
/// registry read would consume the output the assertions still need.
async fn wait_for_output(
    reg: &BackgroundProcesses,
    ctx: &ToolContext,
    id: ProcessId,
    marker: &str,
) {
    let path = status(reg, ctx, id).output_path;
    wait_for(&format!("`{marker}` in the terminal output"), || {
        let log = match std::fs::read(&path) {
            Ok(log) => log,
            Err(e) => {
                return Observed::Impossible(format!("reading {}: {e}", path.display()));
            }
        };
        if log.windows(marker.len()).any(|w| w == marker.as_bytes()) {
            return Observed::Held;
        }
        let seen = format!(
            "{} bytes of output: {:?}",
            log.len(),
            String::from_utf8_lossy(&log)
        );
        let status = status(reg, ctx, id);
        if let Some(error) = status.output_error {
            // Capture failure stops the reader for good, so the marker can
            // no longer arrive however long we wait.
            return Observed::Impossible(format!("the output log failed: {error}; {seen}"));
        }
        if !status.running {
            return Observed::Impossible(format!(
                "the shell exited (code {:?}) without printing it; {seen}",
                status.exit_code
            ));
        }
        Observed::Pending(seen)
    })
    .await;
}

/// Await the process's own reap signal: the exit is observed when it
/// happens, and the log is fully drained by then.
async fn wait_for_exit(reg: &BackgroundProcesses, ctx: &ToolContext, id: ProcessId) {
    let mut reaped = reg.reaped(ctx.agent_id, id).expect("process is registered");
    match tokio::time::timeout(WAIT_CEILING, reaped.wait_for(|done| *done)).await {
        Ok(Ok(_)) => {}
        Ok(Err(e)) => panic!("the task owning the process ended without reaping it: {e}"),
        Err(_) => {
            let processes = reg.list(ctx.agent_id);
            // The tail says whether the shell reacted at all: a terminal
            // echoes `^C` before the signal a stuck shell never handled.
            let tail = processes
                .as_ref()
                .ok()
                .and_then(|all| all.iter().find(|s| s.id == id))
                .map(|s| tail_of(&s.output_path))
                .unwrap_or_else(|| "unknown: the process is gone".into());
            panic!(
                "the process did not exit within {WAIT_CEILING:?}; \
                 agent's processes {processes:?}; output tail {tail:?}"
            )
        }
    }
}

fn tail_of(path: &std::path::Path) -> String {
    match std::fs::read(path) {
        Ok(log) => String::from_utf8_lossy(&log[log.len().saturating_sub(200)..]).into_owned(),
        Err(e) => format!("unreadable: {e}"),
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn output_is_read_once_then_the_process_is_stopped() {
    let dir = tempfile::tempdir().unwrap();
    let ctx = ctx_in(dir.path());
    let processes = Arc::new(BackgroundProcesses::new());
    let started = processes
        .start(ctx.agent_id, "echo hello; sleep 60", &ctx)
        .expect("start");
    let handle = json!({"process_id": started.id.to_string()});

    wait_for("output from the process", || {
        let status = status(&processes, &ctx, started.id);
        if status.pending_bytes > 0 {
            return Observed::Held;
        }
        if let Some(error) = status.output_error {
            return Observed::Impossible(format!("the output log failed: {error}"));
        }
        Observed::Pending(format!("nothing captured yet; running {}", status.running))
    })
    .await;
    let read = ShellOutputTool::new(processes.clone());
    let first = outcome(
        read.execute(handle.clone(), &ctx, CancellationToken::new())
            .await
            .expect("read"),
    );
    assert!(first.content.contains("hello"), "{}", first.content);
    assert_eq!(first.structured.as_ref().unwrap()["running"], true);
    let again = outcome(
        read.execute(handle.clone(), &ctx, CancellationToken::new())
            .await
            .expect("read again"),
    );
    assert!(
        again.content.starts_with("(no new output)"),
        "output already read is not returned again: {}",
        again.content
    );

    let list = ShellListTool::new(processes.clone());
    let listed = outcome(
        list.execute(json!({}), &ctx, CancellationToken::new())
            .await
            .expect("list"),
    );
    assert!(listed.content.contains(&started.id.to_string()));

    let stop = ShellStopTool::new(processes.clone());
    let stopped = outcome(
        stop.execute(handle.clone(), &ctx, CancellationToken::new())
            .await
            .expect("stop"),
    );
    assert!(stopped.content.contains("stopped"), "{}", stopped.content);
    assert_eq!(stopped.side_effect_occurred, Some(true));
    // Gone: a second stop cannot find it, and the listing is empty.
    assert!(
        stop.execute(handle, &ctx, CancellationToken::new())
            .await
            .is_err()
    );
    assert!(processes.list(ctx.agent_id).unwrap().is_empty());
}

#[tokio::test(flavor = "multi_thread")]
async fn a_handle_names_the_process_it_serializes_on() {
    let id = ProcessId::new();
    let keys = resource_of(&json!({"process_id": id.to_string()}));
    assert_eq!(keys, vec![process_resource(id)]);
    assert!(resource_of(&json!({})).is_empty());
    assert!(resource_of(&json!({"process_id": "not-an-id"})).is_empty());
}

#[tokio::test(flavor = "multi_thread")]
async fn a_bad_handle_is_refused_rather_than_guessed() {
    let dir = tempfile::tempdir().unwrap();
    let ctx = ctx_in(dir.path());
    let processes = Arc::new(BackgroundProcesses::new());
    let read = ShellOutputTool::new(processes.clone());
    assert!(matches!(
        read.execute(
            json!({"process_id": "nope"}),
            &ctx,
            CancellationToken::new()
        )
        .await,
        Err(ToolError::InvalidInput(_))
    ));
    // A well-formed handle for a process this agent does not own reads
    // the same as one that never existed.
    let stranger = ProcessId::new();
    assert!(
        read.execute(
            json!({"process_id": stranger.to_string()}),
            &ctx,
            CancellationToken::new()
        )
        .await
        .is_err()
    );
}

#[test]
fn an_empty_listing_says_so() {
    // The exact wording the e2e greps for as the failure signal.
    assert_eq!(render(&[]), "no background processes");
}
#[tokio::test(flavor = "multi_thread")]
async fn terminal_accepts_input_and_enforces_ownership() {
    use crate::builtin::bash::BashTool;
    let dir = tempfile::tempdir().unwrap();
    let ctx = ctx_in(dir.path());
    let registry = Arc::new(BackgroundProcesses::new());
    let bash = BashTool::new(registry.clone());
    let started = outcome(bash.execute(json!({"command":"test -t 0 && test -t 1 && printf 'READY\n'; read -r answer; printf 'ANSWER=%s\n' \"$answer\"", "tty":true}), &ctx, CancellationToken::new()).await.unwrap());
    let id: ProcessId =
        serde_json::from_value(started.structured.as_ref().unwrap()["process_id"].clone()).unwrap();
    let path = started.structured.as_ref().unwrap()["output_path"]
        .as_str()
        .unwrap()
        .to_owned();
    wait_for_output(&registry, &ctx, id, "READY").await;
    let input = WriteStdinTool::new(registry.clone());
    let foreign = ctx_in(dir.path());
    assert!(
        input
            .execute(
                json!({"process_id":id,"chars":"bad\n"}),
                &foreign,
                CancellationToken::new()
            )
            .await
            .is_err()
    );
    let sent = outcome(
        input
            .execute(
                json!({"process_id":id,"chars":"hello🦀\n"}),
                &ctx,
                CancellationToken::new(),
            )
            .await
            .unwrap(),
    );
    assert!(!sent.is_error);
    wait_for_exit(&registry, &ctx, id).await;
    let out = registry.read(ctx.agent_id, id).unwrap();
    assert!(out.text.contains("ANSWER=hello🦀"), "{}", out.text);
    assert_eq!(out.status.exit_code, Some(0));
    assert!(
        out.status.output_error.is_none(),
        "{:?}",
        out.status.output_error
    );
    assert!(
        input
            .execute(
                json!({"process_id":id,"chars":"late\n"}),
                &ctx,
                CancellationToken::new()
            )
            .await
            .is_err()
    );
    registry.stop(ctx.agent_id, id).await.unwrap();
    assert!(
        std::fs::read(path)
            .unwrap()
            .windows("ANSWER=hello🦀".len())
            .any(|w| w == "ANSWER=hello🦀".as_bytes())
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn terminal_stop_kills_the_session_and_pipe_processes_reject_input() {
    let dir = tempfile::tempdir().unwrap();
    let ctx = ctx_in(dir.path());
    let reg = Arc::new(BackgroundProcesses::new());
    let terminal = reg
        .start_with_terminal(
            ctx.agent_id,
            "echo $$ > terminal.pid.tmp && mv terminal.pid.tmp terminal.pid; read -r answer",
            &ctx,
            true,
        )
        .unwrap();
    let pid_path = dir.path().join("terminal.pid");
    wait_for("the terminal to report its pid", || {
        if pid_path.exists() {
            return Observed::Held;
        }
        let status = status(&reg, &ctx, terminal.id);
        if !status.running {
            return Observed::Impossible(format!(
                "the shell exited (code {:?}) without writing the pid file",
                status.exit_code
            ));
        }
        Observed::Pending("no pid file yet".into())
    })
    .await;
    let pid: i32 = std::fs::read_to_string(dir.path().join("terminal.pid"))
        .unwrap()
        .trim()
        .parse()
        .unwrap();
    reg.stop(ctx.agent_id, terminal.id).await.unwrap();
    assert!(nix::sys::signal::kill(nix::unistd::Pid::from_raw(pid), None).is_err());
    let pipe = reg.start(ctx.agent_id, "sleep 60", &ctx).unwrap();
    assert!(
        reg.write_stdin(ctx.agent_id, pipe.id, "input", CancellationToken::new())
            .await
            .is_err()
    );
    reg.stop_all().await;
}
#[tokio::test(flavor = "multi_thread")]
async fn terminal_control_input_and_hidden_environment_are_preserved() {
    aether_spawn::ignore_sigint().unwrap();
    let dir = tempfile::tempdir().unwrap();
    let mut ctx = ctx_in(dir.path());
    ctx.env
        .insert("AETHER_PTY_SECRET".into(), "hidden-test".into());
    ctx.hidden_env = vec!["AETHER_PTY_SECRET".into()].into();
    let reg = BackgroundProcesses::new();
    let p = reg.start_with_terminal(ctx.agent_id,
        r#"test -z "${AETHER_PTY_SECRET+x}" || exit 9; trap 'printf INTERRUPTED; exit 0' INT; printf ENV_HIDDEN_READY; while :; do read -r answer; done"#,
        &ctx, true).unwrap();
    wait_for_output(&reg, &ctx, p.id, "ENV_HIDDEN_READY").await;
    reg.write_stdin(ctx.agent_id, p.id, "\u{3}", CancellationToken::new())
        .await
        .unwrap();
    wait_for_exit(&reg, &ctx, p.id).await;
    let out = reg.read(ctx.agent_id, p.id).unwrap();
    assert!(out.text.contains("INTERRUPTED"), "{}", out.text);
    assert_eq!(out.status.exit_code, Some(0));
    reg.stop(ctx.agent_id, p.id).await.unwrap();
}

#[tokio::test(flavor = "multi_thread")]
async fn a_job_stops_on_group_sigint_although_the_daemon_ignores_it() {
    aether_spawn::ignore_sigint().unwrap();
    let dir = tempfile::tempdir().unwrap();
    let ctx = ctx_in(dir.path());
    let reg = BackgroundProcesses::new();
    // Only builtins before the exec: bash takes a SIGINT that lands while it
    // waits for a child which then exits normally as handled, and runs on.
    let p = reg
        .start(ctx.agent_id, "echo $$ > leader; exec sleep 60", &ctx)
        .unwrap();
    let pid_file = dir.path().join("leader");
    wait_for(
        "the job to record its pid",
        || match std::fs::read_to_string(&pid_file) {
            Ok(line) if line.ends_with('\n') => Observed::Held,
            Ok(line) => Observed::Pending(format!("a partial pid line {line:?}")),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                Observed::Pending("no pid file yet".into())
            }
            Err(e) => Observed::Impossible(format!("reading the pid file: {e}")),
        },
    )
    .await;
    let leader: i32 = std::fs::read_to_string(&pid_file)
        .unwrap()
        .trim()
        .parse()
        .unwrap();
    assert!(leader > 1, "a group id of {leader} would signal other jobs");
    nix::sys::signal::killpg(
        nix::unistd::Pid::from_raw(leader),
        nix::sys::signal::Signal::SIGINT,
    )
    .unwrap();
    let mut reaped = reg.reaped(ctx.agent_id, p.id).unwrap();
    let exited = tokio::time::timeout(Duration::from_secs(10), reaped.wait_for(|done| *done)).await;
    reg.stop_all().await;
    assert!(exited.is_ok(), "SIGINT to the job's group did not stop it");
}
