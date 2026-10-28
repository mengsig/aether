#![cfg(unix)]
#![allow(clippy::expect_used, clippy::unwrap_used, clippy::panic)]

//! `aether doctor` probes the engine CLIs without writing to the user's home.

use std::path::Path;
use std::time::Duration;

/// Write an executable `#!/bin/sh` script.
fn script(path: &Path, body: &str) {
    use std::os::unix::fs::PermissionsExt;
    std::fs::write(path, format!("#!/bin/sh\n{body}")).unwrap();
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o700)).unwrap();
}

/// Every path under `dir`, so a failure names what was written.
fn tree(dir: &Path) -> Vec<String> {
    let mut found = Vec::new();
    let mut pending = vec![dir.to_path_buf()];
    while let Some(next) = pending.pop() {
        for entry in std::fs::read_dir(&next).unwrap() {
            let path = entry.unwrap().path();
            if path.is_dir() {
                pending.push(path.clone());
            }
            found.push(path.display().to_string());
        }
    }
    found
}

/// A fresh `HOME`, `PATH` and Aether directories under `root`, with a codex
/// that writes its state directory on any start, as the real CLI does.
struct Machine {
    home: std::path::PathBuf,
    probes: std::path::PathBuf,
    command: tokio::process::Command,
}

fn machine(root: &Path) -> Machine {
    let home = root.join("home");
    let bin = root.join("bin");
    let aether = root.join("aether");
    for dir in [&home, &bin, &aether] {
        std::fs::create_dir(dir).unwrap();
    }
    let probes = root.join("probes");
    script(
        &bin.join("codex"),
        &format!(
            "printf '%s\\n' \"codex $*\" >> '{log}'\nstate=\"${{CODEX_HOME:-$HOME/.codex}}\"\n/bin/mkdir -p \"$state/tmp/arg0\"\nprintf '%s\\n' 'codex-cli 0.200.0'\n",
            log = probes.display()
        ),
    );
    script(
        &bin.join("claude"),
        "printf '%s\\n' '2.1.269 (Claude Code)'\n",
    );
    let mut command = tokio::process::Command::new(env!("CARGO_BIN_EXE_aether"));
    command
        .env_clear()
        .current_dir(root)
        .env("HOME", &home)
        .env("PATH", &bin)
        .env("AETHER_CONFIG_DIR", aether.join("config"))
        .env("AETHER_DATA_DIR", aether.join("data"))
        .env("AETHER_SOCKET", aether.join("daemon.sock"))
        .args(["doctor", "--json"])
        .kill_on_drop(true);
    Machine {
        home,
        probes,
        command,
    }
}

async fn doctor(command: &mut tokio::process::Command) -> String {
    let output = tokio::time::timeout(Duration::from_secs(60), command.output())
        .await
        .expect("doctor finishes")
        .unwrap();
    String::from_utf8_lossy(&output.stdout).into_owned()
}

#[tokio::test]
async fn doctor_in_an_empty_home_leaves_it_empty() {
    let root = tempfile::tempdir().unwrap();
    let Machine {
        home,
        probes,
        mut command,
    } = machine(root.path());
    let stdout = doctor(&mut command).await;
    assert!(
        stdout.contains("codex"),
        "doctor reported engines: {stdout}"
    );
    let probed = std::fs::read_to_string(&probes).unwrap_or_default();
    assert!(
        probed.contains("codex --version"),
        "codex was probed: {probed}"
    );
    let written = tree(&home);
    assert!(written.is_empty(), "doctor wrote into HOME: {written:?}");
}

/// Without a usable temp dir codex is not probed, and the doctor says why
/// rather than telling the operator to install a CLI they have.
#[tokio::test]
async fn doctor_blames_temp_storage_when_the_codex_scratch_cannot_be_made() {
    let root = tempfile::tempdir().unwrap();
    let Machine {
        home,
        probes,
        mut command,
    } = machine(root.path());
    command.env("TMPDIR", root.path().join("missing"));
    let stdout = doctor(&mut command).await;
    assert!(
        stdout.contains("TMPDIR"),
        "doctor names the cause: {stdout}"
    );
    assert!(
        !stdout.contains("install codex"),
        "no install advice for a temp-storage problem: {stdout}"
    );
    assert!(!probes.exists(), "codex never ran without its scratch");
    assert!(tree(&home).is_empty(), "nothing fell back into HOME");
}
