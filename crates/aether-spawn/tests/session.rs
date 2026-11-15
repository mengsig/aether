#![cfg(target_os = "linux")]
#![allow(clippy::expect_used, clippy::unwrap_used, clippy::panic)]

//! Session detachment and group cleanup under a real controlling terminal.
//! Fixtures rerun this executable on a fresh PTY with bounded waits.

use std::io::{BufRead, BufReader, Read};
use std::os::unix::process::CommandExt;
use std::process::{Child, Command, Stdio};
use std::sync::mpsc;
use std::time::{Duration, Instant};

use nix::errno::Errno;
use nix::sys::signal::{Signal, killpg};
use nix::sys::wait::{WaitPidFlag, WaitStatus, waitpid};
use nix::unistd::Pid;

const FIXTURE_ENV: &str = "AETHER_SESSION_FIXTURE";
const FIXTURE_DEADLINE: Duration = Duration::from_secs(30);
const WAIT: Duration = Duration::from_secs(5);

struct PtyRun {
    success: bool,
    transcript: String,
}

/// Run the test `fixture` of this executable as a session leader on a new PTY.
fn run_in_pty(fixture: &str) -> PtyRun {
    let (pty, pts) = pty_process::blocking::open().expect("open a pty");
    pty.resize(pty_process::Size::new(24, 80))
        .expect("size the pty");
    let exe = std::env::current_exe().expect("path of the test executable");
    let mut child = pty_process::blocking::Command::new(exe)
        .args([fixture, "--exact", "--nocapture", "--test-threads=1"])
        .env(FIXTURE_ENV, fixture)
        .spawn(pts)
        .expect("spawn the fixture on the pty");
    let output = collect_output(pty);

    let began = Instant::now();
    let status = loop {
        if let Some(status) = child.try_wait().expect("poll the fixture") {
            break status;
        }
        if began.elapsed() > FIXTURE_DEADLINE {
            kill_group(child.id());
            child.wait().expect("reap the killed fixture");
            let transcript = drain(&output);
            panic!("fixture {fixture} hung; transcript:\n{transcript}");
        }
        std::thread::sleep(Duration::from_millis(20));
    };
    PtyRun {
        success: status.success(),
        transcript: drain(&output),
    }
}

/// Read the PTY on a thread: the read blocks until every slave fd closes.
fn collect_output(mut pty: pty_process::blocking::Pty) -> mpsc::Receiver<std::io::Result<Vec<u8>>> {
    let (sender, receiver) = mpsc::channel();
    std::thread::spawn(move || {
        let mut buf = [0u8; 4096];
        loop {
            let chunk = match pty.read(&mut buf) {
                Ok(0) => return,
                Ok(n) => Ok(buf[..n].to_vec()),
                Err(error) if error.raw_os_error() == Some(nix::libc::EIO) => return,
                Err(error) if error.kind() == std::io::ErrorKind::Interrupted => continue,
                Err(error) => Err(error),
            };
            let failed = chunk.is_err();
            if sender.send(chunk).is_err() || failed {
                return;
            }
        }
    });
    receiver
}

fn drain(output: &mpsc::Receiver<std::io::Result<Vec<u8>>>) -> String {
    let deadline = Instant::now() + WAIT;
    let mut bytes = Vec::new();
    loop {
        match output.recv_timeout(deadline.saturating_duration_since(Instant::now())) {
            Ok(chunk) => bytes.extend(chunk.expect("read PTY output")),
            Err(mpsc::RecvTimeoutError::Disconnected) => break,
            Err(mpsc::RecvTimeoutError::Timeout) => panic!("PTY remained open after fixture exit"),
        }
    }
    String::from_utf8_lossy(&bytes).into_owned()
}

fn kill_group(pgid: u32) {
    assert!(pgid > 1);
    assert_ne!(pgid, std::process::id());
    let pgid = Pid::from_raw(i32::try_from(pgid).expect("pgid fits i32"));
    match killpg(pgid, Signal::SIGKILL) {
        Ok(()) | Err(Errno::ESRCH) => {}
        Err(e) => panic!("cannot kill process group {pgid}: {e}"),
    }
}

/// Kills the process group on drop, so a failing fixture leaves nothing behind.
struct KillGroupOnDrop(Option<u32>);

impl Drop for KillGroupOnDrop {
    fn drop(&mut self) {
        if let Some(group) = self.0.take() {
            kill_group(group);
        }
    }
}

#[derive(Debug)]
struct ProcStat {
    state: char,
    pgrp: u32,
    session: u32,
    tty_nr: i64,
}

/// `None` once the process no longer exists.
fn proc_stat(pid: u32) -> Option<ProcStat> {
    let text = match std::fs::read_to_string(format!("/proc/{pid}/stat")) {
        Ok(text) => text,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return None,
        Err(e) => panic!("cannot read /proc/{pid}/stat: {e}"),
    };
    // The command name is parenthesised and may hold spaces: parse after it.
    let after_name = &text[text.rfind(')').expect("a command name in stat") + 1..];
    let fields: Vec<&str> = after_name.split_whitespace().collect();
    assert!(fields.len() > 5, "short stat line: {text}");
    Some(ProcStat {
        state: fields[0].chars().next().expect("a state letter"),
        pgrp: fields[2].parse().expect("pgrp"),
        session: fields[3].parse().expect("session"),
        tty_nr: fields[4].parse().expect("tty_nr"),
    })
}

fn is_gone(pid: u32) -> bool {
    proc_stat(pid).is_none_or(|stat| stat.state == 'Z')
}

fn wait_until(what: &str, mut done: impl FnMut() -> bool) {
    let began = Instant::now();
    while !done() {
        assert!(began.elapsed() < WAIT, "timed out waiting for {what}");
        std::thread::sleep(Duration::from_millis(10));
    }
}

fn fixture_requested(name: &str) -> bool {
    std::env::var(FIXTURE_ENV).is_ok_and(|requested| requested == name)
}

fn assert_fixture_passed(run: &PtyRun, markers: &[&str]) {
    assert!(
        run.success,
        "fixture failed; transcript:\n{}",
        run.transcript
    );
    for marker in markers {
        assert!(
            run.transcript.contains(marker),
            "missing {marker:?}; transcript:\n{}",
            run.transcript
        );
    }
}

/// A test runner can ignore or block job-control signals; the reproduction needs defaults.
#[allow(unsafe_code)]
fn default_sigttin(command: &mut Command) {
    use nix::sys::signal::{SigHandler, SigSet, SigmaskHow, signal, sigprocmask};

    // SAFETY: the hook only changes signal state using syscalls and stack data.
    unsafe {
        command.pre_exec(|| {
            signal(Signal::SIGTTIN, SigHandler::SigDfl).map_err(std::io::Error::from)?;
            let mut mask = SigSet::empty();
            mask.add(Signal::SIGTTIN);
            sigprocmask(SigmaskHow::SIG_UNBLOCK, Some(&mask), None).map_err(std::io::Error::from)
        });
    }
}

fn silent(program: &str, script: &str) -> Command {
    let mut command = Command::new(program);
    command
        .args(["-c", script])
        .env_clear()
        .env("PATH", "/usr/bin:/bin")
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    command
}

/// Poll `child` for an exit or a stop; both are reported without blocking.
fn wait_for_exit_or_stop(child: &Child) -> WaitStatus {
    let pid = Pid::from_raw(i32::try_from(child.id()).expect("pid fits i32"));
    let flags = WaitPidFlag::WNOHANG | WaitPidFlag::WUNTRACED;
    let began = Instant::now();
    loop {
        match waitpid(pid, Some(flags)).expect("waitpid") {
            WaitStatus::StillAlive => {}
            status => return status,
        }
        assert!(
            began.elapsed() < WAIT,
            "child neither exited nor stopped within {WAIT:?}"
        );
        std::thread::sleep(Duration::from_millis(10));
    }
}

#[test]
fn a_detached_child_cannot_read_the_controlling_terminal() {
    let run = run_in_pty("fixture_detached_tty_read");
    assert_fixture_passed(&run, &["FIXTURE-OK detached read failed"]);
}

#[test]
fn fixture_detached_tty_read() {
    if !fixture_requested("fixture_detached_tty_read") {
        return;
    }
    let own = proc_stat(std::process::id()).expect("own stat");
    assert_ne!(own.tty_nr, 0, "the fixture must have a controlling tty");
    assert_eq!(
        own.session,
        std::process::id(),
        "the fixture leads its session"
    );

    let began = Instant::now();
    let mut detached = silent(
        "/bin/sh",
        "/bin/sh -c 'read x </dev/tty' </dev/null; status=$?; exit \"$status\"",
    );
    default_sigttin(&mut detached);
    aether_spawn::detach_session(&mut detached);
    // `wait_for_exit_or_stop` reaps it with waitpid, which clippy cannot see.
    #[allow(clippy::zombie_processes)]
    let child = detached.spawn().expect("spawn the detached shell");
    let mut cleanup = KillGroupOnDrop(Some(child.id()));
    let status = wait_for_exit_or_stop(&child);
    if matches!(status, WaitStatus::Exited(..) | WaitStatus::Signaled(..)) {
        cleanup.0 = None;
    }
    match status {
        WaitStatus::Exited(_, code) => assert_ne!(code, 0, "reading /dev/tty must fail"),
        other => panic!("the detached shell did not simply fail: {other:?}"),
    }
    assert!(began.elapsed() < WAIT);
    println!("FIXTURE-OK detached read failed");
}

#[test]
fn a_child_in_the_same_session_without_detaching_is_stopped_by_sigttin() {
    let run = run_in_pty("fixture_background_tty_read");
    assert_fixture_passed(&run, &["FIXTURE-OK background read stopped"]);
}

/// The control: the same read, left in the terminal's session as a
/// background group, stops. It shows the harness can reproduce the hazard.
#[test]
fn fixture_background_tty_read() {
    if !fixture_requested("fixture_background_tty_read") {
        return;
    }
    let mut background = silent("/bin/sh", "read x </dev/tty");
    background.process_group(0);
    default_sigttin(&mut background);
    let mut child = background.spawn().expect("spawn the background shell");
    let mut cleanup = KillGroupOnDrop(Some(child.id()));
    match wait_for_exit_or_stop(&child) {
        WaitStatus::Stopped(_, Signal::SIGTTIN) => {}
        other => panic!("expected the shell to stop on SIGTTIN, got {other:?}"),
    }
    kill_group(child.id());
    cleanup.0 = None;
    child.wait().expect("reap the stopped shell");
    println!("FIXTURE-OK background read stopped");
}

#[test]
fn a_detached_child_leads_its_own_session_and_a_group_kill_reaches_descendants() {
    let run = run_in_pty("fixture_detached_group_kill");
    assert_fixture_passed(&run, &["FIXTURE-OK group kill reached descendant"]);
}

fn read_descendant_pid(child: &mut Child) -> u32 {
    let stdout = child.stdout.take().expect("piped stdout");
    let mut line = String::new();
    BufReader::new(stdout.take(32))
        .read_line(&mut line)
        .expect("read the descendant pid");
    line.trim().parse().expect("a descendant pid")
}

#[test]
fn fixture_detached_group_kill() {
    if !fixture_requested("fixture_detached_group_kill") {
        return;
    }
    let launcher = proc_stat(std::process::id()).expect("own stat");
    let mut command = Command::new("/bin/sh");
    command
        .args(["-c", "sleep 30 & echo $!; wait"])
        .env_clear()
        .env("PATH", "/usr/bin:/bin")
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null());
    aether_spawn::detach_session(&mut command);
    let mut child = command.spawn().expect("spawn the detached shell");
    let leader = child.id();
    let mut cleanup = KillGroupOnDrop(Some(leader));
    let descendant = read_descendant_pid(&mut child);

    for pid in [leader, descendant] {
        let stat = proc_stat(pid).expect("process is alive");
        assert_eq!(stat.pgrp, leader, "pid {pid} is in the leader's group");
        assert_eq!(stat.session, leader, "pid {pid} is in the leader's session");
        assert_eq!(stat.tty_nr, 0, "pid {pid} has no controlling terminal");
    }
    assert_ne!(
        launcher.session, leader,
        "the child left the launcher's session"
    );

    kill_group(leader);
    cleanup.0 = None;
    child.wait().expect("reap the leader");
    wait_until("the descendant to die", || is_gone(descendant));
    println!("FIXTURE-OK group kill reached descendant");
}
