//! Ending every process an agent left behind, once the agent ends or the
//! daemon stops.
//!
//! The processes are found by the agent's tag in their environment, set on
//! its engines and host shell commands, and, on Linux with systemd, in the
//! agent's engine scopes ([`crate::scope`]). A daemon shutdown reaps the
//! agents [`agents_with_processes`] names that its own store ran, in one sweep.
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

/// SIGTERM every live process of `agent`'s engines and shell commands,
/// including those forked meanwhile, then SIGKILL what outlives `grace`.
/// `ended` is asked before each signal: once it is false - the agent was
/// restarted, and its next engine carries the same tag - nothing more is signalled.
#[cfg(target_os = "linux")]
pub async fn reap_agent(agent: AgentId, grace: Duration, ended: impl Fn() -> bool) -> ReapReport {
    let sweep = linux::Sweep::new(&[agent]).await;
    reap(&sweep, grace, ended).await
}

/// [`reap_agent`] for every one of `agents` at once, as the daemon stops:
/// one scan of the processes per round, however many agents there are.
#[cfg(target_os = "linux")]
pub async fn reap_agents(agents: &[AgentId], grace: Duration) -> ReapReport {
    if agents.is_empty() {
        return ReapReport::default();
    }
    let sweep = linux::Sweep::new(agents).await;
    reap(&sweep, grace, || true).await
}

/// The agents some live process of this user is tagged with, or that still
/// have an engine scope, sorted; never the agent this daemon runs under itself.
/// Another daemon's agents are among them: the caller keeps its own.
#[cfg(target_os = "linux")]
pub async fn agents_with_processes() -> Vec<AgentId> {
    let uid = nix::unistd::geteuid().as_raw();
    let mut agents = linux::tagged_agents(std::path::Path::new("/proc"), std::process::id(), uid);
    agents.extend(crate::scope::agents_with_units().await);
    let own: Option<AgentId> = std::env::var(aether_core::agent::PROCESS_AGENT_ENV)
        .ok()
        .and_then(|tag| tag.parse().ok());
    agents.retain(|agent| Some(*agent) != own);
    agents.sort_unstable();
    agents.dedup();
    agents
}

#[cfg(target_os = "linux")]
async fn reap(sweep: &linux::Sweep, grace: Duration, ended: impl Fn() -> bool) -> ReapReport {
    use nix::sys::signal::Signal;

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
                return stopped_early(sweep, report);
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
            return stopped_early(sweep, report);
        }
        sweep.signal(&alive, Signal::SIGKILL).await;
        report.killed.extend(&alive);
        tokio::time::sleep(linux::POLL).await;
    }
    report.killed.sort_unstable();
    report.killed.dedup();
    report.survived = sweep.members();
    log(sweep.agents(), &report);
    report
}

#[cfg(target_os = "linux")]
fn stopped_early(sweep: &linux::Sweep, report: ReapReport) -> ReapReport {
    let agents = sweep.agents();
    tracing::info!(
        ?agents,
        ?report,
        "agent resumed; stopped reaping its processes"
    );
    report
}

#[cfg(not(target_os = "linux"))]
pub async fn reap_agent(agent: AgentId, _grace: Duration, _ended: impl Fn() -> bool) -> ReapReport {
    tracing::debug!(%agent, "agent processes cannot be found again on this platform");
    ReapReport::default()
}

#[cfg(not(target_os = "linux"))]
pub async fn reap_agents(_agents: &[AgentId], _grace: Duration) -> ReapReport {
    ReapReport::default()
}

/// Nothing can be found again on this platform.
#[cfg(not(target_os = "linux"))]
pub async fn agents_with_processes() -> Vec<AgentId> {
    Vec::new()
}

#[cfg(target_os = "linux")]
fn log(agents: &[AgentId], report: &ReapReport) {
    if !report.survived.is_empty() {
        tracing::error!(?agents, ?report, "agent processes survived SIGKILL");
    } else if !report.killed.is_empty() {
        tracing::warn!(
            ?agents,
            ?report,
            "agent processes outlived SIGTERM and were killed"
        );
    } else if !report.terminated.is_empty() {
        tracing::info!(?agents, ?report, "agent processes stopped by SIGTERM");
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

    /// Where some agents' processes are looked for: the scopes listed at the
    /// start, once their engines are in them, and their tags but the one the
    /// daemon carries itself.
    pub(super) struct Sweep {
        agents: Vec<AgentId>,
        units: Vec<Unit>,
        tags: Vec<String>,
    }

    impl Sweep {
        pub(super) async fn new(agents: &[AgentId]) -> Self {
            assert!(!agents.is_empty(), "a sweep needs an agent to look for");
            // An engine spawned before the caller's fence may not be in its scope yet.
            if !crate::scope::settled(agents, crate::scope::ADOPTION_BUDGET).await {
                tracing::warn!(
                    ?agents,
                    "an engine is still joining its scope; sweeping without it"
                );
            }
            Self {
                agents: agents.to_vec(),
                units: crate::scope::agent_units(agents).await,
                tags: agents
                    .iter()
                    .filter_map(|agent| tag_entry(*agent))
                    .collect(),
            }
        }

        pub(super) fn agents(&self) -> &[AgentId] {
            &self.agents
        }

        /// Live pids in the scopes or carrying a tag, sorted, without this process.
        pub(super) fn members(&self) -> Vec<u32> {
            let proc_root = Path::new(PROC);
            let own = std::process::id();
            let uid = nix::unistd::geteuid().as_raw();
            let mut members = if self.tags.is_empty() {
                Vec::new()
            } else {
                tagged(proc_root, &self.tags, own, uid)
            };
            for unit in &self.units {
                members.extend(cgroup_members(proc_root, &unit.procs, own));
            }
            members.sort_unstable();
            members.dedup();
            members
        }

        /// Signal the scopes whole and each of `pids` that still carries a tag.
        pub(super) async fn signal(&self, pids: &[u32], signal: Signal) {
            assert!(!pids.is_empty(), "a signal round needs a target");
            crate::scope::signal_units(&self.units, signal).await;
            if self.tags.is_empty() {
                return;
            }
            let uid = nix::unistd::geteuid().as_raw();
            for pid in pids {
                if let Err(error) = signal_pinned(*pid, &self.tags, uid, signal) {
                    tracing::warn!(pid, ?signal, %error, "could not signal an agent process");
                }
            }
        }
    }

    /// Signal `pid` through a pidfd opened before its tag is checked again, so
    /// a pid reused since the scan is never signalled.
    fn signal_pinned(pid: u32, tags: &[String], uid: u32, signal: Signal) -> io::Result<()> {
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
        if !carries(Path::new(PROC), pid, tags, uid) {
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

    /// Pids under `proc_root` of user `uid` that are alive and carry one of
    /// `tags`, sorted, except `own`.
    pub(super) fn tagged(proc_root: &Path, tags: &[String], own: u32, uid: u32) -> Vec<u32> {
        assert!(!tags.is_empty(), "no tag would select nothing");
        assert!(
            tags.iter().all(|tag| tag.contains('=')),
            "a tag is a whole environment entry: {tags:?}"
        );
        let mut pids: Vec<u32> = candidates(proc_root, own)
            .filter(|pid| carries(proc_root, *pid, tags, uid))
            .collect();
        pids.sort_unstable();
        pids
    }

    /// The agents named by the tag of a live process of `uid` under
    /// `proc_root`, sorted and deduplicated, except `own`'s.
    pub(super) fn tagged_agents(proc_root: &Path, own: u32, uid: u32) -> Vec<AgentId> {
        let key = format!("{AGENT_ENV}=");
        let mut agents: Vec<AgentId> = candidates(proc_root, own)
            .filter_map(|pid| environment(proc_root, pid, uid))
            .filter_map(|environment| {
                let value = environment
                    .split(|byte| *byte == 0)
                    .find_map(|entry| entry.strip_prefix(key.as_bytes()))?;
                std::str::from_utf8(value).ok()?.parse().ok()
            })
            .collect();
        agents.sort_unstable();
        agents.dedup();
        agents
    }

    /// The pids listed under `proc_root`, without init and `own`.
    fn candidates(proc_root: &Path, own: u32) -> impl Iterator<Item = u32> {
        let listed = match std::fs::read_dir(proc_root) {
            Ok(dir) => Some(dir),
            Err(error) => {
                tracing::warn!(root = %proc_root.display(), %error, "cannot list processes to reap");
                None
            }
        };
        listed
            .into_iter()
            .flatten()
            .filter_map(Result::ok)
            .filter_map(|entry| entry.file_name().to_str()?.parse::<u32>().ok())
            .filter(move |pid| *pid > 1 && *pid != own)
    }

    /// Whether `pid` is a live process of `uid` whose environment holds one of `tags` exactly.
    pub(super) fn carries(proc_root: &Path, pid: u32, tags: &[String], uid: u32) -> bool {
        environment(proc_root, pid, uid).is_some_and(|environment| {
            environment
                .split(|byte| *byte == 0)
                .any(|entry| tags.iter().any(|tag| entry == tag.as_bytes()))
        })
    }

    /// The environment of `pid` if it is a live process of `uid`.
    fn environment(proc_root: &Path, pid: u32, uid: u32) -> Option<Vec<u8>> {
        use std::os::unix::fs::MetadataExt as _;

        let dir = proc_root.join(pid.to_string());
        // Another user's process is never ours, whatever it carries.
        if !std::fs::metadata(&dir).is_ok_and(|meta| meta.uid() == uid) {
            return None;
        }
        if !is_live(proc_root, pid) {
            return None;
        }
        // Gone or not readable: not one to signal.
        std::fs::read(dir.join("environ")).ok()
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
