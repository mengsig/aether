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
//! One that also carries an agent's tag, such as a server an agent started, is
//! left to the runtime's start sweep, which stops it only if no live agent holds
//! it, unless that agent's turn was in flight: then it may be the tool command
//! the engine ran outside its group (Claude Code runs each in its own session),
//! which the resumed turn must not race. One carrying the keep entry is never
//! signalled.

use std::collections::HashSet;
use std::io;
use std::path::{Path, PathBuf};
use std::sync::OnceLock;

use aether_core::id::AgentId;
use nix::sys::signal::{Signal, killpg};
use nix::unistd::{Pid, getpgid};

static REGISTRY: OnceLock<PathBuf> = OnceLock::new();

/// Environment variable carrying an engine's token to everything it spawns.
const TOKEN_ENV: &str = "AETHER_ENGINE_RUN";

/// Export a fresh token to `command`'s child, if engines are being recorded.
/// Pass the result to [`crate::lifeline::spawn_recorded`].
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
/// engines this process spawns there. `interrupted` names the agents whose
/// turn that daemon left in flight. Returns the leader pids it killed.
pub fn init(dir: &Path, interrupted: &HashSet<AgentId>) -> io::Result<Vec<u32>> {
    prepare_dir(dir)?;
    let reaped = reap(dir, interrupted)?;
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

/// Kill every recorded group whose leader still matches, and its token
/// carriers but those [`spared`] given `interrupted`, and clear the records.
pub fn reap(dir: &Path, interrupted: &HashSet<AgentId>) -> io::Result<Vec<u32>> {
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
            kill_tagged(token, interrupted)
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

/// SIGKILL every live process of this user whose environment carries `token`,
/// but one an agent's tag or the keep entry spares ([`spared`]).
///
/// The token is random per engine spawn, so a match proves descent from that
/// engine. A pidfd is opened before the environment is read, so the pid
/// cannot be reused between the check and the signal. Unexpected errors are
/// returned so the caller keeps the record and the next start retries.
#[cfg(target_os = "linux")]
fn kill_tagged(token: &str, interrupted: &HashSet<AgentId>) -> io::Result<()> {
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
            || spared(&environment, interrupted)
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

/// Whether a token carrier with `environment` is left alone: it is kept, or
/// runs for an agent whose turn was not `interrupted`, so the runtime's start
/// sweep decides its ownership. A tag the daemon inherited from an agent's
/// shell spares too: that agent's end, under the daemon that runs it, stops it.
#[cfg(target_os = "linux")]
fn spared(environment: &[u8], interrupted: &HashSet<AgentId>) -> bool {
    let agent = format!("{}=", aether_core::agent::PROCESS_AGENT_ENV);
    let keep = format!("{}=1", aether_core::agent::PROCESS_KEEP_ENV);
    let entries = || environment.split(|b| *b == 0);
    if entries().any(|entry| entry == keep.as_bytes()) {
        return true;
    }
    entries().any(|entry| {
        let Some(tag) = entry
            .strip_prefix(agent.as_bytes())
            .filter(|tag| !tag.is_empty())
        else {
            return false;
        };
        // An unreadable tag is no agent of this store, so none whose turn was cut short.
        let cut_short = std::str::from_utf8(tag)
            .ok()
            .and_then(|tag| tag.parse::<AgentId>().ok())
            .is_some_and(|agent| interrupted.contains(&agent));
        !cut_short
    })
}

#[cfg(not(target_os = "linux"))]
fn kill_tagged(_token: &str, _interrupted: &HashSet<AgentId>) -> io::Result<()> {
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
    /// Record engine `pid`, held before its exec, with its [`tag`] token. None
    /// when no registry is set or no start time exists (off Linux); an error
    /// when a set registry cannot hold it, so the engine never runs unrecorded.
    pub(crate) fn register(pid: u32, token: &str) -> io::Result<Option<Self>> {
        let Some(dir) = REGISTRY.get() else {
            return Ok(None);
        };
        assert!(pid > 1, "not an engine pid: {pid}");
        assert!(!token.is_empty(), "an empty token would match any process");
        let Some(started) = start_time(pid) else {
            if cfg!(target_os = "linux") {
                return Err(io::Error::other(format!(
                    "engine {pid} has no readable start time"
                )));
            }
            return Ok(None);
        };
        let path = dir.join(pid.to_string());
        write_record(&path, started, Some(token)).map_err(|error| {
            io::Error::new(
                error.kind(),
                format!("recording engine {pid} in {}: {error}", dir.display()),
            )
        })?;
        Ok(Some(Self(path)))
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
        assert_eq!(reap(dir.path(), &HashSet::new()).unwrap(), vec![pid]);
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
            .env_remove(aether_core::agent::PROCESS_AGENT_ENV)
            .env_remove(aether_core::agent::PROCESS_KEEP_ENV)
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
        reap(dir.path(), &HashSet::new()).unwrap();
        child.wait().unwrap();
        let gone = wait_for_sleep31(false);
        assert!(gone, "the setsid descendant must be killed by its token");
    }

    /// Poll up to 5s until `child` has exited.
    #[allow(clippy::unwrap_used)]
    fn exits_soon(child: &mut std::process::Child) -> bool {
        for _ in 0..100 {
            if child.try_wait().unwrap().is_some() {
                return true;
            }
            std::thread::sleep(std::time::Duration::from_millis(50));
        }
        false
    }

    /// A token carrier with `extra` in its environment, outside any group of ours.
    #[allow(clippy::unwrap_used)]
    fn carrier(token: &str, extra: &[(&str, &str)]) -> std::process::Child {
        let mut command = std::process::Command::new("sleep");
        command
            .arg("30")
            .env(TOKEN_ENV, token)
            .env_remove(aether_core::agent::PROCESS_AGENT_ENV)
            .env_remove(aether_core::agent::PROCESS_KEEP_ENV)
            .process_group(0);
        for (key, value) in extra {
            command.env(key, value);
        }
        command.spawn().unwrap()
    }

    #[test]
    fn spares_token_carriers_an_agent_holds_or_that_are_kept() {
        use aether_core::agent::{PROCESS_AGENT_ENV, PROCESS_KEEP_ENV};
        let dir = tempfile::tempdir().unwrap();
        let token = "tok-spares-agent-servers-0001";
        let agent = aether_core::id::AgentId::new().to_string();
        let mut plain = carrier(token, &[]);
        let mut server = carrier(token, &[(PROCESS_AGENT_ENV, &agent)]);
        let mut kept = carrier(token, &[(PROCESS_KEEP_ENV, "1")]);
        // A dead leader: only the token sweep runs.
        std::fs::write(dir.path().join("4194300"), format!("1\n{token}")).unwrap();
        reap(dir.path(), &HashSet::new()).unwrap();
        assert!(exits_soon(&mut plain), "a carrier no agent holds is killed");
        assert!(alive(&mut server), "an agent's server is left to its owner");
        assert!(alive(&mut kept), "a kept carrier is never signalled");
        for child in [&mut server, &mut kept] {
            child.kill().unwrap();
            child.wait().unwrap();
        }
    }

    /// A recovered agent must not race the tool command its engine was running
    /// when the daemon died, in the engine's group or, as Claude Code runs it,
    /// in a session of its own; its kept server and an idle agent's stay up.
    #[test]
    fn stops_an_interrupted_agents_engine_commands_but_spares_kept_and_idle_servers() {
        use aether_core::agent::{PROCESS_AGENT_ENV, PROCESS_KEEP_ENV};
        let dir = tempfile::tempdir().unwrap();
        let token = "tok-interrupted-engine-group-0001";
        let interrupted = AgentId::new();
        let cut_short = interrupted.to_string();
        let idle = AgentId::new().to_string();
        let in_group = |group: i32| {
            std::process::Command::new("sleep")
                .arg("30")
                .env(TOKEN_ENV, token)
                .env(PROCESS_AGENT_ENV, &cut_short)
                .env_remove(PROCESS_KEEP_ENV)
                .process_group(group)
                .spawn()
                .unwrap()
        };
        let mut engine = in_group(0);
        let mut tool = in_group(engine.id() as i32);
        let mut own_session_tool = carrier(token, &[(PROCESS_AGENT_ENV, &cut_short)]);
        let mut kept = carrier(
            token,
            &[(PROCESS_AGENT_ENV, &cut_short), (PROCESS_KEEP_ENV, "1")],
        );
        let mut idle_server = carrier(token, &[(PROCESS_AGENT_ENV, &idle)]);
        let pid = engine.id();
        let started = start_time(pid).unwrap();
        std::fs::write(
            dir.path().join(pid.to_string()),
            format!("{started}\n{token}"),
        )
        .unwrap();
        let reaped = reap(dir.path(), &HashSet::from([interrupted])).unwrap();
        let stopped = [&mut engine, &mut tool, &mut own_session_tool].map(exits_soon);
        let survived = [&mut kept, &mut idle_server].map(alive);
        for child in [
            &mut engine,
            &mut tool,
            &mut own_session_tool,
            &mut kept,
            &mut idle_server,
        ] {
            child.kill().unwrap();
            child.wait().unwrap();
        }
        assert_eq!(reaped, vec![pid]);
        assert_eq!(
            stopped, [true; 3],
            "the engine and both its tool commands are stopped"
        );
        assert_eq!(
            survived, [true; 2],
            "a kept server and an idle agent's server stay up"
        );
    }

    /// Runs the named test of this binary again with the daemon tagged `tag`.
    #[allow(clippy::unwrap_used)]
    fn rerun_tagged(test: &str, tag: &str) -> std::process::ExitStatus {
        std::process::Command::new(std::env::current_exe().unwrap())
            .args(["--exact", test, "--test-threads=1"])
            .env(aether_core::agent::PROCESS_AGENT_ENV, tag)
            .stdout(std::process::Stdio::null())
            .status()
            .unwrap()
    }

    #[test]
    fn spares_a_carrier_tagged_like_the_daemon_itself() {
        use aether_core::agent::PROCESS_AGENT_ENV;
        const TAG: &str = "agt-the-daemon-inherited-this-tag";
        const NAME: &str = "orphans::tests::spares_a_carrier_tagged_like_the_daemon_itself";
        if std::env::var(PROCESS_AGENT_ENV).as_deref() != Ok(TAG) {
            assert!(rerun_tagged(NAME, TAG).success(), "the tagged rerun passes");
            return;
        }
        let dir = tempfile::tempdir().unwrap();
        let token = "tok-spares-the-daemon-tag-0001";
        let mut server = carrier(token, &[(PROCESS_AGENT_ENV, TAG)]);
        std::fs::write(dir.path().join("4194300"), format!("1\n{token}")).unwrap();
        reap(dir.path(), &HashSet::new()).unwrap();
        let killed = exits_soon(&mut server);
        if !killed {
            server.kill().unwrap();
            server.wait().unwrap();
        }
        assert!(
            !killed,
            "the agent the daemon was started from keeps its server"
        );
    }

    #[test]
    fn spares_a_tagged_process_marked_kept() {
        let dir = tempfile::tempdir().unwrap();
        let token = "tok-spares-kept-0001";
        let mut swept = carrier(token, &[]);
        let mut kept = carrier(token, &[(aether_core::agent::PROCESS_KEEP_ENV, "1")]);
        // No start time: only the token sweep runs for this record.
        std::fs::write(dir.path().join("4194301"), format!("-\n{token}")).unwrap();
        reap(dir.path(), &HashSet::new()).unwrap();
        assert!(
            !swept.wait().unwrap().success(),
            "the unmarked process is killed"
        );
        assert!(alive(&mut kept), "the kept process is spared");
        kept.kill().unwrap();
        kept.wait().unwrap();
    }

    #[test]
    fn keeps_a_record_it_cannot_read() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("4194300"), [0xff, 0xfe]).unwrap();
        assert!(reap(dir.path(), &HashSet::new()).is_err());
        assert!(dir.path().join("4194300").exists());
    }

    #[test]
    fn leaves_a_process_whose_start_time_differs() {
        let dir = tempfile::tempdir().unwrap();
        let mut child = spawn_group();
        let pid = child.id();
        let stale = start_time(pid).unwrap() + 1;
        std::fs::write(dir.path().join(pid.to_string()), stale.to_string()).unwrap();
        assert!(reap(dir.path(), &HashSet::new()).unwrap().is_empty());
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
        assert!(reap(dir.path(), &HashSet::new()).unwrap().is_empty());
        assert!(alive(&mut child));
        child.kill().unwrap();
        child.wait().unwrap();
    }

    #[test]
    fn refuses_a_registry_other_users_can_write() {
        use std::os::unix::fs::PermissionsExt as _;
        let dir = tempfile::tempdir().unwrap();
        std::fs::set_permissions(dir.path(), std::fs::Permissions::from_mode(0o777)).unwrap();
        assert!(init(dir.path(), &HashSet::new()).is_err());
    }

    #[test]
    fn ignores_foreign_files_and_dead_pids() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("notes.txt"), "x").unwrap();
        std::fs::write(dir.path().join("4194300"), "1").unwrap();
        assert!(reap(dir.path(), &HashSet::new()).unwrap().is_empty());
        assert!(dir.path().join("notes.txt").exists());
    }

    /// Runs [`unrecordable_engine_helper`] in its own process: the registry
    /// it sets is process-wide.
    const UNRECORDABLE_ENV: &str = "AETHER_TEST_UNRECORDABLE_ENGINE";

    /// An engine whose record cannot be written never runs: a daemon crash
    /// would leave its commands where the next start cannot find them.
    #[test]
    fn an_engine_that_cannot_be_recorded_never_runs() {
        let work = tempfile::tempdir().unwrap();
        let output = std::process::Command::new(std::env::current_exe().unwrap())
            .args([
                "--exact",
                "orphans::tests::unrecordable_engine_helper",
                "--nocapture",
            ])
            .env(UNRECORDABLE_ENV, work.path())
            .output()
            .unwrap();
        let stdout = String::from_utf8_lossy(&output.stdout);
        if stdout.contains("skipped: the registry stays writable") {
            return;
        }
        let stderr = String::from_utf8_lossy(&output.stderr);
        assert!(output.status.success(), "{stdout}{stderr}");
        assert!(stdout.contains("refused: "), "{stdout}");
        assert!(
            !work.path().join("ran").exists(),
            "the unrecorded engine ran"
        );
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn unrecordable_engine_helper() {
        use std::os::unix::fs::PermissionsExt as _;

        let Some(work) = std::env::var_os(UNRECORDABLE_ENV) else {
            return;
        };
        let work = PathBuf::from(work);
        let registry = work.join("registry");
        init(&registry, &HashSet::new()).unwrap();
        std::fs::set_permissions(&registry, std::fs::Permissions::from_mode(0o500)).unwrap();
        if std::fs::write(registry.join("probe"), "").is_ok() {
            println!("skipped: the registry stays writable");
            return;
        }
        let mut command = tokio::process::Command::new("touch");
        command.arg(work.join("ran"));
        let token = tag(&mut command).expect("the registry is set");
        match crate::lifeline::spawn_recorded(command, Some(&token)) {
            Ok((mut child, _watch, _record)) => {
                let _ = child.wait().await;
                panic!("an engine ran unrecorded");
            }
            Err(error) => println!("refused: {error}"),
        }
    }
}
