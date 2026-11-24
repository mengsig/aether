use std::time::Duration;

use aether_core::agent_slice::{AGENT_SLICE, AgentSliceLimits};

use super::Check;

const NAME: &str = "agent slice";
const SYSTEMCTL_BUDGET: Duration = Duration::from_secs(5);

/// What the user systemd manager holds for the agents' slice.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SliceProbe {
    /// `systemctl show` of the slice: its CPUWeight, IOWeight and MemoryHigh.
    pub shown: String,
    /// The controllers the user manager hands its units, when readable.
    pub controllers: Option<String>,
}

/// The configured slice limits against the user manager. Informational: no
/// systemd is a notice, not a fault, since engines then run as they always did.
pub fn check_agent_slice(
    limits: Option<&AgentSliceLimits>,
    probe: Result<SliceProbe, String>,
) -> Check {
    let Some(limits) = limits else {
        return Check::ok(NAME, "not checked: config.toml does not load");
    };
    if limits.is_off() {
        return Check::ok(NAME, "off: [resources] turns every agent limit off");
    }
    let probe = match probe {
        Ok(probe) => probe,
        Err(why) => {
            return Check::ok(
                NAME,
                format!(
                    "not applied: {why}; engines run at normal priority, as before \
                     (configured {limits})"
                ),
            );
        }
    };
    let mut detail = format!(
        "configured {limits}; {AGENT_SLICE} now has {}",
        one_line(&probe.shown)
    );
    if let Some(missing) = missing_controllers(limits, probe.controllers.as_deref()) {
        detail.push_str(&format!(
            "; the user manager has no {missing} controller, so that limit has no effect"
        ));
    }
    Check::ok(NAME, detail)
}

fn one_line(shown: &str) -> String {
    let fields: Vec<&str> = shown
        .lines()
        .map(str::trim)
        .filter(|l| !l.is_empty())
        .collect();
    if fields.is_empty() {
        return "nothing set".to_owned();
    }
    fields.join(" ")
}

/// The controllers a configured limit needs that `controllers` lacks.
fn missing_controllers(limits: &AgentSliceLimits, controllers: Option<&str>) -> Option<String> {
    let controllers: Vec<&str> = controllers?.split_whitespace().collect();
    let needed = [
        ("cpu", limits.cpu_weight.is_some()),
        ("io", limits.io_weight.is_some()),
        ("memory", limits.memory_high.is_some()),
    ];
    let missing: Vec<&str> = needed
        .into_iter()
        .filter(|(name, used)| *used && !controllers.contains(name))
        .map(|(name, _)| name)
        .collect();
    (!missing.is_empty()).then(|| missing.join(" or "))
}

/// Ask the user manager what the slice carries and which controllers it
/// delegates; why not, where there is no user manager.
pub async fn probe_agent_slice() -> Result<SliceProbe, String> {
    if !cfg!(target_os = "linux") {
        return Err("systemd slices exist only on Linux".to_owned());
    }
    let shown = systemctl(&["show", AGENT_SLICE, "-p", "CPUWeight,IOWeight,MemoryHigh"]).await?;
    let manager = systemctl(&["show", "-p", "ControlGroup", "--value"]).await?;
    let controllers = manager
        .trim()
        .strip_prefix('/')
        .filter(|group| !group.is_empty() && !group.split('/').any(|part| part == ".."))
        .and_then(|group| {
            let path = std::path::Path::new("/sys/fs/cgroup")
                .join(group)
                .join("cgroup.subtree_control");
            std::fs::read_to_string(path).ok()
        });
    Ok(SliceProbe { shown, controllers })
}

async fn systemctl(args: &[&str]) -> Result<String, String> {
    let mut command = tokio::process::Command::new("systemctl");
    command
        .arg("--user")
        .args(args)
        .stdin(std::process::Stdio::null())
        .kill_on_drop(true);
    let output = tokio::time::timeout(SYSTEMCTL_BUDGET, command.output())
        .await
        .map_err(|_| "no systemd user manager answered".to_owned())?
        .map_err(|error| format!("no systemd user manager (cannot run systemctl: {error})"))?;
    if !output.status.success() {
        return Err(format!(
            "no systemd user manager ({})",
            String::from_utf8_lossy(&output.stderr).trim()
        ));
    }
    Ok(String::from_utf8_lossy(&output.stdout).into_owned())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::doctor::Level;

    fn probe(controllers: &str) -> Result<SliceProbe, String> {
        Ok(SliceProbe {
            shown: "CPUWeight=20\nIOWeight=20\nMemoryHigh=8589934592\n".into(),
            controllers: Some(controllers.into()),
        })
    }

    #[test]
    fn reports_what_is_configured_beside_what_systemd_holds() {
        let check = check_agent_slice(
            Some(&AgentSliceLimits::default()),
            probe("cpu io memory pids"),
        );
        assert_eq!(check.level, Level::Ok);
        assert_eq!(
            check.detail,
            "configured CPUWeight=20 IOWeight=20 MemoryHigh=8G; aether-agents.slice now has \
             CPUWeight=20 IOWeight=20 MemoryHigh=8589934592"
        );
    }

    #[test]
    fn names_a_limit_the_user_manager_cannot_apply() {
        let check = check_agent_slice(Some(&AgentSliceLimits::default()), probe("cpu memory pids"));
        assert!(
            check
                .detail
                .ends_with("no io controller, so that limit has no effect"),
            "{}",
            check.detail
        );
        let no_io = AgentSliceLimits {
            io_weight: None,
            ..AgentSliceLimits::default()
        };
        let check = check_agent_slice(Some(&no_io), probe("cpu memory pids"));
        assert!(!check.detail.contains("controller"), "{}", check.detail);
    }

    #[test]
    fn without_systemd_says_so_once_and_does_not_fail() {
        let check = check_agent_slice(
            Some(&AgentSliceLimits::default()),
            Err("no systemd user manager (Failed to connect to bus)".into()),
        );
        assert_eq!(check.level, Level::Ok);
        assert!(
            check
                .detail
                .starts_with("not applied: no systemd user manager")
        );
        assert!(check.detail.contains("normal priority"), "{}", check.detail);
    }

    #[test]
    fn off_and_unloadable_configs_are_reported_plainly() {
        let off = AgentSliceLimits {
            cpu_weight: None,
            io_weight: None,
            memory_high: None,
        };
        let check = check_agent_slice(Some(&off), Err("unused".into()));
        assert!(check.detail.starts_with("off"), "{}", check.detail);
        let check = check_agent_slice(None, Err("unused".into()));
        assert!(check.detail.contains("does not load"), "{}", check.detail);
    }
}
