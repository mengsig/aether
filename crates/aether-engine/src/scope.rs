//! Where an engine runs, so its agent's end can find every process it left.
//!
//! An engine is tagged with its agent's id in its environment, which its
//! descendants inherit. On Linux it also runs in a transient systemd user
//! scope named after the agent when systemd can make one: the scope's cgroup
//! holds the descendants that cleared their environment or left the process
//! group. Without systemd the engine keeps its tag and process group alone; a
//! scope never makes a spawn fail.

use aether_core::id::AgentId;

/// Environment variable naming the agent an engine and its descendants run for.
pub(crate) const AGENT_ENV: &str = "AETHER_ENGINE_AGENT";

/// The command to spawn `program` as an engine of `agent`. The caller still
/// puts it in its own process group.
pub(crate) async fn command(program: &str, agent: Option<AgentId>) -> tokio::process::Command {
    let Some(agent) = agent else {
        return tokio::process::Command::new(program);
    };
    #[cfg(target_os = "linux")]
    let mut command = linux::command(program, agent).await;
    #[cfg(not(target_os = "linux"))]
    let mut command = tokio::process::Command::new(program);
    command.env(AGENT_ENV, agent.to_string());
    command
}

#[cfg(target_os = "linux")]
pub(crate) use linux::{Unit, agent_units, signal_units};

#[cfg(target_os = "linux")]
mod linux {
    use std::path::{Path, PathBuf};
    use std::process::Stdio;
    use std::time::Duration;

    use aether_core::id::AgentId;
    use nix::sys::signal::Signal;
    use tokio::sync::OnceCell;

    /// Set to `0` to run engines in their process group alone.
    const DISABLE_ENV: &str = "AETHER_ENGINE_SCOPE";
    const CGROUP_ROOT: &str = "/sys/fs/cgroup";
    const SYSTEMD_BUDGET: Duration = Duration::from_secs(5);

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

    /// `program` wrapped in a fresh scope of `agent` when systemd can make one
    /// and `program` exists, else `program` itself. `systemd-run --scope` execs
    /// the program in place, so the pid and the process group stay the engine's.
    pub(super) async fn command(program: &str, agent: AgentId) -> tokio::process::Command {
        if !scope_usable().await || !resolves(program) {
            return tokio::process::Command::new(program);
        }
        let unit = format!(
            "{}{}",
            unit_prefix(agent),
            aether_core::id::ProcessId::new()
        );
        let mut command = tokio::process::Command::new("systemd-run");
        command
            .args(["--user", "--scope", "--quiet", "--collect"])
            .arg(format!("--unit={unit}"))
            .arg("--")
            .arg(program);
        command
    }

    /// Whether `systemd-run --user --scope` works here, probed once.
    async fn scope_usable() -> bool {
        static USABLE: OnceCell<bool> = OnceCell::const_new();
        *USABLE
            .get_or_init(|| async {
                if std::env::var(DISABLE_ENV).is_ok_and(|v| v == "0") {
                    return false;
                }
                let mut probe = tokio::process::Command::new("systemd-run");
                probe
                    .args(["--user", "--scope", "--quiet", "--collect", "--", "true"])
                    .stdin(Stdio::null())
                    .stdout(Stdio::null())
                    .stderr(Stdio::null())
                    .kill_on_drop(true);
                let usable = tokio::time::timeout(SYSTEMD_BUDGET, probe.status())
                    .await
                    .is_ok_and(|status| status.is_ok_and(|status| status.success()));
                tracing::info!(usable, "systemd user scopes for engines");
                usable
            })
            .await
    }

    /// Whether `program` names an executable file, directly or on `PATH`, so a
    /// missing engine still fails as not installed rather than inside the wrapper.
    fn resolves(program: &str) -> bool {
        use std::os::unix::fs::PermissionsExt as _;

        let executable = |path: &Path| {
            std::fs::metadata(path)
                .is_ok_and(|m| m.is_file() && m.permissions().mode() & 0o111 != 0)
        };
        if program.contains('/') {
            return executable(Path::new(program));
        }
        std::env::var_os("PATH").is_some_and(|paths| {
            std::env::split_paths(&paths).any(|dir| executable(&dir.join(program)))
        })
    }

    /// The live scopes of `agent`'s engines, but none holding this process:
    /// a daemon started from an agent's shell must not stop itself.
    pub(crate) async fn agent_units(agent: AgentId) -> Vec<Unit> {
        if !scope_usable().await {
            return Vec::new();
        }
        let pattern = format!("{}*", unit_prefix(agent));
        let Some(listed) = systemctl(&[
            "list-units",
            "--type=scope",
            "--all",
            "--plain",
            "--no-legend",
            "--full",
            &pattern,
        ])
        .await
        else {
            return Vec::new();
        };
        let names = unit_names(&listed, &unit_prefix(agent));
        if names.is_empty() {
            return Vec::new();
        }
        let mut args = vec!["show", "--property=Id,ControlGroup"];
        args.extend(names.iter().map(String::as_str));
        let Some(shown) = systemctl(&args).await else {
            return Vec::new();
        };
        let own = std::fs::read_to_string("/proc/self/cgroup").unwrap_or_default();
        units_from_show(&shown, Path::new(CGROUP_ROOT))
            .into_iter()
            .filter(|unit| !holds_own_cgroup(unit, &own))
            .collect()
    }

    /// Signal every process of `units`; a scope that already ended is fine.
    pub(crate) async fn signal_units(units: &[Unit], signal: Signal) {
        if units.is_empty() {
            return;
        }
        let flag = format!("--signal={}", signal.as_str());
        let mut args = vec!["kill", "--kill-whom=all", flag.as_str()];
        args.extend(units.iter().map(|unit| unit.name.as_str()));
        systemctl(&args).await;
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

    async fn systemctl(args: &[&str]) -> Option<String> {
        let mut command = tokio::process::Command::new("systemctl");
        command
            .arg("--user")
            .args(args)
            .stdin(Stdio::null())
            .kill_on_drop(true);
        match tokio::time::timeout(SYSTEMD_BUDGET, command.output()).await {
            Ok(Ok(output)) if output.status.success() => {
                Some(String::from_utf8_lossy(&output.stdout).into_owned())
            }
            Ok(Ok(output)) => {
                tracing::warn!(
                    ?args,
                    status = %output.status,
                    stderr = %String::from_utf8_lossy(&output.stderr).trim(),
                    "systemctl failed for engine scopes"
                );
                None
            }
            Ok(Err(error)) => {
                tracing::warn!(?args, %error, "could not run systemctl for engine scopes");
                None
            }
            Err(_) => {
                tracing::warn!(?args, "systemctl did not answer for engine scopes");
                None
            }
        }
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
