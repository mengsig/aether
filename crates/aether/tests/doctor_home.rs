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

#[tokio::test]
async fn doctor_in_an_empty_home_leaves_it_empty() {
    let root = tempfile::tempdir().unwrap();
    let home = root.path().join("home");
    let bin = root.path().join("bin");
    let aether = root.path().join("aether");
    for dir in [&home, &bin, &aether] {
        std::fs::create_dir(dir).unwrap();
    }
    let probes = root.path().join("probes");
    // Like the real CLI, this codex writes its state directory on any start.
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
        .current_dir(root.path())
        .env("HOME", &home)
        .env("PATH", &bin)
        .env("AETHER_CONFIG_DIR", aether.join("config"))
        .env("AETHER_DATA_DIR", aether.join("data"))
        .env("AETHER_SOCKET", aether.join("daemon.sock"))
        .args(["doctor", "--json"])
        .kill_on_drop(true);
    let output = tokio::time::timeout(Duration::from_secs(60), command.output())
        .await
        .expect("doctor finishes")
        .unwrap();
    let stdout = String::from_utf8_lossy(&output.stdout);
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
