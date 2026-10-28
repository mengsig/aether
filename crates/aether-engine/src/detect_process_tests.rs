//! Real subprocess deadlines, without invoking installed coding harnesses.
#![cfg(target_os = "linux")]

use super::*;

async fn assert_probe_timeout(capability: bool) {
    let dir = tempfile::tempdir().unwrap();
    let script = dir.path().join("probe.sh");
    let pid_file = dir.path().join("probe.sh.pid");
    std::fs::write(&script, "echo $$ > \"$0.pid\"\nexec sleep 30\n").unwrap();
    let script = script.to_str().unwrap();
    let error = if capability {
        probe_capability("/bin/sh", &[script], "app-server", None, None)
            .await
            .unwrap_err()
    } else {
        probe_version_with_args("/bin/sh", &[script], None, None)
            .await
            .unwrap_err()
    };
    assert!(
        matches!(&error, DetectError::TimedOut { probe, .. }
        if probe == if capability { script } else { "--version" }),
        "{error}"
    );
    let pid: i32 = std::fs::read_to_string(pid_file)
        .unwrap()
        .trim()
        .parse()
        .unwrap();
    let reaped = tokio::time::timeout(Duration::from_secs(5), async {
        while std::path::Path::new(&format!("/proc/{pid}")).exists() {
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await;
    if reaped.is_err() {
        nix::sys::signal::kill(
            nix::unistd::Pid::from_raw(pid),
            nix::sys::signal::Signal::SIGKILL,
        )
        .expect("clean up only the surviving test fixture");
    }
    assert!(reaped.is_ok(), "timed-out probe survived: {pid}");
}

#[tokio::test]
async fn version_probe_times_out_and_reaps_child() {
    assert_probe_timeout(false).await;
}

#[tokio::test]
async fn capability_probe_times_out_and_reaps_child() {
    assert_probe_timeout(true).await;
}

/// Codex writes state on every start; the probe must leave none behind.
#[tokio::test]
async fn a_state_writing_cli_is_probed_in_a_scratch_directory_it_removes() {
    let dir = tempfile::tempdir().unwrap();
    let home = dir.path().join("home");
    std::fs::create_dir(&home).unwrap();
    let seen = dir.path().join("seen");
    let script = dir.path().join("codex-probe.sh");
    std::fs::write(
        &script,
        format!(
            "state=\"${{{CODEX_STATE_ENV}:-{}/.codex}}\"\nmkdir -p \"$state/tmp\"\nprintf %s \"$state\" > '{}'\necho codex-cli 0.200.0\n",
            home.display(),
            seen.display()
        ),
    )
    .unwrap();
    let script = script.to_str().unwrap();
    let version = probe_version_with_args("/bin/sh", &[script], None, Some(CODEX_STATE_ENV))
        .await
        .unwrap();
    assert_eq!(version, EngineVersion::new(0, 200, 0));
    assert_eq!(
        std::fs::read_dir(&home).unwrap().count(),
        0,
        "home untouched"
    );
    let used = std::fs::read_to_string(&seen).unwrap();
    assert!(!used.starts_with(home.to_str().unwrap()), "{used}");
    assert!(
        !std::path::Path::new(&used).exists(),
        "the scratch state directory is removed: {used}"
    );
}

/// A probe leaves nothing running: what it started goes with its group.
#[tokio::test]
async fn a_probe_takes_what_it_started_with_it() {
    use crate::lifeline::tests::{KillOnDrop, wait_until_gone};

    let dir = tempfile::tempdir().unwrap();
    let child_file = dir.path().join("child.pid");
    let script = r#"sleep 30 >/dev/null 2>&1 & echo $! > "$1"; echo codex-cli 0.200.0"#;
    let version = probe_version_with_args(
        "/bin/sh",
        &["-c", script, "sh", child_file.to_str().unwrap()],
        None,
        None,
    )
    .await
    .unwrap();
    assert_eq!(version, EngineVersion::new(0, 200, 0));
    let child: u32 = std::fs::read_to_string(&child_file)
        .unwrap()
        .trim()
        .parse()
        .unwrap();
    let _guard = KillOnDrop(child);
    assert!(
        tokio::task::spawn_blocking(move || wait_until_gone(child, Duration::from_secs(5)))
            .await
            .unwrap(),
        "the probe's background child {child} outlived it"
    );
}

/// Re-run as the "daemon" of `a_killed_parent_takes_a_wrapped_probe_with_it`.
const PROBE_HELPER_ENV: &str = "AETHER_TEST_PROBE_PARENT";

/// The helper side: probe a wrapper that starts its CLI and waits on it,
/// print both pids, wait to be killed.
#[tokio::test]
async fn probe_helper_parent() {
    let Some(pids_file) = std::env::var_os(PROBE_HELPER_ENV) else {
        return;
    };
    let pids_file = std::path::PathBuf::from(pids_file);
    let target = pids_file.to_str().unwrap().to_owned();
    let probe = tokio::spawn(async move {
        let script = r#"sleep 30 & echo "$$ $!" > "$1.tmp" && mv "$1.tmp" "$1"; wait"#;
        probe_version_with_args("/bin/sh", &["-c", script, "sh", &target], None, None).await
    });
    let pids = tokio::time::timeout(Duration::from_secs(10), async {
        loop {
            if let Ok(pids) = std::fs::read_to_string(&pids_file) {
                return pids;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .expect("the probed wrapper reports itself and its child");
    for pid in pids.split_whitespace() {
        println!("pid={pid}");
    }
    println!("ready");
    tokio::time::sleep(Duration::from_secs(60)).await;
    probe.abort();
}

/// A daemon killed mid-probe takes the probed wrapper and the CLI it started.
#[test]
fn a_killed_parent_takes_a_wrapped_probe_with_it() {
    let dir = tempfile::tempdir().unwrap();
    crate::lifeline::tests::kill_parent_and_expect_gone(
        "detect::process_tests::probe_helper_parent",
        PROBE_HELPER_ENV,
        dir.path().join("pids").as_os_str(),
    );
}

/// A probe dropped mid-run, as a cancelled caller drops it, still takes the
/// CLI a wrapper started.
#[tokio::test]
async fn a_cancelled_probe_takes_a_wrapped_cli_with_it() {
    use crate::lifeline::tests::{KillOnDrop, wait_until_gone};

    let dir = tempfile::tempdir().unwrap();
    let pids_file = dir.path().join("pids");
    let target = pids_file.to_str().unwrap().to_owned();
    let probe = tokio::spawn(async move {
        let script = r#"sleep 30 & echo "$$ $!" > "$1.tmp" && mv "$1.tmp" "$1"; wait"#;
        probe_version_with_args("/bin/sh", &["-c", script, "sh", &target], None, None).await
    });
    let pids = tokio::time::timeout(Duration::from_secs(10), async {
        loop {
            if let Ok(pids) = std::fs::read_to_string(&pids_file) {
                return pids;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .expect("the probed wrapper reports itself and its child");
    let guards: Vec<KillOnDrop> = pids
        .split_whitespace()
        .map(|pid| KillOnDrop(pid.parse().unwrap()))
        .collect();
    assert_eq!(guards.len(), 2, "the wrapper and its child: {pids}");
    probe.abort();
    assert!(probe.await.unwrap_err().is_cancelled());
    for guard in &guards {
        let pid = guard.0;
        assert!(
            tokio::task::spawn_blocking(move || wait_until_gone(pid, Duration::from_secs(5)))
                .await
                .unwrap(),
            "{pid} outlived its cancelled probe"
        );
    }
}
