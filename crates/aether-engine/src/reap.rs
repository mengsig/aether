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

/// How many SIGKILL rounds catch processes forked while the last one landed.
#[cfg(target_os = "linux")]
const KILL_ROUNDS: usize = 5;

/// The processes an agent's end had to stop.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct ReapReport {
    /// Sent SIGTERM: alive at the start, or forked during the grace.
    pub terminated: Vec<u32>,
    /// Still alive after the grace, then sent SIGKILL.
    pub killed: Vec<u32>,
    /// Alive even after every SIGKILL round.
    pub survived: Vec<u32>,
}

impl ReapReport {
    pub fn is_empty(&self) -> bool {
        self.terminated.is_empty() && self.killed.is_empty() && self.survived.is_empty()
    }
}

/// SIGTERM every live process of `agent`'s engines, including those forked
/// meanwhile, then SIGKILL what outlives `grace`. `ended` is asked before
/// each signal: once it is false - the agent was restarted, and its next
/// engine carries the same tag - nothing more is signalled.
#[cfg(target_os = "linux")]
pub async fn reap_agent(agent: AgentId, grace: Duration, ended: impl Fn() -> bool) -> ReapReport {
    use nix::sys::signal::Signal;

    let sweep = linux::Sweep::new(agent).await;
    let mut report = ReapReport::default();
    let deadline = tokio::time::Instant::now() + grace;
    let mut alive = sweep.members();
    // The first round always runs: even a zero grace sends SIGTERM first.
    while !alive.is_empty() {
        let fresh: Vec<u32> = alive
            .iter()
            .copied()
            .filter(|pid| report.terminated.binary_search(pid).is_err())
            .collect();
        if !fresh.is_empty() {
            if !ended() {
                return stopped_early(agent, report);
            }
            sweep.signal(&fresh, Signal::SIGTERM).await;
            report.terminated.extend(fresh);
            report.terminated.sort_unstable();
        }
        if tokio::time::Instant::now() >= deadline {
            break;
        }
        tokio::time::sleep(linux::POLL).await;
        alive = sweep.members();
    }
    for _ in 0..KILL_ROUNDS {
        let alive = sweep.members();
        if alive.is_empty() {
            break;
        }
        if !ended() {
            return stopped_early(agent, report);
        }
        sweep.signal(&alive, Signal::SIGKILL).await;
        report.killed.extend(&alive);
        tokio::time::sleep(linux::POLL).await;
    }
    report.killed.sort_unstable();
    report.killed.dedup();
    report.survived = sweep.members();
    log(agent, &report);
    report
}

#[cfg(target_os = "linux")]
fn stopped_early(agent: AgentId, report: ReapReport) -> ReapReport {
    tracing::info!(%agent, ?report, "agent resumed; stopped reaping its processes");
    report
}

#[cfg(not(target_os = "linux"))]
pub async fn reap_agent(agent: AgentId, _grace: Duration, _ended: impl Fn() -> bool) -> ReapReport {
    tracing::debug!(%agent, "engine processes cannot be found again on this platform");
    ReapReport::default()
}

#[cfg(target_os = "linux")]
fn log(agent: AgentId, report: &ReapReport) {
    if !report.survived.is_empty() {
        tracing::error!(%agent, ?report, "agent processes survived SIGKILL");
    } else if !report.killed.is_empty() {
        tracing::warn!(%agent, ?report, "agent processes outlived SIGTERM and were killed");
    } else if !report.terminated.is_empty() {
        tracing::info!(%agent, ?report, "agent processes stopped by SIGTERM");
    }
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

    /// Where one agent's processes are looked for: the scopes listed at
    /// its end, and its tag unless the daemon carries that itself.
    pub(super) struct Sweep {
        units: Vec<Unit>,
        tag: Option<String>,
    }

    impl Sweep {
        pub(super) async fn new(agent: AgentId) -> Self {
            Self {
                units: crate::scope::agent_units(agent).await,
                tag: tag_entry(agent),
            }
        }

        /// Live pids in the scopes or carrying the tag, sorted, without this process.
        pub(super) fn members(&self) -> Vec<u32> {
            let proc_root = Path::new(PROC);
            let own = std::process::id();
            let uid = nix::unistd::geteuid().as_raw();
            let mut members = self
                .tag
                .as_deref()
                .map_or_else(Vec::new, |tag| tagged(proc_root, tag, own, uid));
            for unit in &self.units {
                members.extend(cgroup_members(proc_root, &unit.procs, own));
            }
            members.sort_unstable();
            members.dedup();
            members
        }

        /// Signal the scopes whole and each of `pids` that still carries the tag.
        pub(super) async fn signal(&self, pids: &[u32], signal: Signal) {
            assert!(!pids.is_empty(), "a signal round needs a target");
            crate::scope::signal_units(&self.units, signal).await;
            let Some(tag) = self.tag.as_deref() else {
                return;
            };
            let uid = nix::unistd::geteuid().as_raw();
            for pid in pids {
                if let Err(error) = signal_pinned(*pid, tag, uid, signal) {
                    tracing::warn!(pid, ?signal, %error, "could not signal an agent process");
                }
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
