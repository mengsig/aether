//! `aether daemon --workspace` scopes new root agents the same way whether
//! the daemon runs in the foreground or detaches, and a bad path fails both.
#![allow(clippy::expect_used, clippy::panic, clippy::unwrap_used)]

use std::path::Path;
use std::process::{Child, Command, Output, Stdio};
use std::time::{Duration, Instant};

/// Longest any one CLI call may take before the test kills it and fails.
const CALL_LIMIT: Duration = Duration::from_secs(30);
/// How long a stopped daemon gets to exit before it is killed.
const EXIT_LIMIT: Duration = Duration::from_secs(10);

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

/// Wait for `child` until `limit`, then kill it; `None` when it had to be.
fn finish_within(mut child: Child, limit: Duration) -> Option<Output> {
    let deadline = Instant::now() + limit;
    while child.try_wait().ok()?.is_none() {
        if Instant::now() >= deadline {
            let _ = child.kill();
            let _ = child.wait();
            return None;
        }
        std::thread::sleep(Duration::from_millis(20));
    }
    child.wait_with_output().ok()
}

/// One CLI call, killed after `limit`. Never panics, so cleanup can use it.
fn bounded(root: &Path, cwd: &Path, args: &[&str], limit: Duration) -> Option<Output> {
    let child = isolated_command(root, cwd, args)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .ok()?;
    finish_within(child, limit)
}

fn output(root: &Path, cwd: &Path, args: &[&str]) -> Output {
    bounded(root, cwd, args, CALL_LIMIT).unwrap_or_else(|| {
        panic!(
            "`aether {}` did not finish in {CALL_LIMIT:?}",
            args.join(" ")
        )
    })
}

/// The pid of the daemon answering on `root`'s socket, if one answers.
fn daemon_pid(root: &Path) -> Option<u32> {
    let status = bounded(root, root, &["status", "--json"], CALL_LIMIT)?;
    if !status.status.success() {
        return None;
    }
    let status: serde_json::Value = serde_json::from_slice(&status.stdout).ok()?;
    u32::try_from(status["pid"].as_u64()?).ok()
}

fn alive(pid: u32) -> bool {
    Command::new("kill")
        .args(["-0", &pid.to_string()])
        .stderr(Stdio::null())
        .status()
        .is_ok_and(|status| status.success())
}

/// Stops whatever daemon a case started, even when an assertion failed first,
/// and kills it if it will not stop.
struct Daemon<'a> {
    root: &'a Path,
    foreground: Option<Child>,
}

impl Daemon<'_> {
    /// Whether a detached daemon on this root is gone, killing it if need be.
    fn stop_detached(&self) -> bool {
        let Some(pid) = daemon_pid(self.root) else {
            return true;
        };
        let _ = bounded(self.root, self.root, &["stop"], CALL_LIMIT);
        let deadline = Instant::now() + EXIT_LIMIT;
        while alive(pid) && Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(50));
        }
        if !alive(pid) {
            return true;
        }
        let _ = Command::new("kill").args(["-9", &pid.to_string()]).status();
        eprintln!("daemon {pid} ignored `aether stop` and was killed");
        false
    }
}

impl Drop for Daemon<'_> {
    fn drop(&mut self) {
        let clean = match self.foreground.take() {
            Some(child) => {
                let _ = bounded(self.root, self.root, &["stop"], CALL_LIMIT);
                finish_within(child, EXIT_LIMIT).is_some()
            }
            None => self.stop_detached(),
        };
        if !clean && !std::thread::panicking() {
            panic!("the daemon under {} did not stop", self.root.display());
        }
    }
}

/// `status` never starts a daemon, so polling it cannot race one into being.
fn wait_until_listening(root: &Path) {
    let deadline = Instant::now() + Duration::from_secs(20);
    while daemon_pid(root).is_none() {
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
