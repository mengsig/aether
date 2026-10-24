//! Where an engine runs, so its agent's end can find every process it left.
//!
//! An engine is tagged with its agent's id in its environment, which its
//! descendants inherit. On Linux, once it is running, it is also moved into a
//! transient systemd user scope named after the agent: the scope's cgroup holds
//! the descendants that cleared their environment or left the process group.
//! The engine is spawned before systemd is asked, so a missing or failing user
//! manager costs the scope, never the spawn.

use aether_core::id::AgentId;

/// Environment variable naming the agent an engine and its descendants run for.
pub(crate) const AGENT_ENV: &str = "AETHER_ENGINE_AGENT";

/// The command to spawn `program` as an engine of `agent`. The caller still
/// puts it in its own process group and passes the child to [`adopt`].
pub(crate) fn command(program: &str, agent: Option<AgentId>) -> tokio::process::Command {
    let mut command = tokio::process::Command::new(program);
    if let Some(agent) = agent {
        command.env(AGENT_ENV, agent.to_string());
    }
    command
}

/// Move the running engine `pid` of `agent` into a scope of its own, where
/// systemd can make one. A failure is logged; the engine runs on regardless.
pub(crate) async fn adopt(pid: Option<u32>, agent: Option<AgentId>) {
    #[cfg(target_os = "linux")]
    if let (Some(pid), Some(agent)) = (pid, agent) {
        linux::adopt(pid, agent).await;
    }
    #[cfg(not(target_os = "linux"))]
    let _ = (pid, agent);
}

#[cfg(target_os = "linux")]
pub(crate) use linux::{Unit, agent_units, signal_units};

#[cfg(target_os = "linux")]
mod linux {
    use std::path::{Path, PathBuf};
    use std::process::Stdio;
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::time::Duration;

    use aether_core::id::AgentId;
    use nix::sys::signal::Signal;

    /// Set to `0` to run engines in their process group alone.
    const DISABLE_ENV: &str = "AETHER_ENGINE_SCOPE";
    const CGROUP_ROOT: &str = "/sys/fs/cgroup";
    const SYSTEMD_BUDGET: Duration = Duration::from_secs(5);
    const MOVE_POLL: Duration = Duration::from_millis(5);

    /// A live scope of one agent's engines.
    #[derive(Debug, Clone, PartialEq, Eq)]
    pub(crate) struct Unit {
        pub name: String,
        /// Its cgroup's `cgroup.procs`.
        pub procs: PathBuf,
    }

    fn unit_prefix(agent: AgentId) -> String {
        format!("aether-engine-{agent}-")
    }

    fn disabled() -> bool {
        std::env::var(DISABLE_ENV).is_ok_and(|v| v == "0")
    }

    /// Ask the user manager for a transient scope holding `pid`, as
    /// `systemd-run --scope` does for itself; collected once it is empty.
    pub(super) async fn adopt(pid: u32, agent: AgentId) {
        assert!(pid > 1, "not an engine pid: {pid}");
        if disabled() {
            return;
        }
        let unit = format!(
            "{}{}.scope",
            unit_prefix(agent),
            aether_core::id::ProcessId::new()
        );
        let pid = pid.to_string();
        let args = [
            "--user",
            "call",
            "org.freedesktop.systemd1",
            "/org/freedesktop/systemd1",
            "org.freedesktop.systemd1.Manager",
            "StartTransientUnit",
            "ssa(sv)a(sa(sv))",
            &unit,
            "fail",
            "2",
            "PIDs",
            "au",
            "1",
            &pid,
            "CollectMode",
            "s",
            "inactive-or-failed",
            "0",
        ];
        let moved = match run("busctl", &args).await {
            Ok(_) => wait_moved(&pid, &unit).await,
            Err(error) => Err(error),
        };
        if let Err(error) = moved {
            // A machine without a user manager would repeat this for every engine.
            static WARNED: AtomicBool = AtomicBool::new(false);
            if WARNED.swap(true, Ordering::Relaxed) {
                tracing::debug!(%agent, %error, "engine runs without a systemd scope");
            } else {
                tracing::warn!(%agent, %error, "engine runs without a systemd scope; its process group and tag remain");
            }
        }
    }

    /// Wait for the queued job to move `pid` into `unit`: until it has, the
    /// engine's children are still born outside the scope.
    async fn wait_moved(pid: &str, unit: &str) -> Result<(), String> {
        assert!(unit.ends_with(".scope"), "not a scope: {unit}");
        let cgroup = Path::new("/proc").join(pid).join("cgroup");
        let deadline = tokio::time::Instant::now() + SYSTEMD_BUDGET;
        while tokio::time::Instant::now() < deadline {
            match std::fs::read_to_string(&cgroup) {
                Ok(current) if current.contains(&format!("/{unit}")) => return Ok(()),
                Ok(_) => tokio::time::sleep(MOVE_POLL).await,
                // The engine already exited: nothing is left to hold.
                Err(_) => return Ok(()),
            }
        }
        Err(format!(
            "{unit} did not take the engine in {SYSTEMD_BUDGET:?}"
        ))
    }

    /// The live scopes of `agent`'s engines, but none holding this process:
    /// a daemon started from an agent's shell must not stop itself.
    pub(crate) async fn agent_units(agent: AgentId) -> Vec<Unit> {
        if disabled() {
            return Vec::new();
        }
        let pattern = format!("{}*", unit_prefix(agent));
        let listed = systemctl(&[
            "list-units",
            "--type=scope",
            "--all",
            "--plain",
            "--no-legend",
            "--full",
            &pattern,
        ])
        .await;
        // Without a user manager there are no scopes to look for.
        let listed = match listed {
            Ok(listed) => listed,
            Err(error) => {
                tracing::debug!(%agent, %error, "no engine scopes listed");
                return Vec::new();
            }
        };
        let names = unit_names(&listed, &unit_prefix(agent));
        if names.is_empty() {
            return Vec::new();
        }
        let mut args = vec!["show", "--property=Id,ControlGroup"];
        args.extend(names.iter().map(String::as_str));
        let shown = match systemctl(&args).await {
            Ok(shown) => shown,
            Err(error) => {
                tracing::warn!(%agent, %error, "cannot read the agent's engine scopes");
                return Vec::new();
            }
        };
        // Unable to tell which scope holds the daemon, it stops none of them.
        let own = match std::fs::read_to_string("/proc/self/cgroup") {
            Ok(own) => own,
            Err(error) => {
                tracing::warn!(%agent, %error, "cannot read the daemon's own cgroup; sparing engine scopes");
                return Vec::new();
            }
        };
        units_from_show(&shown, Path::new(CGROUP_ROOT))
            .into_iter()
            .filter(|unit| !holds_own_cgroup(unit, &own))
            .collect()
    }

    /// Signal every process of `units`. A scope that ended since it was
    /// listed fails the call; any process truly left shows as a survivor.
    pub(crate) async fn signal_units(units: &[Unit], signal: Signal) {
        if units.is_empty() {
            return;
        }
        let flag = format!("--signal={}", signal.as_str());
        let mut args = vec!["kill", "--kill-whom=all", flag.as_str()];
        args.extend(units.iter().map(|unit| unit.name.as_str()));
        if let Err(error) = systemctl(&args).await {
            tracing::debug!(?signal, %error, "systemctl kill of engine scopes");
        }
    }

    /// The scope names of `list-units --plain --no-legend` output that carry `prefix`.
    pub(super) fn unit_names(listed: &str, prefix: &str) -> Vec<String> {
        assert!(
            !prefix.is_empty(),
            "an empty prefix would match every scope"
        );
        listed
            .lines()
            .filter_map(|line| line.split_whitespace().next())
            .filter(|name| name.starts_with(prefix) && name.ends_with(".scope"))
            .map(str::to_owned)
            .collect()
    }

    /// Units of `show --property=Id,ControlGroup` output that still have a cgroup.
    pub(super) fn units_from_show(shown: &str, cgroup_root: &Path) -> Vec<Unit> {
        assert!(cgroup_root.is_absolute(), "cgroup root must be absolute");
        let mut units = Vec::new();
        for block in shown.split("\n\n") {
            let field = |key: &str| {
                block
                    .lines()
                    .find_map(|line| line.strip_prefix(key)?.strip_prefix('='))
                    .map(str::trim)
                    .filter(|value| !value.is_empty())
            };
            let (Some(name), Some(group)) = (field("Id"), field("ControlGroup")) else {
                continue;
            };
            let relative = group.trim_start_matches('/');
            if relative.is_empty() || relative.split('/').any(|part| part == "..") {
                continue;
            }
            units.push(Unit {
                name: name.to_owned(),
                procs: cgroup_root.join(relative).join("cgroup.procs"),
            });
        }
        units
    }

    /// Whether this process's cgroup v2 path (from `/proc/self/cgroup`) lies in `unit`'s.
    pub(super) fn holds_own_cgroup(unit: &Unit, own_cgroup: &str) -> bool {
        let Some(own) = own_cgroup.lines().find_map(|line| line.strip_prefix("0::")) else {
            return false;
        };
        let Some(unit_dir) = unit.procs.parent() else {
            return false;
        };
        let own = Path::new(CGROUP_ROOT).join(own.trim().trim_start_matches('/'));
        own.starts_with(unit_dir)
    }

    async fn systemctl(args: &[&str]) -> Result<String, String> {
        let mut full = vec!["--user"];
        full.extend_from_slice(args);
        run("systemctl", &full).await
    }

    /// Run a systemd client under a budget; its stdout, or why it failed.
    async fn run(program: &str, args: &[&str]) -> Result<String, String> {
        let mut command = tokio::process::Command::new(program);
        command.args(args).stdin(Stdio::null()).kill_on_drop(true);
        let output = tokio::time::timeout(SYSTEMD_BUDGET, command.output())
            .await
            .map_err(|_| format!("{program} did not answer in {SYSTEMD_BUDGET:?}"))?
            .map_err(|error| format!("cannot run {program}: {error}"))?;
        if !output.status.success() {
            return Err(format!(
                "{program} {}: {}",
                output.status,
                String::from_utf8_lossy(&output.stderr).trim()
            ));
        }
        Ok(String::from_utf8_lossy(&output.stdout).into_owned())
    }
}

#[cfg(all(test, target_os = "linux"))]
mod tests {
    use super::linux::{Unit, holds_own_cgroup, unit_names, units_from_show};
    use std::path::Path;

    #[test]
    fn lists_only_the_agents_scopes() {
        let listed = "aether-engine-agt_A-prc_1.scope loaded active running aether-engine-agt_A-prc_1.scope\n\
                      aether-engine-agt_AB-prc_2.scope loaded active running x\n\
                      other.scope loaded active running y\n";
        assert_eq!(
            unit_names(listed, "aether-engine-agt_A-"),
            vec!["aether-engine-agt_A-prc_1.scope".to_owned()]
        );
        assert!(unit_names("", "aether-engine-agt_A-").is_empty());
    }

    #[test]
    fn reads_cgroups_and_skips_ended_or_odd_ones() {
        let shown = "Id=a.scope\nControlGroup=/user.slice/a.scope\n\n\
                     Id=gone.scope\nControlGroup=\n\n\
                     Id=odd.scope\nControlGroup=/user.slice/../etc\n";
        let units = units_from_show(shown, Path::new("/sys/fs/cgroup"));
        assert_eq!(
            units,
            vec![Unit {
                name: "a.scope".into(),
                procs: "/sys/fs/cgroup/user.slice/a.scope/cgroup.procs".into(),
            }]
        );
    }

    #[test]
    fn spares_a_scope_that_holds_this_process() {
        let unit = Unit {
            name: "a.scope".into(),
            procs: "/sys/fs/cgroup/user.slice/a.scope/cgroup.procs".into(),
        };
        assert!(holds_own_cgroup(&unit, "0::/user.slice/a.scope\n"));
        assert!(holds_own_cgroup(&unit, "0::/user.slice/a.scope/sub\n"));
        assert!(!holds_own_cgroup(&unit, "0::/user.slice/a.scope2\n"));
        assert!(!holds_own_cgroup(&unit, "0::/user.slice/b.scope\n"));
    }
}
