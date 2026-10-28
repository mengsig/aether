#![allow(clippy::expect_used)]
use aether_protocol::{
    ClientFrame, Command as WireCommand, ErrorCode, ErrorReply, Reply, ServerFrame, Welcome,
};
use std::{
    path::PathBuf,
    process::{Command, Output},
};
use tokio::io::{AsyncBufReadExt as _, AsyncWriteExt as _, BufReader};

struct Fixture {
    directory: tempfile::TempDir,
    socket: PathBuf,
}

impl Fixture {
    fn new() -> Self {
        std::fs::create_dir_all("../../target/ssh-tests").expect("test directory");
        let directory = tempfile::tempdir_in("../../target/ssh-tests").expect("fixture");
        let socket = PathBuf::from("../../target/ssh-tests")
            .join(directory.path().file_name().expect("fixture name"))
            .join("daemon.sock");
        let script = directory.path().join("ssh");
        std::os::unix::fs::symlink(
            std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/ssh"),
            script,
        )
        .expect("fake SSH on PATH");
        Self { directory, socket }
    }
    fn cli(&self, args: &[&str]) -> Command {
        let mut command = Command::new(env!("CARGO_BIN_EXE_aether"));
        command
            .args(args)
            .env("AETHER_CONFIG_DIR", self.directory.path().join("config"))
            .env("AETHER_DATA_DIR", self.directory.path().join("data"))
            .env(
                "PATH",
                format!(
                    "{}:{}",
                    self.directory.path().display(),
                    std::env::var("PATH").expect("PATH")
                ),
            )
            .env_remove("AETHER_REMOTE")
            .env_remove("AETHER_SOCKET");
        command
    }
    fn add(&self, target: &str) -> Output {
        self.cli(&[
            "remote",
            "add",
            "box",
            "--ssh",
            target,
            "--executable",
            env!("CARGO_BIN_EXE_aether"),
            "--remote-socket",
            self.socket.to_str().expect("socket"),
            "--json",
        ])
        .output()
        .expect("add")
    }
    fn write_profile(&self, target: &str) {
        let config = self.directory.path().join("config");
        std::fs::create_dir_all(&config).expect("config");
        let profile = format!(
            "[remotes.box]\nssh = {target:?}\nexecutable = {:?}\nsocket = {:?}\n",
            env!("CARGO_BIN_EXE_aether"),
            self.socket
        );
        std::fs::write(config.join("remotes.toml"), profile).expect("profile");
    }
}

#[path = "ssh_remotes/lost_acknowledgement.rs"]
mod lost_acknowledgement;

#[path = "ssh_remotes/spawn_schedules.rs"]
mod spawn_schedules;

/// The receipt horizon the fake host greets protocol 18 clients with.
const HORIZON: &str = "fixture-epoch:0";

type HostLines = tokio::io::Lines<BufReader<tokio::net::unix::OwnedReadHalf>>;

async fn next_frame(lines: &mut HostLines) -> ClientFrame {
    let line = lines.next_line().await.expect("line").expect("frame");
    let frame: ClientFrame = serde_json::from_str(&line).expect("valid protocol");
    assert!(frame.id > 0);
    assert!(!frame.command.name().is_empty());
    frame
}

async fn send_reply(
    write: &mut tokio::net::unix::OwnedWriteHalf,
    id: u64,
    result: Result<Reply, ErrorReply>,
) {
    assert!(id > 0);
    let bytes = aether_protocol::encode(&ServerFrame::Reply { id, result }).expect("reply");
    assert!(bytes.ends_with('\n'));
    write
        .write_all(bytes.as_bytes())
        .await
        .expect("write reply");
}

fn welcome(version: u32) -> Reply {
    Reply::Welcome(Welcome {
        protocol_version: version,
        daemon_version: "fixture".into(),
        instance_id: "fixture-store".into(),
        schema_version: 1,
        pid: std::process::id(),
        features: vec![],
        daemon_protocol_version: Some(version),
        min_protocol_version: Some(version - 1),
        idle_timeout_secs: None,
        device: None,
        submission_horizon: (version >= 18).then(|| HORIZON.to_owned()),
    })
}

async fn hello(listener: &tokio::net::UnixListener, version: u32) {
    let (stream, _) = listener.accept().await.expect("connection");
    let (read, mut write) = stream.into_split();
    let mut lines = BufReader::new(read).lines();
    let line = lines.next_line().await.expect("hello line").expect("hello");
    let frame: ClientFrame = serde_json::from_str(&line).expect("frame");
    assert!(
        matches!(frame.command, WireCommand::Hello(_)),
        "bridge injected no handshake"
    );
    let reply = ServerFrame::Reply {
        id: frame.id,
        result: Ok(welcome(version)),
    };
    write
        .write_all(aether_protocol::encode(&reply).expect("reply").as_bytes())
        .await
        .expect("welcome");
    while lines.next_line().await.expect("EOF").is_some() {}
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn fake_ssh_add_verifies_bridge_and_profile_management() {
    let fixture = Fixture::new();
    let listener = tokio::net::UnixListener::bind(&fixture.socket).expect("listener");
    let host =
        tokio::spawn(async move { hello(&listener, aether_protocol::PROTOCOL_VERSION).await });
    let add = fixture.add("fake-host");
    assert!(
        add.status.success(),
        "{}",
        String::from_utf8_lossy(&add.stderr)
    );
    assert!(String::from_utf8_lossy(&add.stdout).contains("fixture-store"));
    host.await.expect("host");
    let list = fixture
        .cli(&["remote", "list", "--json"])
        .output()
        .expect("list");
    assert!(list.status.success());
    assert!(String::from_utf8_lossy(&list.stdout).contains("fake-host"));
    let remove = fixture
        .cli(&["remote", "remove", "box"])
        .output()
        .expect("remove");
    assert!(remove.status.success());
    assert!(!fixture.directory.path().join("data/aetherd.sock").exists());
}

#[test]
fn auth_failure_and_missing_socket_do_not_autostart_or_save_a_profile() {
    let fixture = Fixture::new();
    let auth = fixture.add("auth-failure");
    assert!(!auth.status.success());
    assert!(String::from_utf8_lossy(&auth.stderr).contains("Permission denied (publickey)"));
    assert!(
        !fixture
            .directory
            .path()
            .join("config/remotes.toml")
            .exists()
    );
    fixture.write_profile("fake-host");
    for args in [
        vec!["--remote", "box", "status"],
        vec!["--remote", "box", "--plain"],
        vec!["--remote", "box", "run", "task"],
    ] {
        let result = fixture.cli(&args).output().expect("CLI");
        assert!(!result.status.success());
        assert!(!fixture.directory.path().join("data/aetherd.sock").exists());
    }
    let bridge = fixture
        .cli(&[
            "--socket",
            fixture.socket.to_str().expect("socket"),
            "bridge",
            "--stdio",
        ])
        .output()
        .expect("bridge");
    assert!(!bridge.status.success());
    assert!(bridge.stdout.is_empty());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn incompatible_versions_report_both_sides_without_remote_restart_advice() {
    let fixture = Fixture::new();
    let listener = tokio::net::UnixListener::bind(&fixture.socket).expect("listener");
    let host = tokio::spawn(async move {
        // One refusal per version the client can speak.
        for _ in aether_protocol::compat::MIN_PROTOCOL_VERSION..=aether_protocol::PROTOCOL_VERSION {
            let (stream, _) = listener.accept().await.expect("connection");
            let (read, mut write) = stream.into_split();
            let line = BufReader::new(read)
                .lines()
                .next_line()
                .await
                .expect("line")
                .expect("hello");
            let frame: ClientFrame = serde_json::from_str(&line).expect("frame");
            let reply = ServerFrame::Reply {
                id: frame.id,
                result: Err(ErrorReply::new(
                    ErrorCode::Incompatible,
                    "daemon serves protocols 15..=16",
                )),
            };
            write
                .write_all(aether_protocol::encode(&reply).expect("reply").as_bytes())
                .await
                .expect("refusal");
        }
    });
    let result = fixture.add("fake-host");
    assert!(!result.status.success());
    let error = String::from_utf8_lossy(&result.stderr);
    assert!(
        error.contains("15..=16")
            && error.contains("protocol 18 also refused")
            && error.contains("protocol 17 also refused"),
        "{error}"
    );
    assert!(!error.contains(" stop"), "{error}");
    host.await.expect("host");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn remote_run_validates_host_workspace_and_creates_with_canonical_scope() {
    let fixture = Fixture::new();
    fixture.write_profile("fake-host");
    let listener = tokio::net::UnixListener::bind(&fixture.socket).expect("listener");
    let host = tokio::spawn(verify_run_workspace(listener));
    let run = fixture
        .cli(&[
            "--remote",
            "box",
            "run",
            "--workspace",
            "/host/project",
            "--model",
            "mock/m",
            "--network=false",
            "task",
        ])
        .output()
        .expect("run");
    assert!(!run.status.success());
    assert!(String::from_utf8_lossy(&run.stderr).contains("fixture verified host scope"));
    host.await.expect("host");
}

async fn verify_run_workspace(listener: tokio::net::UnixListener) {
    use aether_protocol::remote::WorkspaceValidation;
    let (stream, _) = listener.accept().await.expect("connect");
    let (read, mut write) = stream.into_split();
    let mut lines = BufReader::new(read).lines();
    while let Some(line) = lines.next_line().await.expect("line") {
        let frame: ClientFrame = serde_json::from_str(&line).expect("frame");
        let result = match frame.command {
            WireCommand::Hello(_) => Ok(welcome(aether_protocol::PROTOCOL_VERSION)),
            WireCommand::ValidateWorkspace { path } => {
                assert_eq!(path, "/host/project");
                Ok(Reply::WorkspaceValidation(WorkspaceValidation {
                    valid: true,
                    path: Some("/canonical/host/project".into()),
                    reason: None,
                    git: None,
                }))
            }
            WireCommand::Subscribe { .. } => Ok(Reply::Subscribed {
                id: 1,
                last_seq: 10,
            }),
            WireCommand::CreateAgent(agent) => {
                assert_eq!(
                    agent.scope.expect("host scope").workspace_root,
                    Some("/canonical/host/project".into())
                );
                assert!(agent.idempotency_key.is_some());
                Err(ErrorReply::new(
                    ErrorCode::Unavailable,
                    "fixture verified host scope",
                ))
            }
            command => unreachable!("unexpected command: {command:?}"),
        };
        let done = result.is_err();
        send_reply(&mut write, frame.id, result).await;
        if done {
            return;
        }
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn previous_protocol_is_negotiated_on_a_fresh_carrier() {
    let fixture = Fixture::new();
    let listener = tokio::net::UnixListener::bind(&fixture.socket).expect("listener");
    let host = tokio::spawn(async move {
        // Each newer version is refused on a carrier of its own, newest first.
        for version in (18..=aether_protocol::PROTOCOL_VERSION).rev() {
            let (stream, _) = listener.accept().await.expect("a fresh carrier");
            let (read, mut write) = stream.into_split();
            let mut lines = BufReader::new(read).lines();
            let frame = next_frame(&mut lines).await;
            assert!(
                matches!(frame.command, WireCommand::Hello(ref hello) if hello.protocol_version == version)
            );
            send_reply(
                &mut write,
                frame.id,
                Err(ErrorReply::new(
                    ErrorCode::Incompatible,
                    "daemon serves protocols 16..=17",
                )),
            )
            .await;
        }
        hello(&listener, 17).await;
    });
    let add = fixture.add("fake-host");
    assert!(
        add.status.success(),
        "{}",
        String::from_utf8_lossy(&add.stderr)
    );
    assert!(String::from_utf8_lossy(&add.stdout).contains("17"));
    host.await.expect("host");
}
