#![allow(clippy::expect_used, clippy::unwrap_used, clippy::panic)]

//! Auto-start: what a client learns when the daemon it started dies, and
//! what it still does when the daemon comes up.

use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::Duration;

use aether_client::{ClientError, ensure_daemon};
use aether_protocol::{ClientFrame, Command as Rpc, PROTOCOL_VERSION, Reply, ServerFrame, Welcome};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::{UnixListener, UnixStream};

/// A stub daemon: it answers the handshake on `socket` and nothing else,
/// which is all `ensure_daemon` asks of a daemon that has come up.
fn stub_daemon(socket: &Path) -> tokio::task::JoinHandle<()> {
    let listener = UnixListener::bind(socket).expect("bind the stub daemon");
    tokio::spawn(async move {
        while let Ok((stream, _)) = listener.accept().await {
            tokio::spawn(answer_handshake(stream));
        }
    })
}

async fn answer_handshake(stream: UnixStream) {
    let (read, mut write) = stream.into_split();
    let mut lines = BufReader::new(read).lines();
    while let Ok(Some(line)) = lines.next_line().await {
        let frame: ClientFrame = serde_json::from_str(&line).expect("a client frame");
        if !matches!(frame.command, Rpc::Hello(_)) {
            continue;
        }
        let reply = ServerFrame::Reply {
            id: frame.id,
            result: Ok(Reply::Welcome(Welcome {
                protocol_version: PROTOCOL_VERSION,
                daemon_version: "stub".into(),
                instance_id: "stub".into(),
                schema_version: 1,
                pid: std::process::id(),
                features: vec![],
                daemon_protocol_version: None,
                min_protocol_version: None,
                idle_timeout_secs: None,
                device: None,
                submission_horizon: None,
            })),
        };
        let mut encoded = serde_json::to_string(&reply).expect("encode");
        encoded.push('\n');
        if write.write_all(encoded.as_bytes()).await.is_err() {
            return;
        }
    }
}

fn shell(script: &str) -> Command {
    let mut command = Command::new("sh");
    command.arg("-c").arg(script);
    command
}

fn socket_in(dir: &tempfile::TempDir) -> PathBuf {
    dir.path().join("aetherd.sock")
}

/// Where `aether daemon` leaves the error that stopped it. The client reads
/// this path; `aether_protocol::paths::startup_log_path_for` writes it.
fn startup_log_in(dir: &tempfile::TempDir) -> PathBuf {
    dir.path().join("aetherd.startup.log")
}

/// A daemon that cannot start says why, and the client
/// hands that on instead of a timeout that names no cause.
#[tokio::test]
async fn a_daemon_that_dies_at_startup_reports_its_own_error() {
    let dir = tempfile::tempdir().expect("tempdir");
    let socket = socket_in(&dir);
    let store = dir.path().join("aether.db");

    let failure = ensure_daemon(
        &socket,
        shell(&format!(
            "printf 'Error: another aether daemon is already running for {}\\n' > {}; exit 3",
            store.display(),
            startup_log_in(&dir).display()
        )),
    )
    .await
    .expect_err("the daemon exited, so there is nothing to connect to");

    let reported = failure.to_string();
    assert!(
        reported.contains(&format!(
            "another aether daemon is already running for {}",
            store.display()
        )),
        "the daemon's own words must survive: {reported}"
    );
    assert!(
        matches!(failure, ClientError::DaemonStartup(_)) && reported.contains("exit status: 3"),
        "and its exit status with them: {reported}"
    );
}

/// A daemon that dies without writing leaves the client with only its status.
/// What an earlier start wrote is not its account: the client clears the log
/// before it spawns, or every later failure would quote a stale error.
#[tokio::test]
async fn a_daemon_that_dies_without_a_word_does_not_borrow_an_older_one() {
    let dir = tempfile::tempdir().expect("tempdir");
    let socket = socket_in(&dir);
    let log = startup_log_in(&dir);
    std::fs::write(&log, b"Error: what some earlier start said\n").expect("a stale log");

    let failure = ensure_daemon(&socket, shell("exit 1"))
        .await
        .expect_err("the daemon exited")
        .to_string();

    assert!(
        failure.contains(&format!("it wrote nothing to {}", log.display())),
        "{failure}"
    );
    assert!(!failure.contains("earlier start"), "{failure}");
    assert!(
        !log.exists(),
        "the stale log is cleared, not left to be read"
    );
}

/// The daemon a client starts exits whenever another is already coming up,
/// and that other one is what answers. Reporting the exit as the failure
/// would break every second client in a cold start.
#[tokio::test]
async fn a_client_whose_daemon_lost_the_race_waits_for_the_winner() {
    let dir = tempfile::tempdir().expect("tempdir");
    let socket = socket_in(&dir);
    let binding = socket.clone();
    tokio::spawn(async move {
        tokio::time::sleep(Duration::from_millis(500)).await;
        let _serving = stub_daemon(&binding);
        std::future::pending::<()>().await;
    });

    // Exits at once, exactly as a daemon that lost the instance lock does.
    let client = ensure_daemon(
        &socket,
        shell(&format!(
            "printf 'Error: another aether daemon is already running\\n' > {}; exit {}",
            startup_log_in(&dir).display(),
            aether_client::DAEMON_LOCK_BUSY_EXIT
        )),
    )
    .await
    .expect("the winner answers, so the loser's exit is not the failure");
    assert_eq!(
        client.welcome().map(|w| w.daemon_version.as_str()),
        Some("stub")
    );
}

/// The normal auto-start: the client waits for the daemon it started, and a
/// daemon that takes a moment to bind is not mistaken for a dead one.
#[tokio::test]
async fn a_daemon_that_binds_late_is_waited_for() {
    let dir = tempfile::tempdir().expect("tempdir");
    let socket = socket_in(&dir);
    let binding = socket.clone();
    tokio::spawn(async move {
        tokio::time::sleep(Duration::from_millis(250)).await;
        let _serving = stub_daemon(&binding);
        std::future::pending::<()>().await;
    });

    let client = ensure_daemon(&socket, shell("sleep 5"))
        .await
        .expect("the daemon came up while the client waited");
    assert_eq!(
        client.welcome().map(|w| w.daemon_version.as_str()),
        Some("stub")
    );
}

/// A daemon that already answers is connected to, and nothing is spawned.
#[tokio::test]
async fn a_daemon_that_answers_is_never_started_again() {
    let dir = tempfile::tempdir().expect("tempdir");
    let socket = socket_in(&dir);
    let _serving = stub_daemon(&socket);
    let marker = dir.path().join("spawned");

    let client = ensure_daemon(&socket, shell(&format!("touch {}", marker.display())))
        .await
        .expect("the running daemon answers");

    assert!(client.is_connected());
    assert!(
        !marker.exists(),
        "a daemon that answers is not started again"
    );
}
