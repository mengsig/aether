#![cfg(unix)]
#![allow(clippy::expect_used, clippy::unwrap_used, clippy::panic)]

use std::path::Path;
use std::time::Duration;

use aether_core::event::EventKind;
use aether_daemon::DaemonConfig;
use aether_store::{EventFilter, SqliteStore, Store};
use tokio_util::sync::CancellationToken;

const PROBE_ROOT: &str = "AETHER_TEST_STARTUP_PROBE_ROOT";
// Fake probes outlast the liveness guards, so an unbounded wait still fails.
const SLOW_PROBE: &str =
    "printf '%s\\n' \"$0 $*\" >> \"$AETHER_TEST_STARTUP_PROBE_ROOT/probes\"\nexec /bin/sleep 300\n";
const FAST_ENGINE: &str = "printf '%s\\n' '2.1.269'\n";

// The subprocess owns PATH; parallel tests never change the process environment.
// A child writes fixtures so sibling forks cannot inherit their writable descriptors.
async fn run_isolated(test: &str, engine: &str, docker: &str) -> bool {
    if std::env::var_os(PROBE_ROOT).is_some() {
        return false;
    }
    use std::os::unix::fs::PermissionsExt;
    let root = tempfile::tempdir().unwrap();
    for (program, body) in [("claude", engine), ("codex", engine), ("docker", docker)] {
        let path = root.path().join(program);
        let status = std::process::Command::new("/bin/sh")
            .args(["-c", r#"printf '%s' "$1" > "$2""#, "sh"])
            .arg(format!("#!/bin/sh\n{body}"))
            .arg(&path)
            .status()
            .unwrap();
        assert!(status.success());
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o700)).unwrap();
    }
    let output = tokio::time::timeout(
        Duration::from_secs(180),
        tokio::process::Command::new(std::env::current_exe().unwrap())
            .args(["--exact", test, "--nocapture"])
            .env(PROBE_ROOT, root.path())
            .env("PATH", root.path())
            .kill_on_drop(true)
            .output(),
    )
    .await
    .expect("the isolated startup test must finish")
    .unwrap();
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        output.status.success(),
        "{stdout}{}",
        String::from_utf8_lossy(&output.stderr)
    );
    // A stale `--exact` name would run zero tests and still exit 0.
    assert!(stdout.contains("1 passed"), "{stdout}");
    eprint!("{}", String::from_utf8_lossy(&output.stderr));
    true
}

fn config(root: &Path) -> DaemonConfig {
    DaemonConfig {
        socket: root.join("aether.sock"),
        store_path: root.join("aether.db"),
        config_dir: Some(root.join("config")),
        workspace_root: Some(root.join("workspace")),
        max_concurrent_turns: None,
        yolo: false,
        tcp: None,
        allow_remote: false,
    }
}

#[tokio::test]
async fn rejecting_a_pid_file_does_not_run_external_startup_probes() {
    if run_isolated(
        "rejecting_a_pid_file_does_not_run_external_startup_probes",
        SLOW_PROBE,
        SLOW_PROBE,
    )
    .await
    {
        return;
    }
    let root = tempfile::tempdir().unwrap();
    let config = config(root.path());
    let pid = aether_protocol::paths::pid_path_for(&config.socket);
    let victim = root.path().join("keep");
    std::fs::write(&victim, b"keep bytes").unwrap();
    std::fs::hard_link(&victim, &pid).unwrap();
    let error = tokio::time::timeout(
        Duration::from_secs(20),
        aether_daemon::run(config.clone(), CancellationToken::new()),
    )
    .await
    .expect("PID rejection must not wait for a slow engine or Docker")
    .unwrap_err();
    assert!(
        error.to_string().contains("multiple hard links"),
        "{error:#}"
    );
    assert!(!config.socket.exists());
    assert!(
        !config.store_path.exists(),
        "preflight must precede recovery"
    );
    assert!(pid.exists());
    assert_eq!(std::fs::read(&victim).unwrap(), b"keep bytes");
    let probes = Path::new(&std::env::var_os(PROBE_ROOT).unwrap()).join("probes");
    assert!(
        !probes.exists(),
        "rejected startup must run no external probes"
    );
}

#[cfg(target_os = "linux")]
#[tokio::test]
async fn an_unresponsive_docker_sweep_is_bounded_and_its_client_is_reaped() {
    if run_isolated(
        "an_unresponsive_docker_sweep_is_bounded_and_its_client_is_reaped",
        FAST_ENGINE,
        "printf '%s\\n' \"$$\" > \"$AETHER_TEST_STARTUP_PROBE_ROOT/docker.pid\"\nexec /bin/sleep 300\n",
    )
    .await
    {
        run_isolated(
            "an_unresponsive_docker_sweep_is_bounded_and_its_client_is_reaped",
            FAST_ENGINE,
            "if [ \"$1\" = ps ]; then printf '%s\\n' fixture-orphan; exit 0; fi\nprintf '%s\\n' \"$$\" > \"$AETHER_TEST_STARTUP_PROBE_ROOT/docker.pid\"\nexec /bin/sleep 300\n",
        )
        .await;
        return;
    }
    let root = tempfile::tempdir().unwrap();
    let shutdown = CancellationToken::new();
    shutdown.cancel();
    tokio::time::timeout(
        Duration::from_secs(60),
        aether_daemon::run(config(root.path()), shutdown),
    )
    .await
    .expect("an unresponsive Docker service must not block startup forever")
    .unwrap();
    let probes = std::env::var_os(PROBE_ROOT).unwrap();
    let pid = std::fs::read_to_string(Path::new(&probes).join("docker.pid")).unwrap();
    let process = std::path::PathBuf::from(format!("/proc/{}", pid.trim()));
    tokio::time::timeout(Duration::from_secs(10), async {
        while process.exists() {
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .expect("a timed-out Docker client must be killed and reaped");
}

#[tokio::test]
async fn pid_replacement_after_preflight_still_stops_the_runtime_and_cleans_its_socket() {
    if run_isolated(
        "pid_replacement_after_preflight_still_stops_the_runtime_and_cleans_its_socket",
        FAST_ENGINE,
        "printf ready > \"$AETHER_TEST_STARTUP_PROBE_ROOT/ready\"\nwhile [ ! -f \"$AETHER_TEST_STARTUP_PROBE_ROOT/release\" ]; do /bin/sleep 0.01; done\n",
    )
    .await
    {
        return;
    }
    let probes = std::env::var_os(PROBE_ROOT).unwrap();
    let probes = Path::new(&probes);
    let root = tempfile::tempdir().unwrap();
    let config = config(root.path());
    let serving = tokio::spawn(aether_daemon::run(config.clone(), CancellationToken::new()));
    tokio::time::timeout(Duration::from_secs(60), async {
        while !probes.join("ready").exists() {
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .expect("the orphan sweep must follow runtime startup");
    let pid = aether_protocol::paths::pid_path_for(&config.socket);
    let victim = root.path().join("keep");
    std::fs::write(&victim, b"keep bytes").unwrap();
    std::fs::hard_link(&victim, &pid).unwrap();
    std::fs::write(probes.join("release"), b"release").unwrap();
    let error = tokio::time::timeout(Duration::from_secs(60), serving)
        .await
        .unwrap()
        .unwrap()
        .unwrap_err();
    assert!(
        error.to_string().contains("multiple hard links"),
        "{error:#}"
    );
    assert!(!config.socket.exists());
    assert!(pid.exists());
    assert_eq!(std::fs::read(&victim).unwrap(), b"keep bytes");
    let store = SqliteStore::open(config.store_path).unwrap();
    let events = store.events(0, 100, &EventFilter::default()).unwrap();
    assert_eq!(
        events
            .iter()
            .filter(|e| matches!(e.kind, EventKind::DaemonStarted { .. }))
            .count(),
        1
    );
    assert_eq!(
        events
            .iter()
            .filter(|e| matches!(e.kind, EventKind::DaemonStopped { .. }))
            .count(),
        1
    );
}

// Records each probe, then answers only once the test releases it.
const GATED_ENGINE: &str = "printf '%s\\n' \"$0 $*\" >> \"$AETHER_TEST_STARTUP_PROBE_ROOT/probes\"\nwhile [ ! -f \"$AETHER_TEST_STARTUP_PROBE_ROOT/release\" ]; do /bin/sleep 0.01; done\nprintf '%s\\n' '2.1.269'\n";

/// The acceptance bound for a warm start, from process start to an answered hello.
const HELLO_WITHIN: Duration = Duration::from_millis(1500);

async fn first_hello(socket: &Path) -> aether_client::Client {
    tokio::time::timeout(Duration::from_secs(30), async {
        loop {
            if let Ok(client) = aether_client::Client::connect(socket).await {
                return client;
            }
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    })
    .await
    .expect("the daemon answers a hello")
}

#[tokio::test]
async fn hello_is_answered_while_engine_probes_run_and_engine_requests_wait() {
    if run_isolated(
        "hello_is_answered_while_engine_probes_run_and_engine_requests_wait",
        GATED_ENGINE,
        "exit 1\n",
    )
    .await
    {
        return;
    }
    let probes = std::env::var_os(PROBE_ROOT).unwrap();
    let probes = Path::new(&probes);
    let root = tempfile::tempdir().unwrap();
    let config = config(root.path());
    std::fs::create_dir_all(root.path().join("workspace")).unwrap();
    drop(SqliteStore::open(&config.store_path).unwrap());
    let shutdown = CancellationToken::new();
    let started = std::time::Instant::now();
    let serving = tokio::spawn(aether_daemon::run(config.clone(), shutdown.clone()));
    let client = first_hello(&config.socket).await;
    let answered = started.elapsed();
    eprintln!("start to first answered hello: {} ms", answered.as_millis());
    assert!(answered < HELLO_WITHIN, "hello took {answered:?}");
    assert!(!probes.join("release").exists());
    let status = client.status().await.unwrap();
    assert_eq!(status.engines, [aether_protocol::ENGINES_STARTING]);

    let creating = tokio::spawn(async move {
        client
            .create_agent(aether_protocol::CreateAgent {
                objective: Some("wait for the engines".into()),
                start: false,
                model: Some(aether_core::model::ModelRef::new("claude-code", "haiku")),
                ..Default::default()
            })
            .await
    });
    tokio::time::sleep(Duration::from_millis(300)).await;
    assert!(
        !creating.is_finished(),
        "an engine request must wait for detection"
    );
    let probed = std::fs::read_to_string(probes.join("probes")).unwrap();
    assert!(probed.contains("claude"), "{probed}");
    assert!(
        probed.contains("codex"),
        "both engines probe at once: {probed}"
    );

    std::fs::write(probes.join("release"), b"release").unwrap();
    let created = tokio::time::timeout(Duration::from_secs(30), creating)
        .await
        .expect("the waiting request finishes once detection does")
        .unwrap()
        .unwrap();
    assert_eq!(created.model.provider, "claude-code");
    let client = aether_client::Client::connect(&config.socket)
        .await
        .unwrap();
    let engines = client.status().await.unwrap().engines;
    assert!(
        engines.iter().any(|label| label.starts_with("claude-code")),
        "{engines:?}"
    );
    assert!(
        !engines
            .iter()
            .any(|label| label == aether_protocol::ENGINES_STARTING)
    );
    shutdown.cancel();
    tokio::time::timeout(Duration::from_secs(30), serving)
        .await
        .unwrap()
        .unwrap()
        .unwrap();
}
