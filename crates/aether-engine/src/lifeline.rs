//! Engine processes die with the process that spawned them.
//!
//! On Linux every engine process is spawned with a parent-death signal
//! ([`aether_spawn::die_with_parent`]), so a daemon that dies without a clean
//! shutdown, SIGKILL included, takes its engines with it. The kernel sends
//! that signal when the *thread* that spawned the child ends, and tokio
//! retires idle threads, so every spawn runs on one thread that lives as long
//! as the process. That signal reaches only the process spawned, not a CLI
//! wrapper's own child, so each engine's process group also gets a watchdog:
//! a shell in that group that kills the group once its pipe from this process
//! closes. The next daemon start reaps what left the group ([`crate::orphans`]).
//! Elsewhere nothing is marked and no watchdog runs.

use std::io;

use tokio::process::{Child, Command};

/// Spawn `command` leading a process group of its own, so neither it nor its
/// group can outlive this process. Keep the [`GroupWatch`] for as long as the
/// engine runs. Must be called inside a tokio runtime.
#[cfg(target_os = "linux")]
pub(crate) fn spawn(mut command: Command) -> io::Result<(Child, GroupWatch)> {
    use std::os::unix::process::CommandExt as _;

    let runtime = tokio::runtime::Handle::try_current().map_err(io::Error::other)?;
    command.as_std_mut().process_group(0);
    aether_spawn::die_with_parent(command.as_std_mut());
    let (reply_tx, reply_rx) = std::sync::mpsc::sync_channel(1);
    let job: Job = Box::new(move || {
        // tokio registers the child with the caller's runtime for reaping.
        let _entered = runtime.enter();
        let spawned = command.spawn();
        // The caller waits for this reply; if it is gone, dropping the child kills it.
        let _ = reply_tx.send(spawned);
    });
    spawner()?.send(job).map_err(|_| spawner_gone())?;
    let child = reply_rx.recv().map_err(|_| spawner_gone())??;
    let watch = GroupWatch::start(&child);
    Ok((child, watch))
}

#[cfg(not(target_os = "linux"))]
pub(crate) fn spawn(mut command: Command) -> io::Result<(Child, GroupWatch)> {
    use std::os::unix::process::CommandExt as _;

    command.as_std_mut().process_group(0);
    Ok((command.spawn()?, GroupWatch))
}

#[cfg(not(target_os = "linux"))]
impl GroupWatch {
    pub(crate) fn end_group(self) {}
}

/// Kills its engine's process group if this process dies first. Dropping it
/// stops only the watchdog, so a clean end leaves the group to its owner.
#[cfg(target_os = "linux")]
#[derive(Debug)]
pub(crate) struct GroupWatch(Option<Child>);

#[cfg(not(target_os = "linux"))]
#[derive(Debug)]
pub(crate) struct GroupWatch;

/// Blocks until every writer of its stdin is gone, then kills group `$1`. It
/// holds no copy of the pipe's write end, so only this process's death or
/// [`GroupWatch`]'s drop ends the read.
#[cfg(target_os = "linux")]
const WATCHDOG: &str = r#"read -r _; kill -s KILL -- -"$1""#;

#[cfg(target_os = "linux")]
impl GroupWatch {
    /// Join `engine`'s process group with a watchdog. Its membership keeps the
    /// group id from being reused, so its kill cannot reach another group.
    fn start(engine: &Child) -> Self {
        use std::os::unix::process::CommandExt as _;

        let Some(pid) = engine.id() else {
            return Self(None);
        };
        assert!(
            (2..=i32::MAX as u32).contains(&pid),
            "not an engine pid: {pid}"
        );
        let mut command = Command::new("/bin/sh");
        command
            .args(["-c", WATCHDOG, "aether-engine-watchdog", &pid.to_string()])
            .env_clear()
            .current_dir("/")
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null());
        command.as_std_mut().process_group(pid as i32);
        aether_spawn::reset_signals(command.as_std_mut());
        match command.spawn() {
            Ok(watchdog) => {
                assert!(watchdog.stdin.is_some(), "the watchdog reads a pipe");
                Self(Some(watchdog))
            }
            Err(error) => {
                // The next daemon start still reaps the group from its record.
                tracing::warn!(pid, %error, "could not start the engine group's watchdog");
                Self(None)
            }
        }
    }

    /// Kill what is left of the group now, as this process's death would:
    /// closing the pipe without stopping the watchdog lets it fire.
    pub(crate) fn end_group(mut self) {
        let Some(mut watchdog) = self.0.take() else {
            return;
        };
        assert!(
            watchdog.stdin.is_some(),
            "the pipe is closed only here or in drop"
        );
        drop(watchdog.stdin.take());
    }
}

#[cfg(target_os = "linux")]
impl Drop for GroupWatch {
    fn drop(&mut self) {
        let Some(mut watchdog) = self.0.take() else {
            return;
        };
        // Kill before closing the pipe: a pending SIGKILL beats the read's EOF.
        if let Err(error) = watchdog.start_kill() {
            tracing::debug!(%error, "the engine group's watchdog had already exited");
        }
        drop(watchdog.stdin.take());
    }
}

#[cfg(target_os = "linux")]
type Job = Box<dyn FnOnce() + Send>;

#[cfg(target_os = "linux")]
fn spawner_gone() -> io::Error {
    io::Error::other("the engine spawner thread has stopped")
}

/// The one thread that spawns engines, started on first use and never ended.
#[cfg(target_os = "linux")]
fn spawner() -> io::Result<&'static std::sync::mpsc::Sender<Job>> {
    static SPAWNER: std::sync::OnceLock<std::sync::mpsc::Sender<Job>> = std::sync::OnceLock::new();
    if let Some(sender) = SPAWNER.get() {
        return Ok(sender);
    }
    let (sender, jobs) = std::sync::mpsc::channel::<Job>();
    std::thread::Builder::new()
        .name("aether-engine-spawner".into())
        .spawn(move || {
            for job in jobs {
                job();
            }
        })?;
    // A racing first call may win; the loser's thread ends with its sender.
    Ok(SPAWNER.get_or_init(|| sender))
}

#[cfg(all(test, target_os = "linux"))]
pub(crate) mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

    use std::time::Duration;

    /// Re-run as the "daemon" of `a_killed_parent_takes_its_engine_with_it`.
    const HELPER_ENV: &str = "AETHER_TEST_LIFELINE_PARENT";
    const HELPER: &str = "lifeline::tests::lifeline_helper_parent";

    /// An engine stand-in leading its own group, as every engine does.
    fn engine(program: &str, args: &[&str]) -> tokio::process::Command {
        use std::os::unix::process::CommandExt as _;

        let mut command = tokio::process::Command::new(program);
        command.args(args).kill_on_drop(true);
        command.as_std_mut().process_group(0);
        command
    }

    fn sleeper() -> tokio::process::Command {
        engine("sleep", &["30"])
    }

    /// A CLI wrapper that starts the real engine as its child and waits on it,
    /// as the npm `codex` launcher does without `process.execve`.
    fn wrapper(child_pid_file: &std::path::Path) -> tokio::process::Command {
        let script = r#"sleep 30 & echo $! > "$1.tmp" && mv "$1.tmp" "$1"; wait"#;
        engine(
            "/bin/sh",
            &["-c", script, "sh", child_pid_file.to_str().unwrap()],
        )
    }

    pub(crate) fn alive(pid: u32) -> bool {
        // A zombie still has a /proc entry; only a live process is running.
        std::fs::read_to_string(format!("/proc/{pid}/stat"))
            .ok()
            .and_then(|stat| {
                let state = stat.rsplit(')').next()?.split_whitespace().next()?;
                Some(state != "Z")
            })
            .unwrap_or(false)
    }

    /// Kills `pid` on drop unless it is gone, so a failed test leaves nothing.
    pub(crate) struct KillOnDrop(pub(crate) u32);

    impl Drop for KillOnDrop {
        fn drop(&mut self) {
            if alive(self.0) {
                let pid = nix::unistd::Pid::from_raw(self.0 as i32);
                let _ = nix::sys::signal::kill(pid, nix::sys::signal::Signal::SIGKILL);
            }
        }
    }

    pub(crate) fn wait_until_gone(pid: u32, limit: Duration) -> bool {
        let deadline = std::time::Instant::now() + limit;
        while std::time::Instant::now() < deadline {
            if !alive(pid) {
                return true;
            }
            std::thread::sleep(Duration::from_millis(20));
        }
        false
    }

    /// The helper side: spawn an engine stand-in, print the pids to watch,
    /// wait to be killed. `HELPER_ENV` names a file for a wrapper's child pid.
    #[tokio::test]
    async fn lifeline_helper_parent() {
        let Some(mode) = std::env::var_os(HELPER_ENV) else {
            return;
        };
        let child_pid_file = std::path::PathBuf::from(&mode);
        let command = if mode == "direct" {
            sleeper()
        } else {
            wrapper(&child_pid_file)
        };
        let (child, _watch) = super::spawn(command).unwrap();
        println!("pid={}", child.id().unwrap());
        if mode != "direct" {
            let wrapped = tokio::time::timeout(Duration::from_secs(10), async {
                loop {
                    if let Ok(pid) = std::fs::read_to_string(&child_pid_file) {
                        return pid;
                    }
                    tokio::time::sleep(Duration::from_millis(10)).await;
                }
            })
            .await
            .expect("the wrapper reports its child");
            println!("pid={}", wrapped.trim());
        }
        println!("ready");
        tokio::time::sleep(Duration::from_secs(60)).await;
        drop(child);
    }

    /// Run test `helper` as a parent with `env` set to `mode`; it prints
    /// `pid=<pid>` per process to watch, then `ready`. SIGKILL it then, and
    /// assert every process it reported dies within seconds.
    pub(crate) fn kill_parent_and_expect_gone(helper: &str, env: &str, mode: &std::ffi::OsStr) {
        use std::io::BufRead as _;

        let mut parent = std::process::Command::new(std::env::current_exe().unwrap())
            .args(["--exact", helper, "--nocapture"])
            .env(env, mode)
            .stdout(std::process::Stdio::piped())
            .spawn()
            .unwrap();
        let _parent_guard = KillOnDrop(parent.id());
        let stdout = parent.stdout.take().unwrap();
        let (pid_tx, pid_rx) = std::sync::mpsc::channel();
        std::thread::spawn(move || {
            for line in std::io::BufReader::new(stdout).lines() {
                let Ok(line) = line else { return };
                if let Some(pid) = line.strip_prefix("pid=") {
                    let _ = pid_tx.send(Some(pid.trim().parse::<u32>().unwrap()));
                } else if line == "ready" {
                    let _ = pid_tx.send(None);
                    return;
                }
            }
        });
        let mut guards = Vec::new();
        while let Some(pid) = pid_rx
            .recv_timeout(Duration::from_secs(20))
            .expect("the helper reports its engine")
        {
            guards.push(KillOnDrop(pid));
        }
        assert!(!guards.is_empty(), "the helper reports at least its engine");
        for guard in &guards {
            assert!(alive(guard.0), "{} runs while its parent lives", guard.0);
        }

        parent.kill().unwrap();
        parent.wait().unwrap();
        for guard in &guards {
            assert!(
                wait_until_gone(guard.0, Duration::from_secs(5)),
                "{} must die within seconds of a SIGKILL to its parent",
                guard.0
            );
        }
    }

    #[test]
    fn a_killed_parent_takes_its_engine_with_it() {
        kill_parent_and_expect_gone(HELPER, HELPER_ENV, "direct".as_ref());
    }

    /// The parent-death signal reaches only the wrapper; the watchdog takes
    /// the engine it started too.
    #[test]
    fn a_killed_parent_takes_a_wrapped_engine_with_it() {
        let dir = tempfile::tempdir().unwrap();
        kill_parent_and_expect_gone(HELPER, HELPER_ENV, dir.path().join("child.pid").as_os_str());
    }

    /// A clean end drops the watch: that stops the watchdog, not the group.
    #[tokio::test]
    async fn dropping_the_watch_spares_the_engine() {
        let (mut child, watch) = super::spawn(sleeper()).unwrap();
        let pid = child.id().unwrap();
        let _guard = KillOnDrop(pid);
        let watchdog = watch
            .0
            .as_ref()
            .and_then(|w| w.id())
            .expect("a watchdog runs");
        assert_eq!(
            nix::unistd::getpgid(Some(nix::unistd::Pid::from_raw(watchdog as i32))).unwrap(),
            nix::unistd::Pid::from_raw(pid as i32),
            "the watchdog joins the engine's group"
        );
        drop(watch);
        assert!(
            wait_until_gone(watchdog, Duration::from_secs(5)),
            "a dropped watch stops its watchdog"
        );
        assert!(alive(pid), "a dropped watch leaves the engine running");
        child.kill().await.unwrap();
    }

    /// The spawning thread outlives the caller's: an engine spawned from a
    /// thread that then ends keeps running.
    #[test]
    fn an_engine_outlives_the_thread_that_asked_for_it() {
        let (child_tx, child_rx) = std::sync::mpsc::channel();
        std::thread::spawn(move || {
            let runtime = tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .unwrap();
            let child = runtime.block_on(async {
                let (child, watch) = super::spawn(sleeper()).unwrap();
                drop(watch);
                child
            });
            child_tx.send(child.id().unwrap()).unwrap();
            // Keep the child unreaped by tokio: forget it, the guard kills it.
            std::mem::forget(child);
        })
        .join()
        .unwrap();
        let pid = child_rx.recv().unwrap();
        let _guard = KillOnDrop(pid);
        std::thread::sleep(Duration::from_millis(300));
        assert!(
            alive(pid),
            "an ended caller thread must not kill the engine"
        );
    }
}
