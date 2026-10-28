#![cfg(unix)]
#![allow(clippy::expect_used, clippy::unwrap_used, clippy::panic)]

//! Engine preflight as a running daemon sees it: a missing CLI refuses a
//! started agent up front, a CLI fixed after startup is found again without
//! a daemon restart, and paths that do not probe again never refuse on a
//! stale outcome.

mod support;

use std::path::Path;
use std::time::Duration;

use aether_core::agent::AgentConfig;
use aether_core::model::{Effort, ModelRef};
use aether_core::schedule::{
    BusyPolicy, CreateScheduleSpec, MissedRunPolicy, Recurrence, ScheduleTarget, SpawnSpec,
};
use aether_protocol::{AgentQuery, CreateAgent};

const ROOT_ENV: &str = "AETHER_TEST_ENGINE_PREFLIGHT_ROOT";

/// `claude --version` fails the first time, as a CLI mid-update would, then answers.
const FAILS_ONCE: &str = "if [ \"$1\" != --version ]; then exit 1; fi\nif [ ! -f \"$AETHER_TEST_ENGINE_PREFLIGHT_ROOT/failed-once\" ]; then\n  : > \"$AETHER_TEST_ENGINE_PREFLIGHT_ROOT/failed-once\"\n  printf '%s\\n' 'claude is being updated' >&2\n  exit 1\nfi\nprintf '%s\\n' '2.1.269'\n";

/// `claude --version` fails until the test writes `fixed`, as a CLI repaired by hand.
const FAILS_UNTIL_FIXED: &str = "if [ \"$1\" != --version ]; then exit 1; fi\nif [ ! -f \"$AETHER_TEST_ENGINE_PREFLIGHT_ROOT/fixed\" ]; then\n  printf '%s\\n' 'claude is broken' >&2\n  exit 1\nfi\nprintf '%s\\n' '2.1.269'\n";

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

/// A child that inherits its parent's model is rechecked on that model's
/// engine, not on the daemon default's.
#[tokio::test]
async fn a_fixed_engine_is_found_again_for_a_child_inheriting_its_model() {
    if run_isolated(
        "a_fixed_engine_is_found_again_for_a_child_inheriting_its_model",
        &[("claude", FAILS_UNTIL_FIXED)],
    )
    .await
    {
        return;
    }
    let root = std::env::var_os(ROOT_ENV).unwrap();
    // The default runs on another engine, so only the parent names claude-code.
    let daemon =
        support::TestDaemon::start_with_config("[model]\ndefault = \"codex/gpt-6.1-sol\"\n").await;
    let client = aether_client::Client::connect(&daemon.socket)
        .await
        .unwrap();
    detected(&client).await;
    let parent = client
        .create_agent(CreateAgent {
            start: false,
            ..started_claude_agent()
        })
        .await
        .expect("a parked agent does not need its engine yet");
    std::fs::write(Path::new(&root).join("fixed"), b"").unwrap();
    let child = CreateAgent {
        role: "worker".into(),
        objective: Some("say hello".into()),
        start: true,
        parent: Some(parent.id),
        config: Some(AgentConfig {
            effort: Some(Effort::Low),
            ..Default::default()
        }),
        ..Default::default()
    };
    let child = client
        .create_agent(child)
        .await
        .expect("the inherited engine is probed again and accepted");
    assert_eq!(child.model, ModelRef::new("claude-code", "haiku"));
    assert_eq!(child.parent, Some(parent.id));
    daemon.stop().await;
}

/// Schedules do not probe again, so a startup failure must not refuse one:
/// with the CLI fixed since, the schedule is accepted with no restart.
#[tokio::test]
async fn a_spawn_schedule_is_not_refused_on_a_stale_preflight_failure() {
    if run_isolated(
        "a_spawn_schedule_is_not_refused_on_a_stale_preflight_failure",
        &[("claude", FAILS_UNTIL_FIXED)],
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
    std::fs::write(Path::new(&root).join("fixed"), b"").unwrap();
    let template = SpawnSpec {
        role: "worker".into(),
        name: None,
        objective: "say hello".into(),
        model: Some(ModelRef::new("claude-code", "haiku")),
        fallbacks: Vec::new(),
        scope: None,
        config: None,
        approval: None,
    };
    let schedule = client
        .create_schedule(CreateScheduleSpec {
            name: "hourly-hello".into(),
            target: ScheduleTarget::Spawn(template),
            recurrence: Recurrence::Interval {
                seconds: std::num::NonZeroU64::new(3_600).unwrap(),
            },
            missed: MissedRunPolicy::default(),
            busy: BusyPolicy::default(),
            payload: serde_json::Value::Null,
            max_deliveries: None,
        })
        .await
        .expect("a spawn schedule is accepted once its CLI is fixed");
    assert_eq!(schedule.name, "hourly-hello");
    daemon.stop().await;
}

/// A keyed creation that succeeded is answered on retry even after a restart
/// finds its engine broken: the engine check is for new work only.
#[tokio::test]
async fn a_retried_creation_is_answered_after_its_engine_breaks() {
    if run_isolated(
        "a_retried_creation_is_answered_after_its_engine_breaks",
        &[("claude", FAILS_UNTIL_FIXED)],
    )
    .await
    {
        return;
    }
    let fixed = Path::new(&std::env::var_os(ROOT_ENV).unwrap()).join("fixed");
    std::fs::write(&fixed, b"").unwrap();
    let keyed = CreateAgent {
        idempotency_key: Some("started-once".into()),
        ..started_claude_agent()
    };
    let daemon = support::TestDaemon::start().await;
    let client = aether_client::Client::connect(&daemon.socket)
        .await
        .unwrap();
    detected(&client).await;
    let created = client.create_agent(keyed.clone()).await.unwrap();
    std::fs::remove_file(&fixed).unwrap();
    drop(client);
    let daemon = daemon.restart().await;
    let client = aether_client::Client::connect(&daemon.socket)
        .await
        .unwrap();
    detected(&client).await;
    let retried = client
        .create_agent(keyed)
        .await
        .expect("the retry is answered with the agent already created");
    assert_eq!(retried.id, created.id);
    let agents = client.list_agents(AgentQuery::default()).await.unwrap();
    assert_eq!(agents.len(), 1, "no second agent: {agents:?}");
    daemon.stop().await;
}

/// A CLI usable at startup and removed since is probed again, so a started
/// agent is refused before it exists rather than failing its first turn.
#[tokio::test]
async fn an_engine_removed_after_startup_refuses_a_started_agent() {
    if run_isolated(
        "an_engine_removed_after_startup_refuses_a_started_agent",
        &[("claude", "printf '%s\\n' '2.1.269'\n")],
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
    std::fs::remove_file(Path::new(&root).join("claude")).unwrap();
    let refused = client
        .create_agent(started_claude_agent())
        .await
        .expect_err("a removed CLI cannot run a started agent")
        .to_string();
    assert!(refused.contains("install claude"), "{refused}");
    let agents = client.list_agents(AgentQuery::default()).await.unwrap();
    assert!(agents.is_empty(), "no agent is left behind: {agents:?}");
    daemon.stop().await;
}

/// A CLI removed right after a probe found it is refused at once, not served
/// from that probe until the next one may run.
#[tokio::test]
async fn an_engine_removed_right_after_a_recheck_refuses_a_started_agent() {
    if run_isolated(
        "an_engine_removed_right_after_a_recheck_refuses_a_started_agent",
        &[("claude", FAILS_UNTIL_FIXED)],
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
    std::fs::write(Path::new(&root).join("fixed"), "").unwrap();
    let agent = client
        .create_agent(started_claude_agent())
        .await
        .expect("the repaired CLI is probed again and accepted");
    std::fs::remove_file(Path::new(&root).join("claude")).unwrap();
    let refused = client
        .create_agent(started_claude_agent())
        .await
        .expect_err("a CLI removed since the last probe cannot run a started agent")
        .to_string();
    assert!(refused.contains("install claude"), "{refused}");
    let agents = client.list_agents(AgentQuery::default()).await.unwrap();
    assert_eq!(
        agents.iter().map(|a| a.id).collect::<Vec<_>>(),
        vec![agent.id],
        "only the first agent exists"
    );
    daemon.stop().await;
}
