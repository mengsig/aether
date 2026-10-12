#![allow(clippy::expect_used, clippy::unwrap_used, clippy::panic)]

use std::path::Path;
use std::time::Duration;

use aether_core::event::{Actor, Event, EventDraft, EventKind};
use aether_core::id::{AgentId, EventId, RunId};
use aether_protocol::{ClientFrame, Command, Reply, ServerFrame, Welcome};
use serde_json::{Value, json};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::UnixListener;

const SECRET: &str = "fake-credential-DO-NOT-SHARE-376";
const RUN: &str = "run_01J00000000000000000000004";
const AGENT: &str = "agt_01J00000000000000000000001";
const AT: &str = "2026-09-11T03:00:00Z";

fn fixture() -> tempfile::TempDir {
    let root = tempfile::tempdir().unwrap();
    let config = root.path().join("config");
    std::fs::create_dir(&config).unwrap();
    std::fs::write(
        config.join("config.toml"),
        format!(
            r#"
[scheduler]
max_concurrent_turns = 7
[[mcp.servers]]
name = "{SECRET}"
command = "{SECRET}"
args = ["--password", "{SECRET}"]
[mcp.servers.env]
INNOCENT_NAME = "{SECRET}"
"{SECRET}" = "another-secret"
[mcp.servers.classes]
"{SECRET}" = "read_only"
"#
        ),
    )
    .unwrap();
    std::fs::write(
        config.join("providers.toml"),
        format!(
            r#"
[providers.openai]
base_url = "https://user:{SECRET}@example.com?token={SECRET}"
[providers.openai.default_capabilities]
context_window = 1024
max_output_tokens = 100
[providers.openai.default_capabilities.extensions]
"{SECRET}" = {{ nested = "{SECRET}", pin = 123456 }}
[providers.openai.headers]
Authorization = "Bearer {SECRET}"
"{SECRET}" = "{SECRET}"
[providers.openai.auth.oauth]
flow = "generic"
client_id = "{SECRET}"
client_secret = "{SECRET}"
authorize_url = "https://example.com"
token_url = "https://example.com"
[providers.openai.auth.oauth.extra_authorize_params]
"{SECRET}" = "{SECRET}"
[providers.openai.auth.oauth.extra_token_params]
"{SECRET}" = "{SECRET}"
"#
        ),
    )
    .unwrap();
    std::fs::write(config.join("credentials.json"), SECRET).unwrap();
    for dir in [".claude", ".codex"] {
        std::fs::create_dir(root.path().join(dir)).unwrap();
        std::fs::write(root.path().join(dir).join("auth.json"), SECRET).unwrap();
    }
    let logs = root.path().join("data/logs/aetherd.log/segments-v1");
    std::fs::create_dir_all(&logs).unwrap();
    std::fs::write(
        logs.join("events.0.log"),
        format!("{AT} INFO password={SECRET}\n").repeat(1000),
    )
    .unwrap();
    root
}

async fn command(root: &Path, args: &[&str]) -> std::process::Output {
    let mut command = tokio::process::Command::new(env!("CARGO_BIN_EXE_aether"));
    command
        .current_dir(root)
        .env("HOME", root)
        .env("PATH", "")
        .env("AETHER_CONFIG_DIR", root.join("config"))
        .env("AETHER_DATA_DIR", root.join("data"))
        .env("AETHER_SOCKET", root.join("daemon.sock"))
        .env("UNASSUMING_VARIABLE", SECRET)
        .args(args);
    tokio::time::timeout(Duration::from_secs(45), command.output())
        .await
        .unwrap()
        .unwrap()
}

fn read(bundle: &Path, name: &str) -> Value {
    serde_json::from_slice(&std::fs::read(bundle.join(name)).unwrap()).unwrap()
}

fn assert_bundle(bundle: &Path) {
    let manifest = read(bundle, "manifest.json");
    assert_eq!(manifest["format_version"], 1);
    let files = manifest["files"].as_array().unwrap();
    assert_eq!(files.len(), 6);
    assert_eq!(std::fs::read_dir(bundle).unwrap().count(), 7);
    for name in files
        .iter()
        .map(|value| value.as_str().unwrap())
        .chain(["manifest.json"])
    {
        let bytes = std::fs::read(bundle.join(name)).unwrap();
        assert!(
            !String::from_utf8_lossy(&bytes).contains(SECRET),
            "secret in {name}"
        );
        serde_json::from_slice::<Value>(&bytes).unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            assert_eq!(
                std::fs::metadata(bundle.join(name))
                    .unwrap()
                    .permissions()
                    .mode()
                    & 0o777,
                0o600
            );
        }
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        assert_eq!(
            std::fs::metadata(bundle).unwrap().permissions().mode() & 0o777,
            0o700
        );
    }
    assert_eq!(
        read(bundle, "config.json")["config"]["effective"]["scheduler"]["max_concurrent_turns"],
        7
    );
    assert!(read(bundle, "config.json")["providers"]["effective"].is_array());
    assert_eq!(
        read(bundle, "logs.json")["lines_newest_first"]
            .as_array()
            .unwrap()
            .len(),
        200
    );
    assert_eq!(
        read(bundle, "logs.json")["lines_newest_first"][0]["level"],
        "INFO"
    );
    assert_eq!(read(bundle, "logs.json")["byte_window_truncated"], true);
    assert_eq!(
        read(bundle, "build.json")["version"],
        env!("CARGO_PKG_VERSION")
    );
    assert!(
        read(bundle, "doctor.json")["checks"]
            .as_array()
            .unwrap()
            .iter()
            .any(|check| check["name"] == "daemon")
    );
}

#[tokio::test]
async fn daemon_down_writes_redacted_manifest_and_connection_error_without_starting_daemon() {
    let root = fixture();
    let output = command(root.path(), &["diagnose", "--out", "bundle"]).await;
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(String::from_utf8_lossy(&output.stdout).contains("manifest.json"));
    let bundle = root.path().join("bundle");
    assert_bundle(&bundle);
    let daemon = read(&bundle, "daemon.json");
    assert_eq!(daemon["connected"], false);
    assert_eq!(daemon["result"]["error"]["kind"], "NotFound");
    assert_eq!(
        daemon["socket"],
        root.path().join("daemon.sock").display().to_string()
    );
    assert_eq!(read(&bundle, "history.json")["error"], "daemon unavailable");
    assert!(!root.path().join("daemon.sock").exists());
    assert_eq!(
        std::fs::read_to_string(root.path().join("config/credentials.json")).unwrap(),
        SECRET
    );
}

async fn daemon(root: &Path, empty: bool, refuse_events: bool) -> tokio::task::JoinHandle<()> {
    let listener = UnixListener::bind(root.join("daemon.sock")).unwrap();
    tokio::spawn(async move {
        loop {
            let (stream, _) = listener.accept().await.unwrap();
            tokio::spawn(async move {
                let (read, mut write) = stream.into_split();
                let mut lines = BufReader::new(read).lines();
                while let Some(line) = lines.next_line().await.unwrap() {
                    let request: ClientFrame = serde_json::from_str(&line).unwrap();
                    let result = match request.command {
                        Command::Hello(_) => Ok(Reply::Welcome(Welcome {
                            protocol_version: aether_protocol::PROTOCOL_VERSION,
                            daemon_version: SECRET.into(), instance_id: SECRET.into(),
                            schema_version: 1, pid: 1, features: vec![],
                            daemon_protocol_version: None, min_protocol_version: None,
                            idle_timeout_secs: None,
                            device: None,
                            submission_horizon: None,
                        })),
                        Command::Status => Ok(Reply::Status(serde_json::from_value(json!({
                            "instance_id": SECRET, "version": SECRET, "pid": 42, "started_at": AT,
                            "last_seq": 12, "agents_live": 1, "agents_running": 1,
                            "agents_waiting": 0, "agents_blocked": 0, "tasks_active": 0,
                            "pending_approvals": 0, "usage_total": aether_core::model::Usage::default(),
                            "mcp": [{"name": SECRET, "state": "failed", "transport": SECRET, "error": SECRET}]
                        })).unwrap())),
                        Command::AuthStatus => Ok(Reply::Auth(vec![])),
                        Command::ListRuns { limit } => {
                            assert_eq!(limit, 2);
                            Ok(Reply::Runs(if empty { vec![] } else { vec![serde_json::from_value(json!({
                                "run": RUN, "objective": SECRET, "agents": 1, "agents_live": 1,
                                "usage": aether_core::model::Usage::default(), "started_at": AT, "updated_at": AT,
                            })).unwrap()] }))
                        }
                        Command::GetEvents { after, limit, query } => {
                            assert_eq!(after, 0);
                            assert_eq!(limit, 200);
                            assert_eq!(query.before, Some(u64::MAX));
                            assert!(query.run.is_none() || query.run == Some(RUN.parse().unwrap()));
                            if refuse_events {
                                Err(aether_protocol::ErrorReply { code: aether_protocol::ErrorCode::InvalidArgument, message: SECRET.into() })
                            } else {
                                let events = if empty { vec![] } else {
                                    [EventKind::AgentStarted { turn: 1 }, EventKind::AgentFailed { error: SECRET.into() }]
                                        .into_iter().enumerate().map(|(index, kind)| Event::from_draft(
                                            EventDraft::new(Actor::System, kind).agent(AGENT.parse::<AgentId>().unwrap()).run(RUN.parse::<RunId>().unwrap()),
                                            EventId::new(), 11 + index as u64, AT.parse().unwrap(),
                                        )).rev().collect()
                                };
                                Ok(Reply::Events(events))
                            }
                        }
                        other => panic!("diagnose sent unexpected or mutating command: {other:?}"),
                    };
                    let response = ServerFrame::Reply {
                        id: request.id,
                        result,
                    };
                    write
                        .write_all(aether_protocol::encode(&response).unwrap().as_bytes())
                        .await
                        .unwrap();
                }
            });
        }
    })
}

#[tokio::test]
async fn connected_bundle_has_bounded_events_and_replay_cursors_without_payloads() {
    let root = fixture();
    let daemon = daemon(root.path(), false, false).await;
    let output = command(root.path(), &["diagnose", "--out", "bundle", "--last", "2"]).await;
    daemon.abort();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let bundle = root.path().join("bundle");
    assert_bundle(&bundle);
    assert_eq!(read(&bundle, "daemon.json")["result"]["status"]["pid"], 42);
    let history = read(&bundle, "history.json");
    assert_eq!(history["runs"][0]["run"], RUN);
    assert_eq!(history["runs"][0]["replay_cursors"][0]["from_seq"], 11);
    assert_eq!(history["runs"][0]["replay_cursors"][0]["to_seq"], 12);
    assert_eq!(history["runs"][0]["events"][1]["type"], "agent.failed");
    assert!(history["runs"][0]["events"][1].get("kind").is_none());
}

#[tokio::test]
async fn selected_empty_run_and_request_failure_are_recorded() {
    for refuse in [false, true] {
        let root = fixture();
        let daemon = daemon(root.path(), true, refuse).await;
        let output = command(root.path(), &["diagnose", "--out", "bundle", "--run", RUN]).await;
        daemon.abort();
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        let bundle = root.path().join("bundle");
        assert_bundle(&bundle);
        let history = read(&bundle, "history.json");
        assert_eq!(history["runs"][0]["run"], RUN);
        if refuse {
            assert_eq!(history["runs"][0]["error"]["kind"], "protocol");
        } else {
            assert_eq!(history["runs"][0]["events"], json!([]));
            assert_eq!(history["runs"][0]["replay_cursors"], json!([]));
        }
    }
}

#[tokio::test]
async fn malformed_config_does_not_copy_source_or_error_excerpts() {
    let root = fixture();
    std::fs::write(
        root.path().join("config/config.toml"),
        format!("broken = \"{SECRET}"),
    )
    .unwrap();
    let output = command(root.path(), &["diagnose", "--out", "bundle"]).await;
    assert!(output.status.success());
    let bundle = root.path().join("bundle");
    assert!(read(&bundle, "config.json")["config"]["error"].is_string());
    assert!(
        read(&bundle, "doctor.json")["checks"]
            .as_array()
            .unwrap()
            .iter()
            .any(|check| check["name"] == "config" && check["level"] == "fail")
    );
    for entry in std::fs::read_dir(bundle).unwrap() {
        assert!(
            !std::fs::read_to_string(entry.unwrap().path())
                .unwrap()
                .contains(SECRET)
        );
    }
}

#[tokio::test]
async fn refuses_existing_destinations_and_invalid_arguments() {
    let root = fixture();
    std::fs::create_dir(root.path().join("bundle")).unwrap();
    std::fs::write(root.path().join("bundle/keep"), "keep").unwrap();
    let output = command(root.path(), &["diagnose", "--out", "bundle"]).await;
    assert!(!output.status.success());
    assert_eq!(
        std::fs::read_to_string(root.path().join("bundle/keep")).unwrap(),
        "keep"
    );
    for args in [
        vec!["diagnose", "--out", "invalid", "--last", "0"],
        vec!["diagnose", "--out", "invalid", "--last", "101"],
        vec!["diagnose", "--out", "invalid", "--run", "bad"],
        vec!["diagnose", "--out", "invalid", "--run", RUN, "--last", "2"],
    ] {
        assert_eq!(command(root.path(), &args).await.status.code(), Some(2));
        assert!(!root.path().join("invalid").exists());
    }
}

#[tokio::test]
async fn missing_logs_and_config_use_defaults_and_record_missing_source() {
    let root = tempfile::tempdir().unwrap();
    let output = command(root.path(), &["diagnose", "--out", "bundle"]).await;
    assert!(output.status.success());
    let bundle = root.path().join("bundle");
    assert_eq!(read(&bundle, "logs.json")["error"]["kind"], "NotFound");
    assert_eq!(
        read(&bundle, "config.json")["config"]["effective"]["artifacts"]["retain_days"],
        30
    );
    assert!(!root.path().join("config").exists());
    assert!(!root.path().join("data").exists());
}

#[cfg(unix)]
#[tokio::test]
async fn refuses_symlink_destination_and_log_without_changing_targets() {
    let root = fixture();
    let target = root.path().join("target");
    std::fs::create_dir(&target).unwrap();
    std::os::unix::fs::symlink(&target, root.path().join("bundle")).unwrap();
    assert!(
        !command(root.path(), &["diagnose", "--out", "bundle"])
            .await
            .status
            .success()
    );
    assert_eq!(std::fs::read_dir(&target).unwrap().count(), 0);

    // Swap the fixture log for a symlink to a private source. Neither doctor nor
    // the tail reader may follow it or change its permissions.
    let log = root
        .path()
        .join("data/logs/aetherd.log/segments-v1/events.0.log");
    std::fs::rename(&log, root.path().join("original-log")).unwrap();
    std::os::unix::fs::symlink(root.path().join("config/credentials.json"), &log).unwrap();
    let output = command(root.path(), &["diagnose", "--out", "safe-bundle"]).await;
    assert!(output.status.success());
    assert!(read(&root.path().join("safe-bundle"), "logs.json")["error"].is_object());
    assert_eq!(
        std::fs::read_to_string(root.path().join("config/credentials.json")).unwrap(),
        SECRET
    );
}

#[tokio::test]
async fn json_output_reports_the_written_bundle_and_manifest() {
    let root = fixture();
    let output = command(root.path(), &["diagnose", "--out", "bundle", "--json"]).await;
    assert!(output.status.success());
    let result: Value = serde_json::from_slice(&output.stdout).unwrap();
    let bundle = root.path().join("bundle");
    assert_eq!(result["path"], bundle.display().to_string());
    assert_eq!(result["manifest"], read(&bundle, "manifest.json"));
    assert_bundle(&bundle);
}
