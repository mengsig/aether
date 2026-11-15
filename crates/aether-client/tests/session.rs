#![cfg(target_os = "linux")]
#![allow(clippy::expect_used, clippy::unwrap_used, clippy::panic)]

//! Daemon spawning under a real controlling terminal.
//! Fixtures rerun this executable on a fresh PTY with bounded waits.

use std::io::Read;
use std::path::Path;
use std::process::Command;
use std::sync::mpsc;
use std::time::{Duration, Instant};

use aether_client::StartingDaemon;
use rustix::process::{Pid, Signal, kill_process_group};

const FIXTURE: &str = "fixture_daemon_detached_from_terminal";
const FIXTURE_ENV: &str = "AETHER_SESSION_FIXTURE";
const FIXTURE_DEADLINE: Duration = Duration::from_secs(30);
const WAIT: Duration = Duration::from_secs(5);

/// The stand-in daemon records its pid, whether a nested shell could read
/// the terminal, and a descendant's pid, then waits for the group kill.
const DAEMON_SCRIPT: &str = r#"
echo $$ > pid.tmp && mv pid.tmp pid
/bin/sh -c 'read x </dev/tty' </dev/null 2>/dev/null
echo $? > status.tmp && mv status.tmp status
sleep 30 &
echo $! > descendant.tmp && mv descendant.tmp descendant
wait
"#;

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
                Err(error)
                    if error.raw_os_error() == Some(rustix::io::Errno::IO.raw_os_error()) =>
                {
                    return;
                }
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

fn group_of(pgid: u32) -> Pid {
    assert!(pgid > 1);
    assert_ne!(pgid, std::process::id());
    let raw = i32::try_from(pgid).expect("pgid fits i32");
    Pid::from_raw(raw).expect("a positive pgid")
}

fn kill_group(pgid: u32) {
    match kill_process_group(group_of(pgid), Signal::KILL) {
        Ok(()) => {}
        Err(rustix::io::Errno::SRCH) => {}
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

/// Poll until `read` yields a value; the daemon publishes files atomically.
fn wait_for_number(path: &Path) -> i64 {
    let began = Instant::now();
    loop {
        match std::fs::read_to_string(path) {
            Ok(text) => return text.trim().parse().expect("an atomically published number"),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => panic!("cannot read {}: {error}", path.display()),
        }
        assert!(
            began.elapsed() < WAIT,
            "timed out waiting for {}",
            path.display()
        );
        std::thread::sleep(Duration::from_millis(10));
    }
}

fn wait_until(what: &str, mut done: impl FnMut() -> bool) {
    let began = Instant::now();
    while !done() {
        assert!(began.elapsed() < WAIT, "timed out waiting for {what}");
        std::thread::sleep(Duration::from_millis(10));
    }
}

#[test]
fn a_started_daemon_has_no_terminal_and_a_group_kill_reaches_its_descendants() {
    let run = run_in_pty(FIXTURE);
    assert!(
        run.success,
        "fixture failed; transcript:\n{}",
        run.transcript
    );
    assert!(
        run.transcript.contains("FIXTURE-OK daemon detached"),
        "fixture did not finish; transcript:\n{}",
        run.transcript
    );
}

#[test]
fn fixture_daemon_detached_from_terminal() {
    if std::env::var(FIXTURE_ENV).as_deref() != Ok(FIXTURE) {
        return;
    }
    let launcher = proc_stat(std::process::id()).expect("own stat");
    assert_ne!(
        launcher.tty_nr, 0,
        "the fixture must have a controlling tty"
    );

    let dir = tempfile::tempdir().expect("tempdir");
    let socket = dir.path().join("aetherd.sock");
    let mut command = Command::new("/bin/sh");
    command
        .args(["-c", DAEMON_SCRIPT])
        .env_clear()
        .env("PATH", "/usr/bin:/bin")
        .current_dir(dir.path());
    let starting = StartingDaemon::spawn(command, &socket).expect("start the stand-in daemon");

    let leader = u32::try_from(wait_for_number(&dir.path().join("pid"))).expect("pid");
    let mut cleanup = KillGroupOnDrop(Some(leader));
    let status = wait_for_number(&dir.path().join("status"));
    assert_ne!(status, 0, "the daemon's shell must fail to read /dev/tty");
    let descendant = u32::try_from(wait_for_number(&dir.path().join("descendant"))).expect("pid");

    for pid in [leader, descendant] {
        let stat = proc_stat(pid).expect("process is alive");
        assert_eq!(stat.pgrp, leader, "pid {pid} is in the daemon's group");
        assert_eq!(stat.session, leader, "pid {pid} is in the daemon's session");
        assert_eq!(stat.tty_nr, 0, "pid {pid} has no controlling terminal");
    }
    assert_ne!(
        launcher.session, leader,
        "the daemon left the client's session"
    );

    kill_group(leader);
    cleanup.0 = None;
    wait_until("the daemon and descendant to die", || {
        is_gone(leader) && is_gone(descendant)
    });
    starting.detach();
    println!("FIXTURE-OK daemon detached");
}
