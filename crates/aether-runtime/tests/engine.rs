#![allow(clippy::expect_used, clippy::unwrap_used, clippy::panic)]

//! Engine-path semantics against a scripted engine, no CLI involved: a
//! message sent during a running turn is handed over, recorded consumed only
//! once the engine acknowledges it, and kept for the next turn when the
//! engine refuses it.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use aether_core::agent::{
    AgentConfig, AgentStatus, AgentStatusKind, ApprovalPolicy, BlockReason, ExhaustionClass,
};
use aether_core::event::{Actor, EventKind};
use aether_core::id::AgentId;
use aether_core::model::{ContentBlock, Effort, MessageRole, ModelRef};
use aether_engine::{
    Engine, EngineError, EngineEvent, EngineKind, EngineOutcome, EngineRun, EngineTurn,
};
use aether_model::Models;
use aether_runtime::{BridgeConfig, CreateAgentSpec, Runtime, RuntimeConfig};
use aether_store::{EventFilter, SqliteStore, Store};
use aether_tools::ToolRegistry;
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;

use scripted::{Reply, ScriptedEngine};

#[path = "engine/attachments.rs"]
mod attachments;
#[path = "engine/bridge_build.rs"]
mod bridge_build;
#[path = "engine/budgets.rs"]
mod budgets;
#[path = "engine/build_cache.rs"]
mod build_cache;
#[path = "engine/compaction.rs"]
mod compaction;
#[path = "engine/completion_recovery.rs"]
mod completion_recovery;
#[path = "engine/completion_waits.rs"]
mod completion_waits;
#[path = "engine/cost_gap.rs"]
mod cost_gap;
#[path = "engine/effort.rs"]
mod effort;
#[path = "engine/exhaustion.rs"]
mod exhaustion;
#[path = "engine/extended_context.rs"]
mod extended_context;
#[path = "engine/handoff.rs"]
mod handoff;
#[path = "engine/interruptions.rs"]
mod interruptions;
#[path = "engine/launch_context.rs"]
mod launch_context;
#[path = "engine/memory.rs"]
mod memory;
#[path = "engine/native_cache_ttl_pricing.rs"]
mod native_cache_ttl_pricing;
#[path = "engine/native_compaction.rs"]
mod native_compaction;
#[path = "engine/native_completion.rs"]
mod native_completion;
#[path = "engine/native_input.rs"]
mod native_input;
#[path = "engine/native_request_pricing.rs"]
mod native_request_pricing;
#[path = "engine/network_retry.rs"]
mod network_retry;
#[path = "engine/network_retry_safety.rs"]
mod network_retry_safety;
#[path = "engine/prefix.rs"]
mod prefix;
#[path = "engine/pricing.rs"]
mod pricing;
#[path = "engine/questions.rs"]
mod questions;
#[path = "engine/receipt_parity.rs"]
mod receipt_parity;
#[path = "engine/scripted.rs"]
mod scripted;
#[path = "engine/session_recovery.rs"]
mod session_recovery;
#[path = "engine/snapshots.rs"]
mod snapshots;
#[path = "engine/spawn_guard.rs"]
mod spawn_guard;
#[path = "engine/steering.rs"]
mod steering;
#[path = "engine/telemetry.rs"]
mod telemetry;
#[path = "engine/temp_dir.rs"]
mod temp_dir;
#[path = "engine/tool_surface.rs"]
mod tool_surface;
#[path = "engine/transient_retry.rs"]
mod transient_retry;
#[path = "engine/usage.rs"]
mod usage;
#[path = "engine/usage_coalescing.rs"]
mod usage_coalescing;
#[path = "engine/usage_replay.rs"]
mod usage_replay;

struct Harness {
    runtime: Arc<Runtime>,
    store: Arc<dyn Store>,
    prompts: Arc<Mutex<Vec<String>>>,
    systems: Arc<Mutex<Vec<String>>>,
    _workspace: tempfile::TempDir,
}

async fn harness(reply: Reply) -> Harness {
    harness_priced(reply, Vec::new()).await
}

async fn harness_priced(reply: Reply, costs: Vec<Option<f64>>) -> Harness {
    let prompts = Arc::new(Mutex::new(Vec::new()));
    let systems = Arc::new(Mutex::new(Vec::new()));
    let engine: Arc<dyn Engine> = Arc::new(ScriptedEngine {
        reply,
        prompts: prompts.clone(),
        costs: Arc::new(Mutex::new(costs.into())),
        systems: systems.clone(),
        tool_gate: None,
    });
    harness_with_engine(engine, prompts, systems).await
}

async fn harness_with_engine(
    engine: Arc<dyn Engine>,
    prompts: Arc<Mutex<Vec<String>>>,
    systems: Arc<Mutex<Vec<String>>>,
) -> Harness {
    harness_with_engine_and_providers_toml(engine, prompts, systems, None).await
}

/// Like `harness_with_engine`, but with a `providers.toml` seeded before
/// `Models::load` reads it - for a registry model whose capabilities the
/// builtin catalog never ships (the `effort = false` gate).
async fn harness_with_engine_and_providers_toml(
    engine: Arc<dyn Engine>,
    prompts: Arc<Mutex<Vec<String>>>,
    systems: Arc<Mutex<Vec<String>>>,
    providers_toml: Option<&str>,
) -> Harness {
    build_harness(engine, prompts, systems, providers_toml, None, None, |_| {}).await
}

/// Like `harness_with_engine`, with the runtime config adjusted first.
async fn harness_with_engine_config(
    engine: Arc<dyn Engine>,
    configure: impl FnOnce(&mut RuntimeConfig),
) -> Harness {
    build_harness(
        engine,
        Arc::default(),
        Arc::default(),
        None,
        None,
        None,
        configure,
    )
    .await
}

async fn build_harness(
    engine: Arc<dyn Engine>,
    prompts: Arc<Mutex<Vec<String>>>,
    systems: Arc<Mutex<Vec<String>>>,
    providers_toml: Option<&str>,
    config_toml: Option<&str>,
    store: Option<Arc<dyn Store>>,
    configure: impl FnOnce(&mut RuntimeConfig),
) -> Harness {
    let workspace = tempfile::tempdir().expect("tempdir");
    let workspace_root = workspace.path().canonicalize().expect("canonical");
    let config_dir = tempfile::tempdir().expect("config dir");
    if let Some(toml) = providers_toml {
        std::fs::write(config_dir.path().join("providers.toml"), toml)
            .expect("write providers.toml");
    }
    if let Some(toml) = config_toml {
        std::fs::write(config_dir.path().join("config.toml"), toml).expect("write config.toml");
    }
    let store = store.unwrap_or_else(|| Arc::new(SqliteStore::open_in_memory().expect("store")));
    let models = Arc::new(Models::load(Some(config_dir.path().to_path_buf())).expect("models"));
    std::mem::forget(config_dir);
    let mut engines: HashMap<EngineKind, Arc<dyn Engine>> = HashMap::new();
    engines.insert(EngineKind::CLAUDE_CODE, engine.clone());
    // The same script under the other kind, so a move across the engine
    // boundary has somewhere to land.
    engines.insert(EngineKind::CODEX, engine);
    let mut config = RuntimeConfig {
        holder: "test".into(),
        max_concurrent_turns: 2,
        lease_ttl: Duration::from_secs(30),
        default_model: Some(ModelRef::new("claude-code", "haiku")),
        workspace_root: Some(workspace_root.clone()),
        bridge: Some(BridgeConfig {
            command: "/bin/true".into(),
            socket: workspace_root.join("aetherd.sock"),
        }),
        // Transient retries back off in milliseconds and never probe the
        // real provider hosts; a test that needs either sets its own.
        engine_transient_retry_floor: Duration::from_millis(10),
        engine_probe_targets: Some(Vec::new()),
        ..Default::default()
    };
    configure(&mut config);
    let runtime = Runtime::start_with_engines(
        config,
        store.clone(),
        models,
        Arc::new(
            ToolRegistry::with_builtins()
                .hiding(["ENGINE_TEST_DAEMON_SECRET".into()])
                .hiding_from_tools(["ENGINE_TEST_PROVIDER_KEY".into()]),
        ),
        None,
        engines,
    )
    .await
    .expect("runtime");
    Harness {
        runtime,
        store,
        prompts,
        systems,
        _workspace: workspace,
    }
}

fn chat_agent(h: &Harness) -> AgentId {
    let (agent, _) = h
        .runtime
        .create_agent(
            CreateAgentSpec {
                role: "assistant".into(),
                start: true,
                config: Some(AgentConfig {
                    long_lived: true,
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

async fn wait_for(
    store: &dyn Store,
    id: AgentId,
    pred: impl Fn(&aether_core::agent::Agent) -> bool,
) -> aether_core::agent::Agent {
    // Counted in polls, not wall-clock: a host stall freezes the runtime too,
    // so a deadline could expire before the runtime ran again.
    const POLLS: u32 = 750;
    for _ in 0..POLLS {
        let agent = store.agent(id).expect("agent").expect("exists");
        if pred(&agent) {
            return agent;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    let agent = store.agent(id).expect("agent").expect("exists");
    panic!(
        "timed out; status {:?} turns {}",
        agent.status, agent.stats.turns
    );
}

fn count(store: &dyn Store, id: AgentId, kind: &str) -> usize {
    store
        .events(0, 10_000, &EventFilter::for_agent(id))
        .expect("events")
        .iter()
        .filter(|e| e.type_name() == kind)
        .count()
}

fn spoken(store: &dyn Store, id: AgentId) -> Vec<(MessageRole, String)> {
    store
        .transcript(id, 0)
        .expect("transcript")
        .into_iter()
        .map(|e| (e.message.role, e.message.text()))
        .collect()
}

/// Ordinary turns must not replace the native system instructions.
const PREFIX_BYTES: usize = 0;

/// Send `text` and wait for the turn it starts to finish.
async fn engine_turn(h: &Harness, id: AgentId, text: &str) {
    let before = h
        .store
        .agent(id)
        .expect("agent")
        .expect("exists")
        .stats
        .turns;
    h.runtime
        .send_message(id, vec![ContentBlock::text(text)], Actor::user())
        .expect("send");
    wait_for(h.store.as_ref(), id, |a| {
        a.stats.turns > before && a.status.kind() == AgentStatusKind::Waiting
    })
    .await;
}

/// The stored snapshot of the agent's most recent turn.
fn latest_snapshot(h: &Harness, id: AgentId) -> aether_core::snapshot::InvocationSnapshot {
    let snapshot_id = h
        .store
        .events(0, 10_000, &EventFilter::for_agent(id))
        .expect("events")
        .into_iter()
        .rev()
        .find_map(|e| match e.kind {
            EventKind::SnapshotCreated { snapshot, .. } => Some(snapshot),
            _ => None,
        })
        .expect("an engine turn recorded a snapshot");
    h.store
        .snapshot(snapshot_id)
        .expect("snapshot")
        .expect("every announced snapshot is stored")
}

fn budgeted_agent(h: &Harness, tokens: u64) -> AgentId {
    let (agent, _) = h
        .runtime
        .create_agent(
            CreateAgentSpec {
                role: "assistant".into(),
                start: true,
                config: Some(AgentConfig {
                    long_lived: true,
                    approval: ApprovalPolicy::none(),
                    token_budget: Some(tokens),
                    ..Default::default()
                }),
                ..Default::default()
            },
            Actor::user(),
        )
        .expect("create");
    agent.id
}

/// Each run replays the next scripted list of events and records its prompt;
/// a run with none left closes its stream, which the runtime reads as a failed engine.
struct QueuedEngine {
    sessions: Mutex<Vec<Option<String>>>,
    runs: Mutex<std::collections::VecDeque<Vec<EngineEvent>>>,
    prompts: Mutex<Vec<String>>,
}

impl QueuedEngine {
    fn new(runs: Vec<Vec<EngineEvent>>) -> Arc<Self> {
        Arc::new(Self {
            runs: Mutex::new(runs.into()),
            sessions: Mutex::new(Vec::new()),
            prompts: Mutex::new(Vec::new()),
        })
    }
}

#[async_trait::async_trait]
impl Engine for QueuedEngine {
    fn kind(&self) -> EngineKind {
        EngineKind::CLAUDE_CODE
    }

    fn adapter(&self) -> String {
        "queued/1".into()
    }

    async fn run(
        &self,
        turn: EngineTurn,
        _cancel: CancellationToken,
    ) -> Result<EngineRun, EngineError> {
        let (tx, rx) = mpsc::channel(64);
        self.sessions.lock().unwrap().push(turn.session);
        self.prompts.lock().unwrap().push(turn.prompt);
        let events = self.runs.lock().unwrap().pop_front().unwrap_or_default();
        tokio::spawn(async move {
            for event in events {
                if tx.send(event).await.is_err() {
                    return;
                }
            }
        });
        Ok(EngineRun {
            events: rx,
            input: None,
            decisions: None,
            stop: None,
        })
    }
}

/// A run in which the model answered with `text`.
fn answered(text: &str) -> Vec<EngineEvent> {
    vec![
        EngineEvent::Assistant {
            content: vec![ContentBlock::text(text)],
            usage: None,
            model: None,
        },
        EngineEvent::Done(EngineOutcome {
            closing_accounting: true,
            text: Some(text.into()),
            ..Default::default()
        }),
    ]
}

/// A run that ended cleanly without the model ever running: what Claude Code
/// reported on a resume that first closed an orphaned background shell.
fn never_invoked() -> Vec<EngineEvent> {
    vec![EngineEvent::Done(EngineOutcome {
        closing_accounting: true,
        text: Some(String::new()),
        ..Default::default()
    })]
}

fn engine_worker(h: &Harness, objective: &str) -> (AgentId, aether_core::id::TaskId) {
    let (agent, task) = h
        .runtime
        .create_agent(
            CreateAgentSpec {
                role: "worker".into(),
                start: true,
                objective: Some(objective.into()),
                model: Some(ModelRef::new("claude-code", "haiku")),
                config: Some(AgentConfig {
                    approval: ApprovalPolicy::none(),
                    ..Default::default()
                }),
                ..Default::default()
            },
            Actor::user(),
        )
        .expect("create");
    (agent.id, task.expect("task").id)
}

fn failed_invocation_errors(store: &dyn Store, id: AgentId) -> Vec<String> {
    store
        .events(0, 10_000, &EventFilter::own_events(id))
        .expect("events")
        .into_iter()
        .filter_map(|e| match e.kind {
            EventKind::InvocationFailed { error, .. } => Some(error),
            _ => None,
        })
        .collect()
}

/// The one memory the `remember` bridge call would leave behind.
fn memories(h: &Harness) -> Vec<String> {
    h.store
        .memories(&aether_store::MemoryQuery {
            limit: 50,
            ..Default::default()
        })
        .expect("memories")
        .into_iter()
        .map(|m| m.text)
        .collect()
}

/// Hands over bridge credentials, then ends with no model output when told to.
struct SilentBridgeEngine {
    outcome: EngineOutcome,
    handover: Mutex<Option<BridgeHandover>>,
    finish: Mutex<Option<tokio::sync::oneshot::Receiver<()>>>,
}

#[async_trait::async_trait]
impl Engine for SilentBridgeEngine {
    fn kind(&self) -> EngineKind {
        EngineKind::CLAUDE_CODE
    }

    fn adapter(&self) -> String {
        "silent-bridge/1".into()
    }

    async fn run(
        &self,
        turn: EngineTurn,
        _cancel: CancellationToken,
    ) -> Result<EngineRun, EngineError> {
        let args = &turn.mcp.as_ref().expect("the turn carries a bridge").args;
        let after = |flag: &str| -> String {
            let at = args.iter().position(|a| a == flag).expect("bridge flag");
            args[at + 1].clone()
        };
        let credentials = (
            after("--agent").parse::<AgentId>().expect("agent id"),
            after("--token"),
        );
        let handover = self.handover.lock().unwrap().take().expect("one turn");
        let finish = self.finish.lock().unwrap().take().expect("one turn");
        let outcome = self.outcome.clone();
        let (tx, rx) = mpsc::channel(1);
        tokio::spawn(async move {
            let _ = handover.send(credentials);
            let _ = finish.await;
            let _ = tx.send(EngineEvent::Done(outcome)).await;
        });
        Ok(EngineRun {
            events: rx,
            input: None,
            decisions: None,
            stop: None,
        })
    }
}

/// Sends the running turn's bridge credentials to the test, once.
type BridgeHandover = tokio::sync::oneshot::Sender<(AgentId, String)>;

struct CheckpointEngine {
    turns: Arc<Mutex<Vec<EngineTurn>>>,
    valid: bool,
}

#[async_trait::async_trait]
impl Engine for CheckpointEngine {
    fn kind(&self) -> EngineKind {
        EngineKind::CLAUDE_CODE
    }
    fn adapter(&self) -> String {
        "checkpoint-test/1".into()
    }
    async fn run(
        &self,
        turn: EngineTurn,
        _cancel: CancellationToken,
    ) -> Result<EngineRun, EngineError> {
        self.turns.lock().unwrap().push(turn.clone());
        let text = if turn.summary_only {
            if self.valid {
                serde_json::json!({"checkpoint":{"objective":"retain the secret word", "state":"The word is saffron", "decisions":[], "constraints":[], "open_loops":[], "artifacts":[]}}).to_string()
            } else {
                "invalid checkpoint".into()
            }
        } else {
            "remembered saffron".into()
        };
        let (tx, rx) = mpsc::channel(4);
        tx.send(EngineEvent::Session {
            id: turn.new_session_id,
            model: None,
            tools: vec![],
        })
        .await
        .unwrap();
        tx.send(EngineEvent::Assistant {
            content: vec![ContentBlock::text(&text)],
            usage: None,
            model: None,
        })
        .await
        .unwrap();
        let usage = aether_core::model::Usage {
            input_tokens: if turn.summary_only { 40 } else { 10 },
            output_tokens: if turn.summary_only { 7 } else { 3 },
            ..Default::default()
        };
        tx.send(EngineEvent::UsageDelta { usage }).await.unwrap();
        tx.send(EngineEvent::Done(EngineOutcome {
            closing_accounting: true,
            text: Some(text),
            usage,
            cost_usd: Some(if turn.summary_only { 1.5 } else { 0.25 }),
            ..Default::default()
        }))
        .await
        .unwrap();
        Ok(EngineRun {
            events: rx,
            input: None,
            decisions: None,
            stop: None,
        })
    }
}

#[path = "engine/cost_gap_review.rs"]
mod cost_gap_review;
