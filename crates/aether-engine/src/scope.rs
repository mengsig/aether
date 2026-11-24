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
pub(crate) const AGENT_ENV: &str = aether_core::agent::PROCESS_AGENT_ENV;

/// The command to spawn `program` as an engine of `agent`. The caller still
/// spawns it in a new session with [`crate::spawn_gated`] and passes the
/// child to [`adopt`].
pub(crate) fn command(program: &str, agent: Option<AgentId>) -> tokio::process::Command {
    let mut command = tokio::process::Command::new(program);
    if let Some(agent) = agent {
        command.env(AGENT_ENV, agent.to_string());
    }
    command
}

/// The longest a sweep waits for its agents' engines to settle into their
/// scopes: an adoption's three systemd waits (slice, scope, move), and a margin.
#[cfg(target_os = "linux")]
pub(crate) const ADOPTION_BUDGET: std::time::Duration = std::time::Duration::from_secs(16);

/// Agents with an engine spawned but not yet in its scope, once per engine.
static ADOPTING: std::sync::Mutex<Vec<AgentId>> = std::sync::Mutex::new(Vec::new());
static ADOPTED: tokio::sync::Notify = tokio::sync::Notify::const_new();

fn adopting() -> std::sync::MutexGuard<'static, Vec<AgentId>> {
    // The list stays valid whatever panicked while holding it.
    ADOPTING
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}

/// An engine of an agent spawned but not yet moved into its scope. Begun
/// under the spawn gate, so a sweep after a fence waits for it ([`settled`]).
#[derive(Debug)]
pub(crate) struct Adoption(Option<AgentId>);

impl Adoption {
    pub(crate) fn begin(agent: Option<AgentId>) -> Self {
        if let Some(agent) = agent {
            adopting().push(agent);
        }
        Self(agent)
    }
}

impl Drop for Adoption {
    fn drop(&mut self) {
        let Some(agent) = self.0 else {
            return;
        };
        let mut adopting = adopting();
        match adopting.iter().position(|pending| *pending == agent) {
            Some(at) => drop(adopting.swap_remove(at)),
            None => tracing::error!(%agent, "an engine adoption was not recorded"),
        }
        drop(adopting);
        ADOPTED.notify_waiters();
    }
}

/// Wait until no engine of `agents` is between its spawn and its scope, at
/// most `budget`; false when one still is.
#[cfg(target_os = "linux")]
pub(crate) async fn settled(agents: &[AgentId], budget: std::time::Duration) -> bool {
    assert!(!agents.is_empty(), "no agent to wait for");
    let deadline = tokio::time::Instant::now() + budget;
    loop {
        let notified = ADOPTED.notified();
        tokio::pin!(notified);
        notified.as_mut().enable();
        if !adopting().iter().any(|pending| agents.contains(pending)) {
            return true;
        }
        if tokio::time::timeout_at(deadline, notified).await.is_err() {
            return false;
        }
    }
}

/// Move the running engine `pid` of `adoption`'s agent into a scope of its
/// own, where systemd can make one. A failure is logged; the engine runs on.
pub(crate) async fn adopt(pid: Option<u32>, adoption: Adoption) {
    #[cfg(target_os = "linux")]
    if let (Some(pid), Some(agent)) = (pid, adoption.0) {
        linux::adopt(pid, agent).await;
    }
    #[cfg(not(target_os = "linux"))]
    let _ = pid;
    drop(adoption);
}

mod slice;
pub use slice::{agent_slice_status, set_agent_slice};

#[cfg(target_os = "linux")]
pub(crate) use linux::{Unit, agent_units, agents_with_units, belongs_to, in_unit};

#[cfg(target_os = "linux")]
mod linux {
    use std::path::{Path, PathBuf};
    use std::process::Stdio;
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::time::Duration;

    use aether_core::id::AgentId;

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

    /// What every engine scope's name starts with.
    const UNIT_STEM: &str = "aether-engine-";

    fn unit_prefix(agent: AgentId) -> String {
        format!("{UNIT_STEM}{agent}-")
    }

    /// Whether `unit` is a scope of `agent`'s engines.
    pub(crate) fn belongs_to(unit: &Unit, agent: AgentId) -> bool {
        unit.name.starts_with(&unit_prefix(agent))
    }

    pub(super) fn disabled() -> bool {
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
        let slice = super::slice::joined().await;
        if let Err(error) = start_scope(pid, &unit, slice).await {
            // A machine without a user manager would repeat this for every engine.
            static WARNED: AtomicBool = AtomicBool::new(false);
            if WARNED.swap(true, Ordering::Relaxed) {
                tracing::debug!(%agent, %error, "engine runs without a systemd scope");
            } else {
                tracing::warn!(%agent, %error, "engine runs without a systemd scope; its process group and tag remain");
            }
        }
    }

    /// Start the transient scope `unit` holding `pid`, in `slice` when given,
    /// and wait until `pid` is in it.
    pub(super) async fn start_scope(
        pid: u32,
        unit: &str,
        slice: Option<&str>,
    ) -> Result<(), String> {
        let pid = pid.to_string();
        run("busctl", &start_scope_args(&pid, unit, slice)).await?;
        wait_moved(&pid, unit).await
    }

    /// `busctl` arguments for `StartTransientUnit`, as `systemd-run --scope`
    /// sends it; the scope is collected once it is empty.
    pub(super) fn start_scope_args<'a>(
        pid: &'a str,
        unit: &'a str,
        slice: Option<&'a str>,
    ) -> Vec<&'a str> {
        assert!(unit.ends_with(".scope"), "not a scope: {unit}");
        assert!(
            slice.is_none_or(|slice| slice.ends_with(".slice")),
            "not a slice: {slice:?}"
        );
        let mut args = vec![
            "--user",
            "call",
            "org.freedesktop.systemd1",
            "/org/freedesktop/systemd1",
            "org.freedesktop.systemd1.Manager",
            "StartTransientUnit",
            "ssa(sv)a(sa(sv))",
            unit,
            "fail",
            if slice.is_some() { "3" } else { "2" },
            "PIDs",
            "au",
            "1",
            pid,
            "CollectMode",
            "s",
            "inactive-or-failed",
        ];
        if let Some(slice) = slice {
            args.extend(["Slice", "s", slice]);
        }
        args.push("0");
        args
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

    /// The live scopes of `agents`' engines, but none holding this process:
    /// a daemon started from an agent's shell must not stop itself.
    pub(crate) async fn agent_units(agents: &[AgentId]) -> Vec<Unit> {
        if disabled() || agents.is_empty() {
            return Vec::new();
        }
        let prefixes: Vec<String> = agents.iter().map(|agent| unit_prefix(*agent)).collect();
        let pattern = match prefixes.as_slice() {
            [one] => format!("{one}*"),
            _ => format!("{UNIT_STEM}*"),
        };
        let Some(listed) = list_scopes(&pattern).await else {
            return Vec::new();
        };
        let names = unit_names(&listed, &prefixes);
        if names.is_empty() {
            return Vec::new();
        }
        let mut args = vec!["show", "--property=Id,ControlGroup"];
        args.extend(names.iter().map(String::as_str));
        let shown = match systemctl(&args).await {
            Ok(shown) => shown,
            Err(error) => {
                tracing::warn!(?agents, %error, "cannot read the agents' engine scopes");
                return Vec::new();
            }
        };
        // Unable to tell which scope holds the daemon, it stops none of them.
        let own = match std::fs::read_to_string("/proc/self/cgroup") {
            Ok(own) => own,
            Err(error) => {
                tracing::warn!(?agents, %error, "cannot read the daemon's own cgroup; sparing engine scopes");
                return Vec::new();
            }
        };
        units_from_show(&shown, Path::new(CGROUP_ROOT))
            .into_iter()
            .filter(|unit| !in_unit(unit, &own))
            .collect()
    }

    /// The agents any engine scope of this user manager is named after, of
    /// this daemon or another.
    pub(crate) async fn agents_with_units() -> Vec<AgentId> {
        if disabled() {
            return Vec::new();
        }
        let Some(listed) = list_scopes(&format!("{UNIT_STEM}*")).await else {
            return Vec::new();
        };
        unit_agents(&listed)
    }

    /// `list-units` output for the scopes matching `pattern`; none without a
    /// user manager.
    async fn list_scopes(pattern: &str) -> Option<String> {
        assert!(
            pattern.starts_with(UNIT_STEM),
            "not an engine scope pattern: {pattern}"
        );
        let listed = systemctl(&[
            "list-units",
            "--type=scope",
            "--all",
            "--plain",
            "--no-legend",
            "--full",
            pattern,
        ])
        .await;
        match listed {
            Ok(listed) => Some(listed),
            Err(error) => {
                tracing::debug!(%error, "no engine scopes listed");
                None
            }
        }
    }

    /// The scope names of `list-units --plain --no-legend` output that carry one of `prefixes`.
    pub(super) fn unit_names(listed: &str, prefixes: &[String]) -> Vec<String> {
        assert!(
            prefixes.iter().all(|prefix| prefix.starts_with(UNIT_STEM)),
            "a prefix that is not an engine's would match foreign scopes: {prefixes:?}"
        );
        listed
            .lines()
            .filter_map(|line| line.split_whitespace().next())
            .filter(|name| name.ends_with(".scope"))
            .filter(|name| {
                prefixes
                    .iter()
                    .any(|prefix| name.starts_with(prefix.as_str()))
            })
            .map(str::to_owned)
            .collect()
    }

    /// The agents the engine scopes of `list-units` output are named after,
    /// sorted and deduplicated.
    pub(super) fn unit_agents(listed: &str) -> Vec<AgentId> {
        let mut agents: Vec<AgentId> = listed
            .lines()
            .filter_map(|line| line.split_whitespace().next())
            .filter(|name| name.ends_with(".scope"))
            .filter_map(|name| {
                name.strip_prefix(UNIT_STEM)?
                    .split('-')
                    .next()?
                    .parse()
                    .ok()
            })
            .collect();
        agents.sort_unstable();
        agents.dedup();
        agents
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

    /// Whether the process whose `/proc/<pid>/cgroup` reads `cgroup` lies in
    /// `unit`'s cgroup v2 tree. Signalled one by one, a kept process is spared.
    pub(crate) fn in_unit(unit: &Unit, cgroup: &str) -> bool {
        let Some(own) = cgroup.lines().find_map(|line| line.strip_prefix("0::")) else {
            return false;
        };
        let Some(unit_dir) = unit.procs.parent() else {
            return false;
        };
        let own = Path::new(CGROUP_ROOT).join(own.trim().trim_start_matches('/'));
        own.starts_with(unit_dir)
    }

    pub(super) async fn systemctl(args: &[&str]) -> Result<String, String> {
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
    use super::linux::{
        Unit, in_unit, start_scope, start_scope_args, unit_agents, unit_names, units_from_show,
    };
    use super::slice::{AgentSlice, set_property_args};
    use super::{Adoption, settled};
    use aether_core::agent_slice::AgentSliceLimits;
    use aether_core::id::AgentId;
    use std::path::Path;
    use std::time::Duration;

    /// A sweep waits for its own agents' engines to join their scopes, and
    /// for no other agent's.
    #[tokio::test]
    async fn a_sweep_waits_for_its_agents_adoptions_alone() {
        let (agent, other) = (AgentId::new(), AgentId::new());
        let adoption = Adoption::begin(Some(agent));
        assert!(
            !settled(&[agent], Duration::from_millis(50)).await,
            "settled while an engine was joining its scope"
        );
        assert!(
            settled(&[other], Duration::from_millis(50)).await,
            "waited for another agent's engine"
        );
        let finished = tokio::spawn(async move {
            tokio::time::sleep(Duration::from_millis(50)).await;
            drop(adoption);
        });
        assert!(
            settled(&[agent], Duration::from_secs(30)).await,
            "never settled once the engine joined its scope"
        );
        finished.await.unwrap();
        drop(Adoption::begin(None));
        assert!(settled(&[agent], Duration::ZERO).await);
    }

    #[test]
    fn a_scope_joins_the_agents_slice_only_when_given_one() {
        let alone = start_scope_args("42", "a.scope", None);
        assert_eq!(alone[9], "2");
        assert!(!alone.contains(&"Slice"));
        assert_eq!(alone.last(), Some(&"0"));
        let joined = start_scope_args("42", "a.scope", Some("aether-agents.slice"));
        assert_eq!(joined[9], "3", "the property count names the slice too");
        let at = joined.iter().position(|arg| *arg == "Slice").unwrap();
        assert_eq!(joined[at..], ["Slice", "s", "aether-agents.slice", "0"]);
        assert_eq!(joined[10..at], alone[10..alone.len() - 1]);
    }

    #[test]
    fn the_slice_gets_its_limits_and_the_parent_its_weights() {
        let limits = aether_core::agent_slice::AgentSliceLimits::default();
        assert_eq!(
            set_property_args("aether-agents.slice", limits.slice_properties()),
            [
                "set-property",
                "--runtime",
                "aether-agents.slice",
                "CPUWeight=20",
                "IOWeight=20",
                "MemoryHigh=8G"
            ]
        );
        assert_eq!(
            set_property_args("aether.slice", limits.weight_properties()),
            [
                "set-property",
                "--runtime",
                "aether.slice",
                "CPUWeight=20",
                "IOWeight=20"
            ]
        );
    }

    fn user_manager() -> bool {
        let manager = std::process::Command::new("systemctl")
            .args(["--user", "show-environment"])
            .output();
        manager.is_ok_and(|out| out.status.success())
    }

    /// A throwaway agents' slice and its parent named after `stem`, removed on drop.
    fn throwaway(stem: &str) -> (ThrowawaySlices, AgentSlice) {
        let slices = ThrowawaySlices([format!("{stem}-agents.slice"), format!("{stem}.slice")]);
        let slice: &'static str = slices.0[0].clone().leak();
        let parent: &'static str = slices.0[1].clone().leak();
        (slices, AgentSlice::new(parent, slice))
    }

    #[allow(clippy::unwrap_used)]
    fn assert_limits(unit: &str, want: &[&str]) {
        let out = std::process::Command::new("systemctl")
            .args([
                "--user",
                "show",
                unit,
                "-p",
                "CPUWeight,IOWeight,MemoryHigh",
            ])
            .output()
            .unwrap();
        let shown = String::from_utf8(out.stdout).unwrap();
        for want in want {
            assert!(shown.lines().any(|line| line == *want), "{unit}: {shown}");
        }
    }

    #[allow(clippy::unwrap_used)]
    fn cgroup_of(pid: u32) -> String {
        std::fs::read_to_string(format!("/proc/{pid}/cgroup")).unwrap()
    }

    /// Against the real user manager, on slices of the test's own: the first
    /// scope sets the limits and joins; all off later resets them, keeps out.
    #[tokio::test]
    async fn the_first_scope_sets_the_slice_and_all_off_resets_it() {
        if !user_manager() {
            eprintln!("skipped: no systemd user manager");
            return;
        }
        let stem = format!("aethertest{}", std::process::id());
        let (slices, agents) = throwaway(&stem);
        let [slice, parent] = &slices.0;
        agents.set(AgentSliceLimits {
            cpu_weight: Some(20),
            io_weight: Some(30),
            memory_high: Some("1G".into()),
        });
        assert_eq!(agents.joined().await, Some(slice.as_str()));
        let status = agents.status().unwrap();
        assert_eq!(
            status,
            format!("{slice} CPUWeight=20 IOWeight=30 MemoryHigh=1G")
        );
        let mut engine = tokio::process::Command::new("sleep")
            .arg("30")
            .kill_on_drop(true)
            .spawn()
            .unwrap();
        let pid = engine.id().unwrap();
        let unit = format!("{stem}-engine.scope");
        start_scope(pid, &unit, Some(slice))
            .await
            .expect("scope started");
        let placed = format!("/{parent}/{slice}/{unit}");
        assert!(cgroup_of(pid).contains(&placed), "{}", cgroup_of(pid));
        assert_limits(
            slice,
            &["CPUWeight=20", "IOWeight=30", "MemoryHigh=1073741824"],
        );
        assert_limits(
            parent,
            &["CPUWeight=20", "IOWeight=30", "MemoryHigh=infinity"],
        );

        // A later daemon with every limit off lifts them from the scope still there.
        agents.set(AgentSliceLimits {
            cpu_weight: None,
            io_weight: None,
            memory_high: None,
        });
        assert_eq!(agents.joined().await, None, "an all-off slice took a scope");
        assert_eq!(agents.status().as_deref(), Some("off"));
        assert_eq!(
            agents.joined().await,
            None,
            "settled off, it still takes none"
        );
        let reset = [
            "CPUWeight=[not set]",
            "IOWeight=[not set]",
            "MemoryHigh=infinity",
        ];
        assert_limits(slice, &reset);
        assert_limits(parent, &reset);
        assert!(cgroup_of(pid).contains(&placed), "{}", cgroup_of(pid));
        engine.kill().await.unwrap();
    }

    /// Limits systemd refuses leave scopes where they ran before, once and for all.
    #[tokio::test]
    async fn a_slice_systemd_refuses_keeps_scopes_out() {
        if !user_manager() {
            eprintln!("skipped: no systemd user manager");
            return;
        }
        let (_slices, agents) = throwaway(&format!("aethertestrefused{}", std::process::id()));
        agents.set(AgentSliceLimits {
            memory_high: Some("8X".into()),
            ..AgentSliceLimits::default()
        });
        assert_eq!(agents.joined().await, None, "a refused slice took a scope");
        let status = agents.status().unwrap();
        assert!(status.contains("not set ("), "{status}");
        assert!(status.ends_with("engines run outside it"), "{status}");
        assert_eq!(
            agents.joined().await,
            None,
            "a refused slice took a later scope"
        );
    }

    /// Stops the test's slices and drops their runtime settings, even on a
    /// failed assertion.
    struct ThrowawaySlices([String; 2]);

    impl Drop for ThrowawaySlices {
        fn drop(&mut self) {
            for verb in ["stop", "revert"] {
                let done = std::process::Command::new("systemctl")
                    .args(["--user", verb])
                    .args(&self.0)
                    .output();
                if !done.as_ref().is_ok_and(|out| out.status.success()) {
                    eprintln!("systemctl {verb} {:?}: {done:?}", self.0);
                }
            }
        }
    }

    #[test]
    fn lists_only_the_agents_scopes() {
        let listed = "aether-engine-agt_A-prc_1.scope loaded active running aether-engine-agt_A-prc_1.scope\n\
                      aether-engine-agt_AB-prc_2.scope loaded active running x\n\
                      other.scope loaded active running y\n";
        let a = "aether-engine-agt_A-".to_owned();
        assert_eq!(
            unit_names(listed, std::slice::from_ref(&a)),
            vec!["aether-engine-agt_A-prc_1.scope".to_owned()]
        );
        assert_eq!(
            unit_names(listed, &[a.clone(), "aether-engine-agt_AB-".to_owned()]).len(),
            2
        );
        assert!(unit_names("", &[a]).is_empty());
    }

    #[test]
    fn names_the_agents_of_engine_scopes_only() {
        let a = aether_core::id::AgentId::new();
        let b = aether_core::id::AgentId::new();
        let listed = format!(
            "aether-engine-{a}-prc_1.scope loaded active running x\n\
             aether-engine-{a}-prc_2.scope loaded active running x\n\
             aether-engine-{b}-prc_3.scope loaded inactive dead x\n\
             aether-engine-garbage-prc_4.scope loaded active running x\n\
             other-{a}.scope loaded active running y\n"
        );
        let mut expected = vec![a, b];
        expected.sort_unstable();
        assert_eq!(unit_agents(&listed), expected);
        assert!(unit_agents("").is_empty());
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
        assert!(in_unit(&unit, "0::/user.slice/a.scope\n"));
        assert!(in_unit(&unit, "0::/user.slice/a.scope/sub\n"));
        assert!(!in_unit(&unit, "0::/user.slice/a.scope2\n"));
        assert!(!in_unit(&unit, "0::/user.slice/b.scope\n"));
    }
}
