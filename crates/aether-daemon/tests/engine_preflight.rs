#![cfg(unix)]
#![allow(clippy::expect_used, clippy::unwrap_used, clippy::panic)]

//! Engine preflight as a running daemon sees it: a missing CLI refuses a
//! started agent up front, and a CLI fixed after startup is found again
//! without a daemon restart.

mod support;

use std::path::Path;
use std::time::Duration;

use aether_core::model::ModelRef;
use aether_protocol::{AgentQuery, CreateAgent};

const ROOT_ENV: &str = "AETHER_TEST_ENGINE_PREFLIGHT_ROOT";

/// `claude --version` fails the first time, as a CLI mid-update would, then answers.
const FAILS_ONCE: &str = "if [ \"$1\" != --version ]; then exit 1; fi\nif [ ! -f \"$AETHER_TEST_ENGINE_PREFLIGHT_ROOT/failed-once\" ]; then\n  : > \"$AETHER_TEST_ENGINE_PREFLIGHT_ROOT/failed-once\"\n  printf '%s\\n' 'claude is being updated' >&2\n  exit 1\nfi\nprintf '%s\\n' '2.1.269'\n";

/// Re-run `test` alone with `PATH` holding only `programs`, so the daemon's
/// probes find exactly them. Returns false inside that run.
async fn run_isolated(test: &str, programs: &[(&str, &str)]) -> bool {
    if std::env::var_os(ROOT_ENV).is_some() {
        return false;
    }
    use std::os::unix::fs::PermissionsExt;
    let root = tempfile::tempdir().unwrap();
    for (program, body) in programs {
        let path = root.path().join(program);
        std::fs::write(&path, format!("#!/bin/sh\n{body}")).unwrap();
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o700)).unwrap();
    }
    let output = tokio::time::timeout(
        Duration::from_secs(120),
        tokio::process::Command::new(std::env::current_exe().unwrap())
            .args(["--exact", test, "--nocapture"])
            .env(ROOT_ENV, root.path())
            .env("PATH", root.path())
            .kill_on_drop(true)
            .output(),
    )
    .await
    .expect("the isolated preflight test must finish")
    .unwrap();
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        output.status.success(),
        "{stdout}{}",
        String::from_utf8_lossy(&output.stderr)
    );
    // A stale `--exact` name would run zero tests and still exit 0.
    assert!(stdout.contains("1 passed"), "{stdout}");
    true
}

async fn detected(client: &aether_client::Client) {
    tokio::time::timeout(Duration::from_secs(60), async {
        while client.status().await.unwrap().engines == [aether_protocol::ENGINES_STARTING] {
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    })
    .await
    .expect("engine detection finishes");
}

fn started_claude_agent() -> CreateAgent {
    CreateAgent {
        role: "worker".into(),
        objective: Some("say hello".into()),
        start: true,
        model: Some(ModelRef::new("claude-code", "haiku")),
        ..Default::default()
    }
}

#[tokio::test]
async fn a_missing_engine_refuses_a_started_agent_and_creates_none() {
    if run_isolated(
        "a_missing_engine_refuses_a_started_agent_and_creates_none",
        &[],
    )
    .await
    {
        return;
    }
    let daemon = support::TestDaemon::start().await;
    let client = aether_client::Client::connect(&daemon.socket)
        .await
        .unwrap();
    detected(&client).await;
    let refused = client
        .create_agent(started_claude_agent())
        .await
        .expect_err("a started agent needs its engine")
        .to_string();
    assert!(refused.contains("claude-code engine"), "{refused}");
    assert!(refused.contains("install claude"), "{refused}");
    let agents = client.list_agents(AgentQuery::default()).await.unwrap();
    assert!(agents.is_empty(), "no agent is left behind: {agents:?}");
    daemon.stop().await;
}

#[tokio::test]
async fn an_engine_fixed_after_startup_is_found_again_without_a_restart() {
    if run_isolated(
        "an_engine_fixed_after_startup_is_found_again_without_a_restart",
        &[("claude", FAILS_ONCE)],
    )
    .await
    {
        return;
    }
    let root = std::env::var_os(ROOT_ENV).unwrap();
    let daemon = support::TestDaemon::start().await;
    let client = aether_client::Client::connect(&daemon.socket)
        .await
        .unwrap();
    detected(&client).await;
    assert!(
        Path::new(&root).join("failed-once").exists(),
        "startup detection saw the broken CLI"
    );
    let agent = client
        .create_agent(started_claude_agent())
        .await
        .expect("the fixed CLI is probed again and accepted");
    assert_eq!(agent.model, ModelRef::new("claude-code", "haiku"));
    daemon.stop().await;
}
