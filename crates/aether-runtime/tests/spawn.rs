#![allow(clippy::expect_used, clippy::unwrap_used, clippy::panic)]

//! Spawning children from a coordinator, driven through the
//! engine bridge one tool call at a time so ordering is exact and nothing is
//! timing-dependent.

use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

use aether_core::agent::{AgentConfig, AgentStatus, AgentStatusKind, ApprovalPolicy};
use aether_core::event::Actor;
use aether_core::id::AgentId;
use aether_core::model::ModelRef;
use aether_engine::{Engine, EngineError, EngineEvent, EngineKind, EngineRun, EngineTurn};
use aether_model::Models;
use aether_runtime::{BridgeConfig, CreateAgentSpec, Runtime, RuntimeConfig};
use aether_store::{AgentFilter, SqliteStore, Store};
use aether_tools::ToolRegistry;
use serde_json::json;
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;

/// Bridge credentials lifted out of the turn's MCP args, the way `aether
/// mcp-bridge` lifts them out of its own argv.
#[derive(Debug, Clone)]
struct BridgeCredentials {
    agent: AgentId,
    token: String,
}

fn credentials_of(turn: &EngineTurn) -> BridgeCredentials {
    let args = &turn.mcp.as_ref().expect("the turn carries a bridge").args;
    let after = |flag: &str| -> String {
        let at = args
            .iter()
            .position(|a| a == flag)
            .unwrap_or_else(|| panic!("{flag} missing from {args:?}"));
        args[at + 1].clone()
    };
    BridgeCredentials {
        agent: after("--agent").parse().expect("agent id"),
        token: after("--token"),
    }
}

/// An engine that hands out its bridge credentials, then holds the turn open
/// until the runtime cancels it - like a real CLI process would be killed on
/// interrupt - so a test can drive its own tool calls through the bridge one
/// at a time and, when it wants to, interrupt the turn that is holding them.
struct HeldEngine {
    ready: mpsc::Sender<BridgeCredentials>,
}

#[async_trait::async_trait]
impl Engine for HeldEngine {
    fn kind(&self) -> EngineKind {
        EngineKind::CLAUDE_CODE
    }

    fn adapter(&self) -> String {
        "held/1".into()
    }

    fn validate_network_policy(&self, network_access: bool) -> Result<(), EngineError> {
        if network_access {
            Ok(())
        } else {
            Err(EngineError::Unsupported(
                "fixture offline dependency missing".into(),
            ))
        }
    }

    async fn run(
        &self,
        turn: EngineTurn,
        cancel: CancellationToken,
    ) -> Result<EngineRun, EngineError> {
        let credentials = credentials_of(&turn);
        let ready = self.ready.clone();
        let (events, rx) = mpsc::channel(64);
        tokio::spawn(async move {
            let _ = events
                .send(EngineEvent::Session {
                    id: format!("s-{}", credentials.agent),
                    model: None,
                    tools: vec![],
                })
                .await;
            let _ = ready.send(credentials).await;
            // Held open until interrupted or cancelled; then the events
            // channel is simply dropped, exactly like a killed process
            // closing its stdout.
            cancel.cancelled().await;
        });
        Ok(EngineRun {
            events: rx,
            input: None,
            decisions: None,
            stop: None,
        })
    }
}

struct Harness {
    runtime: Arc<Runtime>,
    store: Arc<dyn Store>,
    ready: mpsc::Receiver<BridgeCredentials>,
    _workspace: tempfile::TempDir,
    _config: tempfile::TempDir,
}

async fn harness(max_concurrent_turns: usize) -> Harness {
    harness_over(None, max_concurrent_turns, "", None).await
}

/// A harness with extra `providers.toml` text appended to the base fixture,
/// for tests that need a custom provider (e.g. an unconfigured one, or a
/// second provider sharing a model id).
async fn harness_with_providers(max_concurrent_turns: usize, extra_providers: &str) -> Harness {
    harness_over(None, max_concurrent_turns, extra_providers, None).await
}

/// A second daemon over an existing store, as a restart is - the old
/// harness's workspace stays alive so its directory outlives it, matching a
/// real restart.
async fn second_daemon_over(h: &Harness, max_concurrent_turns: usize) -> Harness {
    harness_over(Some(h.store.clone()), max_concurrent_turns, "", None).await
}

async fn harness_over(
    store: Option<Arc<dyn Store>>,
    max_concurrent_turns: usize,
    extra_providers: &str,
    delegated_workspace_root: Option<std::path::PathBuf>,
) -> Harness {
    let workspace = tempfile::tempdir().expect("tempdir");
    let workspace_root = workspace.path().canonicalize().expect("canonical");
    let config_dir = tempfile::tempdir().expect("config dir");
    std::fs::write(
        config_dir.path().join("providers.toml"),
        format!(
            "[providers.claude-code.auth]\nnone = true\n[providers.codex.auth]\nnone = true\n{extra_providers}"
        ),
    )
    .expect("fixture provider authentication");
    let store: Arc<dyn Store> =
        store.unwrap_or_else(|| Arc::new(SqliteStore::open_in_memory().expect("store")));
    let models = Arc::new(Models::load(Some(config_dir.path().to_path_buf())).expect("models"));

    let (ready_tx, ready) = mpsc::channel(16);
    let engine: Arc<dyn Engine> = Arc::new(HeldEngine { ready: ready_tx });
    let mut engines: HashMap<EngineKind, Arc<dyn Engine>> = HashMap::new();
    engines.insert(EngineKind::CLAUDE_CODE, engine);
    // The mock intentionally never exits on its own, so integration tests
    // shorten only its terminal receipt window before checking child status.
    let park_grace = delegated_workspace_root
        .as_ref()
        .map_or(aether_runtime::DEFAULT_ENGINE_PARK_GRACE, |_| {
            Duration::from_millis(50)
        });

    let runtime = Runtime::start_with_engines(
        RuntimeConfig {
            holder: "test".into(),
            max_concurrent_turns,
            lease_ttl: Duration::from_secs(30),
            default_model: Some(ModelRef::new("claude-code", "haiku")),
            workspace_root: Some(workspace_root.clone()),
            delegated_workspace_root,
            engine_park_grace: park_grace,
            bridge: Some(BridgeConfig {
                command: "/bin/true".into(),
                socket: workspace_root.join("aetherd.sock"),
            }),
            ..Default::default()
        },
        store.clone(),
        models,
        Arc::new(ToolRegistry::with_builtins()),
        None,
        engines,
    )
    .await
    .expect("runtime");

    Harness {
        runtime,
        store,
        ready,
        _workspace: workspace,
        _config: config_dir,
    }
}

fn start_coordinator(h: &Harness, concurrency: Option<usize>) -> AgentId {
    let (agent, _) = h
        .runtime
        .create_agent(
            CreateAgentSpec {
                role: "coordinator".into(),
                objective: Some("spawn some workers".into()),
                start: true,
                config: Some(AgentConfig {
                    long_lived: true,
                    concurrency,
                    approval: ApprovalPolicy::none(),
                    ..Default::default()
                }),
                ..Default::default()
            },
            Actor::user(),
        )
        .expect("create");
    agent.id
}

async fn wait_until(
    store: &dyn Store,
    id: AgentId,
    pred: impl Fn(&AgentStatus) -> bool,
) -> AgentStatus {
    let deadline = tokio::time::Instant::now() + Duration::from_secs(15);
    loop {
        let agent = store.agent(id).expect("agent").expect("exists");
        if pred(&agent.status) {
            return agent.status;
        }
        assert!(
            tokio::time::Instant::now() < deadline,
            "timed out waiting; status {:?}",
            agent.status
        );
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
}

fn children_of(store: &dyn Store, parent: AgentId) -> Vec<AgentId> {
    store
        .agents(&AgentFilter {
            parent: Some(parent),
            ..Default::default()
        })
        .expect("agents")
        .into_iter()
        .map(|a| a.id)
        .collect()
}

/// Delegate one child through the bridge, the way the coordinator's own
/// engine would, and return its id - visible the instant this call returns,
/// since `create_agent` commits synchronously before the reply is sent.
async fn delegate(h: &Harness, bridge: &BridgeCredentials, objective: &str) -> (AgentId, String) {
    let before = children_of(h.store.as_ref(), bridge.agent);
    let (reply, is_error) = h
        .runtime
        .engine_tool(
            bridge.agent,
            &bridge.token,
            "delegate".into(),
            json!({"model":"claude-code/haiku", "effort":"low", "role": "worker", "objective": objective, "wait": false}),
        )
        .await
        .expect("answered");
    assert!(!is_error, "{reply}");
    let after = children_of(h.store.as_ref(), bridge.agent);
    let new = after
        .into_iter()
        .find(|id| !before.contains(id))
        .unwrap_or_else(|| panic!("delegate did not create a child: {reply}"));
    (new, reply)
}

#[path = "spawn/validation.rs"]
mod validation;

#[path = "spawn/lifecycle.rs"]
mod lifecycle;

#[path = "spawn/workspace_integration.rs"]
mod workspace_integration;

#[path = "spawn/idempotent.rs"]
mod idempotent;

#[path = "spawn/readers.rs"]
mod readers;
