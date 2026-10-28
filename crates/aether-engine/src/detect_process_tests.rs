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
