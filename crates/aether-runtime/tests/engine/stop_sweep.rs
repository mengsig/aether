//! A daemon stop ends the command an engine was running, even one the engine
//! ran in a session of its own, as Claude Code runs each, while sparing a kept
//! process: no record names that command once its engine is gone.

use super::*;

/// Runs [`stop_sweep_helper`] in its own process: the engine registry it sets
/// is process-wide.
const HELPER_ENV: &str = "AETHER_TEST_STOP_SWEEP";
const HELPER: &str = "stop_sweep::stop_sweep_helper";

/// A `codex exec` stand-in that starts one command in its own session and
/// one kept the same way, reports the first running, and waits.
const FAKE_CODEX: &str = r#"#!/bin/sh
dir=$(dirname "$0")
printf '{"type":"thread.started","thread_id":"thread-%s"}\n' "$$"
setsid sh -c 'echo $$ > "$1/command.tmp" && mv "$1/command.tmp" "$1/command.pid"; exec sleep 300' \
  sh "$dir" </dev/null >/dev/null 2>&1 &
AETHER_KEEP_PROCESS=1 setsid sh -c 'echo $$ > "$1/kept.tmp" && mv "$1/kept.tmp" "$1/kept.pid"; exec sleep 300' \
  sh "$dir" </dev/null >/dev/null 2>&1 &
printf '%s\n' '{"type":"item.started","item":{"id":"item_0","type":"command_execution","command":"make test","status":"in_progress"}}'
exec sleep 300
"#;

fn running(pid: i32) -> bool {
    std::fs::read_to_string(format!("/proc/{pid}/stat")).is_ok_and(|stat| {
        !stat
            .rsplit(')')
            .next()
            .unwrap_or_default()
            .starts_with(" Z")
    })
}

async fn pid_in(path: &std::path::Path) -> i32 {
    for _ in 0..500 {
        if let Ok(text) = std::fs::read_to_string(path) {
            return text.trim().parse().unwrap();
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    panic!("{} was never written", path.display());
}

#[test]
fn a_stop_ends_the_command_an_engine_ran_in_its_own_session_but_spares_a_kept_one() {
    let output = std::process::Command::new(std::env::current_exe().unwrap())
        .args(["--exact", HELPER, "--nocapture"])
        .env(HELPER_ENV, "1")
        .output()
        .unwrap();
    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(output.status.success(), "{stdout}{stderr}");
    // A stale `--exact` name would run zero tests and still exit 0.
    assert!(stdout.contains("1 passed"), "{stdout}");
}

#[tokio::test]
async fn stop_sweep_helper() {
    if std::env::var_os(HELPER_ENV).is_none() {
        return;
    }
    let dir = tempfile::tempdir().unwrap();
    let status = std::process::Command::new("/bin/sh")
        .args(["-c", r#"printf '%s' "$1" > "$2" && chmod 700 "$2""#, "sh"])
        .arg(FAKE_CODEX)
        .arg(dir.path().join("codex"))
        .status()
        .unwrap();
    assert!(status.success());
    aether_engine::orphans::init(&dir.path().join("engines"), &Default::default()).unwrap();
    let engine: Arc<dyn Engine> = Arc::new(aether_engine::CodexEngine::new(
        dir.path().join("codex").to_string_lossy(),
        "0.164.0",
    ));
    let auth = "[providers.claude-code.auth]\nnone = true\n[providers.codex.auth]\nnone = true\n";
    let h = build_harness(
        engine,
        Arc::default(),
        Arc::default(),
        Some(auth),
        None,
        None,
        |_| {},
    )
    .await;
    h.runtime
        .create_agent(
            CreateAgentSpec {
                role: "tester".into(),
                objective: Some("run the tests".into()),
                start: true,
                config: Some(AgentConfig {
                    approval: ApprovalPolicy::none(),
                    ..Default::default()
                }),
                ..Default::default()
            },
            Actor::user(),
        )
        .unwrap();
    let command = pid_in(&dir.path().join("command.pid")).await;
    let kept = pid_in(&dir.path().join("kept.pid")).await;
    h.runtime.shutdown().await;
    let mut stopped = false;
    for _ in 0..100 {
        stopped = !running(command);
        if stopped {
            break;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    let kept_ran = running(kept);
    for pid in [command, kept] {
        let _ = nix::sys::signal::kill(
            nix::unistd::Pid::from_raw(pid),
            nix::sys::signal::Signal::SIGKILL,
        );
    }
    assert!(
        stopped,
        "the interrupted engine's command outlived the stop"
    );
    assert!(kept_ran, "the stop ended a kept process");
}
