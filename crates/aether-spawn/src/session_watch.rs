//! A Linux watchdog born inside the engine's session, but reaped by its parent.
use std::io::{self, Read};
use std::os::fd::{AsRawFd, RawFd};
use std::os::unix::process::CommandExt;

use nix::libc;

/// The parent's half of a watchdog being created before the engine execs.
#[derive(Debug)]
pub struct PendingWatch {
    pid_read: io::PipeReader,
    control: io::PipeWriter,
    ready_write: io::PipeWriter,
}

/// Pins the group until EOF kills it or a clean drop kills only the watchdog.
/// Reap `id()` only after dropping this handle or calling `end_group`.
#[derive(Debug)]
pub struct SessionWatch {
    pid: u32,
    control: Option<io::PipeWriter>,
}

impl PendingWatch {
    /// Receive the watchdog's pid; none means an earlier spawn hook failed.
    pub fn finish(mut self) -> io::Result<Option<SessionWatch>> {
        let mut bytes = [0; 4];
        match self.pid_read.read_exact(&mut bytes) {
            Ok(()) => {}
            Err(error) if error.kind() == io::ErrorKind::UnexpectedEof => return Ok(None),
            Err(error) => return Err(error),
        }
        // The watchdog exists; release our writer so an early death reports EOF.
        drop(self.ready_write);
        let pid = u32::from_ne_bytes(bytes);
        assert!(pid > 1, "watchdog must be a child process");
        assert!(pid <= i32::MAX as u32, "watchdog pid must fit pid_t");
        Ok(Some(SessionWatch {
            pid,
            control: Some(self.control),
        }))
    }
}

impl SessionWatch {
    pub fn id(&self) -> u32 {
        self.pid
    }

    /// Close without disarming: the watchdog kills its own group on EOF.
    pub fn end_group(mut self) {
        drop(self.control.take());
    }
}

impl Drop for SessionWatch {
    fn drop(&mut self) {
        let Some(control) = self.control.take() else {
            return;
        };
        // The caller has not reaped this child, so its pid cannot be reused.
        let pid = nix::unistd::Pid::from_raw(self.pid as i32);
        if let Err(error) = nix::sys::signal::kill(pid, nix::sys::signal::Signal::SIGKILL)
            && error != nix::errno::Errno::ESRCH
        {
            // Closing the pipe on failure safely stops the whole group instead.
            eprintln!("could not stop engine watchdog: {error}");
        }
        // Pending SIGKILL takes precedence over EOF, including for a stopped watch.
        drop(control);
    }
}

/// Wait for readability while retaining ownership of the registered descriptor.
pub async fn wait_readable(fd: std::os::fd::OwnedFd) -> io::Result<()> {
    // SAFETY: OwnedFd keeps the same descriptor open until AsyncFd is dropped.
    let fd = unsafe {
        tokio::io::unix::AsyncFd::register_with_interest(fd, tokio::io::Interest::READABLE)
    }
    .map_err(io::Error::from)?;
    fd.readable().await.map(drop)
}

struct WatchArgs {
    control: RawFd,
    ready: RawFd,
    max_fd: i32,
    script: &'static std::ffi::CStr,
}

/// Install after setsid and before the start gate. The caller must reap the
/// returned pid: CLONE_PARENT makes the watchdog its child, not the engine's.
pub fn watch_session(command: &mut std::process::Command) -> io::Result<PendingWatch> {
    watch_session_command(command, c"printf r; exec 1>&-; read -r _; kill -s KILL 0")
}

fn watch_session_command(
    command: &mut std::process::Command,
    script: &'static std::ffi::CStr,
) -> io::Result<PendingWatch> {
    let (control_read, control) = io::pipe()?;
    let (pid_read, pid_write) = io::pipe()?;
    let (ready_read, ready_write) = io::pipe()?;
    let parent_fds = [control.as_raw_fd(), pid_read.as_raw_fd()];
    let ready_fd = ready_write.as_raw_fd();
    // Allocate the clone stack before fork; u128 supplies its 16-byte alignment.
    let mut stack = vec![0u128; 4096];
    // SAFETY: sysconf reads a process limit, outside the pre-exec hook.
    let max_fd = unsafe { libc::sysconf(libc::_SC_OPEN_MAX) };
    let max_fd = i32::try_from(max_fd).map_err(io::Error::other)?;
    if max_fd <= 0 {
        return Err(io::Error::other("invalid open-file limit"));
    }
    // SAFETY: the hook and clone callback use only syscalls and preallocated data.
    unsafe {
        command.pre_exec(move || {
            let pid = libc::getpid();
            if libc::getpgrp() != pid || libc::getsid(0) != pid {
                return Err(io::Error::from(nix::errno::Errno::EINVAL));
            }
            for fd in parent_fds {
                libc::close(fd);
            }
            let mut args = WatchArgs {
                control: control_read.as_raw_fd(),
                ready: ready_fd,
                max_fd,
                script,
            };
            start_watchdog(&mut args, &mut stack, &pid_write, &ready_read)
        });
    }
    Ok(PendingWatch {
        pid_read,
        control,
        ready_write,
    })
}

/// Fork a sibling in this session and wait for its ready byte.
/// Safety: call after setsid, with live pipes and a 16-byte-aligned downward-growing stack.
unsafe fn start_watchdog(
    args: &mut WatchArgs,
    stack: &mut [u128],
    pid_write: &io::PipeWriter,
    ready_read: &io::PipeReader,
) -> io::Result<()> {
    // SAFETY: clone copies the address space; stack and args live until it returns.
    let pid = unsafe {
        let stack_top = stack.as_mut_ptr().add(stack.len()).cast();
        libc::clone(
            watchdog,
            stack_top,
            libc::CLONE_PARENT | libc::SIGCHLD,
            (args as *mut WatchArgs).cast(),
        )
    };
    if pid < 0 {
        return Err(io::Error::last_os_error());
    }
    // SAFETY: only the forked engine's copy closes; the watchdog owns the writer now.
    unsafe {
        libc::close(args.ready);
    }
    let bytes = (pid as u32).to_ne_bytes();
    if nix::unistd::write(pid_write, &bytes)? != bytes.len() {
        return Err(io::Error::from(nix::errno::Errno::EIO));
    }
    let mut ready = [0u8];
    loop {
        match nix::unistd::read(ready_read, &mut ready) {
            Ok(1) if ready[0] == b'r' => return Ok(()),
            Err(nix::errno::Errno::EINTR) => continue,
            Err(error) => return Err(error.into()),
            Ok(_) => return Err(io::Error::from(nix::errno::Errno::EIO)),
        }
    }
}

extern "C" fn watchdog(raw: *mut libc::c_void) -> libc::c_int {
    // SAFETY: clone copied args and its stack; only async-signal-safe syscalls run.
    unsafe {
        let args = &*(raw as *const WatchArgs);
        if libc::dup2(args.control, 0) < 0
            || libc::dup2(args.ready, 1) < 0
            || libc::fcntl(0, libc::F_SETFD, 0) < 0
            || libc::fcntl(1, libc::F_SETFD, 0) < 0
            || libc::chdir(c"/".as_ptr()) < 0
        {
            libc::write(args.ready, b"e".as_ptr().cast(), 1);
            return 1;
        }
        let descriptors = WatchArgs {
            control: 0,
            ready: 1,
            max_fd: args.max_fd,
            script: args.script,
        };
        close_inherited(&descriptors);
        for signal in [libc::SIGHUP, libc::SIGINT, libc::SIGTERM] {
            if libc::signal(signal, libc::SIG_IGN) == libc::SIG_ERR {
                libc::write(1, b"e".as_ptr().cast(), 1);
                return 1;
            }
        }
        let argv = [
            c"/bin/sh".as_ptr(),
            c"-c".as_ptr(),
            args.script.as_ptr(),
            c"aether-engine-watchdog".as_ptr(),
            std::ptr::null(),
        ];
        let env = [std::ptr::null()];
        // The shell acknowledges only after exec released the daemon's copied address space.
        libc::execve(argv[0], argv.as_ptr(), env.as_ptr());
        libc::write(1, b"e".as_ptr().cast(), 1);
        1
    }
}

/// Close everything except the two lifeline descriptors.
/// Safety: call only in the cloned watchdog, before exec, with its live descriptors.
unsafe fn close_inherited(args: &WatchArgs) {
    let low = args.control.min(args.ready) as u32;
    let high = args.control.max(args.ready) as u32;
    let ranges = [
        (0, low.checked_sub(1)),
        (low + 1, high.checked_sub(1)),
        (high + 1, Some(u32::MAX)),
    ];
    for (first, last) in ranges {
        let Some(last) = last else { continue };
        if first > last {
            continue;
        }
        // SAFETY: ranges exclude the only descriptors the watchdog needs.
        if unsafe { libc::syscall(libc::SYS_close_range, first, last, 0) } == 0 {
            continue;
        }
        // Older Linux kernels lack close_range; bounded by the captured fd limit.
        for fd in first..=last.min(args.max_fd as u32 - 1) {
            unsafe {
                libc::close(fd as i32);
            }
        }
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;

    #[test]
    fn a_watchdog_refuses_an_inherited_session_before_starting_any_process() {
        for group_leader in [false, true] {
            let mut command = std::process::Command::new("/bin/sh");
            command.args(["-c", "exit 0"]);
            if group_leader {
                command.process_group(0);
            }
            let _pending = watch_session(&mut command).unwrap();
            let error = command.spawn().expect_err("an inherited session is unsafe");
            assert_eq!(error.raw_os_error(), Some(libc::EINVAL));
            assert_eq!(error.kind(), io::ErrorKind::InvalidInput);
        }
    }

    #[test]
    fn a_watchdog_that_exits_without_ready_never_runs_the_engine() {
        use nix::sys::{
            signal::{Signal, killpg},
            wait::waitpid,
        };
        use nix::unistd::{Pid, getpgid};
        use std::time::Duration;

        let dir = tempfile::tempdir().unwrap();
        let marker = dir.path().join("ran");
        let mut command = std::process::Command::new("/bin/sh");
        command.args(["-c", "echo ran > \"$1\"", "sh"]).arg(&marker);
        crate::detach_session(&mut command);
        let pending = watch_session_command(&mut command, c"exit 0").unwrap();
        let (sender, receiver) = std::sync::mpsc::channel();
        let spawning = std::thread::spawn(move || sender.send(command.output()).unwrap());
        let watch = pending.finish().unwrap().expect("watchdog forked");
        let pid = Pid::from_raw(watch.id() as i32);
        let group = getpgid(Some(pid)).unwrap();
        assert_ne!(group, nix::unistd::getpgrp());
        let result = receiver.recv_timeout(Duration::from_secs(5));
        if result.is_err() {
            killpg(group, Signal::SIGKILL).unwrap();
        }
        drop(watch);
        waitpid(pid, None).unwrap();
        spawning.join().unwrap();
        let error = result
            .expect("spawn must fail within seconds")
            .expect_err("EOF without ready is not a working watchdog");
        assert_eq!(error.raw_os_error(), Some(libc::EIO));
        assert!(!marker.exists(), "the unwatched engine must never run");
    }
}
