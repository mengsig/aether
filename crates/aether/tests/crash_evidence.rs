#![allow(clippy::expect_used, clippy::unwrap_used)]

use std::path::Path;
use std::process::{Command, Output};

use aether_core::crash::{self, CrashKind};
use serde_json::Value;

fn isolated_command(root: &Path) -> Command {
    let mut command = Command::new(env!("CARGO_BIN_EXE_aether"));
    command
        .current_dir(root)
        .env("HOME", root)
        .env("PATH", "")
        .env("XDG_CONFIG_HOME", root.join("config"))
        .env("XDG_DATA_HOME", root.join("data"))
        .env("XDG_RUNTIME_DIR", root.join("runtime"))
        .env("AETHER_CONFIG_DIR", root.join("config"))
        .env("AETHER_DATA_DIR", root.join("data"))
        .env("AETHER_SOCKET", root.join("daemon.sock"))
        .env("AETHER_LOG", "info")
        .env_remove("AETHER_REMOTE");
    command
}

fn command(root: &Path, args: &[&str]) -> Output {
    isolated_command(root).args(args).output().unwrap()
}

#[test]
fn a_fatal_daemon_start_is_reported_by_offline_status_and_doctor() {
    let root = tempfile::tempdir().unwrap();
    let result = command(
        root.path(),
        &["daemon", "--foreground", "--workspace", "missing"],
    );
    assert!(!result.status.success());
    let store = root.path().join("data/aether.db");
    let record = crash::read(&store).unwrap().unwrap();
    assert_eq!(record.kind, CrashKind::FatalError);
    assert!(record.message.contains("workspace missing"));
    assert!(record.location.is_some());
    let status = command(root.path(), &["status", "--json"]);
    assert!(!status.status.success());
    let status: Value = serde_json::from_slice(&status.stdout).unwrap();
    assert!(
        status["error"]["message"]
            .as_str()
            .unwrap()
            .contains("workspace missing")
    );
    let doctor = command(root.path(), &["doctor", "--json"]);
    let report: Value = serde_json::from_slice(&doctor.stdout).unwrap();
    let check = report["checks"]
        .as_array()
        .unwrap()
        .iter()
        .find(|c| c["name"] == "crash")
        .unwrap();
    assert_eq!(check["level"], "warn");
    assert!(
        check["detail"]
            .as_str()
            .unwrap()
            .contains("workspace missing")
    );
    assert_eq!(crash::read(&store).unwrap().unwrap(), record);
}

#[test]
fn an_existing_store_or_socket_owner_does_not_replace_crash_evidence() {
    use std::os::unix::fs::OpenOptionsExt;

    for lock_name in ["data/aether.db.lock", "daemon.sock.lock"] {
        let root = tempfile::tempdir().unwrap();
        let store = root.path().join("data/aether.db");
        crash::record(
            &store,
            CrashKind::Panic,
            "original panic",
            Some("worker.rs:7"),
        )
        .unwrap();
        let marker = crash::path_for(&store);
        let original = std::fs::read(&marker).unwrap();
        let lock = std::fs::OpenOptions::new()
            .create(true)
            .truncate(false)
            .read(true)
            .write(true)
            .mode(0o600)
            .open(root.path().join(lock_name))
            .unwrap();
        lock.lock().unwrap();
        let result = command(root.path(), &["daemon", "--foreground"]);
        assert!(!result.status.success());
        let stderr = String::from_utf8_lossy(&result.stderr);
        assert!(stderr.contains("another aether daemon"), "{stderr}");
        assert!(stderr.contains("aether status"), "{stderr}");
        assert_eq!(std::fs::read(marker).unwrap(), original);
    }
}

#[test]
fn even_log_initialization_failure_leaves_persistent_evidence() {
    let root = tempfile::tempdir().unwrap();
    let data = root.path().join("data");
    std::fs::create_dir(&data).unwrap();
    std::fs::write(data.join("logs"), "not a directory").unwrap();
    let result = command(root.path(), &["daemon", "--foreground"]);
    assert!(!result.status.success());
    let record = crash::read(&data.join("aether.db")).unwrap().unwrap();
    assert_eq!(record.kind, CrashKind::FatalError);
    assert!(record.message.contains("initialize log stream"));
    assert!(!record.acknowledged);
}

#[test]
fn doctor_reports_malformed_evidence_without_replacing_it() {
    let root = tempfile::tempdir().unwrap();
    let store = root.path().join("data/aether.db");
    std::fs::create_dir(store.parent().unwrap()).unwrap();
    std::fs::write(crash::path_for(&store), "{").unwrap();
    let doctor = command(root.path(), &["doctor", "--json"]);
    let report: Value = serde_json::from_slice(&doctor.stdout).unwrap();
    let check = report["checks"]
        .as_array()
        .unwrap()
        .iter()
        .find(|c| c["name"] == "crash")
        .unwrap();
    assert_eq!(check["level"], "warn");
    assert!(
        check["detail"]
            .as_str()
            .unwrap()
            .contains("Cannot read daemon crash evidence")
    );
    assert_eq!(std::fs::read(crash::path_for(&store)).unwrap(), b"{");
}

async fn clean_daemon_run(root: &Path) -> Output {
    use std::process::Stdio;
    use std::time::Duration;
    let mut command = isolated_command(root);
    command
        .args(["daemon", "--foreground"])
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    let mut child = tokio::process::Command::from(command)
        .kill_on_drop(true)
        .spawn()
        .unwrap();
    let client = tokio::time::timeout(Duration::from_secs(20), async {
        loop {
            assert!(
                child.try_wait().unwrap().is_none(),
                "daemon exited before listening"
            );
            if let Ok(client) = aether_client::Client::connect(&root.join("daemon.sock")).await {
                break client;
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    })
    .await
    .unwrap();
    // The start acknowledged the evidence, so it is history rather than a problem (#267).
    let status = client.status().await.unwrap();
    assert!(
        !status
            .problems
            .iter()
            .any(|p| p.contains("once-only regression")),
        "{:?}",
        status.problems
    );
    client.shutdown().await.unwrap();
    let output = tokio::time::timeout(Duration::from_secs(20), child.wait_with_output())
        .await
        .unwrap()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    output
}

#[tokio::test]
async fn startup_prints_evidence_once_and_clean_shutdown_keeps_it() {
    let root = tempfile::tempdir().unwrap();
    let store = root.path().join("data/aether.db");
    crash::record(
        &store,
        CrashKind::Panic,
        "once-only regression",
        Some("worker.rs:9:2"),
    )
    .unwrap();
    let first = clean_daemon_run(root.path()).await;
    assert_eq!(
        String::from_utf8_lossy(&first.stderr)
            .matches("once-only regression")
            .count(),
        1
    );
    let evidence = crash::read(&store).unwrap().unwrap();
    assert!(evidence.acknowledged);
    let doctor = command(root.path(), &["doctor", "--json"]);
    let report: Value = serde_json::from_slice(&doctor.stdout).unwrap();
    let check = report["checks"]
        .as_array()
        .unwrap()
        .iter()
        .find(|c| c["name"] == "crash")
        .unwrap();
    assert_eq!(check["level"], "ok");
    assert!(
        check["detail"]
            .as_str()
            .unwrap()
            .contains("once-only regression")
    );
    let second = clean_daemon_run(root.path()).await;
    assert!(!String::from_utf8_lossy(&second.stderr).contains("once-only regression"));
    assert_eq!(crash::read(&store).unwrap().unwrap(), evidence);
}
