#![allow(clippy::expect_used, clippy::unwrap_used, clippy::panic)]

//! Parking an engine-backed agent. An engine owns its model loop, so a
//! harness action that ends the turn (`wait_for`, `delegate` with a wait,
//! `complete_task`, `fail_task`, `ask_user`) has to stop it: without that the
//! engine reads the reply as an ordinary tool result and keeps calling tools,
//! burning context for a turn that is already over.
//!
//! The fake engine here reports what the runtime did to it - the stdin close,
//! the stop verb, the cancel, the session it was asked to resume - so every
//! mechanism the fix adds is asserted rather than inferred.

use std::collections::HashMap;
use std::sync::Arc;
use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::time::Duration;

use aether_core::agent::{
    AgentConfig, AgentStatus, AgentStatusKind, ApprovalPolicy, WaitCondition,
};
use aether_core::approval::{RuleMatch, RuleScope};
use aether_core::event::{Actor, Event, EventKind};
use aether_core::id::{AgentId, TaskId};
use aether_core::model::{ContentBlock, ModelRef, ToolSpec, Usage};
use aether_core::schedule::{CreateScheduleSpec, ScheduleStatus};
use aether_core::task::{Task, TaskResult, TaskStatus};
use aether_core::time::now;
use aether_core::tool::{
    ApprovalDecision, ResourceKey, SideEffectClass, ToolDescriptor, ToolOutcome,
};
use aether_engine::{
    Engine, EngineError, EngineEvent, EngineKind, EngineOutcome, EngineRun, EngineTurn,
    PermissionDecision,
};
use aether_model::Models;
use aether_runtime::{BridgeConfig, CreateAgentSpec, Runtime, RuntimeConfig};
use aether_store::{AgentFilter, Commit, EventFilter, MemoryQuery, Mutation, SqliteStore, Store};
use aether_tools::{Tool, ToolContext, ToolError, ToolExecution, ToolRegistry};
use serde_json::{Value, json};
use tokio::sync::{mpsc, oneshot};
use tokio_util::sync::CancellationToken;

struct Harness {
    runtime: Arc<Runtime>,
    store: Arc<dyn Store>,
    started: mpsc::Receiver<TurnStarted>,
    /// Turns that started out of order, kept until a test asks for them.
    stashed: Vec<TurnStarted>,
    _workspace: tempfile::TempDir,
    _config: tempfile::TempDir,
}

/// What the fake engine asks when a test scripts a question.
const QUESTION: &str = "which half should I do first?";

/// What a person sends while a turn is winding down.
const MESSAGE: &str = "the second half is more urgent";

async fn harness(park_grace: Duration) -> Harness {
    harness_with(
        park_grace,
        OnKill::Report(vec![cancelled_with(Usage::default())]),
    )
    .await
}

async fn harness_with(park_grace: Duration, on_kill: OnKill) -> Harness {
    harness_full(park_grace, on_kill, Script::default()).await
}

async fn harness_full(park_grace: Duration, on_kill: OnKill, script: Script) -> Harness {
    build(park_grace, on_kill, script, None).await
}

/// A harness whose registry serves `tool` under whatever harness tool name
/// its descriptor claims, so the engine bridge offers the double.
async fn harness_with_tool(park_grace: Duration, tool: Arc<dyn Tool>) -> Harness {
    build(
        park_grace,
        OnKill::Report(vec![cancelled_with(Usage::default())]),
        Script::default(),
        Some(tool),
    )
    .await
}

async fn build(
    park_grace: Duration,
    on_kill: OnKill,
    script: Script,
    extra_tool: Option<Arc<dyn Tool>>,
) -> Harness {
    let workspace = tempfile::tempdir().expect("tempdir");
    let workspace_root = workspace.path().canonicalize().expect("canonical");
    let config_dir = tempfile::tempdir().expect("config dir");
    std::fs::write(
        config_dir.path().join("providers.toml"),
        "[providers.claude-code.auth]\nnone = true\n\
         [providers.codex.auth]\nnone = true\n",
    )
    .expect("fixture provider authentication");
    let store: Arc<dyn Store> = Arc::new(SqliteStore::open_in_memory().expect("store"));
    let models = Arc::new(Models::load(Some(config_dir.path().to_path_buf())).expect("models"));

    let (started_tx, started) = mpsc::channel(16);
    let mut engines: HashMap<EngineKind, Arc<dyn Engine>> = HashMap::new();
    // Both kinds: the park is engine-agnostic, and Codex is the adapter that
    // publishes no price, so the accounting on the killed path lands there.
    for kind in [EngineKind::CLAUDE_CODE, EngineKind::CODEX] {
        engines.insert(
            kind,
            Arc::new(ParkEngine {
                kind,
                started: started_tx.clone(),
                on_kill: on_kill.clone(),
                script: script.clone(),
            }),
        );
    }

    let mut tools = ToolRegistry::with_builtins();
    if let Some(tool) = extra_tool {
        tools.register(tool);
    }
    let runtime = Runtime::start_with_engines(
        RuntimeConfig {
            holder: "test".into(),
            max_concurrent_turns: 8,
            lease_ttl: Duration::from_secs(30),
            default_model: Some(ModelRef::new("claude-code", "haiku")),
            workspace_root: Some(workspace_root.clone()),
            engine_park_grace: park_grace,
            bridge: Some(BridgeConfig {
                command: "/bin/true".into(),
                socket: workspace_root.join("aetherd.sock"),
            }),
            ..Default::default()
        },
        store.clone(),
        models,
        Arc::new(tools),
        None,
        engines,
    )
    .await
    .expect("runtime");

    Harness {
        runtime,
        store,
        started,
        stashed: Vec::new(),
        _workspace: workspace,
        _config: config_dir,
    }
}

/// A root agent that owns its own task, so `complete_task` and `fail_task`
/// have something to end.
fn start_agent(h: &Harness, spec: CreateAgentSpec) -> AgentId {
    let (agent, _) = h.runtime.create_agent(spec, Actor::user()).expect("create");
    agent.id
}

fn coordinator_spec() -> CreateAgentSpec {
    CreateAgentSpec {
        role: "coordinator".into(),
        objective: Some("spawn a worker and wait for it".into()),
        start: true,
        config: Some(AgentConfig {
            long_lived: true,
            approval: ApprovalPolicy::none(),
            ..Default::default()
        }),
        ..Default::default()
    }
}

fn start_coordinator(h: &Harness) -> AgentId {
    start_agent(h, coordinator_spec())
}

/// The next turn this agent starts, however many other agents' turns arrive
/// first.
async fn turn_of(h: &mut Harness, agent: AgentId) -> TurnStarted {
    let deadline = tokio::time::Instant::now() + Duration::from_secs(15);
    loop {
        if let Some(at) = h.stashed.iter().position(|t| t.agent == agent) {
            return h.stashed.remove(at);
        }
        let next = tokio::time::timeout_at(deadline, h.started.recv())
            .await
            .unwrap_or_else(|_| panic!("{agent} never started a turn"))
            .expect("the engine channel stays open");
        h.stashed.push(next);
    }
}

async fn wait_until(
    store: &dyn Store,
    id: AgentId,
    pred: impl Fn(&aether_core::agent::Agent) -> bool,
) -> aether_core::agent::Agent {
    let deadline = tokio::time::Instant::now() + Duration::from_secs(15);
    loop {
        let agent = store.agent(id).expect("agent").expect("exists");
        if pred(&agent) {
            return agent;
        }
        assert!(
            tokio::time::Instant::now() < deadline,
            "timed out; status {:?} turns {}",
            agent.status,
            agent.stats.turns
        );
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
}

/// Wait until the fake engine has seen the runtime stop its process, and
/// report how long that took. Panics rather than hanging.
async fn wait_until_stopped(observed: &Observed, within: Duration) -> Duration {
    let started = tokio::time::Instant::now();
    while !observed.cancelled() {
        assert!(
            started.elapsed() < within,
            "the process was not stopped within {within:?}"
        );
        tokio::time::sleep(Duration::from_millis(5)).await;
    }
    started.elapsed()
}

/// Wait until the fake engine has been asked to stop in its own verb.
/// Panics rather than hanging.
async fn wait_until_asked_to_stop(observed: &Observed, within: Duration) {
    let started = tokio::time::Instant::now();
    while !observed.stopped() {
        assert!(
            started.elapsed() < within,
            "the engine was never asked to stop within {within:?}"
        );
        tokio::time::sleep(Duration::from_millis(5)).await;
    }
}

/// A coordinator with a token grant, so a bridge call can find it crossed.
fn budgeted_spec(tokens: u64) -> CreateAgentSpec {
    CreateAgentSpec {
        config: Some(AgentConfig {
            long_lived: true,
            approval: ApprovalPolicy::none(),
            token_budget: Some(tokens),
            ..Default::default()
        }),
        ..coordinator_spec()
    }
}

/// An idle child, created before the turn under test: `create_agent` goes
/// through the runtime, and creating one mid-turn would wake the scheduler
/// into the sweep these tests are not about.
fn idle_child(h: &Harness, parent: AgentId) -> AgentId {
    let (child, _) = h
        .runtime
        .create_agent(
            CreateAgentSpec {
                role: "worker".into(),
                parent: Some(parent),
                config: Some(aether_core::agent::AgentConfig {
                    effort: Some(aether_core::model::Effort::Low),
                    ..Default::default()
                }),
                start: false,
                ..Default::default()
            },
            Actor::user(),
        )
        .expect("child");
    child.id
}

/// Spend that puts its parent's subtree over the parent's grant. Written
/// straight to the store: an `apply` would wake the scheduler, and these
/// tests are about the gate inside the turn, not the sweep outside it.
fn spend_over_budget(h: &Harness, child: AgentId, tokens: u64) {
    let mut row = h.store.agent(child).expect("read").expect("child");
    row.stats.usage.input_tokens = tokens;
    h.store
        .commit(aether_store::Commit::new().mutate(aether_store::Mutation::UpsertAgent(row)))
        .expect("commit");
}

/// The agent's terminal error, or a panic naming what it did instead.
fn failed_with(agent: &aether_core::agent::Agent) -> String {
    match &agent.status {
        AgentStatus::Failed { error, .. } => error.clone(),
        other => panic!("not a failure: {other:?}"),
    }
}

/// Wait until an event matching `pred` is in the log.
async fn wait_for_event(store: &dyn Store, pred: impl Fn(&Event) -> bool) {
    let deadline = tokio::time::Instant::now() + Duration::from_secs(15);
    while !events(store).iter().any(&pred) {
        assert!(
            tokio::time::Instant::now() < deadline,
            "the event never arrived"
        );
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
}

async fn wait_for_park(h: &Harness, agent: AgentId) -> aether_core::agent::Agent {
    wait_until(h.store.as_ref(), agent, |a| {
        a.status.kind() == AgentStatusKind::Waiting
    })
    .await
}

/// Every event in the store's global order, so a test can assert what
/// happened between two others rather than sleeping and hoping.
fn events(store: &dyn Store) -> Vec<Event> {
    store
        .events(0, 100_000, &EventFilter::all())
        .expect("events")
}

fn seqs(store: &dyn Store, pred: impl Fn(&Event) -> bool) -> Vec<u64> {
    events(store)
        .into_iter()
        .filter(|e| pred(e))
        .map(|e| e.seq)
        .collect()
}

fn turns_started(store: &dyn Store, agent: AgentId) -> usize {
    seqs(store, |e| {
        e.agent_id == Some(agent) && matches!(e.kind, EventKind::AgentStarted { .. })
    })
    .len()
}

/// The payload of the last `Custom` event this agent recorded under `name`.
fn custom(store: &dyn Store, agent: AgentId, name: &str) -> Option<Value> {
    events(store).into_iter().rev().find_map(|e| match e.kind {
        EventKind::Custom {
            name: ref n,
            ref payload,
        } if n == name && e.agent_id == Some(agent) => Some(payload.clone()),
        _ => None,
    })
}

fn waited_on(status: &AgentStatus) -> Vec<TaskId> {
    let AgentStatus::Waiting { condition } = status else {
        panic!("not waiting: {status:?}");
    };
    fn walk(c: &WaitCondition, out: &mut Vec<TaskId>) {
        match c {
            WaitCondition::Task { id } => out.push(*id),
            WaitCondition::Any { of } | WaitCondition::All { of } => {
                of.iter().for_each(|c| walk(c, out));
            }
            WaitCondition::AtLeast { of, .. } => of.iter().for_each(|c| walk(c, out)),
            _ => {}
        }
    }
    let mut out = Vec::new();
    walk(condition, &mut out);
    out
}

fn condition_of(status: &AgentStatus) -> &WaitCondition {
    let AgentStatus::Waiting { condition } = status else {
        panic!("not waiting: {status:?}");
    };
    condition
}

/// Whether `target` occurs anywhere in `condition`, through `with_deadline`'s
/// own `Any`/`All` wrapping - used where the exact wrapped shape is an
/// internal timing detail the test should not have to hardcode.
fn condition_contains(condition: &WaitCondition, target: &WaitCondition) -> bool {
    if condition == target {
        return true;
    }
    match condition {
        WaitCondition::Any { of } | WaitCondition::All { of } => {
            of.iter().any(|c| condition_contains(c, target))
        }
        WaitCondition::AtLeast { of, .. } => of.iter().any(|c| condition_contains(c, target)),
        _ => false,
    }
}

/// What the model is told when it calls a harness tool in a turn that a
/// harness tool already ended.
const TURN_IS_OVER: &str = "This turn is over: a harness tool already ended it.";

/// A bridge call, bounded. An approval wait inside the drive loop is
/// unbounded by design, so a regression that lets one happen has to fail the
/// test rather than hang it.
async fn call(h: &Harness, turn: &TurnStarted, name: &str, input: Value) -> (String, bool) {
    let parks = matches!(name, "wait_for" | "ask_user")
        || (name == "delegate" && input.get("wait") != Some(&json!(false)));
    let id = format!("fixture-{}", aether_core::id::TaskId::new());
    if parks && let Some(events) = turn.events.upgrade() {
        events
            .send(EngineEvent::Assistant {
                content: vec![ContentBlock::ToolUse {
                    id: id.clone(),
                    name: format!("mcp__aether__{name}"),
                    input: input.clone(),
                }],
                usage: None,
                model: None,
            })
            .await
            .expect("native tool request");
    }
    let reply = raw_call(h, turn, name, input).await;
    if parks && !reply.1 {
        if let Some(events) = turn.events.upgrade() {
            // A zero-grace park may already have closed the stream.
            if events
                .send(EngineEvent::ToolResult {
                    tool_use_id: id,
                    content: reply.0.clone(),
                    is_error: reply.1,
                })
                .await
                .is_err()
            {
                assert!(turn.observed.cancelled());
            }
        }
        wait_until_asked_to_stop(&turn.observed, Duration::from_secs(2)).await;
    }
    reply
}

async fn raw_call(h: &Harness, turn: &TurnStarted, name: &str, input: Value) -> (String, bool) {
    tokio::time::timeout(
        Duration::from_secs(10),
        h.runtime
            .engine_tool(turn.agent, &turn.token, name.into(), input),
    )
    .await
    .unwrap_or_else(|_| panic!("`{name}` was never answered; the drive loop is suspended"))
    .expect("answered")
}

fn task_of(reply: &str) -> TaskId {
    let fields: serde_json::Value =
        serde_json::from_str(reply.lines().last().expect("structured line")).expect("json");
    fields["task_id"]
        .as_str()
        .expect("task id")
        .parse()
        .expect("task id")
}

fn assigned_worker_spec() -> CreateAgentSpec {
    CreateAgentSpec {
        config: Some(AgentConfig {
            approval: ApprovalPolicy::none(),
            ..Default::default()
        }),
        ..coordinator_spec()
    }
}

/// A read-only harness tool that takes far longer than the park grace,
/// standing in for a slow or hostile MCP endpoint. Registered under
/// `list_tasks` so the engine bridge advertises it like any other.
#[derive(Debug)]
struct SlowTool(Duration);

#[async_trait::async_trait]
impl Tool for SlowTool {
    fn descriptor(&self) -> ToolDescriptor {
        ToolDescriptor {
            spec: ToolSpec {
                name: "list_tasks".into(),
                description: "a call that never comes back inside the grace".into(),
                input_schema: json!({"type": "object", "properties": {}}),
                strict: false,
            },
            side_effect: SideEffectClass::ReadOnly,
            needs_network: false,
        }
    }

    fn resources(&self, _: &Value, _: &ToolContext) -> Vec<ResourceKey> {
        vec![]
    }

    async fn execute(
        &self,
        _: Value,
        _: &ToolContext,
        _: CancellationToken,
    ) -> Result<ToolExecution, ToolError> {
        tokio::time::sleep(self.0).await;
        Ok(ToolExecution::Outcome(ToolOutcome::ok("eventually")))
    }
}

#[path = "engine_wait/engine_fixture.rs"]
mod engine_fixture;
use engine_fixture::{Observed, OnKill, ParkEngine, Script, TurnStarted, cancelled_with};

#[path = "engine_wait/wait_sources.rs"]
mod wait_sources;

#[path = "engine_wait/parking.rs"]
mod parking;
#[path = "engine_wait/reply_boundary.rs"]
mod reply_boundary;

#[path = "engine_wait/settlement.rs"]
mod settlement;

#[path = "engine_wait/approvals.rs"]
mod approvals;

#[path = "engine_wait/background.rs"]
mod background;

#[path = "engine_wait/budgets.rs"]
mod budgets;

#[path = "engine_wait/accounting.rs"]
mod accounting;

#[path = "engine_wait/stream_failure.rs"]
mod stream_failure;

#[path = "support/tui_projection.rs"]
mod tui_projection;

#[path = "engine_wait/stream_cancel.rs"]
mod stream_cancel;

#[path = "engine_wait/stream_endings.rs"]
mod stream_endings;

#[path = "engine_wait/reconnect.rs"]
mod reconnect;
