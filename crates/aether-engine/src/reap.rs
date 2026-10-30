//! Ending every process an agent left behind, once the agent ends or the
//! daemon stops.
//!
//! The processes are found by the agent's tag in their environment, set on
//! its engines and host shell commands, and, on Linux with systemd, in the
//! agent's engine scopes ([`crate::scope`]). A process whose environment
//! sets `AETHER_KEEP_PROCESS=1` is never signalled. A daemon shutdown reaps the
//! agents [`agents_with_processes`] names that its own store ran, in one sweep.
//! Elsewhere than Linux nothing can be found again after a turn, so only the
//! turn's own process-group stop applies.

use std::time::Duration;

use aether_core::id::AgentId;
use aether_spawn::SpawnGate;

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

/// A live process of some agent, as an owner taking it over is told of it.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct AgentProcess {
    pub pid: u32,
    /// Its command line, shortened; its name when the command line is empty.
    pub command: String,
}

/// SIGTERM every live process of `agent`'s engines and shell commands,
/// including those forked meanwhile, then SIGKILL what outlives `grace`.
/// `ended` is asked before each signal: once it is false - the agent was
/// restarted, and its next engine carries the same tag - nothing more is signalled.
#[cfg(target_os = "linux")]
pub async fn reap_agent(agent: AgentId, grace: Duration, ended: impl Fn() -> bool) -> ReapReport {
    reap_agents(&[agent], grace, |_| ended(), &[]).await
}

/// [`reap_agent`] for every one of `agents` at once, as an owner ends with
/// what it took over or the daemon stops: one scan of the processes per round.
/// An agent `ended` no longer holds is dropped from the sweep; the rest go on.
/// Each round holds `gates` from asking `ended` to signalling, so a process an
/// agent restarted meanwhile starts is never among those signalled.
#[cfg(target_os = "linux")]
pub async fn reap_agents(
    agents: &[AgentId],
    grace: Duration,
    ended: impl Fn(AgentId) -> bool,
    gates: &[SpawnGate],
) -> ReapReport {
    if agents.is_empty() {
        return ReapReport::default();
    }
    let mut sweep = linux::Sweep::new(agents).await;
    reap(&mut sweep, grace, Rounds { ended, gates }).await
}

/// What each signal round of a reap asks first, and holds while it signals.
#[cfg(target_os = "linux")]
struct Rounds<'a, F> {
    ended: F,
    gates: &'a [SpawnGate],
}

#[cfg(target_os = "linux")]
impl<F: Fn(AgentId) -> bool> Rounds<'_, F> {
    /// Signal the sweep's live members but those in the sorted `skip`, with
    /// every spawn held off; `None`, signalling nothing, once no agent is ended.
    fn signal(
        &self,
        sweep: &mut linux::Sweep,
        skip: &[u32],
        signal: nix::sys::signal::Signal,
    ) -> Option<Vec<u32>> {
        let _held: Vec<_> = self.gates.iter().map(SpawnGate::hold).collect();
        if !sweep.keep_ended(&self.ended) {
            return None;
        }
        let targets = unsignalled(&sweep.members(), skip);
        if !targets.is_empty() {
            sweep.signal(&targets, signal);
        }
        Some(targets)
    }
}

/// The live processes a sweep of `agents` would signal now, by pid.
#[cfg(target_os = "linux")]
pub async fn live_processes(agents: &[AgentId]) -> Vec<AgentProcess> {
    if agents.is_empty() {
        return Vec::new();
    }
    let sweep = linux::Sweep::new(agents).await;
    sweep
        .members()
        .into_iter()
        .filter_map(|pid| {
            let command = linux::command_line(std::path::Path::new("/proc"), pid)?;
            Some(AgentProcess { pid, command })
        })
        .collect()
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
async fn reap(
    sweep: &mut linux::Sweep,
    grace: Duration,
    rounds: Rounds<'_, impl Fn(AgentId) -> bool>,
) -> ReapReport {
    use nix::sys::signal::Signal;

    let mut report = ReapReport::default();
    let deadline = tokio::time::Instant::now() + grace;
    let mut alive = sweep.members();
    // The first round always runs: even a zero grace sends SIGTERM first.
    while !alive.is_empty() {
        if !unsignalled(&alive, &report.terminated).is_empty() {
            let Some(fresh) = rounds.signal(sweep, &report.terminated, Signal::SIGTERM) else {
                return stopped_early(sweep, report);
            };
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
        if sweep.members().is_empty() {
            break;
        }
        let Some(killed) = rounds.signal(sweep, &[], Signal::SIGKILL) else {
            return stopped_early(sweep, report);
        };
        if killed.is_empty() {
            break;
        }
        report.killed.extend(killed);
        tokio::time::sleep(linux::POLL).await;
    }
    report.killed.sort_unstable();
    report.killed.dedup();
    report.survived = sweep.members();
    log(sweep.agents(), &report);
    report
}

/// The pids of `alive` not yet in `terminated`, which is sorted.
#[cfg(target_os = "linux")]
fn unsignalled(alive: &[u32], terminated: &[u32]) -> Vec<u32> {
    debug_assert!(terminated.is_sorted(), "terminated pids are kept sorted");
    alive
        .iter()
        .copied()
        .filter(|pid| terminated.binary_search(pid).is_err())
        .collect()
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
pub async fn reap_agents(
    _agents: &[AgentId],
    _grace: Duration,
    _ended: impl Fn(AgentId) -> bool,
    _gates: &[SpawnGate],
) -> ReapReport {
    ReapReport::default()
}

#[cfg(not(target_os = "linux"))]
pub async fn live_processes(_agents: &[AgentId]) -> Vec<AgentProcess> {
    Vec::new()
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

    /// The environment entry that spares a process every sweep.
    const KEEP_ENTRY: &str = "AETHER_KEEP_PROCESS=1";
    /// The longest command line an owner is shown for a process it took over.
    const COMMAND_CHARS: usize = 200;

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

    /// The tag entries of `agents`, sorted for [`carries`].
    fn tag_entries(agents: &[AgentId]) -> Vec<String> {
        let mut tags: Vec<String> = agents
            .iter()
            .filter_map(|agent| tag_entry(*agent))
            .collect();
        tags.sort_unstable();
        tags
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
                tags: tag_entries(agents),
            }
        }

        /// Drop the agents `ended` no longer holds, with their tags and
        /// scopes: a restarted one's processes are its next engine's. False once none is left.
        pub(super) fn keep_ended(&mut self, ended: &impl Fn(AgentId) -> bool) -> bool {
            let before = self.agents.len();
            self.agents.retain(|agent| ended(*agent));
            if self.agents.len() != before {
                tracing::info!(remaining = ?self.agents, "an agent resumed; sparing its processes");
                self.tags = tag_entries(&self.agents);
                let agents = &self.agents;
                self.units.retain(|unit| {
                    agents
                        .iter()
                        .any(|agent| crate::scope::belongs_to(unit, *agent))
                });
            }
            !self.agents.is_empty()
        }

        pub(super) fn agents(&self) -> &[AgentId] {
            &self.agents
        }

        /// Live pids in the scopes or carrying a tag, sorted, without this
        /// process and any process kept past its owner.
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
            members.retain(|pid| !kept(proc_root, *pid, uid));
            members
        }

        /// Signal each of `pids` that is still one of the sweep's: it carries
        /// a tag or lies in a scope, and is not kept.
        pub(super) fn signal(&self, pids: &[u32], signal: Signal) {
            assert!(!pids.is_empty(), "a signal round needs a target");
            let uid = nix::unistd::geteuid().as_raw();
            for pid in pids {
                if let Err(error) = signal_pinned(*pid, signal, |pid| self.holds(pid, uid)) {
                    tracing::warn!(pid, ?signal, %error, "could not signal an agent process");
                }
            }
        }

        /// Whether `pid` belongs to this sweep right now.
        fn holds(&self, pid: u32, uid: u32) -> bool {
            assert_ne!(pid, std::process::id(), "the daemon never sweeps itself");
            let proc_root = Path::new(PROC);
            let tagged = !self.tags.is_empty() && carries(proc_root, pid, &self.tags, uid);
            let scoped = || {
                std::fs::read_to_string(proc_root.join(pid.to_string()).join("cgroup")).is_ok_and(
                    |cgroup| {
                        self.units
                            .iter()
                            .any(|unit| crate::scope::in_unit(unit, &cgroup))
                    },
                )
            };
            (tagged || scoped()) && !kept(proc_root, pid, uid)
        }
    }

    /// Signal `pid` through a pidfd opened before `holds` checks it again, so
    /// a pid reused since the scan is never signalled.
    fn signal_pinned(pid: u32, signal: Signal, holds: impl Fn(u32) -> bool) -> io::Result<()> {
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
        if !holds(pid) {
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
        let mut pids = Vec::new();
        for_each_candidate(proc_root, own, |pid| {
            if carries(proc_root, pid, tags, uid) {
                pids.push(pid);
            }
        });
        pids.sort_unstable();
        pids
    }

    /// The agents named by the tag of a live process of `uid` under
    /// `proc_root`, sorted and deduplicated, except `own`'s.
    pub(super) fn tagged_agents(proc_root: &Path, own: u32, uid: u32) -> Vec<AgentId> {
        let key = format!("{AGENT_ENV}=");
        let named = |environment: Vec<u8>| -> Option<AgentId> {
            let value = environment
                .split(|byte| *byte == 0)
                .find_map(|entry| entry.strip_prefix(key.as_bytes()))?;
            std::str::from_utf8(value).ok()?.parse().ok()
        };
        let mut agents = Vec::new();
        for_each_candidate(proc_root, own, |pid| {
            agents.extend(environment(proc_root, pid, uid).and_then(named));
        });
        agents.sort_unstable();
        agents.dedup();
        agents
    }

    /// Visit each pid listed under `proc_root`, without init and `own`, as it
    /// is listed: one forked meanwhile is still visited.
    fn for_each_candidate(proc_root: &Path, own: u32, mut visit: impl FnMut(u32)) {
        let listed = aether_spawn::visit_processes(proc_root, |pid| {
            if pid > 1 && pid != own {
                visit(pid);
            }
            Ok(())
        });
        if let Err(error) = listed {
            tracing::warn!(root = %proc_root.display(), %error, "cannot list processes to reap");
        }
    }

    /// Whether live `pid` sets the keep entry exactly, or may: an environment
    /// that cannot be read (a non-dumpable process) cannot prove it is not kept.
    pub(super) fn kept(proc_root: &Path, pid: u32, uid: u32) -> bool {
        match environment(proc_root, pid, uid) {
            Some(environment) => environment
                .split(|byte| *byte == 0)
                .any(|entry| entry == KEEP_ENTRY.as_bytes()),
            None => is_live(proc_root, pid),
        }
    }

    /// `pid`'s command line with spaces for its separators, shortened, or its
    /// name in brackets when empty; none once it is gone.
    pub(super) fn command_line(proc_root: &Path, pid: u32) -> Option<String> {
        let dir = proc_root.join(pid.to_string());
        let raw = std::fs::read(dir.join("cmdline")).ok()?;
        let joined = String::from_utf8_lossy(&raw).replace('\0', " ");
        let line = joined.trim();
        if !line.is_empty() {
            return Some(line.chars().take(COMMAND_CHARS).collect());
        }
        let name = std::fs::read_to_string(dir.join("comm")).ok()?;
        Some(format!("[{}]", name.trim()))
    }

    /// Whether `pid` is a live process of `uid` whose environment holds one
    /// of the sorted `tags` exactly.
    pub(super) fn carries(proc_root: &Path, pid: u32, tags: &[String], uid: u32) -> bool {
        debug_assert!(tags.is_sorted(), "tags are searched, so kept sorted");
        environment(proc_root, pid, uid).is_some_and(|environment| {
            environment.split(|byte| *byte == 0).any(|entry| {
                tags.binary_search_by(|tag| tag.as_bytes().cmp(entry))
                    .is_ok()
            })
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
        // Gone, unreadable or never settled out of `exec`: not proven tagged, nor unkept.
        aether_spawn::read_environ(&dir).ok()
    }

    /// Live pids listed in a cgroup's `cgroup.procs` and its descendant
    /// cgroups', except `own`. A cgroup that is gone has none.
    pub(super) fn cgroup_members(proc_root: &Path, procs: &Path, own: u32) -> Vec<u32> {
        assert!(
            procs.ends_with("cgroup.procs"),
            "not a cgroup listing: {procs:?}"
        );
        let Some(top) = procs.parent() else {
            return Vec::new();
        };
        let mut members = Vec::new();
        let mut cgroups = vec![top.to_path_buf()];
        while let Some(cgroup) = cgroups.pop() {
            // Gone meanwhile, or a plain directory of the tree: nothing listed.
            if let Ok(listed) = std::fs::read_to_string(cgroup.join("cgroup.procs")) {
                members.extend(
                    listed
                        .lines()
                        .filter_map(|line| line.trim().parse::<u32>().ok())
                        .filter(|pid| *pid > 1 && *pid != own && is_live(proc_root, *pid)),
                );
            }
            let Ok(children) = std::fs::read_dir(&cgroup) else {
                continue;
            };
            cgroups.extend(
                children
                    .filter_map(Result::ok)
                    .filter(|child| child.file_type().is_ok_and(|kind| kind.is_dir()))
                    .map(|child| child.path()),
            );
        }
        members
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
