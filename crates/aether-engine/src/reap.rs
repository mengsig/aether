//! Ending every process an agent's engines left behind, once the agent ends.
//!
//! The processes are found by the agent's tag in their environment and, on
//! Linux with systemd, in the agent's engine scopes ([`crate::scope`]). A
//! daemon shutdown never calls this: the processes of running agents survive
//! it, and the next start recovers their engines through [`crate::orphans`].
//! Elsewhere than Linux nothing can be found again after a turn, so only the
//! turn's own process-group stop applies.

use std::time::Duration;

use aether_core::id::AgentId;

/// The processes an agent's end had to stop.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct ReapReport {
    /// Alive when SIGTERM was sent.
    pub terminated: Vec<u32>,
    /// Still alive after the grace, then sent SIGKILL.
    pub killed: Vec<u32>,
}

impl ReapReport {
    pub fn is_empty(&self) -> bool {
        self.terminated.is_empty() && self.killed.is_empty()
    }
}

/// SIGTERM every live process of `agent`'s engines, then SIGKILL what
/// outlives `grace`. Returns what it signalled. Only the processes and scopes
/// found at the start are signalled, so an agent restarted within the grace
/// keeps its new engine.
#[cfg(target_os = "linux")]
pub async fn reap_agent(agent: AgentId, grace: Duration) -> ReapReport {
    use nix::sys::signal::Signal;

    let units = crate::scope::agent_units(agent).await;
    let tag = linux::tag_entry(agent);
    let terminated = linux::members(&units, tag.as_deref());
    if terminated.is_empty() {
        return ReapReport::default();
    }
    linux::signal(&units, tag.as_deref(), &terminated, Signal::SIGTERM).await;
    let deadline = tokio::time::Instant::now() + grace;
    let mut survivors = linux::survivors(&units, tag.as_deref(), &terminated);
    while !survivors.is_empty() && tokio::time::Instant::now() < deadline {
        tokio::time::sleep(linux::POLL).await;
        survivors = linux::survivors(&units, tag.as_deref(), &terminated);
    }
    if !survivors.is_empty() {
        linux::signal(&units, tag.as_deref(), &survivors, Signal::SIGKILL).await;
    }
    let report = ReapReport {
        terminated,
        killed: survivors,
    };
    log(agent, &report);
    report
}

#[cfg(not(target_os = "linux"))]
pub async fn reap_agent(agent: AgentId, _grace: Duration) -> ReapReport {
    tracing::debug!(%agent, "engine processes cannot be found again on this platform");
    ReapReport::default()
}

#[cfg(target_os = "linux")]
fn log(agent: AgentId, report: &ReapReport) {
    if report.killed.is_empty() {
        tracing::info!(
            %agent,
            terminated = report.terminated.len(),
            pids = ?report.terminated,
            "agent processes stopped by SIGTERM"
        );
        return;
    }
    tracing::warn!(
        %agent,
        terminated = report.terminated.len(),
        killed = report.killed.len(),
        pids = ?report.terminated,
        killed_pids = ?report.killed,
        "agent processes survived SIGTERM and were killed"
    );
}

#[cfg(target_os = "linux")]
mod linux {
    use std::io;
    use std::path::Path;
    use std::time::Duration;

    use aether_core::id::AgentId;
    use nix::sys::signal::Signal;

    use crate::scope::{AGENT_ENV, Unit};

    pub(super) const POLL: Duration = Duration::from_millis(100);
    const PROC: &str = "/proc";

    /// The environment entry tagging `agent`'s processes, or none when this
    /// daemon carries it itself: it was started from that agent's shell, and
    /// its own children would match.
    pub(super) fn tag_entry(agent: AgentId) -> Option<String> {
        let tag = agent.to_string();
        if std::env::var(AGENT_ENV).is_ok_and(|own| own == tag) {
            tracing::warn!(%agent, "the daemon runs under this agent's tag; sparing tagged processes");
            return None;
        }
        Some(format!("{AGENT_ENV}={tag}"))
    }

    /// Live pids in `units` or carrying `tag`, sorted and without this process.
    pub(super) fn members(units: &[Unit], tag: Option<&str>) -> Vec<u32> {
        let proc_root = Path::new(PROC);
        let own = std::process::id();
        let uid = nix::unistd::geteuid().as_raw();
        let mut members = tag.map_or_else(Vec::new, |tag| tagged(proc_root, tag, own, uid));
        for unit in units {
            members.extend(cgroup_members(proc_root, &unit.procs, own));
        }
        members.sort_unstable();
        members.dedup();
        members
    }

    /// The members of `first` still alive: a process forked since is ended
    /// with its scope, never by its tag alone.
    pub(super) fn survivors(units: &[Unit], tag: Option<&str>, first: &[u32]) -> Vec<u32> {
        debug_assert!(first.is_sorted(), "members are sorted");
        let mut alive = members(units, tag);
        alive.retain(|pid| first.binary_search(pid).is_ok());
        alive
    }

    /// Signal `units` whole and each of `pids` that still carries `tag`.
    pub(super) async fn signal(units: &[Unit], tag: Option<&str>, pids: &[u32], signal: Signal) {
        crate::scope::signal_units(units, signal).await;
        let Some(tag) = tag else {
            return;
        };
        let uid = nix::unistd::geteuid().as_raw();
        for pid in pids {
            if let Err(error) = signal_pinned(*pid, tag, uid, signal) {
                tracing::warn!(pid, ?signal, %error, "could not signal an agent process");
            }
        }
    }

    /// Signal `pid` through a pidfd opened before its tag is checked again, so
    /// a pid reused since the scan is never signalled.
    fn signal_pinned(pid: u32, tag: &str, uid: u32, signal: Signal) -> io::Result<()> {
        use rustix::process::{Pid, PidfdFlags, Signal as RustixSignal};

        assert!(pid > 1, "not an agent process: {pid}");
        let Some(raw) = Pid::from_raw(pid as i32) else {
            return Ok(());
        };
        let pidfd = match rustix::process::pidfd_open(raw, PidfdFlags::empty()) {
            Ok(pidfd) => pidfd,
            Err(rustix::io::Errno::SRCH) => return Ok(()),
            Err(errno) => return Err(errno.into()),
        };
        if !carries(Path::new(PROC), pid, tag, uid) {
            return Ok(());
        }
        let signal = match signal {
            Signal::SIGTERM => RustixSignal::TERM,
            Signal::SIGKILL => RustixSignal::KILL,
            other => return Err(io::Error::other(format!("{other} does not end a process"))),
        };
        match rustix::process::pidfd_send_signal(&pidfd, signal) {
            Ok(()) | Err(rustix::io::Errno::SRCH) => Ok(()),
            Err(errno) => Err(errno.into()),
        }
    }

    /// Pids under `proc_root` of user `uid` that are alive and carry `tag`, sorted, except `own`.
    pub(super) fn tagged(proc_root: &Path, tag: &str, own: u32, uid: u32) -> Vec<u32> {
        assert!(
            tag.contains('='),
            "a tag is a whole environment entry: {tag}"
        );
        let Ok(dir) = std::fs::read_dir(proc_root) else {
            tracing::warn!(root = %proc_root.display(), "cannot list processes to reap");
            return Vec::new();
        };
        let mut pids: Vec<u32> = dir
            .filter_map(Result::ok)
            .filter_map(|entry| entry.file_name().to_str()?.parse::<u32>().ok())
            .filter(|pid| *pid > 1 && *pid != own && carries(proc_root, *pid, tag, uid))
            .collect();
        pids.sort_unstable();
        pids
    }

    /// Whether `pid` is a live process of `uid` whose environment holds `tag` exactly.
    pub(super) fn carries(proc_root: &Path, pid: u32, tag: &str, uid: u32) -> bool {
        use std::os::unix::fs::MetadataExt as _;

        let dir = proc_root.join(pid.to_string());
        // Another user's process is never ours, whatever it carries.
        if !std::fs::metadata(&dir).is_ok_and(|meta| meta.uid() == uid) {
            return false;
        }
        if !is_live(proc_root, pid) {
            return false;
        }
        // Gone or not readable: not one to signal.
        let Ok(environment) = std::fs::read(dir.join("environ")) else {
            return false;
        };
        environment
            .split(|byte| *byte == 0)
            .any(|entry| entry == tag.as_bytes())
    }

    /// Live pids listed in a cgroup's `cgroup.procs`, except `own`. A cgroup
    /// that is gone has none.
    pub(super) fn cgroup_members(proc_root: &Path, procs: &Path, own: u32) -> Vec<u32> {
        let Ok(listed) = std::fs::read_to_string(procs) else {
            return Vec::new();
        };
        listed
            .lines()
            .filter_map(|line| line.trim().parse::<u32>().ok())
            .filter(|pid| *pid > 1 && *pid != own && is_live(proc_root, *pid))
            .collect()
    }

    /// Whether `pid` exists and is not a zombie, which no signal can end.
    pub(super) fn is_live(proc_root: &Path, pid: u32) -> bool {
        let Ok(stat) = std::fs::read_to_string(proc_root.join(pid.to_string()).join("stat")) else {
            return false;
        };
        // The command name may hold spaces and parentheses; the state follows the last ')'.
        let state = stat
            .rfind(')')
            .and_then(|end| stat[end + 1..].split_whitespace().next());
        state.is_some_and(|state| state != "Z" && state != "X")
    }
}

#[cfg(all(test, target_os = "linux"))]
mod tests;
