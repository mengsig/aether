//! Engine processes die with the process that spawned them.
//!
//! On Linux every engine process is spawned with a parent-death signal
//! ([`aether_spawn::die_with_parent`]), so a daemon that dies without a clean
//! shutdown, SIGKILL included, takes its engines with it. The kernel sends
//! that signal when the *thread* that spawned the child ends, and tokio
//! retires idle threads, so every spawn runs on one thread that lives as long
//! as the process. That signal reaches only the process spawned, not a CLI
//! wrapper's own child, so each engine's process group also gets a watchdog:
//! a watchdog born in that session that kills the group when its pipe closes.
//! The engine is held between fork and exec until its watchdog is ready, so
//! it cannot start a child first. The next daemon start reaps what
//! left the group ([`crate::orphans`]).
//! Elsewhere nothing is marked and no watchdog runs.

use std::io;

use tokio::process::{Child, Command};

use crate::orphans::EngineRecord;

/// Spawn `command` leading a process group of its own, so neither it nor its
/// group can outlive this process. Keep the [`GroupWatch`] for as long as the
/// engine runs. Must be called inside a tokio runtime.
pub(crate) fn spawn(command: Command) -> io::Result<(Child, GroupWatch)> {
    spawn_recorded(command, None).map(|(child, watch, _)| (child, watch))
}

/// [`spawn`], and with the [`crate::orphans::tag`] `token`, record the engine
/// before it execs, so a daemon crash never leaves an engine the next start
/// cannot find. Keep the record for as long as the engine runs.
#[cfg(target_os = "linux")]
pub(crate) fn spawn_recorded(
    mut command: Command,
    token: Option<&str>,
) -> io::Result<(Child, GroupWatch, Option<EngineRecord>)> {
    let runtime = tokio::runtime::Handle::try_current().map_err(io::Error::other)?;
    aether_spawn::detach_session(command.as_std_mut());
    aether_spawn::die_with_parent(command.as_std_mut());
    let pending_watch = aether_spawn::watch_session(command.as_std_mut())?;
    let mut gate = aether_spawn::gate_start(command.as_std_mut())?;
    let (reply_tx, reply_rx) = std::sync::mpsc::sync_channel(1);
    let job: Job = Box::new(move || {
        // tokio registers the child with the caller's runtime for reaping.
        let _entered = runtime.enter();
        let spawned = command.spawn();
        // The caller waits for this reply; if it is gone, dropping the child kills it.
        let _ = reply_tx.send(spawned);
    });
    spawner()?.send(job).map_err(|_| spawner_gone())?;
    let watch = GroupWatch(pending_watch.finish()?, tokio::runtime::Handle::current());
    // An early return drops the gate unopened, which fails the spawn.
    let pid = gate.child_pid().map_err(|error| {
        io::Error::new(
            error.kind(),
            format!("reading the engine's pid at its start gate: {error}"),
        )
    })?;
    let record = match (pid, token) {
        (Some(pid), Some(token)) => EngineRecord::register(pid, token).map_err(|error| {
            io::Error::new(error.kind(), format!("the engine was not started: {error}"))
        })?,
        _ => None,
    };
    #[cfg(test)]
    if let Some(pid) = pid {
        tests::pause_before_watch(pid);
        tests::fail_watchdog_if_asked().map_err(|error| {
            io::Error::new(error.kind(), format!("starting engine watchdog: {error}"))
        })?;
    }
    let opened = gate.open();
    let child = reply_rx.recv().map_err(|_| spawner_gone())??;
    debug_assert!(opened.is_ok(), "a child execs only through an opened gate");
    debug_assert_eq!(child.id(), pid, "the gate reports the spawned engine");
    opened?;
    Ok((child, watch, record))
}

/// No start time is readable off Linux, so nothing is recorded.
#[cfg(not(target_os = "linux"))]
pub(crate) fn spawn_recorded(
    mut command: Command,
    _token: Option<&str>,
) -> io::Result<(Child, GroupWatch, Option<EngineRecord>)> {
    aether_spawn::detach_session(command.as_std_mut());
    Ok((command.spawn()?, GroupWatch, None))
}

#[cfg(not(target_os = "linux"))]
impl GroupWatch {
    fn end_group(self) {}
}

/// A [`GroupWatch`] that ends its group once dropped, cancellation included,
/// for a process whose leftovers must not outlive it.
#[derive(Debug)]
pub(crate) struct GroupEnd(Option<GroupWatch>);

impl GroupWatch {
    pub(crate) fn ending_on_drop(self) -> GroupEnd {
        GroupEnd(Some(self))
    }
}

impl Drop for GroupEnd {
    fn drop(&mut self) {
        if let Some(watch) = self.0.take() {
            watch.end_group();
        }
    }
}

/// Kills its engine's process group if this process dies first. Dropping it
/// stops only the watchdog, so a clean end leaves the group to its owner.
#[cfg(target_os = "linux")]
#[derive(Debug)]
pub(crate) struct GroupWatch(Option<aether_spawn::SessionWatch>, tokio::runtime::Handle);

#[cfg(not(target_os = "linux"))]
#[derive(Debug)]
pub(crate) struct GroupWatch;

#[cfg(target_os = "linux")]
impl GroupWatch {
    /// EOF stops the group; ordinary drop kills only the watchdog instead.
    fn end_group(mut self) {
        if let Some(watch) = self.0.take() {
            let pid = watch.id();
            watch.end_group();
            self.1.spawn(reap_watchdog(pid));
        }
    }
}

#[cfg(target_os = "linux")]
impl Drop for GroupWatch {
    fn drop(&mut self) {
        if let Some(watch) = self.0.take() {
            let pid = watch.id();
            drop(watch);
            self.1.spawn(reap_watchdog(pid));
        }
    }
}

/// Reap our CLONE_PARENT children using pidfd readiness when available.
#[cfg(target_os = "linux")]
async fn reap_watchdog(pid: u32) {
    use rustix::process::{Pid, PidfdFlags};

    let Some(raw) = Pid::from_raw(pid as i32) else {
        unreachable!("validated watchdog pid");
    };
    let readiness = rustix::process::pidfd_open(raw, PidfdFlags::empty())
        .map_err(io::Error::from)
        .and_then(tokio::io::unix::AsyncFd::new);
    match readiness {
        Ok(fd) => {
            if let Err(error) = fd.readable().await {
                tracing::warn!(pid, %error, "watchdog readiness failed; polling its exit");
            }
        }
        Err(error) => {
            tracing::warn!(pid, %error, "watchdog pidfd unavailable; polling its exit");
        }
    }
    loop {
        match nix::sys::wait::waitpid(
            nix::unistd::Pid::from_raw(pid as i32),
            Some(nix::sys::wait::WaitPidFlag::WNOHANG),
        ) {
            Ok(nix::sys::wait::WaitStatus::StillAlive) | Err(nix::errno::Errno::EINTR) => {
                tokio::time::sleep(std::time::Duration::from_millis(100)).await;
            }
            Ok(_) | Err(nix::errno::Errno::ECHILD) => return,
            Err(error) => {
                tracing::error!(pid, %error, "could not reap engine watchdog");
                return;
            }
        }
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

    /// When set, [`super::spawn`] reports the gated engine and pauses this long
    /// before its start gate opens, as a slow daemon might.
    static PAUSE_BEFORE_WATCH_MS: std::sync::atomic::AtomicU64 =
        std::sync::atomic::AtomicU64::new(0);

    std::thread_local! {
        /// When set, the watchdog setup fails on this thread, as a
        /// spawn out of file descriptors or processes would.
        static FAIL_WATCHDOG: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
    }

    pub(super) fn fail_watchdog_if_asked() -> std::io::Result<()> {
        if FAIL_WATCHDOG.get() {
            return Err(std::io::Error::from_raw_os_error(nix::libc::EAGAIN));
        }
        Ok(())
    }

    pub(super) fn pause_before_watch(pid: u32) {
        let pause = PAUSE_BEFORE_WATCH_MS.load(std::sync::atomic::Ordering::SeqCst);
        if pause == 0 {
            return;
        }
        println!("pid={pid}");
        // Time for an engine that was let run too early to start its child.
        std::thread::sleep(Duration::from_millis(300));
        println!("ready");
        std::thread::sleep(Duration::from_millis(pause));
    }

    /// An engine stand-in leading its own group, as every engine does.
    fn engine(program: &str, args: &[&str]) -> tokio::process::Command {
        let mut command = tokio::process::Command::new(program);
        command.args(args).kill_on_drop(true);
        command
    }

    fn sleeper() -> tokio::process::Command {
        engine("sleep", &["30"])
    }

    /// A CLI wrapper that starts the real engine as its child and waits on it,
    /// as the npm `codex` launcher does without `process.execve`. With
    /// `ignore_term` both ignore SIGTERM, as a test runner might.
    fn wrapper(child_pid_file: &str, ignore_term: bool) -> tokio::process::Command {
        let script = r#"sleep 30 & echo $! > "$1.tmp" && mv "$1.tmp" "$1"; wait"#;
        let script = if ignore_term {
            format!("trap '' TERM; {script}")
        } else {
            script.to_owned()
        };
        engine("/bin/sh", &["-c", &script, "sh", child_pid_file])
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
    /// wait to be killed. `HELPER_ENV` is `direct`, or a file for a wrapper's
    /// child pid; prefixed `term:`, the group is sent SIGTERM before `ready`,
    /// and prefixed `gated:` the spawn pauses before its start gate opens.
    #[tokio::test]
    async fn lifeline_helper_parent() {
        let Some(mode) = std::env::var_os(HELPER_ENV) else {
            return;
        };
        let mode = mode.into_string().unwrap();
        if let Some(child_pid_file) = mode.strip_prefix("gated:") {
            PAUSE_BEFORE_WATCH_MS.store(60_000, std::sync::atomic::Ordering::SeqCst);
            let _ = super::spawn(wrapper(child_pid_file, false));
            panic!("the spawn pauses until this helper is killed");
        }
        let (stopping, child_pid_file) = match mode.strip_prefix("term:") {
            Some(file) => (true, file),
            None => (false, mode.as_str()),
        };
        let command = if mode == "direct" {
            sleeper()
        } else {
            wrapper(child_pid_file, stopping)
        };
        let (child, watch) = super::spawn(command).unwrap();
        let pid = child.id().unwrap();
        println!("pid={pid}");
        println!("pid={}", watch.0.as_ref().unwrap().id());
        if mode != "direct" {
            let wrapped = tokio::time::timeout(Duration::from_secs(10), async {
                loop {
                    if let Ok(pid) = std::fs::read_to_string(child_pid_file) {
                        return pid;
                    }
                    tokio::time::sleep(Duration::from_millis(10)).await;
                }
            })
            .await
            .expect("the wrapper reports its child");
            println!("pid={}", wrapped.trim());
        }
        if stopping {
            // The first step of a group stop; its KILL would follow the grace.
            let group = nix::unistd::Pid::from_raw(pid as i32);
            nix::sys::signal::killpg(group, nix::sys::signal::Signal::SIGTERM).unwrap();
            tokio::time::sleep(Duration::from_millis(300)).await;
            assert!(alive(pid), "the wrapper ignores TERM");
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

    /// A daemon killed in a group stop's grace, after its TERM and before its
    /// KILL, still takes the members that ignore TERM.
    #[test]
    fn a_parent_killed_mid_stop_takes_a_term_ignoring_engine_with_it() {
        let dir = tempfile::tempdir().unwrap();
        let mode = format!("term:{}", dir.path().join("child.pid").display());
        kill_parent_and_expect_gone(HELPER, HELPER_ENV, mode.as_ref());
    }

    /// A daemon killed after its engine forked but before the start gate opened
    /// leaves nothing: the engine has not run, so it started no child.
    #[test]
    fn a_parent_killed_before_the_gate_opens_leaves_no_wrapped_engine() {
        let dir = tempfile::tempdir().unwrap();
        let child_pid_file = dir.path().join("child.pid");
        let mode = format!("gated:{}", child_pid_file.display());
        kill_parent_and_expect_gone(HELPER, HELPER_ENV, mode.as_ref());
        // Only an engine let run before its start gate opened writes this.
        if let Ok(pid) = std::fs::read_to_string(&child_pid_file) {
            let pid: u32 = pid.trim().parse().unwrap();
            let _guard = KillOnDrop(pid);
            panic!("the engine ran before its start gate opened; started {pid}");
        }
    }

    /// A clean end drops the watch: that stops the watchdog, not the group.
    #[tokio::test]
    async fn dropping_the_watch_spares_the_engine() {
        let (mut child, watch) = super::spawn(sleeper()).unwrap();
        let pid = child.id().unwrap();
        let _guard = KillOnDrop(pid);
        let watchdog = watch.0.as_ref().map(|w| w.id()).expect("a watchdog runs");
        assert_eq!(
            nix::unistd::getpgid(Some(nix::unistd::Pid::from_raw(watchdog as i32))).unwrap(),
            nix::unistd::Pid::from_raw(pid as i32),
            "the watchdog is born in the engine's group"
        );
        assert_eq!(
            nix::unistd::getsid(Some(nix::unistd::Pid::from_raw(watchdog as i32))).unwrap(),
            nix::unistd::Pid::from_raw(pid as i32),
            "watchdog shares the engine's new session"
        );
        assert!(
            std::fs::read(format!("/proc/{watchdog}/environ"))
                .unwrap()
                .is_empty()
        );
        drop(watch);
        tokio::time::timeout(Duration::from_secs(5), async {
            while std::path::Path::new(&format!("/proc/{watchdog}")).exists() {
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        })
        .await
        .expect("the watchdog exits and is reaped");
        assert!(alive(pid), "a dropped watch leaves the engine running");
        child.kill().await.unwrap();
    }

    /// A watchdog that cannot start fails the spawn: the engine never runs.
    #[tokio::test]
    async fn an_engine_whose_watchdog_cannot_start_never_runs() {
        let marker = tempfile::tempdir().unwrap();
        let ran = marker.path().join("ran");
        let touch = engine(
            "/bin/sh",
            &["-c", r#": > "$1""#, "sh", ran.to_str().unwrap()],
        );
        FAIL_WATCHDOG.set(true);
        let spawned = super::spawn(touch);
        FAIL_WATCHDOG.set(false);
        let error = spawned.expect_err("an unwatched engine is not started");
        assert!(error.to_string().contains("watchdog"), "{error}");
        std::thread::sleep(Duration::from_millis(300));
        assert!(!ran.exists(), "the engine never ran");
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
