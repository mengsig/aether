#![allow(clippy::expect_used, clippy::unwrap_used, clippy::panic)]

//! The container execution backend against real Docker.
//!
//! Its own lane: every test here is `#[ignore]`d, so `cargo test` never runs
//! them and never reports them green either. They ran as no-ops before, on
//! any machine without the image cached - three passing test names having
//! asserted nothing, while the row they are the only evidence for said
//! "done".
//!
//! Run the lane deliberately, with a local image named:
//!
//! ```text
//! AETHER_TEST_CONTAINER_IMAGE=python:3.12-slim cargo test -p aether-tools \
//!     --test container_backend -- --ignored
//! ```
//!
//! In the lane a missing variable, a missing Docker or a missing image is a
//! failure, not a skip: the point is behavioural evidence, and the command
//! Aether builds is already covered deterministically by the unit tests in
//! `src/exec.rs`. The image needs bash.

use std::collections::BTreeMap;
use std::sync::Arc;

use aether_core::agent::{ExecutionBackend, NetworkPolicy, Scope};
use aether_core::artifact::{Artifact, ArtifactKind};
use aether_core::id::{AgentId, TaskId};
use aether_core::tool::ToolOutcome;
use aether_tools::background::BackgroundProcesses;
use aether_tools::builtin::bash::BashTool;
use aether_tools::{ArtifactSink, Tool, ToolContext, ToolError, ToolExecution};
use async_trait::async_trait;
use serde_json::json;
use tokio_util::sync::CancellationToken;

#[derive(Debug)]
struct UnusedSink;

#[async_trait]
impl ArtifactSink for UnusedSink {
    async fn store(
        &self,
        _kind: ArtifactKind,
        _name: &str,
        _summary: &str,
        _content_type: Option<&str>,
        _bytes: Vec<u8>,
        _agent: AgentId,
        _task: Option<TaskId>,
    ) -> Result<Artifact, ToolError> {
        Err(ToolError::Internal("no artifact was expected here".into()))
    }
}

/// The image this lane runs against. Named explicitly, and present locally,
/// or the lane fails: a run that pulls would depend on the network, and one
/// that skips would be evidence of nothing while looking like evidence.
fn required_image() -> String {
    let image = std::env::var("AETHER_TEST_CONTAINER_IMAGE").expect(
        "AETHER_TEST_CONTAINER_IMAGE must name a local image with bash; this lane proves the \
         container behaves as promised and cannot do that without one",
    );
    let listed = std::process::Command::new("docker")
        .args(["image", "inspect", &image])
        .output()
        .unwrap_or_else(|e| panic!("docker is not usable here: {e}"));
    assert!(
        listed.status.success(),
        "{image} is not present locally; pull it before running this lane"
    );
    image
}

fn ctx(root: &std::path::Path, image: String) -> ToolContext {
    ToolContext {
        agent_id: AgentId::new(),
        task_id: None,
        instance_id: "test-instance".into(),
        scope: Scope {
            workspace_root: Some(root.to_path_buf()),
            backend: ExecutionBackend::Container { image },
            ..Default::default()
        },
        cwd: root.to_path_buf(),
        artifacts: Arc::new(UnusedSink),
        env: BTreeMap::new(),
        hidden_env: Vec::new().into(),
        temp_dir: None,
    }
}

async fn run(ctx: &ToolContext, command: &str, cancel: CancellationToken) -> ToolOutcome {
    match BashTool::new(Arc::new(BackgroundProcesses::new()))
        .execute(json!({ "command": command }), ctx, cancel)
        .await
        .expect("execute")
    {
        ToolExecution::Outcome(o) => o,
        other => panic!("unexpected {other:?}"),
    }
}

/// Containers Aether started that are still running. Empty is the only
/// acceptable answer once a call has ended.
fn live_containers(agent: AgentId) -> Vec<String> {
    let output = std::process::Command::new("docker")
        .args(["ps", "--format", "{{.Names}}", "--filter"])
        .arg(format!("label=aether.agent={agent}"))
        .output()
        .expect("docker ps");
    String::from_utf8_lossy(&output.stdout)
        .lines()
        .map(str::to_owned)
        .collect()
}

#[tokio::test]
#[ignore = "needs docker and AETHER_TEST_CONTAINER_IMAGE; run with -- --ignored"]
async fn a_command_runs_in_the_container_and_writes_through_the_workspace_mount() {
    let image = required_image();
    let dir = tempfile::tempdir().expect("tempdir");
    let ctx = ctx(dir.path(), image);

    // Docker creates /.dockerenv in every container and nowhere else, so this
    // fails if the command quietly ran on this machine instead.
    let out = run(
        &ctx,
        "test -e /.dockerenv && echo contained",
        CancellationToken::new(),
    )
    .await;
    assert!(!out.is_error, "{}", out.content);
    assert!(out.content.contains("contained"), "{}", out.content);

    // The workspace is mounted at the same path and is writable, and the file
    // belongs to whoever runs the daemon rather than to root.
    let out = run(&ctx, "echo written > note.txt", CancellationToken::new()).await;
    assert!(!out.is_error, "{}", out.content);
    let note = dir.path().join("note.txt");
    assert_eq!(
        std::fs::read_to_string(&note).expect("the file is on the host"),
        "written\n"
    );
    assert!(
        std::fs::File::open(&note).is_ok(),
        "a file the container wrote must be readable here"
    );

    assert!(
        live_containers(ctx.agent_id).is_empty(),
        "nothing is left running"
    );
}

#[tokio::test]
#[ignore = "needs docker and AETHER_TEST_CONTAINER_IMAGE; run with -- --ignored"]
async fn scope_decides_the_network_and_what_is_writable() {
    let image = required_image();
    let dir = tempfile::tempdir().expect("tempdir");
    let outside = tempfile::tempdir().expect("outside");
    std::fs::write(outside.path().join("readable"), "hello").expect("seed");
    let mut ctx = ctx(dir.path(), image);
    ctx.scope.allowed_paths = vec![outside.path().to_path_buf()];

    // Denied network: only the loopback interface exists.
    let out = run(&ctx, "ls /sys/class/net", CancellationToken::new()).await;
    assert!(!out.is_error, "{}", out.content);
    assert!(out.content.contains("lo"), "{}", out.content);
    assert!(
        !out.content.contains("eth0"),
        "a denied scope has no network interface: {}",
        out.content
    );

    // Outside the workspace is readable and not writable.
    let readable = outside.path().join("readable");
    let out = run(
        &ctx,
        &format!("cat {}", readable.display()),
        CancellationToken::new(),
    )
    .await;
    assert!(out.content.contains("hello"), "{}", out.content);
    let out = run(
        &ctx,
        &format!("echo no > {}", readable.display()),
        CancellationToken::new(),
    )
    .await;
    assert!(out.is_error, "a read-only mount must refuse a write");
    assert_eq!(
        std::fs::read_to_string(&readable).expect("still there"),
        "hello"
    );

    ctx.scope.network = NetworkPolicy::Allowed;
    let out = run(&ctx, "ls /sys/class/net", CancellationToken::new()).await;
    assert!(
        out.content.contains("eth0"),
        "an allowed scope gets an interface: {}",
        out.content
    );

    assert!(
        live_containers(ctx.agent_id).is_empty(),
        "nothing is left running"
    );
}

#[tokio::test]
#[ignore = "needs docker and AETHER_TEST_CONTAINER_IMAGE; run with -- --ignored"]
async fn an_allowed_directory_with_a_live_nested_socket_is_denied() {
    let workspace = tempfile::tempdir().expect("workspace");
    let outside = tempfile::tempdir().expect("outside");
    let nested = outside.path().join("run/service");
    std::fs::create_dir_all(&nested).expect("nested directories");
    let socket = nested.join("api.sock");
    let listener = std::os::unix::net::UnixListener::bind(&socket).expect("socket");
    listener
        .set_nonblocking(true)
        .expect("nonblocking listener");
    let mut ctx = ctx(workspace.path(), required_image());
    ctx.scope.allowed_paths = vec![outside.path().to_path_buf()];

    let result = BashTool::new(Arc::new(BackgroundProcesses::new()))
        .execute(
            json!({ "command": format!(
                "python3 -c 'import socket; s = socket.socket(socket.AF_UNIX); s.connect(\"{}\")'",
                socket.display()
            ) }),
            &ctx,
            CancellationToken::new(),
        )
        .await;
    assert!(matches!(result, Err(ToolError::Denied(_))), "{result:?}");
    assert_eq!(
        listener
            .accept()
            .expect_err("no connection reaches the host")
            .kind(),
        std::io::ErrorKind::WouldBlock
    );
    assert!(live_containers(ctx.agent_id).is_empty());
}

#[tokio::test]
#[ignore = "needs docker and AETHER_TEST_CONTAINER_IMAGE; run with -- --ignored"]
async fn cancelling_a_call_kills_its_container() {
    let image = required_image();
    let dir = tempfile::tempdir().expect("tempdir");
    let ctx = ctx(dir.path(), image);
    let cancel = CancellationToken::new();
    let cancelling = cancel.clone();
    let agent = ctx.agent_id;
    let started = tokio::spawn(async move {
        // Cancel only once the container is actually up, so a run that never
        // containerized cannot pass this test by leaving nothing behind.
        for _ in 0..80 {
            if !live_containers(agent).is_empty() {
                cancelling.cancel();
                return true;
            }
            tokio::time::sleep(std::time::Duration::from_millis(250)).await;
        }
        cancelling.cancel();
        false
    });
    let result = BashTool::new(Arc::new(BackgroundProcesses::new()))
        .execute(json!({"command": "sleep 300"}), &ctx, cancel)
        .await;
    assert!(
        started.await.expect("watcher"),
        "the call never started a container; result: {result:?}"
    );
    assert!(
        matches!(result, Err(ToolError::Cancelled)),
        "{:?}",
        result.map(|_| ())
    );
    // `docker kill` is fire-and-forget; give it a moment to land.
    for _ in 0..40 {
        if live_containers(ctx.agent_id).is_empty() {
            return;
        }
        tokio::time::sleep(std::time::Duration::from_millis(250)).await;
    }
    panic!(
        "a cancelled call left a container running: {:?}",
        live_containers(ctx.agent_id)
    );
}

/// Start a detached container labelled as one agent of one daemon instance,
/// the way `prepare` labels the ones it runs. Returns its name.
fn labelled_container(image: &str, instance: &str) -> String {
    let name = format!("aether-sweep-test-{}", aether_core::id::ProcessId::new());
    let status = std::process::Command::new("docker")
        .args(["run", "--rm", "--detach", "--name", &name])
        .args([
            "--label",
            &format!("{}={}", aether_tools::exec::AGENT_LABEL, AgentId::new()),
        ])
        .args([
            "--label",
            &format!("{}={instance}", aether_tools::exec::INSTANCE_LABEL),
        ])
        .arg(image)
        .args(["bash", "-lc", "sleep 120"])
        .stdout(std::process::Stdio::null())
        .status()
        .expect("docker run");
    assert!(status.success(), "could not start {name}");
    name
}

fn is_running(name: &str) -> bool {
    let output = std::process::Command::new("docker")
        .args(["ps", "--quiet", "--filter", &format!("name=^{name}$")])
        .output()
        .expect("docker ps");
    !String::from_utf8_lossy(&output.stdout).trim().is_empty()
}

fn force_remove(name: &str) {
    let _ = std::process::Command::new("docker")
        .args(["rm", "--force", name])
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status();
}

#[tokio::test]
#[ignore = "needs docker and AETHER_TEST_CONTAINER_IMAGE; run with -- --ignored"]
async fn the_orphan_sweep_reaps_only_this_instances_containers() {
    // Two daemons on one machine are a supported shape - the socket, the
    // store and the instance id are all per-instance. An unscoped sweep made
    // starting the second kill the first's in-flight calls, which surfaced as
    // failed tool calls in an unrelated run.
    let image = required_image();
    let ours = format!("inst-{}", aether_core::id::ProcessId::new());
    let theirs = format!("inst-{}", aether_core::id::ProcessId::new());
    let mine = labelled_container(&image, &ours);
    let other = labelled_container(&image, &theirs);

    aether_tools::exec::sweep_orphan_containers(&ours).await;

    // `docker kill` returns before the container is gone from `ps`.
    let mut mine_gone = false;
    for _ in 0..50 {
        if !is_running(&mine) {
            mine_gone = true;
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(200)).await;
    }
    let other_alive = is_running(&other);
    force_remove(&mine);
    force_remove(&other);

    assert!(mine_gone, "a daemon must reap its own instance's orphans");
    assert!(
        other_alive,
        "a daemon must not reap another instance's running containers"
    );
}

#[tokio::test]
#[ignore = "requires Docker and AETHER_TEST_CONTAINER_IMAGE"]
async fn interactive_terminal_stays_in_the_container_and_accepts_input() {
    let dir = tempfile::tempdir().unwrap();
    let ctx = ctx(dir.path(), required_image());
    let processes = BackgroundProcesses::new();
    let process = processes.start_with_terminal(ctx.agent_id,
        "test -t 0 && test -t 1 && test -e /.dockerenv && printf 'READY\n'; read -r answer; printf 'ANSWER=%s\n' \"$answer\"",
        &ctx, true).unwrap();
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(60);
    loop {
        let bytes = std::fs::read(&process.output_path).unwrap();
        if bytes.windows(5).any(|w| w == b"READY") {
            break;
        }
        assert!(
            std::time::Instant::now() < deadline,
            "container terminal never became ready: {}",
            String::from_utf8_lossy(&bytes)
        );
        tokio::time::sleep(std::time::Duration::from_millis(20)).await;
    }
    processes
        .write_stdin(
            ctx.agent_id,
            process.id,
            "container input\n",
            CancellationToken::new(),
        )
        .await
        .unwrap();
    let mut seen = String::new();
    loop {
        let out = processes.read(ctx.agent_id, process.id).unwrap();
        seen.push_str(&out.text);
        if !out.status.running {
            assert_eq!(out.status.exit_code, Some(0), "{seen}");
            assert!(
                out.status.output_error.is_none(),
                "{:?}",
                out.status.output_error
            );
            break;
        }
        assert!(
            std::time::Instant::now() < deadline,
            "container terminal never exited: {seen}"
        );
        tokio::time::sleep(std::time::Duration::from_millis(20)).await;
    }
    assert!(seen.contains("ANSWER=container input"), "{seen}");
    processes.stop(ctx.agent_id, process.id).await.unwrap();
    assert!(process.output_path.exists());
}
