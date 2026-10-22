//! Engines a crashed daemon left running.
//!
//! Every engine process group the daemon spawns is recorded as a file named
//! by its leader's pid, holding the leader's kernel start time. The next
//! daemon start reaps a recorded group only while a live process still has
//! that pid *and* start time, so a reused pid is never signalled. Only Linux
//! can read a start time; elsewhere nothing is recorded or reaped.
//!
//! Descendants that left the engine's group (`setsid`, a shell tool's own
//! group) are found by a random token the engine exports in its environment
//! and the record also holds: any live process of this user carrying it is ours.

use std::io;
use std::path::{Path, PathBuf};
use std::sync::OnceLock;

use nix::sys::signal::{Signal, killpg};
use nix::unistd::{Pid, getpgid};

static REGISTRY: OnceLock<PathBuf> = OnceLock::new();

/// Environment variable carrying an engine's token to everything it spawns.
const TOKEN_ENV: &str = "AETHER_ENGINE_RUN";

/// Export a fresh token to `command`'s child, if engines are being recorded.
/// Pass the result to [`EngineRecord::register`].
pub(crate) fn tag(command: &mut tokio::process::Command) -> Option<String> {
    REGISTRY.get()?;
    let token = aether_core::id::ProcessId::new().to_string();
    command.env(TOKEN_ENV, &token);
    Some(token)
}

/// The fields of `/proc/<pid>/stat` from field 3 (state) on.
#[cfg(target_os = "linux")]
fn stat_fields(pid: u32) -> Option<Vec<String>> {
    let stat = std::fs::read_to_string(format!("/proc/{pid}/stat")).ok()?;
    // The command name may hold spaces and parentheses; fields follow the last ')'.
    let rest = &stat[stat.rfind(')')? + 1..];
    Some(rest.split_whitespace().map(str::to_owned).collect())
}

/// Kernel start time of `pid` in clock ticks since boot, if it is alive.
#[cfg(target_os = "linux")]
fn start_time(pid: u32) -> Option<u64> {
    // Field 22 is the start time; `stat_fields` begins at field 3.
    stat_fields(pid)?.get(19)?.parse().ok()
}

/// Wait until `pid` is stopped: SIGSTOP is queued, not yet delivered, on return.
#[cfg(target_os = "linux")]
fn wait_stopped(pid: u32) -> bool {
    for _ in 0..100 {
        match stat_fields(pid).as_deref().and_then(|f| f.first()) {
            Some(state) if state == "T" => return true,
            Some(_) => std::thread::sleep(std::time::Duration::from_millis(10)),
            None => return false,
        }
    }
    false
}

#[cfg(not(target_os = "linux"))]
fn start_time(_pid: u32) -> Option<u64> {
    None
}

/// Reap engines recorded in `dir` by a previous daemon, then record the
/// engines this process spawns there. Returns the leader pids it killed.
pub fn init(dir: &Path) -> io::Result<Vec<u32>> {
    prepare_dir(dir)?;
    let reaped = reap(dir)?;
    // A second init in one process keeps the first directory.
    let _ = REGISTRY.set(dir.to_owned());
    Ok(reaped)
}

/// Create `dir` private to this user, or refuse one that is not: records in a
/// directory someone else controls could point the reaper at any process.
fn prepare_dir(dir: &Path) -> io::Result<()> {
    use std::os::unix::fs::{DirBuilderExt as _, MetadataExt as _};

    std::fs::DirBuilder::new()
        .recursive(true)
        .mode(0o700)
        .create(dir)?;
    let meta = std::fs::symlink_metadata(dir)?;
    let unsafe_dir =
        !meta.is_dir() || meta.mode() & 0o077 != 0 || meta.uid() != nix::unistd::geteuid().as_raw();
    if unsafe_dir {
        return Err(io::Error::new(
            io::ErrorKind::PermissionDenied,
            format!("{} is not a private directory of this user", dir.display()),
        ));
    }
    Ok(())
}

/// Kill every recorded group whose leader still matches, and clear the records.
pub fn reap(dir: &Path) -> io::Result<Vec<u32>> {
    let mut reaped = Vec::new();
    for entry in std::fs::read_dir(dir)? {
        let path = entry?.path();
        let Some(pid) = path
            .file_name()
            .and_then(|name| name.to_str())
            .and_then(|name| name.parse::<u32>().ok())
        else {
            continue;
        };
        let text = match std::fs::read_to_string(&path) {
            Ok(text) => text,
            // The engine's own run already dropped its record.
            Err(error) if error.kind() == io::ErrorKind::NotFound => continue,
            Err(error) => {
                return Err(io::Error::new(
                    error.kind(),
                    format!(
                        "reading engine record {}: {error}; fix its permissions or remove it, then restart",
                        path.display()
                    ),
                ));
            }
        };
        let mut lines = text.lines();
        let recorded = lines
            .next()
            .and_then(|line| line.trim().parse::<u64>().ok());
        let token = lines.next().map(str::trim).filter(|t| !t.is_empty());
        if recorded.is_some_and(|started| kill_matching_group(pid, started)) {
            reaped.push(pid);
        }
        if let Some(token) = token {
            kill_tagged(token)
                .map_err(|e| io::Error::new(e.kind(), format!("sweeping engine {pid}: {e}")))?;
        }
        // A killed engine's own run may already have dropped its record.
        match std::fs::remove_file(&path) {
            Ok(()) => {}
            Err(error) if error.kind() == io::ErrorKind::NotFound => {}
            Err(error) => return Err(error),
        }
    }
    Ok(reaped)
}

/// SIGKILL the group `pid` leads, if `pid` still has start time `recorded`.
///
/// A pidfd pins the process and SIGSTOP keeps it from exiting, so its pid
/// cannot be reused between the identity check and the group signal.
#[cfg(target_os = "linux")]
fn kill_matching_group(pid: u32, recorded: u64) -> bool {
    use rustix::process::{Pid as RustixPid, PidfdFlags, Signal as RustixSignal};

    assert!(
        (2..=i32::MAX as u32).contains(&pid),
        "not an engine pid: {pid}"
    );
    let Some(raw) = RustixPid::from_raw(pid as i32) else {
        return false;
    };
    let Ok(pidfd) = rustix::process::pidfd_open(raw, PidfdFlags::empty()) else {
        return false;
    };
    if start_time(pid) != Some(recorded) {
        return false;
    }
    // Once stopped it cannot exit, so its pid stays ours until the group is signalled.
    if rustix::process::pidfd_send_signal(&pidfd, RustixSignal::STOP).is_err() {
        return false;
    }
    let stopped = wait_stopped(pid) && start_time(pid) == Some(recorded);
    let group = Pid::from_raw(pid as i32);
    // Engines are spawned as their own group leaders; anything else is not ours.
    let killed = stopped
        && getpgid(Some(group)) == Ok(group)
        && match killpg(group, Signal::SIGKILL) {
            Ok(()) => true,
            Err(errno) => {
                tracing::warn!(pid, %errno, "could not kill an orphaned engine process group");
                false
            }
        };
    // A leader that was not ours is resumed; a killed one dies with its group.
    let resume = if killed {
        RustixSignal::KILL
    } else {
        RustixSignal::CONT
    };
    if let Err(errno) = rustix::process::pidfd_send_signal(&pidfd, resume) {
        tracing::warn!(pid, %errno, "could not finish signalling an orphaned engine");
    }
    killed
}

#[cfg(not(target_os = "linux"))]
fn kill_matching_group(_pid: u32, _recorded: u64) -> bool {
    false
}

/// SIGKILL every live process of this user whose environment carries `token`.
///
/// The token is random per engine spawn, so a match proves descent from that
/// engine. A pidfd is opened before the environment is read, so the pid
/// cannot be reused between the check and the signal. Unexpected errors are
/// returned so the caller keeps the record and the next start retries.
#[cfg(target_os = "linux")]
fn kill_tagged(token: &str) -> io::Result<()> {
    use rustix::process::{Pid as RustixPid, PidfdFlags, Signal as RustixSignal};

    assert!(!token.is_empty(), "an empty token would match any process");
    let entry_token = format!("{TOKEN_ENV}={token}");
    let own = std::process::id();
    for entry in std::fs::read_dir("/proc")? {
        let entry = entry?;
        let Ok(pid) = entry.file_name().to_string_lossy().parse::<u32>() else {
            continue;
        };
        if pid <= 1 || pid == own {
            continue;
        }
        let Some(raw) = RustixPid::from_raw(pid as i32) else {
            continue;
        };
        let pidfd = match rustix::process::pidfd_open(raw, PidfdFlags::empty()) {
            Ok(pidfd) => pidfd,
            Err(rustix::io::Errno::SRCH) => continue,
            Err(errno) => return Err(errno.into()),
        };
        // Gone, or not ours to inspect (another user, non-dumpable): skip.
        let environment = match std::fs::read(entry.path().join("environ")) {
            Ok(environment) => environment,
            Err(error)
                if matches!(
                    error.kind(),
                    io::ErrorKind::NotFound | io::ErrorKind::PermissionDenied
                ) || error.raw_os_error() == Some(nix::libc::ESRCH) =>
            {
                continue;
            }
            Err(error) => return Err(error),
        };
        if !environment
            .split(|b| *b == 0)
            .any(|e| e == entry_token.as_bytes())
        {
            continue;
        }
        match rustix::process::pidfd_send_signal(&pidfd, RustixSignal::KILL) {
            Ok(()) | Err(rustix::io::Errno::SRCH) => {}
            Err(errno) => return Err(errno.into()),
        }
    }
    Ok(())
}

#[cfg(not(target_os = "linux"))]
fn kill_tagged(_token: &str) -> io::Result<()> {
    Ok(())
}

/// Write a fresh private record; an existing entry, even a link, is replaced.
fn write_record(path: &Path, started: u64, token: Option<&str>) -> io::Result<()> {
    use std::io::Write as _;
    use std::os::unix::fs::OpenOptionsExt as _;

    match std::fs::remove_file(path) {
        Ok(()) => {}
        Err(error) if error.kind() == io::ErrorKind::NotFound => {}
        Err(error) => return Err(error),
    }
    let mut file = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .custom_flags(nix::libc::O_NOFOLLOW)
        .open(path)?;
    file.write_all(format!("{started}\n{}", token.unwrap_or_default()).as_bytes())
}

/// A live engine's record; dropping it removes the file.
#[derive(Debug)]
pub(crate) struct EngineRecord(PathBuf);

impl EngineRecord {
    /// Record `pid` and its [`tag`] token, if a registry is set and its start
    /// time is readable.
    pub(crate) fn register(pid: Option<u32>, token: Option<&str>) -> Option<Self> {
        let dir = REGISTRY.get()?;
        let pid = pid?;
        let path = dir.join(pid.to_string());
        let started = start_time(pid)?;
        match write_record(&path, started, token) {
            Ok(()) => Some(Self(path)),
            Err(error) => {
                tracing::warn!(pid, %error, "could not record the engine process");
                None
            }
        }
    }
}

impl Drop for EngineRecord {
    fn drop(&mut self) {
        match std::fs::remove_file(&self.0) {
            Ok(()) => {}
            Err(error) if error.kind() == io::ErrorKind::NotFound => {}
            Err(error) => {
                tracing::warn!(path = %self.0.display(), %error, "could not remove an engine record");
            }
        }
    }
}

#[cfg(all(test, target_os = "linux"))]
mod tests {
    use super::*;
    use std::os::unix::process::CommandExt as _;

    #[allow(clippy::unwrap_used)]
    fn spawn_group() -> std::process::Child {
        std::process::Command::new("sleep")
            .arg("30")
            .process_group(0)
            .spawn()
            .unwrap()
    }

    #[allow(clippy::unwrap_used)]
    fn alive(child: &mut std::process::Child) -> bool {
        child.try_wait().unwrap().is_none()
    }

    #[test]
    fn reaps_a_matching_record_and_clears_it() {
        let dir = tempfile::tempdir().unwrap();
        let mut child = spawn_group();
        let pid = child.id();
        std::fs::write(
            dir.path().join(pid.to_string()),
            start_time(pid).unwrap().to_string(),
        )
        .unwrap();
        assert_eq!(reap(dir.path()).unwrap(), vec![pid]);
        child.wait().expect("killed child is reaped");
        assert_eq!(std::fs::read_dir(dir.path()).unwrap().count(), 0);
    }

    /// Poll up to 5s until a `sleep 31` process exists (or not), per `running`.
    #[allow(clippy::unwrap_used)]
    fn wait_for_sleep31(running: bool) -> bool {
        for _ in 0..100 {
            let found = std::process::Command::new("pgrep")
                .args(["-f", "^sleep 31$"])
                .output()
                .unwrap();
            if found.stdout.is_empty() != running {
                return true;
            }
            std::thread::sleep(std::time::Duration::from_millis(50));
        }
        false
    }

    #[test]
    fn reaps_descendants_outside_the_group_by_token() {
        let dir = tempfile::tempdir().unwrap();
        let token = "tok-reaps-descendants-0001";
        let mut child = std::process::Command::new("sh")
            .args(["-c", "setsid sleep 31 & wait"])
            .env(TOKEN_ENV, token)
            .process_group(0)
            .spawn()
            .unwrap();
        let pid = child.id();
        let started = start_time(pid).unwrap();
        std::fs::write(
            dir.path().join(pid.to_string()),
            format!("{started}\n{token}"),
        )
        .unwrap();
        assert!(
            wait_for_sleep31(true),
            "the setsid sleep must be running before the reap"
        );
        reap(dir.path()).unwrap();
        child.wait().unwrap();
        let gone = wait_for_sleep31(false);
        assert!(gone, "the setsid descendant must be killed by its token");
    }

    #[test]
    fn keeps_a_record_it_cannot_read() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("4194300"), [0xff, 0xfe]).unwrap();
        assert!(reap(dir.path()).is_err());
        assert!(dir.path().join("4194300").exists());
    }

    #[test]
    fn leaves_a_process_whose_start_time_differs() {
        let dir = tempfile::tempdir().unwrap();
        let mut child = spawn_group();
        let pid = child.id();
        let stale = start_time(pid).unwrap() + 1;
        std::fs::write(dir.path().join(pid.to_string()), stale.to_string()).unwrap();
        assert!(reap(dir.path()).unwrap().is_empty());
        assert!(
            alive(&mut child),
            "mismatched start time must not be signalled"
        );
        assert_eq!(std::fs::read_dir(dir.path()).unwrap().count(), 0);
        child.kill().unwrap();
        child.wait().unwrap();
    }

    #[test]
    fn leaves_a_matching_process_that_is_not_a_group_leader() {
        let dir = tempfile::tempdir().unwrap();
        let mut child = std::process::Command::new("sleep")
            .arg("30")
            .spawn()
            .unwrap();
        let pid = child.id();
        std::fs::write(
            dir.path().join(pid.to_string()),
            start_time(pid).unwrap().to_string(),
        )
        .unwrap();
        assert!(reap(dir.path()).unwrap().is_empty());
        assert!(alive(&mut child));
        child.kill().unwrap();
        child.wait().unwrap();
    }

    #[test]
    fn refuses_a_registry_other_users_can_write() {
        use std::os::unix::fs::PermissionsExt as _;
        let dir = tempfile::tempdir().unwrap();
        std::fs::set_permissions(dir.path(), std::fs::Permissions::from_mode(0o777)).unwrap();
        assert!(init(dir.path()).is_err());
    }

    #[test]
    fn ignores_foreign_files_and_dead_pids() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("notes.txt"), "x").unwrap();
        std::fs::write(dir.path().join("4194300"), "1").unwrap();
        assert!(reap(dir.path()).unwrap().is_empty());
        assert!(dir.path().join("notes.txt").exists());
    }
}
