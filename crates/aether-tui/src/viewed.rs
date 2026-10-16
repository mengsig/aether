//! The agent a running TUI views, published in a private file beside the
//! daemon socket so a CLI command on the same machine can name it `@viewed`.
//! With several TUIs open, the one whose view changed last wins.

use std::io::{self, Read as _};
use std::path::{Path, PathBuf};

use aether_core::id::AgentId;
use anyhow::Context as _;
use serde::{Deserialize, Serialize};

/// A record is two short fields; anything longer is not one.
const MAX_RECORD_BYTES: u64 = 4096;

#[derive(Debug, PartialEq, Eq, Serialize, Deserialize)]
struct Record {
    pid: u32,
    agent: AgentId,
}

/// `<socket file name>.viewed`, beside the daemon socket.
pub fn viewed_path(daemon_socket: &Path) -> PathBuf {
    let mut name = daemon_socket.file_name().unwrap_or_default().to_owned();
    name.push(".viewed");
    daemon_socket.with_file_name(name)
}

/// The agent the most recent live TUI published, if any. A record left by a
/// TUI that died without withdrawing it names nothing.
pub fn read(path: &Path) -> anyhow::Result<Option<AgentId>> {
    let record = read_record(path)?;
    Ok(record.filter(|r| process_is_alive(r.pid)).map(|r| r.agent))
}

/// Keeps the published record in step with what one TUI views.
#[derive(Debug)]
pub struct Publisher {
    path: PathBuf,
    published: Option<AgentId>,
}

impl Publisher {
    pub fn new(path: PathBuf) -> Self {
        Self {
            path,
            published: None,
        }
    }

    /// Publish `viewed` if it changed since the last call. A failure is not
    /// retried until the view changes again, so it is reported once.
    pub fn sync(&mut self, viewed: Option<AgentId>) -> anyhow::Result<()> {
        if viewed == self.published {
            return Ok(());
        }
        self.published = viewed;
        match viewed {
            Some(agent) => publish(&self.path, agent),
            None => withdraw(&self.path),
        }
    }

    /// Withdraw this TUI's record as it exits.
    pub fn finish(mut self) -> anyhow::Result<()> {
        self.sync(None)
    }
}

fn publish(path: &Path, agent: AgentId) -> anyhow::Result<()> {
    let record = Record {
        pid: std::process::id(),
        agent,
    };
    let bytes = serde_json::to_vec(&record)?;
    debug_assert!(
        bytes.len() as u64 <= MAX_RECORD_BYTES,
        "a record reads back"
    );
    aether_core::private_file::write(path, &bytes)
        .with_context(|| format!("publishing the viewed agent to {}", path.display()))
}

/// Remove the record only if this process wrote it: another TUI's stays.
fn withdraw(path: &Path) -> anyhow::Result<()> {
    let Some(record) = read_record(path)? else {
        return Ok(());
    };
    if record.pid != std::process::id() {
        return Ok(());
    }
    match std::fs::remove_file(path) {
        Err(error) if error.kind() != io::ErrorKind::NotFound => Err(error)
            .with_context(|| format!("withdrawing the viewed agent at {}", path.display())),
        _ => Ok(()),
    }
}

fn read_record(path: &Path) -> anyhow::Result<Option<Record>> {
    let file = match std::fs::File::open(path) {
        Ok(file) => file,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(None),
        Err(error) => {
            return Err(error).with_context(|| format!("reading {}", path.display()));
        }
    };
    let mut bytes = Vec::new();
    file.take(MAX_RECORD_BYTES + 1)
        .read_to_end(&mut bytes)
        .with_context(|| format!("reading {}", path.display()))?;
    anyhow::ensure!(
        bytes.len() as u64 <= MAX_RECORD_BYTES,
        "{} is too large to be a viewed-agent record",
        path.display()
    );
    let record = serde_json::from_slice(&bytes)
        .with_context(|| format!("{} is not a viewed-agent record", path.display()))?;
    Ok(Some(record))
}

#[cfg(unix)]
fn process_is_alive(pid: u32) -> bool {
    use nix::errno::Errno;
    // Zero or a negative pid would name a process group, not a TUI.
    let Some(pid) = i32::try_from(pid).ok().filter(|pid| *pid > 0) else {
        return false;
    };
    // Signal 0 only checks: EPERM means the process exists under another uid.
    matches!(
        nix::sys::signal::kill(nix::unistd::Pid::from_raw(pid), None),
        Ok(()) | Err(Errno::EPERM)
    )
}

#[cfg(not(unix))]
fn process_is_alive(_pid: u32) -> bool {
    true
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scratch() -> (tempfile::TempDir, PathBuf) {
        let dir = tempfile::tempdir().expect("scratch dir");
        let path = viewed_path(&dir.path().join("aether.sock"));
        (dir, path)
    }

    #[test]
    fn the_viewed_agent_is_published_followed_and_withdrawn() {
        let (_dir, path) = scratch();
        assert!(path.ends_with("aether.sock.viewed"));
        assert_eq!(read(&path).expect("nothing yet"), None);
        let mut publisher = Publisher::new(path.clone());
        let (first, second) = (AgentId::new(), AgentId::new());
        publisher.sync(Some(first)).expect("publish");
        assert_eq!(read(&path).expect("read"), Some(first));
        publisher.sync(Some(second)).expect("follow");
        assert_eq!(read(&path).expect("read"), Some(second));
        publisher.finish().expect("withdraw");
        assert!(!path.exists(), "an exiting TUI removes its record");
    }

    #[test]
    fn another_tuis_record_survives_and_a_dead_ones_names_nothing() {
        let (_dir, path) = scratch();
        let agent = AgentId::new();
        // Pid 1 is always alive and never this test.
        let other = serde_json::to_vec(&Record { pid: 1, agent }).expect("json");
        std::fs::write(&path, other).expect("other TUI");
        withdraw(&path).expect("leave it");
        assert_eq!(read(&path).expect("read"), Some(agent));

        let mut child = std::process::Command::new("true").spawn().expect("child");
        let pid = child.id();
        child.wait().expect("reaped");
        let dead = serde_json::to_vec(&Record { pid, agent }).expect("json");
        std::fs::write(&path, dead).expect("dead TUI");
        assert_eq!(read(&path).expect("read"), None);

        std::fs::write(&path, b"not json").expect("garbage");
        assert!(read(&path).is_err(), "a corrupt record is reported");
    }
}
