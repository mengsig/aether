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

#[cfg(unix)]
pub use unix::{die_with_parent, reset_child_signals, reset_pty_signals, reset_signals};

#[cfg(not(unix))]
pub fn reset_signals(_command: &mut std::process::Command) {}

#[cfg(not(unix))]
pub fn die_with_parent(_command: &mut std::process::Command) {}

#[cfg(all(test, unix))]
#[allow(unsafe_code, clippy::unwrap_used)]
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
