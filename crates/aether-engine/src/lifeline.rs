//! Engine processes die with the process that spawned them.
//!
//! On Linux every engine process is spawned with a parent-death signal
//! ([`aether_spawn::die_with_parent`]), so a daemon that dies without a clean
//! shutdown, SIGKILL included, takes its engines with it. The kernel sends
//! that signal when the *thread* that spawned the child ends, and tokio
//! retires idle threads, so every spawn runs on one thread that lives as long
//! as the process. Descendants an engine starts are not marked; the next
//! daemon start reaps those ([`crate::orphans`]). Elsewhere nothing is marked.

use std::io;

use tokio::process::{Child, Command};

/// Spawn `command` so its process cannot outlive this one. Must be called
/// inside a tokio runtime, like [`Command::spawn`].
#[cfg(target_os = "linux")]
pub(crate) fn spawn(mut command: Command) -> io::Result<Child> {
    let runtime = tokio::runtime::Handle::try_current().map_err(io::Error::other)?;
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
    reply_rx.recv().map_err(|_| spawner_gone())?
}

#[cfg(not(target_os = "linux"))]
pub(crate) fn spawn(mut command: Command) -> io::Result<Child> {
    command.spawn()
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
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

    use std::time::Duration;

    /// Re-run as the "daemon" of `a_killed_parent_takes_its_engine_with_it`.
    const HELPER_ENV: &str = "AETHER_TEST_LIFELINE_PARENT";

    fn sleeper() -> tokio::process::Command {
        let mut command = tokio::process::Command::new("sleep");
        command.arg("30").kill_on_drop(true);
        command
    }

    fn alive(pid: u32) -> bool {
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
    struct KillOnDrop(u32);

    impl Drop for KillOnDrop {
        fn drop(&mut self) {
            if alive(self.0) {
                let pid = nix::unistd::Pid::from_raw(self.0 as i32);
                let _ = nix::sys::signal::kill(pid, nix::sys::signal::Signal::SIGKILL);
            }
        }
    }

    fn wait_until_gone(pid: u32, limit: Duration) -> bool {
        let deadline = std::time::Instant::now() + limit;
        while std::time::Instant::now() < deadline {
            if !alive(pid) {
                return true;
            }
            std::thread::sleep(Duration::from_millis(20));
        }
        false
    }

    /// The helper side: spawn an engine stand-in, print its pid, wait to be killed.
    #[tokio::test]
    async fn lifeline_helper_parent() {
        if std::env::var_os(HELPER_ENV).is_none() {
            return;
        }
        let child = super::spawn(sleeper()).unwrap();
        println!("engine-pid={}", child.id().unwrap());
        tokio::time::sleep(Duration::from_secs(60)).await;
        drop(child);
    }

    #[test]
    fn a_killed_parent_takes_its_engine_with_it() {
        use std::io::BufRead as _;

        let mut parent = std::process::Command::new(std::env::current_exe().unwrap())
            .args([
                "--exact",
                "lifeline::tests::lifeline_helper_parent",
                "--nocapture",
            ])
            .env(HELPER_ENV, "1")
            .stdout(std::process::Stdio::piped())
            .spawn()
            .unwrap();
        let _parent_guard = KillOnDrop(parent.id());
        let stdout = parent.stdout.take().unwrap();
        let (pid_tx, pid_rx) = std::sync::mpsc::channel();
        std::thread::spawn(move || {
            for line in std::io::BufReader::new(stdout).lines() {
                let Ok(line) = line else { return };
                if let Some(pid) = line.strip_prefix("engine-pid=") {
                    let _ = pid_tx.send(pid.trim().parse::<u32>().unwrap());
                    return;
                }
            }
        });
        let engine = pid_rx
            .recv_timeout(Duration::from_secs(20))
            .expect("the helper reports its engine pid");
        let _engine_guard = KillOnDrop(engine);
        assert!(alive(engine), "the engine runs while its parent lives");

        parent.kill().unwrap();
        parent.wait().unwrap();
        assert!(
            wait_until_gone(engine, Duration::from_secs(5)),
            "the engine must die within seconds of a SIGKILL to its parent"
        );
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
            let child = runtime.block_on(async { super::spawn(sleeper()).unwrap() });
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
