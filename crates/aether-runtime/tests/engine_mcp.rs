#![allow(clippy::expect_used, clippy::unwrap_used, clippy::panic)]

//! The engine-side tool surface: what the MCP bridge is told it may use, what
//! happens when it calls one, and what class a call the engine ran itself is
//! recorded with.
//!
//! Driven by a scripted engine that reads the per-turn token out of its
//! `EngineTurn` exactly as the real `aether mcp-bridge` reads it from argv,
//! then speaks to the runtime through the same two entry points the bridge
//! uses. No CLI, no real engine, and the MCP server is the in-repo mock.

use std::collections::{BTreeMap, HashMap};
use std::sync::Arc;
use std::time::Duration;

use aether_core::agent::{AgentConfig, AgentStatusKind, ApprovalPolicy, NetworkPolicy, Scope};
use aether_core::event::{Actor, EventKind};
use aether_core::id::AgentId;
use aether_core::mcp::McpServerState;
use aether_core::model::{ContentBlock, ModelRef};
use aether_core::tool::{ApprovalDecision, SideEffectClass};
use aether_engine::{
    Engine, EngineError, EngineEvent, EngineKind, EngineOutcome, EngineRun, EngineTurn,
};
use aether_model::Models;
use aether_runtime::{BridgeConfig, CreateAgentSpec, Runtime, RuntimeConfig};
use aether_store::{EventFilter, SqliteStore, Store};
use aether_tools::ToolRegistry;
use aether_tools::mcp::{McpManager, McpServerSpec, McpTransportSpec};
use serde_json::json;
use tokio::sync::{mpsc, oneshot};
use tokio_util::sync::CancellationToken;

const MOCK: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../../scripts/mock_mcp_server.sh"
);

/// What the bridge needs to reach the running turn, lifted out of the turn
/// the way `aether mcp-bridge` lifts it out of its own argv.
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

/// An engine that hands its bridge credentials to the test, records one tool
/// call it made in its own process, and waits to be told the turn may end.
struct BridgeEngine {
    ready: mpsc::Sender<BridgeCredentials>,
    finish: Arc<std::sync::Mutex<Option<oneshot::Receiver<()>>>>,
}

#[async_trait::async_trait]
impl Engine for BridgeEngine {
    fn kind(&self) -> EngineKind {
        EngineKind::CLAUDE_CODE
    }

    fn adapter(&self) -> String {
        "scripted/1".into()
    }

    async fn run(
        &self,
        turn: EngineTurn,
        _cancel: CancellationToken,
    ) -> Result<EngineRun, EngineError> {
        let credentials = credentials_of(&turn);
        let ready = self.ready.clone();
        let finish = self.finish.lock().expect("lock").take();
        let (events, rx) = mpsc::channel(64);
        tokio::spawn(async move {
            let _ = events
                .send(EngineEvent::Session {
                    id: "s1".into(),
                    model: None,
                    tools: vec![],
                })
                .await;
            // A call the engine ran itself, named the dotted way the contract
            // offers alongside the underscored one.
            let _ = events
                .send(EngineEvent::Assistant {
                    content: vec![ContentBlock::ToolUse {
                        id: "call-1".into(),
                        name: "mcp__aether.write_file".into(),
                        input: json!({"file_path": "note.txt", "content": "hi"}),
                    }],
                    usage: None,
                    model: None,
                })
                .await;
            let _ = ready.send(credentials).await;
            if let Some(finish) = finish {
                let _ = finish.await;
            }
            let _ = events
                .send(EngineEvent::Done(EngineOutcome {
                    text: Some("done".into()),
                    ..Default::default()
                }))
                .await;
        });
        Ok(EngineRun {
            events: rx,
            input: None,
            decisions: None,
            stop: None,
        })
    }
}

/// An engine that hands out its bridge credentials and then panics inside
/// the turn, which is the one exit path a hand-written cleanup misses.
struct PanicEngine {
    ready: mpsc::UnboundedSender<BridgeCredentials>,
}

#[async_trait::async_trait]
impl Engine for PanicEngine {
    fn kind(&self) -> EngineKind {
        EngineKind::CLAUDE_CODE
    }

    fn adapter(&self) -> String {
        "panicking/1".into()
    }

    async fn run(
        &self,
        turn: EngineTurn,
        _cancel: CancellationToken,
    ) -> Result<EngineRun, EngineError> {
        let _ = self.ready.send(credentials_of(&turn));
        panic!("the engine turn panicked");
    }
}

struct Harness {
    runtime: Arc<Runtime>,
    store: Arc<dyn Store>,
    ready: mpsc::Receiver<BridgeCredentials>,
    finish: Option<oneshot::Sender<()>>,
    _workspace: tempfile::TempDir,
}

async fn harness() -> Harness {
    harness_with(None).await
}

/// The same harness, optionally with an engine that panics as soon as the
/// turn hands out its bridge credentials.
async fn harness_with(
    panic_credentials: Option<mpsc::UnboundedSender<BridgeCredentials>>,
) -> Harness {
    let manager = McpManager::new(
        vec![McpServerSpec {
            name: "mock".into(),
            transport: McpTransportSpec::Stdio {
                command: "bash".into(),
                args: vec![MOCK.to_owned()],
                env: BTreeMap::new(),
                cwd: None,
            },
            // `write_note` stays unclassified, so it is `External` and the
            // default policy holds it for a person.
            classes: BTreeMap::from([
                ("read_note".to_owned(), SideEffectClass::ReadOnly),
                ("echo_request".to_owned(), SideEffectClass::ReadOnly),
            ]),
            send_caller_meta: true,
            auth: None,
        }],
        None,
        Vec::new().into(),
    );
    manager.discover_all().await;
    assert_eq!(manager.health()[0].state, McpServerState::Ready);

    let workspace = tempfile::tempdir().expect("tempdir");
    let workspace_root = workspace.path().canonicalize().expect("canonical");
    let config_dir = tempfile::tempdir().expect("config dir");
    let store: Arc<dyn Store> = Arc::new(SqliteStore::open_in_memory().expect("store"));
    let models = Arc::new(Models::load(Some(config_dir.path().to_path_buf())).expect("models"));
    std::mem::forget(config_dir);

    let (ready_tx, ready) = mpsc::channel(4);
    let (finish_tx, finish_rx) = oneshot::channel();
    let engine: Arc<dyn Engine> = Arc::new(BridgeEngine {
        ready: ready_tx,
        finish: Arc::new(std::sync::Mutex::new(Some(finish_rx))),
    });
    let engine: Arc<dyn Engine> = match panic_credentials {
        Some(ready) => Arc::new(PanicEngine { ready }),
        None => engine,
    };
    let mut engines: HashMap<EngineKind, Arc<dyn Engine>> = HashMap::new();
    engines.insert(EngineKind::CLAUDE_CODE, engine);

    let runtime = Runtime::start_with_engines(
        RuntimeConfig {
            holder: "test".into(),
            max_concurrent_turns: 2,
            lease_ttl: Duration::from_secs(30),
            default_model: Some(ModelRef::new("claude-code", "haiku")),
            workspace_root: Some(workspace_root.clone()),
            bridge: Some(BridgeConfig {
                command: "/bin/true".into(),
                socket: workspace_root.join("aetherd.sock"),
            }),
            ..Default::default()
        },
        store.clone(),
        models,
        Arc::new(ToolRegistry::with_builtins().with_mcp(manager)),
        None,
        engines,
    )
    .await
    .expect("runtime");

    Harness {
        runtime,
        store,
        ready,
        finish: Some(finish_tx),
        _workspace: workspace,
    }
}

/// An agent whose scope allows the network, which every MCP server needs.
fn start_agent(h: &Harness) -> AgentId {
    let (agent, _) = h
        .runtime
        .create_agent(
            CreateAgentSpec {
                role: "worker".into(),
                objective: Some("use the note server".into()),
                start: true,
                scope: Some(Scope {
                    network: NetworkPolicy::Allowed,
                    ..Default::default()
                }),
                config: Some(AgentConfig {
                    long_lived: true,
                    ..Default::default()
                }),
                // The gate is what this file tests, so it is asked for
                // outright rather than left to the shipped default (yolo).
                approval: Some(ApprovalPolicy::default()),
                ..Default::default()
            },
            Actor::user(),
        )
        .expect("create");
    agent.id
}

async fn wait_for_approval(store: &dyn Store) -> aether_core::tool::ApprovalRequest {
    let deadline = tokio::time::Instant::now() + Duration::from_secs(15);
    loop {
        if let Some(pending) = store.pending_approvals().expect("pending").first() {
            return pending.clone();
        }
        assert!(
            tokio::time::Instant::now() < deadline,
            "no approval was ever raised"
        );
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
}

#[tokio::test]
async fn the_bridge_is_offered_the_harness_and_mcp_tools_and_nothing_else() {
    let mut h = harness().await;
    let agent = start_agent(&h);
    let bridge = h.ready.recv().await.expect("the turn started");
    assert_eq!(bridge.agent, agent);

    let offered: Vec<String> = h
        .runtime
        .engine_tools(bridge.agent, &bridge.token)
        .expect("the bridge may ask")
        .into_iter()
        .map(|s| s.name)
        .collect();
    // Every harness tool an engine's agent steers itself with, plus the
    // MCP surface. Losing one silently strands the agent mid-turn.
    for expected in [
        "provider_usage",
        "refresh_provider_usage",
        "list_agents",
        "list_tasks",
        "read_task",
        "complete_task",
        "fail_task",
        "delegate",
        "wait_for",
        "send_message",
        "ask_user",
        "remember",
        "mcp__mock__read_note",
    ] {
        assert!(offered.contains(&expected.to_owned()), "{offered:?}");
    }
    // The engine has its own shell and file tools; Aether does not offer a
    // second set of them.
    for absent in ["bash", "read_file", "web_fetch"] {
        assert!(!offered.contains(&absent.to_owned()), "{offered:?}");
    }

    let mut altered = bridge.token.clone();
    let last = altered.pop().expect("nonempty token");
    altered.push(if last == '0' { '1' } else { '0' });
    assert_eq!(altered.len(), 64);
    assert!(h.runtime.engine_tools(bridge.agent, &altered).is_err());

    // A stale token cannot reach the turn at all.
    assert!(
        h.runtime
            .engine_tools(bridge.agent, "not-the-token")
            .is_err(),
        "the per-turn token is what authenticates the bridge"
    );

    if let Some(finish) = h.finish.take() {
        let _ = finish.send(());
    }
}

#[tokio::test]
async fn a_bridge_call_passes_the_same_gate_an_api_call_would() {
    let mut h = harness().await;
    let agent = start_agent(&h);
    let bridge = h.ready.recv().await.expect("the turn started");

    // Naming a tool the bridge never advertised does not reach it, and says
    // why rather than pretending the tool does not exist.
    let (reply, is_error) = h
        .runtime
        .engine_tool(
            bridge.agent,
            &bridge.token,
            "bash".into(),
            json!({"command": "true"}),
        )
        .await
        .expect("answered");
    assert!(is_error, "{reply}");
    assert!(reply.contains("bridge"), "{reply}");

    // A read-only MCP tool the policy does not gate runs straight through.
    let (reply, is_error) = h
        .runtime
        .engine_tool(
            bridge.agent,
            &bridge.token,
            "mcp__mock__read_note".into(),
            json!({"key": "k"}),
        )
        .await
        .expect("answered");
    assert!(!is_error, "{reply}");
    assert_eq!(reply, "note contents");

    // An unclassified one is `External`: a person is asked, with the call's
    // own argument and resources on the card, and a denial reaches the model.
    let calling = {
        let runtime = h.runtime.clone();
        let bridge = bridge.clone();
        tokio::spawn(async move {
            runtime
                .engine_tool(
                    bridge.agent,
                    &bridge.token,
                    "mcp__mock__write_note".into(),
                    json!({"key": "k", "text": "hi"}),
                )
                .await
        })
    };
    let pending = wait_for_approval(h.store.as_ref()).await;
    assert_eq!(pending.tool, "mcp__mock__write_note");
    assert_eq!(pending.side_effect, SideEffectClass::External);
    assert_eq!(
        pending.argument.as_deref(),
        Some(json!({"key": "k", "text": "hi"}).to_string().as_str()),
        "the card carries what a remembered rule would match"
    );
    assert!(
        pending
            .resources
            .contains(&aether_core::tool::ResourceKey("mcp:mock".into())),
        "{:?}",
        pending.resources
    );

    h.runtime
        .resolve_approval(
            pending.id,
            ApprovalDecision::Denied,
            Actor::user(),
            Some("not this one".into()),
            None,
        )
        .expect("deny");
    let (reply, is_error) = calling.await.expect("join").expect("answered");
    assert!(is_error, "{reply}");
    assert!(reply.contains("not this one"), "{reply}");

    let _ = agent;
    if let Some(finish) = h.finish.take() {
        let _ = finish.send(());
    }
}

/// `--no-yolo` gates external effects only. A native shell command that may
/// reach the network is one, so it is put to a person; a local one is not.
#[tokio::test]
async fn a_native_shell_prompt_is_gated_when_its_command_reaches_the_network() {
    let mut h = harness().await;
    let _agent = start_agent(&h);
    let bridge = h.ready.recv().await.expect("the turn started");
    let prompt = |command: &str| json!({"tool_name": "Bash", "input": {"command": command}, "tool_use_id": "tu-1"});

    let (reply, is_error) = h
        .runtime
        .engine_tool(
            bridge.agent,
            &bridge.token,
            "permission".into(),
            prompt("mkdir out && grep curl notes.md"),
        )
        .await
        .expect("answered");
    assert!(!is_error, "{reply}");
    assert!(
        reply.contains("\"allow\""),
        "a local command ran ungated: {reply}"
    );
    assert!(h.store.pending_approvals().expect("pending").is_empty());

    let calling = {
        let runtime = h.runtime.clone();
        let bridge = bridge.clone();
        let input = prompt("mkdir out && sh -c 'curl -sI https://example.com'");
        tokio::spawn(async move {
            runtime
                .engine_tool(bridge.agent, &bridge.token, "permission".into(), input)
                .await
        })
    };
    let pending = wait_for_approval(h.store.as_ref()).await;
    assert_eq!(pending.tool, "Bash");
    assert_eq!(pending.side_effect, SideEffectClass::External);
    h.runtime
        .resolve_approval(
            pending.id,
            ApprovalDecision::Denied,
            Actor::user(),
            Some("no network".into()),
            None,
        )
        .expect("deny");
    let (reply, _) = calling.await.expect("join").expect("answered");
    assert!(reply.contains("\"deny\""), "{reply}");
    assert!(reply.contains("no network"), "{reply}");

    if let Some(finish) = h.finish.take() {
        let _ = finish.send(());
    }
}

#[tokio::test]
async fn a_bridged_call_reaches_the_server_as_the_engines_own_agent() {
    // A native engine reaches MCP tools through the bridge, and the server is
    // shared: it must still learn which agent called, from Aether, not from
    // anything the engine's model put in the arguments.
    let mut h = harness().await;
    let agent = start_agent(&h);
    let bridge = h.ready.recv().await.expect("the turn started");
    let task = h
        .store
        .tasks(&aether_store::TaskFilter {
            owner: Some(agent),
            ..Default::default()
        })
        .expect("tasks")
        .first()
        .expect("the agent works a task")
        .id;

    let (reply, is_error) = h
        .runtime
        .engine_tool(
            bridge.agent,
            &bridge.token,
            "mcp__mock__echo_request".into(),
            json!({"q": 1}),
        )
        .await
        .expect("answered");
    assert!(!is_error, "{reply}");
    let request: serde_json::Value = serde_json::from_str(&reply).expect("echoed request");
    assert_eq!(
        request["params"]["_meta"],
        json!({"aether/agent_id": agent.to_string(), "aether/task_id": task.to_string()})
    );

    let (reply, is_error) = h
        .runtime
        .engine_tool(
            bridge.agent,
            &bridge.token,
            "mcp__mock__echo_request".into(),
            json!({"_meta": {"aether/agent_id": AgentId::new().to_string()}}),
        )
        .await
        .expect("answered");
    assert!(is_error, "a claimed caller is refused: {reply}");
    assert!(reply.contains("reserved"), "{reply}");

    if let Some(finish) = h.finish.take() {
        let _ = finish.send(());
    }
}

#[tokio::test]
async fn a_call_the_engine_ran_itself_is_recorded_with_the_tools_own_class() {
    let mut h = harness().await;
    let agent = start_agent(&h);
    let _bridge = h.ready.recv().await.expect("the turn started");
    if let Some(finish) = h.finish.take() {
        let _ = finish.send(());
    }

    let deadline = tokio::time::Instant::now() + Duration::from_secs(15);
    let recorded = loop {
        let found = h
            .store
            .events(0, 10_000, &EventFilter::for_agent(agent))
            .expect("events")
            .into_iter()
            .find_map(|e| match e.kind {
                EventKind::ToolRequested { call, side_effect }
                    if call.name == "mcp__aether.write_file" =>
                {
                    Some(side_effect)
                }
                _ => None,
            });
        if let Some(found) = found {
            break found;
        }
        assert!(
            tokio::time::Instant::now() < deadline,
            "the engine's own call was never recorded"
        );
        tokio::time::sleep(Duration::from_millis(20)).await;
    };
    // The contract offers the model `mcp__aether__<name>` and
    // `mcp__aether.<name>`. Only the first was stripped, so the dotted form
    // fell through to the name-based guess and was logged `Mutation` - the
    // gate and the log disagreeing for exactly the form they exist for.
    assert_eq!(
        recorded,
        SideEffectClass::IdempotentMutation,
        "the dotted form must resolve to write_file's own class"
    );
    assert_ne!(
        recorded,
        aether_engine::side_effect_of("mcp__aether.write_file"),
        "and not to what the name alone would suggest"
    );

    let status = h
        .store
        .agent(agent)
        .expect("agent")
        .expect("exists")
        .status
        .kind();
    assert_ne!(status, AgentStatusKind::Failed);
}

#[tokio::test]
async fn a_panicking_turn_leaves_no_live_per_turn_token() {
    // The token authenticates on the MCP HTTP endpoint, so one left behind by
    // a panic keeps working until the agent's next turn overwrites it.
    // Removal is a drop guard, not a line on each exit path.
    let (ready_tx, mut ready) = mpsc::unbounded_channel();
    let h = harness_with(Some(ready_tx)).await;
    let _agent = start_agent(&h);
    let bridge = ready.recv().await.expect("the turn started");

    let deadline = tokio::time::Instant::now() + Duration::from_secs(15);
    loop {
        if h.runtime.engine_tools(bridge.agent, &bridge.token).is_err() {
            break;
        }
        assert!(
            tokio::time::Instant::now() < deadline,
            "the per-turn token still authenticates after the turn panicked"
        );
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
}

#[tokio::test]
async fn the_bridge_can_inspect_its_assigned_work_without_approval() {
    let mut h = harness().await;
    let agent = start_agent(&h);
    let bridge = h.ready.recv().await.expect("turn started");
    let (reply, failed) = h
        .runtime
        .engine_tool(agent, &bridge.token, "list_agents".into(), json!({}))
        .await
        .unwrap();
    assert!(!failed, "{reply}");
    let agents: serde_json::Value = serde_json::from_str(&reply).unwrap();
    assert_eq!(agents["agents"].as_array().unwrap().len(), 1);
    assert_eq!(agents["agents"][0]["id"], json!(agent));
    let (reply, failed) = h
        .runtime
        .engine_tool(
            agent,
            &bridge.token,
            "list_tasks".into(),
            json!({"limit":1}),
        )
        .await
        .unwrap();
    assert!(!failed, "{reply}");
    let tasks: serde_json::Value = serde_json::from_str(&reply).unwrap();
    let task = tasks["tasks"][0]["id"].clone();
    assert!(task.is_string(), "{tasks}");
    let (reply, failed) = h
        .runtime
        .engine_tool(
            agent,
            &bridge.token,
            "read_task".into(),
            json!({"task":task,"max_chars":8192}),
        )
        .await
        .unwrap();
    assert!(!failed, "{reply}");
    let page: serde_json::Value = serde_json::from_str(&reply).unwrap();
    assert!(page["next_offset"].is_null());
    let record: aether_core::task::Task =
        serde_json::from_str(page["text"].as_str().unwrap()).unwrap();
    assert_eq!(record.owner, Some(agent));
    assert_eq!(record.objective, "use the note server");
    assert!(h.store.pending_approvals().unwrap().is_empty());
    if let Some(finish) = h.finish.take() {
        finish.send(()).unwrap();
    }
    h.runtime.shutdown().await;
}

#[tokio::test]
async fn provider_usage_bridge_tool_returns_unknown_enabled_providers_read_only() {
    let mut h = harness().await;
    let agent = start_agent(&h);
    let bridge = tokio::time::timeout(Duration::from_secs(5), h.ready.recv())
        .await
        .unwrap()
        .unwrap();
    let (reply, is_error) = tokio::time::timeout(
        Duration::from_secs(5),
        h.runtime
            .engine_tool(agent, &bridge.token, "provider_usage".into(), json!({})),
    )
    .await
    .unwrap()
    .unwrap();
    assert!(!is_error, "{reply}");
    let reply: serde_json::Value = serde_json::from_str(&reply).unwrap();
    let rows = reply["providers"].as_array().unwrap();
    let registry = aether_model::registry::Registry::builtin();
    let expected: Vec<_> = registry
        .providers()
        .iter()
        .filter(|p| p.enabled)
        .map(|p| p.id.as_str())
        .collect();
    let actual: Vec<_> = rows
        .iter()
        .map(|row| row["provider"].as_str().unwrap())
        .collect();
    assert_eq!(actual, expected);
    for row in rows {
        assert_eq!(row["headroom"]["windows"], json!([]));
        assert_eq!(row["headroom"]["stale"], true);
        assert_eq!(row["headroom"]["observed_at"], serde_json::Value::Null);
    }
    assert!(h.store.pending_approvals().unwrap().is_empty());
    h.finish.take().unwrap().send(()).unwrap();
    h.runtime.shutdown().await;
}
