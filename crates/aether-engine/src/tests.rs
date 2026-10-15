use super::*;

/// `Max` is a real Codex level on current CLIs (`codex debug
/// models` lists it for most of Aether's catalog) - every level maps.
#[test]
fn codex_effort_arg_maps_every_level() {
    assert_eq!(codex_effort_arg(Effort::Low), "low");
    assert_eq!(codex_effort_arg(Effort::Medium), "medium");
    assert_eq!(codex_effort_arg(Effort::High), "high");
    assert_eq!(codex_effort_arg(Effort::XHigh), "xhigh");
    assert_eq!(codex_effort_arg(Effort::Max), "max");
}

#[test]
fn session_ids_look_like_uuids() {
    let id = new_session_id();
    assert_eq!(id.len(), 36);
    assert_eq!(
        id.split('-').map(str::len).collect::<Vec<_>>(),
        vec![8, 4, 4, 4, 12]
    );
    assert_ne!(id, new_session_id());
}

/// The daemon's secrets and its own socket must not reach an engine
/// child either: the engine drives its own shell. HOME stands in for the
/// credential - the test process has it, a login shell does not put it
/// back, and setting a variable is `unsafe`, which the workspace forbids.
#[tokio::test(flavor = "multi_thread")]
async fn an_engine_child_cannot_read_a_variable_the_daemon_hides() {
    assert!(std::env::var("HOME").is_ok(), "the test needs HOME set");
    let mut run = run_process(
        "bash",
        vec!["-lc".into(), "printenv".into()],
        PathBuf::from("/"),
        &["HOME".to_owned()],
        None,
        String::new(),
        false,
        None,
        CancellationToken::new(),
        |line| vec![EngineEvent::Note(line.to_owned())],
    )
    .await
    .expect("spawn bash");
    let mut printed = Vec::new();
    while let Some(event) = run.events.recv().await {
        if let EngineEvent::Note(line) = event {
            printed.push(line);
        }
    }
    assert!(
        printed.iter().any(|l| l.starts_with("PATH=")),
        "printenv did not run: {printed:?}"
    );
    assert!(
        !printed.iter().any(|l| l.starts_with("HOME=")),
        "the hidden variable reached the engine child: {printed:?}"
    );
}

/// An engine CLI and everything it starts find the agent's own temp dir,
/// even when the daemon hides its own TMPDIR.
#[tokio::test(flavor = "multi_thread")]
async fn an_engine_child_gets_the_agent_temp_dir() {
    let dir = PathBuf::from("/var/agent-temp/0123456789AB");
    let mut run = run_process(
        "bash",
        vec!["-lc".into(), "printenv".into()],
        PathBuf::from("/"),
        &["TMPDIR".to_owned(), "TEMP".to_owned()],
        Some(&dir),
        String::new(),
        false,
        None,
        CancellationToken::new(),
        |line| vec![EngineEvent::Note(line.to_owned())],
    )
    .await
    .expect("spawn bash");
    let mut printed = Vec::new();
    while let Some(event) = run.events.recv().await {
        if let EngineEvent::Note(line) = event {
            printed.push(line);
        }
    }
    for name in aether_core::agent_temp::ENV_VARS {
        let expected = format!("{name}={}", dir.display());
        assert!(
            printed.contains(&expected),
            "{expected} missing: {printed:?}"
        );
    }
}

/// The escalation is the whole point of the ladder: a group member that
/// ignores TERM has to be killed, and the runtime must still be there to
/// do it. `exec` keeps the ignored disposition, so the `sleep` that
/// survives the shell ignores TERM too.
#[tokio::test(flavor = "multi_thread")]
async fn a_group_that_ignores_term_is_killed_after_the_grace() {
    use std::os::unix::process::{CommandExt as _, ExitStatusExt as _};

    let mut command = tokio::process::Command::new("sh");
    command
        .args(["-c", "trap '' TERM; exec sleep 30"])
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .kill_on_drop(true);
    command.as_std_mut().process_group(0);
    let mut child = command.spawn().expect("spawn sh");
    let pid = child.id();
    // The disposition is set by the shell; signalling before it runs
    // would test nothing.
    tokio::time::sleep(std::time::Duration::from_millis(300)).await;

    let started = std::time::Instant::now();
    kill_group(pid).await;
    let status = tokio::time::timeout(std::time::Duration::from_secs(5), child.wait())
        .await
        .expect("the ladder left the group running")
        .expect("wait");

    assert_eq!(
        status.signal(),
        Some(nix::sys::signal::Signal::SIGKILL as i32),
        "TERM was honoured after all, or the escalation never ran: {status:?}"
    );
    assert!(
        started.elapsed() >= GROUP_TERM_GRACE,
        "the group was killed without being given the TERM grace"
    );
}

/// A cancel is reported when it is decided, not when the group is finally
/// gone: awaiting the ladder first made every cancel - an operator's
/// included - land a whole `GROUP_TERM_GRACE` late.
#[tokio::test(flavor = "multi_thread")]
async fn a_cancel_is_reported_before_the_term_ladder_runs() {
    let cancel = CancellationToken::new();
    let mut run = run_process(
        "sh",
        vec!["-c".into(), "exec sleep 30".into()],
        PathBuf::from("/"),
        &[],
        None,
        String::new(),
        false,
        None,
        cancel.clone(),
        |line| vec![EngineEvent::Note(line.to_owned())],
    )
    .await
    .expect("spawn sh");

    let started = std::time::Instant::now();
    cancel.cancel();
    let event = tokio::time::timeout(GROUP_TERM_GRACE, run.events.recv())
        .await
        .expect("the cancel was reported only after the ladder")
        .expect("the run reports its cancel");
    assert!(
        matches!(&event, EngineEvent::Done(o) if !o.closing_accounting && o.error.as_deref() == Some("cancelled")),
        "{event:?}"
    );
    assert!(
        started.elapsed() < GROUP_TERM_GRACE,
        "the report waited on the ladder: {:?}",
        started.elapsed()
    );
}

#[test]
fn side_effect_classes() {
    use aether_core::tool::SideEffectClass::*;
    assert_eq!(side_effect_of("Read"), ReadOnly);
    assert_eq!(side_effect_of("Edit"), IdempotentMutation);
    assert_eq!(side_effect_of("Bash"), Mutation);
    assert_eq!(side_effect_of("mcp__aether__delegate"), ReadOnly);
}

#[tokio::test]
async fn stdout_read_error_preserves_stderr_and_session_loss_for_retry() {
    let dir = tempfile::tempdir().unwrap();
    let mut run = run_process(
        "bash",
        vec![
            "-c".into(),
            "printf 'session not found: old conversation\\n' >&2; printf '\\377\\n'; sleep 60"
                .into(),
        ],
        dir.path().to_path_buf(),
        &[],
        None,
        String::new(),
        false,
        None,
        CancellationToken::new(),
        |_| Vec::new(),
    )
    .await
    .unwrap();
    let outcome = tokio::time::timeout(std::time::Duration::from_secs(10), async {
        while let Some(event) = run.events.recv().await {
            if let EngineEvent::Done(outcome) = event {
                return outcome;
            }
        }
        panic!("missing Done");
    })
    .await
    .unwrap();
    assert!(outcome.is_error);
    assert!(outcome.session_lost);
    let error = outcome.error.unwrap();
    assert!(error.contains("stdout read error"), "{error}");
    assert!(error.contains("old conversation"), "{error}");
}

#[tokio::test]
async fn process_crash_without_a_result_has_no_closing_accounting() {
    let mut run = run_process(
        "sh",
        vec!["-c".into(), "exit 1".into()],
        PathBuf::from("/"),
        &[],
        None,
        String::new(),
        false,
        None,
        CancellationToken::new(),
        |_| Vec::new(),
    )
    .await
    .expect("spawn sh");
    let Some(EngineEvent::Done(outcome)) = run.events.recv().await else {
        panic!("expected crash outcome");
    };
    assert!(outcome.is_error);
    assert!(
        outcome
            .error
            .as_deref()
            .unwrap()
            .contains("without a result")
    );
    assert!(!outcome.closing_accounting);
    assert_eq!(outcome.usage, Usage::default());
}
