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
pub use unix::{reset_child_signals, reset_pty_signals, reset_signals};

#[cfg(not(unix))]
pub fn reset_signals(_command: &mut std::process::Command) {}

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

    /// `SigIgn`/`SigBlk` masks from /proc, as hex bitsets.
    const MASKS: &str = "grep -E '^(SigIgn|SigBlk)' /proc/self/status";

    #[cfg(target_os = "linux")]
    #[test]
    fn child_starts_with_default_signal_state() {
        poison_parent();
        let mut command = Command::new("sh");
        command.args(["-c", MASKS]);
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
}
