//! A spawn schedule made through a remote runs on the host, so its workspace
//! is a host path, chosen and validated the way remote `run` does it.
use super::{Fixture, HostLines, next_frame, send_reply, welcome};
use aether_core::schedule::ScheduleTarget;
use aether_protocol::remote::WorkspaceValidation;
use aether_protocol::{Command as WireCommand, ErrorCode, ErrorReply, Reply};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use tokio::io::{AsyncBufReadExt as _, BufReader};

const CREATE: &[&str] = &[
    "--remote",
    "box",
    "schedule",
    "create",
    "tick",
    "--every",
    "5m",
    "--model",
    "mock/m",
    "--network=false",
    "--objective",
    "check the build",
];

/// Answers one connection; the client's request for a schedule ends it.
async fn host(listener: tokio::net::UnixListener, connected: Arc<AtomicBool>) {
    let (stream, _) = listener.accept().await.expect("connect");
    connected.store(true, Ordering::SeqCst);
    let (read, mut write) = stream.into_split();
    let mut lines: HostLines = BufReader::new(read).lines();
    loop {
        let frame = next_frame(&mut lines).await;
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
            WireCommand::CreateSchedule { spec, .. } => {
                let ScheduleTarget::Spawn(template) = spec.target else {
                    unreachable!("the client sent a delivery schedule");
                };
                assert_eq!(
                    template.scope.expect("host scope").workspace_root,
                    Some("/canonical/host/project".into())
                );
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
async fn remote_spawn_schedule_without_workspace_fails_before_connecting() {
    let fixture = Fixture::new();
    fixture.write_profile("fake-host");
    let listener = tokio::net::UnixListener::bind(&fixture.socket).expect("listener");
    let connected = Arc::new(AtomicBool::new(false));
    let host = tokio::spawn(host(listener, connected.clone()));
    let create = fixture.cli(CREATE).output().expect("create");
    let error = String::from_utf8_lossy(&create.stderr);
    assert!(!create.status.success());
    assert!(
        error.contains("requires --workspace /path/on/host"),
        "{error}"
    );
    assert!(!connected.load(Ordering::SeqCst), "the host was contacted");
    host.abort();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn remote_spawn_schedule_validates_host_workspace_and_persists_it() {
    let fixture = Fixture::new();
    fixture.write_profile("fake-host");
    let listener = tokio::net::UnixListener::bind(&fixture.socket).expect("listener");
    let host = tokio::spawn(host(listener, Arc::default()));
    let mut args = CREATE.to_vec();
    args.extend(["--workspace", "/host/project"]);
    let create = fixture.cli(&args).output().expect("create");
    let error = String::from_utf8_lossy(&create.stderr);
    assert!(!create.status.success());
    assert!(error.contains("fixture verified host scope"), "{error}");
    host.await.expect("host");
}
