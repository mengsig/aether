//! Opt-in checks of the loopback sandbox with real bwrap, socat and seccomp but a fake
//! exec-server: every exit path ends the namespace, and no socket reaches the host.
//! `cargo test -p aether-engine --lib codex_app::tests::sandbox -- --ignored`

use std::path::{Path, PathBuf};
use std::time::Duration;

use super::super::loopback;
use crate::EngineError;

const READY: &str = "#!/bin/sh\necho ws://fake\nexec sleep 600\n";
const FAILING: &str = "#!/bin/sh\necho fixture-exec-server-unavailable >&2\nexit 44\n";

/// A unique cwd, which every sandbox process inherits, and a fake codex in it.
fn fixture(script: &str) -> (tempfile::TempDir, PathBuf, String) {
    use std::os::unix::fs::PermissionsExt as _;
    let dir = tempfile::tempdir().unwrap();
    let cwd = std::fs::canonicalize(dir.path()).unwrap();
    let program = cwd.join("codex");
    std::fs::write(&program, script).unwrap();
    std::fs::set_permissions(&program, std::fs::Permissions::from_mode(0o755)).unwrap();
    let program = program.to_str().unwrap().to_owned();
    (dir, cwd, program)
}

/// Processes whose cwd is `cwd`, across mount namespaces.
fn processes_in(cwd: &Path) -> Vec<u32> {
    std::fs::read_dir("/proc")
        .unwrap()
        .filter_map(|entry| entry.ok()?.file_name().to_str()?.parse::<u32>().ok())
        .filter(|pid| std::fs::read_link(format!("/proc/{pid}/cwd")).is_ok_and(|c| c == cwd))
        .collect()
}

/// Wait up to five seconds for every process in `cwd` to end.
async fn assert_all_gone(cwd: &Path) {
    for _ in 0..50 {
        if processes_in(cwd).is_empty() {
            return;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    panic!("sandbox processes survived: {:?}", processes_in(cwd));
}

#[tokio::test]
#[ignore = "needs bwrap and socat on PATH"]
async fn exec_server_exit_fails_closed_promptly_and_leaves_nothing() {
    let (_dir, cwd, program) = fixture(FAILING);
    let started = tokio::time::Instant::now();
    let result = tokio::time::timeout(
        Duration::from_secs(15),
        loopback::start(&program, &cwd, &[], None),
    )
    .await
    .expect("a failed exec-server must not hang startup");
    assert!(started.elapsed() < Duration::from_secs(10));
    let Err(EngineError::Unsupported(reason)) = result else {
        panic!("expected Unsupported");
    };
    assert!(
        reason.contains("fixture-exec-server-unavailable"),
        "{reason}"
    );
    assert_all_gone(&cwd).await;
}

#[tokio::test]
#[ignore = "needs bwrap and socat on PATH"]
async fn dropping_the_sandbox_ends_the_namespace() {
    let (_dir, cwd, program) = fixture(READY);
    let sandbox = loopback::start(&program, &cwd, &[], None)
        .await
        .unwrap_or_else(|e| panic!("sandbox did not start: {e}"));
    assert!(processes_in(&cwd).len() >= 4, "{:?}", processes_in(&cwd));
    drop(sandbox);
    assert_all_gone(&cwd).await;
}

/// The exec-server, and so every command it runs, gets the agent temp dir
/// and can write to it inside the namespace.
#[tokio::test]
#[ignore = "needs bwrap and socat on PATH"]
async fn the_exec_server_writes_to_the_agent_temp_dir() {
    const PROBE: &str = "#!/bin/sh\nprintf '%s|%s|%s' \"$TMPDIR\" \"$TMP\" \"$TEMP\" > \"$TMPDIR/seen\"\necho ws://fake\nexec sleep 600\n";
    let (_dir, cwd, program) = fixture(PROBE);
    let leaf = cwd.join("0123456789AB");
    std::fs::create_dir(&leaf).unwrap();
    let sandbox = loopback::start(&program, &cwd, &[], Some(&leaf))
        .await
        .unwrap_or_else(|e| panic!("sandbox did not start: {e}"));
    let seen = std::fs::read_to_string(leaf.join("seen")).expect("written inside the sandbox");
    assert_eq!(seen, format!("{0}|{0}|{0}", leaf.display()));
    drop(sandbox);
    assert_all_gone(&cwd).await;
}

/// A dead exec-server must end the namespace, so the driver can fail the turn.
#[tokio::test]
#[ignore = "needs bwrap and socat on PATH"]
async fn exec_server_death_mid_turn_is_reported() {
    let (_dir, cwd, program) = fixture(READY);
    let mut sandbox = loopback::start(&program, &cwd, &[], None)
        .await
        .unwrap_or_else(|e| panic!("sandbox did not start: {e}"));
    let server = processes_in(&cwd)
        .into_iter()
        .find(|pid| {
            std::fs::read_to_string(format!("/proc/{pid}/comm")).is_ok_and(|c| c.trim() == "sleep")
        })
        .expect("the fake exec-server must be running");
    nix::sys::signal::kill(
        nix::unistd::Pid::from_raw(server as i32),
        nix::sys::signal::Signal::SIGKILL,
    )
    .unwrap();
    let status = tokio::time::timeout(Duration::from_secs(5), sandbox.exited())
        .await
        .expect("the sandbox must report the exec-server's death");
    assert!(!status.is_empty());
    assert_all_gone(&cwd).await;
}

/// Parent death skips Drop; the closed stdin pipe alone must end the namespace.
#[tokio::test]
#[ignore = "needs bwrap and socat on PATH"]
async fn parent_exit_ends_the_namespace() {
    let (_dir, cwd, program) = fixture(READY);
    // Without Drop the socket directory outlives the fixture, so keep it in ours.
    let tmp = cwd.join("tmp");
    std::fs::create_dir(&tmp).unwrap();
    let output = tokio::process::Command::new(std::env::current_exe().unwrap())
        .args([
            "--exact",
            "codex_app::tests::sandbox::parent_exit_fixture",
            "--ignored",
            "--nocapture",
        ])
        .env("AETHER_LOOPBACK_FIXTURE_CWD", &cwd)
        .env("AETHER_LOOPBACK_FIXTURE_PROGRAM", &program)
        .env("TMPDIR", &tmp)
        .output()
        .await
        .unwrap();
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        stdout.contains("SANDBOX STARTED"),
        "fixture failed:\n{stdout}\n{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert_all_gone(&cwd).await;
}

#[tokio::test]
#[ignore = "subprocess fixture, launched by parent_exit_ends_the_namespace"]
async fn parent_exit_fixture() {
    // Only meaningful when launched by parent_exit_ends_the_namespace.
    let Some(cwd) = std::env::var_os("AETHER_LOOPBACK_FIXTURE_CWD") else {
        return;
    };
    let cwd = PathBuf::from(cwd);
    let program = std::env::var("AETHER_LOOPBACK_FIXTURE_PROGRAM").unwrap();
    let _sandbox = loopback::start(&program, &cwd, &[], None)
        .await
        .unwrap_or_else(|e| panic!("sandbox did not start: {e}"));
    println!("SANDBOX STARTED");
    // Exits without running Drop, like a crashed or killed daemon.
    std::process::exit(0);
}

/// Host listeners a proxy could use; the sandboxed probe must reach none of them.
#[tokio::test]
#[ignore = "needs bwrap and socat on PATH"]
async fn seccomp_keeps_sockets_inside_the_namespace() {
    use std::os::unix::net::{UnixDatagram, UnixListener};
    let exe = std::env::current_exe().unwrap();
    let exe = exe.to_str().unwrap();
    let dir = tempfile::tempdir().unwrap();
    let cwd = std::fs::canonicalize(dir.path()).unwrap();
    let host_dgram = UnixDatagram::bind(cwd.join("host-dgram.sock")).unwrap();
    host_dgram.set_nonblocking(true).unwrap();
    let host_stream = UnixListener::bind(cwd.join("host-stream.sock")).unwrap();
    host_stream.set_nonblocking(true).unwrap();
    let host_seqpacket = seqpacket_listener(&cwd.join("host-seqpacket.sock"));
    let report = cwd.join("report.txt");
    assert!(!format!("{exe}{}", cwd.display()).contains('\''));
    // The probe replaces the exec-server, so the sandbox then fails to start.
    let (_probe_dir, _, program) = fixture(&format!(
        "#!/bin/sh\nAETHER_LOOPBACK_PROBE_DIR='{}' exec '{exe}' --exact \
         codex_app::tests::sandbox::socket_probe_fixture --ignored --nocapture\n",
        cwd.display()
    ));
    let result = loopback::start(&program, &cwd, &[], None).await;
    assert!(matches!(result, Err(EngineError::Unsupported(_))));
    let report = std::fs::read_to_string(&report)
        .unwrap_or_else(|e| panic!("the probe must write its report: {e}"));
    println!("sandboxed socket probe:\n{report}");
    let outcome = |probe: &str| {
        report
            .lines()
            .find_map(|line| line.strip_prefix(&format!("{probe}: ")))
            .unwrap_or_else(|| panic!("no {probe} in:\n{report}"))
    };
    for probe in [
        "unix stream socket",
        "unix datagram socket",
        "vsock socket",
        "unix datagram pair",
        "unix raw pair",
    ] {
        assert_eq!(outcome(probe), "EPERM", "{probe}:\n{report}");
    }
    for probe in ["stream pair ipc", "seqpacket pair ipc", "loopback tcp"] {
        assert_eq!(outcome(probe), "ok", "{probe}:\n{report}");
    }
    for probe in [
        "stream pair connect to host",
        "stream pair sendto host",
        "seqpacket pair connect to host",
        "egress",
    ] {
        assert_ne!(outcome(probe), "ok", "{probe}:\n{report}");
    }
    let mut buffer = [0; 64];
    let received = host_dgram.recv(&mut buffer);
    assert!(
        received
            .as_ref()
            .is_err_and(|e| e.kind() == std::io::ErrorKind::WouldBlock),
        "the host datagram socket received {received:?}:\n{report}"
    );
    let accepted = host_stream.accept();
    assert!(
        accepted
            .as_ref()
            .is_err_and(|e| e.kind() == std::io::ErrorKind::WouldBlock),
        "the host stream socket accepted {accepted:?}:\n{report}"
    );
    let accepted = nix::sys::socket::accept4(
        std::os::fd::AsRawFd::as_raw_fd(&host_seqpacket),
        nix::sys::socket::SockFlag::SOCK_CLOEXEC,
    );
    assert_eq!(
        accepted,
        Err(nix::errno::Errno::EAGAIN),
        "the host seqpacket socket accepted:\n{report}"
    );
}

const BROWSER_MARKER: &str = "aether-loopback-page";

/// A headless browser drives a page served on loopback, and an outside page renders empty.
#[tokio::test]
#[ignore = "needs bwrap, socat and AETHER_TEST_HEADLESS_BROWSER or Playwright's chrome-headless-shell"]
async fn headless_browser_renders_only_loopback_pages() {
    let browser = headless_browser();
    let exe = std::env::current_exe().unwrap();
    let dir = tempfile::tempdir().unwrap();
    let cwd = std::fs::canonicalize(dir.path()).unwrap();
    let quoted = format!("{}{}{}", exe.display(), cwd.display(), browser.display());
    assert!(!quoted.contains('\''), "{quoted}");
    // The probe replaces the exec-server, so the sandbox then fails to start.
    let (_probe_dir, _, program) = fixture(&format!(
        "#!/bin/sh\nAETHER_LOOPBACK_BROWSER_DIR='{}' AETHER_LOOPBACK_BROWSER='{}' exec '{}' \
         --exact codex_app::tests::sandbox::browser_probe_fixture --ignored --nocapture\n",
        cwd.display(),
        browser.display(),
        exe.display()
    ));
    let result = loopback::start(&program, &cwd, &[], None).await;
    assert!(matches!(result, Err(EngineError::Unsupported(_))));
    let read = |name: &str| {
        std::fs::read_to_string(cwd.join(name))
            .unwrap_or_else(|e| panic!("the browser probe must write {name}: {e}"))
    };
    let report = read("browser-report.txt");
    println!("sandboxed browser probe:\n{report}");
    assert!(read("loopback.html").contains(BROWSER_MARKER), "{report}");
    assert!(read("egress.html").contains("<body></body>"), "{report}");
}

/// `AETHER_TEST_HEADLESS_BROWSER`, else the newest `chrome-headless-shell` that
/// Playwright installed, which it launches for headless Chromium by default.
fn headless_browser() -> PathBuf {
    if let Some(browser) = std::env::var_os("AETHER_TEST_HEADLESS_BROWSER") {
        return PathBuf::from(browser);
    }
    let home = PathBuf::from(std::env::var_os("HOME").expect("HOME"));
    let cache = home.join(".cache/ms-playwright");
    let mut shells: Vec<PathBuf> = std::fs::read_dir(&cache)
        .into_iter()
        .flatten()
        .filter_map(|entry| entry.ok().map(|entry| entry.path()))
        .filter(|path| {
            path.file_name()
                .and_then(|name| name.to_str())
                .is_some_and(|name| name.starts_with("chromium_headless_shell-"))
        })
        .flat_map(|install| std::fs::read_dir(install).into_iter().flatten())
        .filter_map(|entry| Some(entry.ok()?.path().join("chrome-headless-shell")))
        .filter(|path| path.is_file())
        .collect();
    shells.sort();
    shells.pop().unwrap_or_else(|| {
        panic!(
            "no headless browser: set AETHER_TEST_HEADLESS_BROWSER or run \
             `npx playwright install chromium-headless-shell` ({} has none)",
            cache.display()
        )
    })
}

#[test]
#[ignore = "subprocess fixture, launched by headless_browser_renders_only_loopback_pages"]
fn browser_probe_fixture() {
    use std::io::{BufRead as _, Write as _};
    // Only meaningful inside the sandbox started by headless_browser_renders_only_loopback_pages.
    let Some(dir) = std::env::var_os("AETHER_LOOPBACK_BROWSER_DIR") else {
        return;
    };
    let dir = PathBuf::from(dir);
    let browser = std::env::var_os("AETHER_LOOPBACK_BROWSER").unwrap();
    let server = std::net::TcpListener::bind(("127.0.0.1", 0)).unwrap();
    let url = format!("http://{}/", server.local_addr().unwrap());
    std::thread::spawn(move || {
        for stream in server.incoming().flatten() {
            let mut reader = std::io::BufReader::new(&stream);
            let mut line = String::new();
            while reader.read_line(&mut line).is_ok_and(|n| n > 0) && line != "\r\n" {
                line.clear();
            }
            let body = format!("<html><body><p>{BROWSER_MARKER}</p></body></html>");
            let _ = write!(
                &stream,
                "HTTP/1.1 200 OK\r\nContent-Type: text/html\r\nContent-Length: {}\r\n\
                 Connection: close\r\n\r\n{body}",
                body.len()
            );
        }
    });
    let mut report = Vec::new();
    for (name, url) in [("loopback", url.as_str()), ("egress", "http://1.1.1.1/")] {
        let output = std::process::Command::new(&browser)
            .args([
                "--headless",
                "--no-sandbox",
                "--disable-gpu",
                "--no-first-run",
            ])
            .arg(format!("--user-data-dir={}", dir.join(name).display()))
            .args(["--dump-dom", url])
            .output()
            .unwrap();
        std::fs::write(dir.join(format!("{name}.html")), &output.stdout).unwrap();
        let stderr = String::from_utf8_lossy(&output.stderr);
        let tail: Vec<_> = stderr.lines().rev().take(5).collect();
        report.push(format!("{name}: {} {tail:?}", output.status));
    }
    std::fs::write(dir.join("browser-report.txt"), report.join("\n")).unwrap();
}

/// A non-blocking host seqpacket listener; std has none.
fn seqpacket_listener(path: &Path) -> std::os::fd::OwnedFd {
    use nix::sys::socket::{AddressFamily, Backlog, SockFlag, SockType, UnixAddr};
    let listener = nix::sys::socket::socket(
        AddressFamily::Unix,
        SockType::SeqPacket,
        SockFlag::SOCK_NONBLOCK | SockFlag::SOCK_CLOEXEC,
        None,
    )
    .unwrap();
    let address = UnixAddr::new(path).unwrap();
    nix::sys::socket::bind(std::os::fd::AsRawFd::as_raw_fd(&listener), &address).unwrap();
    nix::sys::socket::listen(&listener, Backlog::new(8).unwrap()).unwrap();
    listener
}

#[test]
#[ignore = "subprocess fixture, launched by seccomp_keeps_sockets_inside_the_namespace"]
fn socket_probe_fixture() {
    use nix::sys::socket::{
        AddressFamily, MsgFlags, SockFlag, SockType, UnixAddr, connect, sendto, socket, socketpair,
    };
    use std::io::{Read as _, Write as _};
    use std::os::fd::AsRawFd as _;
    // Only meaningful inside the sandbox started by seccomp_keeps_sockets_inside_the_namespace.
    let Some(dir) = std::env::var_os("AETHER_LOOPBACK_PROBE_DIR") else {
        return;
    };
    let dir = PathBuf::from(dir);
    let host_dgram = UnixAddr::new(&dir.join("host-dgram.sock")).unwrap();
    let host_stream = UnixAddr::new(&dir.join("host-stream.sock")).unwrap();
    let host_seqpacket = UnixAddr::new(&dir.join("host-seqpacket.sock")).unwrap();
    let errno = |e: std::io::Error| nix::errno::Errno::from_raw(e.raw_os_error().unwrap_or(0));
    let pair = |kind| socketpair(AddressFamily::Unix, kind, None, SockFlag::empty());
    let mut report = Vec::new();
    let mut record = |probe: &str, result: Result<(), nix::errno::Errno>| {
        let outcome = result.map_or_else(|e| format!("{e:?}"), |()| "ok".into());
        report.push(format!("{probe}: {outcome}"));
    };

    for (probe, family, kind) in [
        ("unix stream socket", AddressFamily::Unix, SockType::Stream),
        (
            "unix datagram socket",
            AddressFamily::Unix,
            SockType::Datagram,
        ),
        ("vsock socket", AddressFamily::Vsock, SockType::Stream),
    ] {
        record(
            probe,
            socket(family, kind, SockFlag::empty(), None).map(drop),
        );
    }
    // The bypass the filter closes: a datagram pair reconnected to a host socket.
    record(
        "unix datagram pair",
        std::os::unix::net::UnixDatagram::pair()
            .map_err(errno)
            .and_then(|(a, _)| {
                connect(a.as_raw_fd(), &host_dgram)?;
                a.send(b"proxy").map(drop).map_err(errno)
            }),
    );
    record("unix raw pair", pair(SockType::Raw).map(drop));
    record(
        "stream pair ipc",
        std::os::unix::net::UnixStream::pair()
            .and_then(|(mut a, mut b)| {
                a.write_all(b"x")?;
                let mut byte = [0];
                b.read_exact(&mut byte)
            })
            .map_err(errno),
    );
    let (stream, _peer) = pair(SockType::Stream).unwrap();
    record(
        "stream pair connect to host",
        connect(stream.as_raw_fd(), &host_stream),
    );
    record(
        "stream pair sendto host",
        sendto(stream.as_raw_fd(), b"proxy", &host_dgram, MsgFlags::empty()).map(drop),
    );
    let (seqpacket, peer) = pair(SockType::SeqPacket).unwrap();
    record(
        "seqpacket pair ipc",
        nix::sys::socket::send(seqpacket.as_raw_fd(), b"x", MsgFlags::empty())
            .and_then(|_| nix::sys::socket::recv(peer.as_raw_fd(), &mut [0], MsgFlags::empty()))
            .map(drop),
    );
    record(
        "seqpacket pair connect to host",
        connect(seqpacket.as_raw_fd(), &host_seqpacket),
    );
    // A seqpacket socket sends to its peer whatever address it is given.
    let sent = sendto(
        seqpacket.as_raw_fd(),
        b"proxy",
        &host_dgram,
        MsgFlags::empty(),
    );
    record("seqpacket pair sendto host", sent.map(drop));
    record(
        "loopback tcp",
        std::net::TcpListener::bind(("127.0.0.1", 0))
            .and_then(|listener| {
                let address = listener.local_addr()?;
                let _client = std::net::TcpStream::connect(address)?;
                listener.accept().map(drop)
            })
            .map_err(errno),
    );
    record(
        "egress",
        std::net::TcpStream::connect_timeout(
            &std::net::SocketAddr::from(([1, 1, 1, 1], 443)),
            Duration::from_secs(3),
        )
        .map(drop)
        .map_err(errno),
    );
    std::fs::write(dir.join("report.txt"), report.join("\n")).unwrap();
}
