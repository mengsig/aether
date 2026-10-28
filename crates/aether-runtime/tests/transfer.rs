#![allow(clippy::expect_used, clippy::unwrap_used, clippy::panic)]

//! Moving a run between two daemons: export it from one store, import it
//! into another, and hold the two reports of it to the same body.

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use aether_core::agent::{AgentConfig, AgentStatus, ApprovalPolicy};
use aether_core::event::Actor;
use aether_core::id::{AgentId, RunId};
use aether_core::model::ModelRef;
use aether_core::report;
use aether_core::time::now;
use aether_model::Models;
use aether_model::testing::ScriptedClient;
use aether_runtime::transfer;
use aether_runtime::{AgentUpdate, CreateAgentSpec, CreateTaskSpec, Runtime, RuntimeConfig};
use aether_store::{AgentFilter, EventFilter, SqliteStore, Store};
use aether_tools::ToolRegistry;
use serde_json::json;

struct Daemon {
    runtime: Arc<Runtime>,
    store: Arc<dyn Store>,
    _workspace: tempfile::TempDir,
}

/// One daemon over its own store, driven by a scripted model.
async fn daemon(responses: Vec<aether_model::testing::ScriptedResponse>) -> Daemon {
    let workspace = tempfile::tempdir().expect("workspace");
    let config_dir = tempfile::tempdir().expect("config dir");
    let store: Arc<dyn Store> = Arc::new(SqliteStore::open_in_memory().expect("store"));
    let models = Arc::new(Models::load(Some(config_dir.path().to_path_buf())).expect("models"));
    std::mem::forget(config_dir);
    let runtime = Runtime::start_with(
        RuntimeConfig {
            holder: "transfer-test".into(),
            max_concurrent_turns: 1,
            lease_ttl: Duration::from_secs(30),
            default_model: Some(ModelRef::new("anthropic", "claude-sonnet-5-5")),
            workspace_root: Some(workspace.path().canonicalize().expect("canonical")),
            ..Default::default()
        },
        store.clone(),
        models,
        Arc::new(ToolRegistry::with_builtins()),
        Some(Arc::new(ScriptedClient::with_responses(responses))),
    )
    .await
    .expect("runtime");
    Daemon {
        runtime,
        store,
        _workspace: workspace,
    }
}

/// A second daemon over a store that already has rows in it - which is what
/// a restart is, and the moment recovery runs.
async fn restart_over(store: Arc<dyn Store>) -> Daemon {
    let workspace = tempfile::tempdir().expect("workspace");
    let config_dir = tempfile::tempdir().expect("config dir");
    let models = Arc::new(Models::load(Some(config_dir.path().to_path_buf())).expect("models"));
    std::mem::forget(config_dir);
    let runtime = Runtime::start_with(
        RuntimeConfig {
            holder: "transfer-test-restart".into(),
            max_concurrent_turns: 1,
            lease_ttl: Duration::from_secs(30),
            default_model: Some(ModelRef::new("anthropic", "claude-sonnet-5-5")),
            workspace_root: Some(workspace.path().canonicalize().expect("canonical")),
            ..Default::default()
        },
        store.clone(),
        models,
        Arc::new(ToolRegistry::with_builtins()),
        Some(Arc::new(ScriptedClient::with_responses(Vec::new()))),
    )
    .await
    .expect("runtime");
    Daemon {
        runtime,
        store,
        _workspace: workspace,
    }
}

async fn wait_terminal(store: &dyn Store, id: AgentId) -> AgentStatus {
    let deadline = tokio::time::Instant::now() + Duration::from_secs(15);
    loop {
        let agent = store.agent(id).expect("agent").expect("exists");
        if agent.status.is_terminal() {
            return agent.status;
        }
        assert!(
            tokio::time::Instant::now() < deadline,
            "timed out; status {:?}",
            agent.status
        );
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
}

/// A run whose tool output is past the 30 000-char inline bound, so it
/// produces an artifact with real bytes for the export to carry.
async fn finished_run() -> (Daemon, RunId) {
    let source = daemon(vec![
        ScriptedClient::tool_call(
            "bash",
            json!({"command": "for i in $(seq 1 4000); do echo \"line $i of the log\"; done"}),
        ),
        ScriptedClient::tool_call("complete_task", json!({"summary": "read the log"})),
    ])
    .await;
    let (agent, _) = source
        .runtime
        .create_agent(
            CreateAgentSpec {
                role: "worker".into(),
                objective: Some("read the log".into()),
                start: true,
                config: Some(AgentConfig {
                    approval: ApprovalPolicy::none(),
                    ..Default::default()
                }),
                ..Default::default()
            },
            Actor::user(),
        )
        .expect("create");
    wait_terminal(source.store.as_ref(), agent.id).await;
    let run = source
        .store
        .agent(agent.id)
        .expect("agent")
        .expect("exists")
        .run;
    (source, run)
}

fn report_of(store: &dyn Store, run: RunId) -> report::RunReport {
    let events = store
        .events(0, 100_000, &EventFilter::for_run(run))
        .expect("events");
    report::build(run, &events, now())
}

/// The import is the half that runs against a file it did not write, so the
/// refusal has to hold for a file that says whatever it likes. This one is
/// rewritten to carry the worst plausible agent: `ready`, unpaused, no
/// approval required for any class, every tool exposed, network open and the
/// whole filesystem as its workspace.
fn forge_live_agents(file: &std::path::Path) {
    let text = std::fs::read_to_string(file).expect("read export");
    let mut out = String::new();
    let mut forged = 0usize;
    for line in text.lines() {
        let mut record: serde_json::Value = serde_json::from_str(line).expect("record");
        if record.get("record").and_then(|r| r.as_str()) == Some("agent") {
            record["status"] = json!({"state": "ready"});
            record["paused"] = json!(false);
            record["config"]["approval"] = json!({"require_for": []});
            record["config"]["tools"] = json!({"mode": "all"});
            record["scope"]["network"] = json!("allowed");
            record["scope"]["workspace_root"] = json!("/");
            forged += 1;
        }
        out.push_str(&serde_json::to_string(&record).expect("line"));
        out.push('\n');
    }
    assert!(forged > 0, "the export carried no agent to forge");
    std::fs::write(file, out).expect("write export");
}

/// Every run the store can name, the way `runs list` groups them.
fn aether_daemon_runs(store: &dyn Store) -> Vec<RunId> {
    let mut runs: Vec<RunId> = store
        .agents(&AgentFilter::default())
        .expect("agents")
        .into_iter()
        .map(|a| a.run)
        .collect();
    runs.sort();
    runs.dedup();
    runs
}

/// A run whose approval was answered by a remembered rule, so the log carries
/// `Actor::Rule` and the report has to survive the transfer with it.
///
/// The round-trip fixture runs with `ApprovalPolicy::none()` and the report
/// unit fixture's one approval is answered by `user`, so nothing else here
/// exercises the actor that names a rule by id.
async fn run_answered_by_a_rule() -> (Daemon, RunId) {
    let source = daemon(vec![
        ScriptedClient::tool_call("write_file", json!({"path": "a.txt", "content": "one"})),
        ScriptedClient::tool_call("write_file", json!({"path": "a.txt", "content": "two"})),
        ScriptedClient::tool_call("complete_task", json!({"summary": "wrote it twice"})),
    ])
    .await;
    let (agent, _) = source
        .runtime
        .create_agent(
            CreateAgentSpec {
                role: "worker".into(),
                objective: Some("write a note".into()),
                start: true,
                config: Some(AgentConfig {
                    approval: ApprovalPolicy {
                        require_for: [aether_core::tool::SideEffectClass::IdempotentMutation]
                            .into(),
                    },
                    ..Default::default()
                }),
                ..Default::default()
            },
            Actor::user(),
        )
        .expect("create");

    // Answer the first ask and remember it; the second call is then answered
    // by the rule, which is the actor this test exists for.
    let deadline = tokio::time::Instant::now() + Duration::from_secs(15);
    let mut remembered = false;
    loop {
        let pending = source.store.pending_approvals().expect("pending");
        if let Some(first) = pending.first()
            && !remembered
        {
            source
                .runtime
                .resolve_approval(
                    first.id,
                    aether_core::tool::ApprovalDecision::Approved,
                    Actor::user(),
                    None,
                    Some(aether_runtime::Remember::narrow(None)),
                )
                .expect("approve and remember");
            remembered = true;
        }
        let agent = source
            .store
            .agent(agent.id)
            .expect("agent")
            .expect("exists");
        if agent.status.is_terminal() {
            break;
        }
        assert!(
            tokio::time::Instant::now() < deadline,
            "timed out; status {:?}, remembered {remembered}",
            agent.status
        );
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    assert!(remembered, "no approval was ever asked for");
    let run = source
        .store
        .agent(agent.id)
        .expect("agent")
        .expect("exists")
        .run;
    (source, run)
}

#[path = "transfer/roundtrip.rs"]
mod roundtrip;

#[path = "transfer/isolation.rs"]
mod isolation;

#[path = "transfer/integrity.rs"]
mod integrity;

#[path = "transfer/privacy.rs"]
mod privacy;

#[path = "transfer/sessions.rs"]
mod sessions;

#[path = "transfer/schedules.rs"]
mod schedules;

#[path = "transfer/artifacts.rs"]
mod artifacts;

#[path = "transfer/known_secrets.rs"]
mod known_secrets;
