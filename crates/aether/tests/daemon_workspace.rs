//! `aether daemon --workspace` scopes new root agents the same way whether
//! the daemon runs in the foreground or detaches, and a bad path fails both.
#![allow(clippy::expect_used, clippy::unwrap_used)]

use std::path::Path;
use std::process::{Child, Command, Output, Stdio};
use std::time::{Duration, Instant};

fn isolated_command(root: &Path, cwd: &Path, args: &[&str]) -> Command {
    let mut command = Command::new(env!("CARGO_BIN_EXE_aether"));
    command
        .args(args)
        .current_dir(cwd)
        .stdin(Stdio::null())
        .env("HOME", root)
        .env("PATH", "")
        .env("XDG_CONFIG_HOME", root.join("config"))
        .env("XDG_DATA_HOME", root.join("data"))
        .env("XDG_RUNTIME_DIR", root.join("runtime"))
        .env("AETHER_CONFIG_DIR", root.join("config"))
        .env("AETHER_DATA_DIR", root.join("data"))
        .env("AETHER_SOCKET", root.join("daemon.sock"))
        .env_remove("AETHER_REMOTE")
        .env_remove("AETHER_TCP_ADDR");
    command
}

fn output(root: &Path, cwd: &Path, args: &[&str]) -> Output {
    isolated_command(root, cwd, args).output().unwrap()
}

/// Stops whatever daemon a case started, even when an assertion failed first.
struct Daemon<'a> {
    root: &'a Path,
    foreground: Option<Child>,
}

impl Drop for Daemon<'_> {
    fn drop(&mut self) {
        let _ = output(self.root, self.root, &["stop"]);
        let Some(child) = self.foreground.as_mut() else {
            return;
        };
        let deadline = Instant::now() + Duration::from_secs(10);
        while matches!(child.try_wait(), Ok(None)) && Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(50));
        }
        let _ = child.kill();
        let _ = child.wait();
    }
}

/// `status` never starts a daemon, so polling it cannot race one into being.
fn wait_until_listening(root: &Path) {
    let deadline = Instant::now() + Duration::from_secs(20);
    while !output(root, root, &["status", "--json"]).status.success() {
        assert!(Instant::now() < deadline, "the daemon never listened");
        std::thread::sleep(Duration::from_millis(50));
    }
}

fn start<'a>(root: &'a Path, alpha: &Path, foreground: bool) -> Daemon<'a> {
    if foreground {
        let child = isolated_command(
            root,
            alpha,
            &["daemon", "--foreground", "--workspace", "../beta"],
        )
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .unwrap();
        let daemon = Daemon {
            root,
            foreground: Some(child),
        };
        wait_until_listening(root);
        return daemon;
    }
    let daemon = Daemon {
        root,
        foreground: None,
    };
    let started = output(root, alpha, &["daemon", "--workspace", "../beta"]);
    let stderr = String::from_utf8_lossy(&started.stderr);
    assert!(started.status.success(), "{stderr}");
    daemon
}

#[test]
fn a_detached_daemon_roots_new_agents_in_its_workspace_like_a_foreground_one() {
    for foreground in [true, false] {
        let root = tempfile::tempdir().unwrap();
        let alpha = root.path().join("alpha");
        let beta = root.path().join("beta");
        std::fs::create_dir_all(&alpha).unwrap();
        std::fs::create_dir_all(&beta).unwrap();
        let _daemon = start(root.path(), &alpha, foreground);

        let created = output(
            root.path(),
            &alpha,
            &["new", "--model", "anthropic/claude-sonnet-5-5", "--json"],
        );
        let stderr = String::from_utf8_lossy(&created.stderr);
        assert!(created.status.success(), "{stderr}");
        let agent: serde_json::Value = serde_json::from_slice(&created.stdout).unwrap();
        let expected = std::fs::canonicalize(&beta).unwrap();
        assert_eq!(
            agent["scope"]["workspace_root"].as_str(),
            expected.to_str(),
            "foreground: {foreground}"
        );
    }
}

#[test]
fn a_missing_workspace_fails_the_daemon_in_both_modes() {
    for args in [
        &["daemon", "--foreground", "--workspace", "missing"][..],
        &["daemon", "--workspace", "missing"][..],
    ] {
        let root = tempfile::tempdir().unwrap();
        let _daemon = Daemon {
            root: root.path(),
            foreground: None,
        };
        let result = output(root.path(), root.path(), args);
        let stderr = String::from_utf8_lossy(&result.stderr);
        assert!(!result.status.success(), "{args:?} started: {stderr}");
        assert!(stderr.contains("workspace missing"), "{args:?}: {stderr}");
    }
}
