//! Direct detached starts work both as a shell's group leader and as its child.
#![cfg(target_os = "linux")]
#![allow(clippy::expect_used, clippy::unwrap_used)]

use std::os::fd::OwnedFd;
use std::os::unix::process::CommandExt;
use std::path::Path;
use std::process::{Child, Command, Stdio};
use std::time::Duration;

use aether_client::Client;
use rustix::process::{Pid, PidfdFlags, Signal, pidfd_open, pidfd_send_signal};

struct Daemon {
    launcher: Child,
    process: Option<OwnedFd>,
    _pty: Option<pty_process::blocking::Pty>,
}

impl Drop for Daemon {
    fn drop(&mut self) {
        if let Some(process) = &self.process {
            let result = pidfd_send_signal(process, Signal::KILL);
            assert!(result.is_ok() || result == Err(rustix::io::Errno::SRCH));
        }
        // The launcher remains our unreaped child, so its pid cannot be reused.
        let _ = self.launcher.kill();
        self.launcher.wait().unwrap();
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Launcher {
    Child,
    GroupLeader,
    PtySessionLeader,
}

fn launch(root: &Path, mode: Launcher) -> Daemon {
    assert!(root.is_dir());
    assert!(root.is_absolute());
    // The readiness deadline measures the detached launch. Initialize the fixture store first.
    drop(
        aether_store::SqliteStore::open(root.join("data/aether.db"))
            .expect("prepare fixture store"),
    );
    let mut command = Command::new(env!("CARGO_BIN_EXE_aether"));
    command
        .args(["daemon", "--foreground", "--detached"])
        .current_dir(root)
        .env("HOME", root)
        .env("PATH", "")
        .env("AETHER_CONFIG_DIR", root.join("config"))
        .env("AETHER_DATA_DIR", root.join("data"))
        .env("AETHER_SOCKET", root.join("daemon.sock"))
        .env("XDG_CONFIG_HOME", root.join("config"))
        .env("XDG_DATA_HOME", root.join("data"))
        .env("XDG_RUNTIME_DIR", root.join("runtime"))
        .env_remove("AETHER_REMOTE")
        .env_remove("AETHER_TCP_ADDR")
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    if mode == Launcher::GroupLeader {
        command.process_group(0);
    }
    let (launcher, pty) = if mode == Launcher::PtySessionLeader {
        let (child, pty) = spawn_on_pty(command);
        (child, Some(pty))
    } else {
        (command.spawn().unwrap(), None)
    };
    Daemon {
        launcher,
        process: None,
        _pty: pty,
    }
}

fn spawn_on_pty(command: Command) -> (Child, pty_process::blocking::Pty) {
    let (pty, pts) = pty_process::blocking::open().unwrap();
    let mut child = pty_process::blocking::Command::new(command.get_program())
        .args(command.get_args())
        .current_dir(command.get_current_dir().unwrap());
    for (name, value) in command.get_envs() {
        child = match value {
            Some(value) => child.env(name, value),
            None => child.env_remove(name),
        };
    }
    (child.spawn(pts).unwrap(), pty)
}

async fn listening(root: &Path) -> Client {
    let socket = root.join("daemon.sock");
    assert!(socket.is_absolute());
    assert!(root.is_dir());
    tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            if let Ok(client) = Client::connect(&socket).await {
                return client;
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    })
    .await
    .expect("the private daemon listens within five seconds")
}

#[tokio::test]
async fn direct_detached_daemon_leads_a_session_without_a_terminal() {
    for mode in [
        Launcher::Child,
        Launcher::GroupLeader,
        Launcher::PtySessionLeader,
    ] {
        let root = tempfile::tempdir().unwrap();
        let mut daemon = launch(root.path(), mode);
        let client = listening(root.path()).await;
        let pid = client.welcome().unwrap().pid;
        assert!(pid > 1);
        daemon.process =
            Some(pidfd_open(Pid::from_raw(pid as i32).unwrap(), PidfdFlags::empty()).unwrap());
        let stat = std::fs::read_to_string(format!("/proc/{pid}/stat")).unwrap();
        let fields: Vec<_> = stat
            .rsplit_once(')')
            .unwrap()
            .1
            .split_whitespace()
            .collect();
        assert_eq!(fields[2], pid.to_string(), "daemon owns its process group");
        assert_eq!(fields[3], pid.to_string(), "daemon owns its session");
        assert_eq!(fields[4], "0", "daemon has no controlling terminal");
        assert_eq!(
            pid != daemon.launcher.id(),
            mode != Launcher::Child,
            "a group leader or PTY session leader needs another child"
        );
    }
}
