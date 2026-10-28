//! A child Aether spawns starts with default signal state.
//!
//! An ignored signal (`SIG_IGN`) and the signal mask both survive `exec`. A
//! daemon launched with `&` from a non-interactive shell has SIGINT ignored,
//! and every child would inherit that, so ^C could not stop it.

#[cfg(unix)]
#[allow(unsafe_code)]
mod unix {
    use std::io;
    use std::os::unix::process::CommandExt;

    use nix::sys::signal::{SigHandler, SigSet, SigmaskHow, Signal, signal, sigprocmask};

    const RESET: [Signal; 4] = [
        Signal::SIGINT,
        Signal::SIGQUIT,
        Signal::SIGPIPE,
        Signal::SIGTERM,
    ];

    /// Reset dispositions to default and unblock every signal.
    ///
    /// Only raw `sigaction` and `sigprocmask` calls on stack data, so it is
    /// async-signal-safe and may run between `fork` and `exec`.
    pub fn reset_child_signals() -> io::Result<()> {
        for sig in RESET {
            // SAFETY: installing SIG_DFL runs no Rust code in the handler.
            unsafe { signal(sig, SigHandler::SigDfl) }.map_err(io::Error::from)?;
        }
        sigprocmask(SigmaskHow::SIG_SETMASK, Some(&SigSet::empty()), None).map_err(io::Error::from)
    }

    /// [`reset_signals`] for a PTY-backed command, which runs it after
    /// making the child a session leader.
    pub fn reset_pty_signals(command: pty_process::Command) -> pty_process::Command {
        // SAFETY: same async-signal-safe closure as `reset_signals`.
        unsafe { command.pre_exec(reset_child_signals) }
    }

    /// Make `command`'s child call [`reset_child_signals`] before exec; a
    /// failure aborts the spawn instead of running with a bad signal state.
    pub fn reset_signals(command: &mut std::process::Command) {
        // SAFETY: the closure is async-signal-safe and allocates nothing.
        unsafe { command.pre_exec(reset_child_signals) };
    }

    /// Make `command`'s child receive SIGKILL when the thread that spawns it
    /// ends, so it cannot outlive a parent killed without a clean shutdown.
    ///
    /// The kernel signals when the spawning *thread* ends, not the process:
    /// spawn from a thread that lives as long as the process. The mark is
    /// kept across `exec`, except into a set-user-ID program.
    #[cfg(target_os = "linux")]
    pub fn die_with_parent(command: &mut std::process::Command) {
        use nix::errno::Errno;
        use nix::unistd::{getpid, getppid};

        let parent = getpid();
        // SAFETY: prctl and getppid are async-signal-safe syscalls, and the
        // closure allocates nothing: the error is a plain errno.
        unsafe {
            command.pre_exec(move || {
                nix::sys::prctl::set_pdeathsig(Signal::SIGKILL).map_err(io::Error::from)?;
                // A parent gone before prctl ran would never send the signal.
                if getppid() != parent {
                    return Err(io::Error::from(Errno::ESRCH));
                }
                Ok(())
            })
        };
    }

    /// No parent-death signal outside Linux; see the Linux variant.
    #[cfg(not(target_os = "linux"))]
    pub fn die_with_parent(_command: &mut std::process::Command) {}

    /// The parent's side of [`gate_start`]: the waiting child's pid, and the
    /// gate that lets it exec. Dropped unopened, it makes the child fail.
    #[cfg(target_os = "linux")]
    #[derive(Debug)]
    pub struct StartGate {
        pid_read: io::PipeReader,
        gate_write: io::PipeWriter,
    }

    #[cfg(target_os = "linux")]
    impl StartGate {
        /// Block until the child waits at the gate and return its pid; `None`
        /// when it never got there, as when the fork or an earlier hook failed.
        pub fn child_pid(&mut self) -> io::Result<Option<u32>> {
            use std::io::Read as _;

            let mut pid = [0u8; 4];
            match self.pid_read.read_exact(&mut pid) {
                Ok(()) => {}
                Err(error) if error.kind() == io::ErrorKind::UnexpectedEof => return Ok(None),
                Err(error) => return Err(error),
            }
            let pid = u32::from_ne_bytes(pid);
            assert!(pid > 1, "a forked child's pid: {pid}");
            Ok(Some(pid))
        }

        /// Let the child go on to exec.
        pub fn open(mut self) -> io::Result<()> {
            use std::io::Write as _;

            self.gate_write.write_all(b"g")
        }
    }

    /// Hold `command`'s child after its fork, before it execs, until the
    /// returned gate is opened. Register after the hooks that must run first.
    #[cfg(target_os = "linux")]
    pub fn gate_start(command: &mut std::process::Command) -> io::Result<StartGate> {
        use std::os::fd::AsRawFd as _;

        use nix::errno::Errno;

        let (pid_read, pid_write) = io::pipe()?;
        let (gate_read, gate_write) = io::pipe()?;
        let parent_ends = [pid_read.as_raw_fd(), gate_write.as_raw_fd()];
        // SAFETY: close, getpid, write and read are async-signal-safe syscalls
        // on fds and stack data; the closure allocates nothing.
        unsafe {
            command.pre_exec(move || {
                // Without the parent's copies here, its death or drop reads as EOF.
                for fd in parent_ends {
                    nix::libc::close(fd);
                }
                let pid = nix::unistd::getpid().as_raw().to_ne_bytes();
                if nix::unistd::write(&pid_write, &pid)? != pid.len() {
                    return Err(io::Error::from(Errno::EIO));
                }
                let mut byte = [0u8; 1];
                loop {
                    match nix::unistd::read(&gate_read, &mut byte) {
                        Ok(1) => return Ok(()),
                        Ok(_) => return Err(io::Error::from(Errno::ECANCELED)),
                        Err(Errno::EINTR) => {}
                        Err(errno) => return Err(io::Error::from(errno)),
                    }
                }
            })
        };
        Ok(StartGate {
            pid_read,
            gate_write,
        })
    }

    /// Signals a process group is stopped with before its SIGKILL.
    const STOP: [Signal; 3] = [Signal::SIGHUP, Signal::SIGINT, Signal::SIGTERM];

    /// Make `command`'s child ignore SIGHUP, SIGINT and SIGTERM from the start,
    /// so only SIGKILL ends it. Register after [`reset_signals`], which it overrides.
    pub fn ignore_stop_signals(command: &mut std::process::Command) {
        // SAFETY: installing SIG_IGN is a raw sigaction call on stack data.
        unsafe {
            command.pre_exec(|| {
                for sig in STOP {
                    signal(sig, SigHandler::SigIgn).map_err(io::Error::from)?;
                }
                Ok(())
            })
        };
    }
}

/// Ignore SIGINT in this process, as a launcher's `&` would.
#[cfg(all(unix, feature = "test-support"))]
#[allow(unsafe_code)]
pub fn ignore_sigint() -> std::io::Result<()> {
    use nix::sys::signal::{SigHandler, Signal, signal};
    // SAFETY: SIG_IGN installs no handler.
    unsafe { signal(Signal::SIGINT, SigHandler::SigIgn) }
        .map(drop)
        .map_err(std::io::Error::from)
}

#[cfg(target_os = "linux")]
pub use unix::{StartGate, gate_start};
#[cfg(unix)]
pub use unix::{
    die_with_parent, ignore_stop_signals, reset_child_signals, reset_pty_signals, reset_signals,
};

#[cfg(not(unix))]
pub fn reset_signals(_command: &mut std::process::Command) {}

#[cfg(not(unix))]
pub fn die_with_parent(_command: &mut std::process::Command) {}

#[cfg(all(test, unix))]
#[allow(unsafe_code, clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use std::process::Command;

    use nix::sys::signal::{SigHandler, SigSet, SigmaskHow, Signal, signal, sigprocmask};

    use super::*;

    /// Ignore SIGINT and block SIGTERM in this process, as a launcher might.
    fn poison_parent() {
        // SAFETY: SIG_IGN installs no handler.
        unsafe { signal(Signal::SIGINT, SigHandler::SigIgn) }.unwrap();
        let mut blocked = SigSet::empty();
        blocked.add(Signal::SIGTERM);
        sigprocmask(SigmaskHow::SIG_BLOCK, Some(&blocked), None).unwrap();
    }

    /// Print the child's own `SigIgn`/`SigBlk` lines from /proc, as hex bitsets.
    /// No shell in between: dash (Ubuntu's `sh`) clears its signal mask itself.
    fn read_masks() -> Command {
        let mut command = Command::new("grep");
        command.args(["-E", "^(SigIgn|SigBlk)", "/proc/self/status"]);
        command
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn child_starts_with_default_signal_state() {
        poison_parent();
        let mut command = read_masks();
        let plain = command.output().unwrap();
        let plain = String::from_utf8(plain.stdout).unwrap();
        assert!(!plain.contains("SigIgn:\t0000000000000000"), "{plain}");
        assert!(!plain.contains("SigBlk:\t0000000000000000"), "{plain}");

        reset_signals(&mut command);
        let reset = String::from_utf8(command.output().unwrap().stdout).unwrap();
        assert!(reset.contains("SigBlk:\t0000000000000000"), "{reset}");
        let ignored = reset.lines().find(|l| l.starts_with("SigIgn:")).unwrap();
        let bits = u64::from_str_radix(ignored.rsplit('\t').next().unwrap(), 16).unwrap();
        assert_eq!(bits & (1 << (Signal::SIGINT as u64 - 1)), 0, "{reset}");
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn a_stop_signal_ignoring_child_ignores_them_after_a_reset() {
        let mut command = read_masks();
        reset_signals(&mut command);
        ignore_stop_signals(&mut command);
        let status = String::from_utf8(command.output().unwrap().stdout).unwrap();
        let ignored = status.lines().find(|l| l.starts_with("SigIgn:")).unwrap();
        let bits = u64::from_str_radix(ignored.rsplit('\t').next().unwrap(), 16).unwrap();
        for sig in [Signal::SIGHUP, Signal::SIGINT, Signal::SIGTERM] {
            assert_ne!(
                bits & (1 << (sig as u64 - 1)),
                0,
                "{sig} not ignored: {status}"
            );
        }
        assert_eq!(bits & (1 << (Signal::SIGQUIT as u64 - 1)), 0, "{status}");
    }

    /// Spawn `command` through a gate on another thread, as spawning blocks
    /// until the child execs; returns the gate and the pending spawn.
    #[cfg(target_os = "linux")]
    fn gated(
        mut command: Command,
    ) -> (
        StartGate,
        std::thread::JoinHandle<std::io::Result<std::process::Child>>,
    ) {
        let gate = gate_start(&mut command).unwrap();
        (gate, std::thread::spawn(move || command.spawn()))
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn a_gated_child_runs_only_once_its_gate_opens() {
        let dir = tempfile::tempdir().unwrap();
        let ran = dir.path().join("ran");
        let mut command = Command::new("/bin/sh");
        command.args(["-c", r#": > "$1""#, "sh", ran.to_str().unwrap()]);
        let (mut gate, spawning) = gated(command);
        let pid = gate
            .child_pid()
            .unwrap()
            .expect("the child waits at the gate");
        std::thread::sleep(std::time::Duration::from_millis(300));
        assert!(!ran.exists(), "the child ran before its gate opened");
        assert!(!spawning.is_finished(), "spawn waits for the exec");
        gate.open().unwrap();
        let mut child = spawning.join().unwrap().unwrap();
        assert_eq!(child.id(), pid, "the gate reported the spawned child");
        assert!(child.wait().unwrap().success());
        assert!(ran.exists(), "the child ran once its gate opened");
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn a_dropped_gate_fails_the_spawn_without_running_the_child() {
        let dir = tempfile::tempdir().unwrap();
        let ran = dir.path().join("ran");
        let mut command = Command::new("/bin/sh");
        command.args(["-c", r#": > "$1""#, "sh", ran.to_str().unwrap()]);
        let (mut gate, spawning) = gated(command);
        assert!(gate.child_pid().unwrap().is_some());
        drop(gate);
        let error = spawning
            .join()
            .unwrap()
            .expect_err("an unopened gate fails the spawn");
        assert_eq!(error.raw_os_error(), Some(nix::libc::ECANCELED), "{error}");
        assert!(!ran.exists(), "the child never ran");
    }

    /// Spawn `sleep 30` from a thread that ends at once; `die` marks it.
    #[cfg(target_os = "linux")]
    fn spawn_from_ended_thread(die: bool) -> std::process::Child {
        let spawned = std::thread::spawn(move || {
            let mut command = Command::new("sleep");
            command.arg("30");
            if die {
                die_with_parent(&mut command);
            }
            command.spawn().unwrap()
        });
        spawned.join().unwrap()
    }

    /// Whether `child` exits by SIGKILL within five seconds; killed either way.
    #[cfg(target_os = "linux")]
    fn killed_within_five_seconds(child: &mut std::process::Child) -> bool {
        use std::os::unix::process::ExitStatusExt as _;
        for _ in 0..100 {
            if let Some(status) = child.try_wait().unwrap() {
                return status.signal() == Some(Signal::SIGKILL as i32);
            }
            std::thread::sleep(std::time::Duration::from_millis(50));
        }
        child.kill().unwrap();
        child.wait().unwrap();
        false
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn a_marked_child_dies_with_the_thread_that_spawned_it() {
        let mut marked = spawn_from_ended_thread(true);
        assert!(killed_within_five_seconds(&mut marked));

        let mut plain = spawn_from_ended_thread(false);
        std::thread::sleep(std::time::Duration::from_millis(200));
        let survived = plain.try_wait().unwrap().is_none();
        plain.kill().unwrap();
        plain.wait().unwrap();
        assert!(survived, "an unmarked child outlives its spawning thread");
    }
}
