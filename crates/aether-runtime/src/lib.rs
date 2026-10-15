//! The Aether kernel: authoritative state, atomic commits, deterministic
//! scheduling, and the agent turn loop.
//!
//! Everything that changes state goes through [`Runtime::apply`], which
//! commits events and mutations together and then publishes the events on
//! the bus. The scheduler decides who runs; the turn executor runs one model
//! invocation plus its tool calls and hands the agent back to the scheduler.

pub mod approval;
pub mod artifacts;
pub mod attachments;
pub mod bus;
mod cancellation;
pub mod commands;
mod compaction_client;
mod disk;
mod exhaustion;
pub mod github;
mod inspection;
pub mod memory_context;
pub mod memory_curation;
mod memory_feedback;
pub mod recovery;
mod repository_aliases;
pub mod resources;
mod retention;
pub mod scheduler;
pub mod schedules;
pub mod storage;
pub mod submission;
mod tool_interruption;
pub mod transfer;
pub mod turn;
pub mod turn_engine;
pub mod wait;
mod workspace;
pub mod workspace_gc;

use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use std::time::{Duration, Instant};

use aether_context::ContextManager;
use aether_core::event::{Actor, Event, EventDraft, EventKind};
use aether_core::id::AgentId;
use aether_core::model::{ModelError, ModelRef};
use aether_core::provider::{ErrorWindow, FallbackRecord};
use aether_core::time::{Timestamp, now};
use aether_memory::MemoryService;
use aether_model::{ModelClient, Models, ModelsError};
use aether_store::{Commit, Store, StoreError};
use aether_tools::{ToolRegistry, ToolRuntime};
use tokio::sync::Notify;
use tokio_util::sync::CancellationToken;

pub use approval::{Gate, Remember};
pub use bus::{BusMessage, DeltaKind, EventBus};
pub use commands::{
    AgentUpdate, CreateAgentSpec, CreateTaskSpec, MessageDelivery, RuntimeStatus, pending_question,
};
pub use retention::HistorySweep;

#[derive(Debug, thiserror::Error)]
pub enum RuntimeError {
    #[error(transparent)]
    Store(#[from] StoreError),
    #[error(transparent)]
    Model(#[from] ModelError),
    #[error(transparent)]
    Models(#[from] ModelsError),
    #[error(transparent)]
    Memory(#[from] aether_memory::MemoryError),
    #[error(transparent)]
    Transition(#[from] aether_core::error::TransitionError),
    #[error("{0} not found")]
    NotFound(String),
    #[error("{0}")]
    Invalid(String),
    #[error("internal: {0}")]
    Internal(String),
}

impl RuntimeError {
    /// The disk refused a write; see [`StoreError::is_unwritable`].
    pub fn is_store_unwritable(&self) -> bool {
        match self {
            Self::Store(e) | Self::Memory(aether_memory::MemoryError::Store(e)) => {
                e.is_unwritable()
            }
            _ => false,
        }
    }
}

pub type Result<T> = std::result::Result<T, RuntimeError>;

mod config;
mod engine_access;
mod provider_health;

pub use config::{
    BridgeConfig, DEFAULT_ENGINE_PARK_GRACE, DEFAULT_ENGINE_TRANSIENT_RETRY_FLOOR,
    DEFAULT_ENGINE_TRANSIENT_RETRY_WINDOW, DEFAULT_EXHAUSTION_PROBE_FLOOR,
    DEFAULT_MAX_CONCURRENT_TURNS, DEFAULT_RESOURCE_WAIT_TIMEOUT, DiskGuard, RuntimeConfig,
};
use engine_access::detect_engines;

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub(crate) struct CancelRequest {
    pub by: Actor,
    pub reason: Option<String>,
}

/// A turn in flight.
#[derive(Debug)]
pub(crate) struct RunningTurn {
    /// The open task carried by this turn, retained through native wind-down.
    pub task: Option<aether_core::id::TaskId>,
    /// Ends the turn and the agent.
    pub cancel: CancellationToken,
    /// Tokens carry no actor or reason; retain the first explicit request
    /// until turn completion and its descendant cancellation walk.
    pub cancel_request: Option<CancelRequest>,
    /// Reporting only: why the scheduler stopped this turn before its terminal commit.
    pub stop_reason: Option<String>,
    /// Child of `cancel`: ends the turn only; the agent waits for its next
    /// message with what it has so far.
    pub interrupt: CancellationToken,
    /// Admission bypassed scheduling limits for an unread operator message.
    /// Carry that decision into the first provider claim, before inbox consumption.
    pub addressed: bool,
    /// Provider carrying this turn, for the per-provider limit.
    pub provider: String,
    /// The provider carrying this turn is an engine, so the turn counts
    /// against the daemon's engine cap. Kept in step with `provider` as the
    /// turn walks its fallback chain.
    pub engine: bool,
    /// Set while the turn is parked waiting for a resource key. It is making
    /// no provider call, so it holds no concurrency slot of any kind.
    pub on_provider: bool,
}

/// A provider the scheduler is avoiding, and why.
#[derive(Debug, Clone)]
pub(crate) struct Backoff {
    pub until: Instant,
    /// Same instant as wall-clock, for events and projections.
    pub until_at: Timestamp,
    pub reason: String,
}

#[derive(Debug, Default)]
pub(crate) struct SchedulerState {
    pub running: HashMap<AgentId, RunningTurn>,
    /// When each ready candidate first became eligible, so wait time survives
    /// a row mutation that resets `updated_at`. Stamped and pruned by
    /// `dispatch`; only limit 6's ordering reads it.
    pub ready_since: HashMap<AgentId, Timestamp>,
    /// Agents whose turn was admitted but could enter no link, with when to
    /// offer them a turn again. Such a refusal writes no invocation row, so
    /// the failure streak never grows: without this the turn is handed back
    /// and re-dispatched on the same tick, forever.
    pub entry_refused_until: HashMap<AgentId, Instant>,
    /// Turns queued in `turn::retake_engine_slot`: parked on nothing but an
    /// engine slot, holding the resource keys they just won. Counted with the
    /// running ones so a fresh dispatch cannot take a freed slot from under a
    /// waiter that is already holding keys others queue behind.
    pub engine_retakers: HashSet<AgentId>,
    /// Providers that asked us to slow down, with the instant to resume.
    pub backoff: HashMap<String, Backoff>,
    /// The one agent released to probe each spent provider, from its release
    /// until its turn ends. Losing it on restart costs one extra probe.
    pub probing: HashMap<String, AgentId>,
    /// Latest subscription window each provider reported.
    pub quotas: HashMap<String, aether_core::provider::ProviderUsage>,
    /// Commit count as of the last budget pass; `None` before the first.
    pub budgets_checked_after: Option<u64>,
    /// Newest normalized failures each provider returned.
    pub errors: HashMap<String, ErrorWindow>,
    /// Where a turn last went after giving up on each provider.
    pub fallbacks: HashMap<String, FallbackRecord>,
}

impl SchedulerState {
    /// Turns actually consuming a concurrency slot. A turn parked on a
    /// resource key is making no model call and holds none of the limits.
    pub(crate) fn on_provider(&self) -> impl Iterator<Item = (&AgentId, &RunningTurn)> {
        self.running.iter().filter(|(_, r)| r.on_provider)
    }

    /// Turns consuming a slot on an engine right now - what the daemon's
    /// engine cap is spent on, and what a projection reports as in flight.
    pub(crate) fn on_engine(&self) -> usize {
        self.on_provider().filter(|(_, r)| r.engine).count()
    }

    /// Whether one more engine turn fits under `cap`, counting only turns
    /// actually running. `None` is no cap.
    pub(crate) fn engine_slot_free(&self, cap: Option<usize>) -> bool {
        cap.is_none_or(|cap| self.on_engine() < cap)
    }

    /// Whether a turn not yet queued may take an engine slot: queued
    /// retakers are counted, so they get first refusal on the next free one.
    pub(crate) fn engine_slot_free_for_newcomer(&self, cap: Option<usize>) -> bool {
        cap.is_none_or(|cap| self.on_engine() + self.engine_retakers.len() < cap)
    }
}

/// A hold on the single write path, taken while a commit is being built.
/// Nothing can commit between the reads the build makes and the commit it
/// produces. Shared with [`MemoryService`], which curates the same way.
pub(crate) type CommitHold<'a> = aether_store::Hold<'a>;

/// Coordination tools a leaf agent never sees: it cannot delegate, so
/// the fleet, task and schedule surfaces would only cost it prefix tokens.
pub(crate) const LEAF_WITHHELD_TOOLS: &[&str] = &[
    "delegate",
    "integrate_workspace",
    "cancel_agent",
    "create_schedule",
    "update_schedule",
    "list_schedules",
    "list_agents",
    "list_tasks",
    "read_task",
    "list_models",
];

pub struct Runtime {
    pub(crate) store: Arc<dyn Store>,
    pub(crate) models: Arc<Models>,
    /// Model used for agents created without one; resolved lazily so a login
    /// after daemon start is picked up without a restart.
    pub(crate) default_model: parking_lot::RwLock<Option<ModelRef>>,
    /// Test hook: bypass provider resolution and use this client for every turn.
    pub(crate) client_override: Option<Arc<dyn ModelClient>>,
    /// External engines found on this machine.
    pub(crate) engines: HashMap<aether_engine::EngineKind, Arc<dyn aether_engine::Engine>>,
    /// Why an engine absent from `engines` is unusable, when preflight ran
    /// and found a specific reason - empty for engines supplied
    /// directly via `start_with_engines`, which have no such history.
    pub(crate) engine_unavailable: HashMap<aether_engine::EngineKind, String>,
    /// Engine turns in flight, keyed by agent, for routing bridge calls.
    pub(crate) engine_turns: parking_lot::Mutex<HashMap<AgentId, turn_engine::EngineTurnHandle>>,
    pub(crate) tools: Arc<ToolRegistry>,
    /// Resource keys held by mutating tool calls in flight, across agents.
    pub(crate) resources: Arc<resources::ResourceLeases>,
    pub(crate) tool_runtime: ToolRuntime,
    pub(crate) context: ContextManager,
    pub(crate) memory: MemoryService,
    pub(crate) bus: EventBus,
    pub(crate) config: RuntimeConfig,
    pub(crate) sched: parking_lot::Mutex<SchedulerState>,
    pub(crate) notify: Notify,
    /// Woken whenever a turn gives an engine slot back, so a turn queueing
    /// for one does not have to poll. Separate from `notify`, which only the
    /// scheduler loop waits on: sharing it would let a queued turn swallow a
    /// dispatch wake-up.
    pub(crate) engine_slot: Notify,
    pub(crate) shutdown: CancellationToken,
    pub(crate) started_at: Timestamp,
    commit_lock: aether_store::WriteGate,
    /// Shared-payload lifecycle; acquired before the commit gate and SQLite connection.
    artifact_lock: tokio::sync::Mutex<()>,
    storage_gc_lock: parking_lot::Mutex<()>,
    storage_last_run: parking_lot::Mutex<Option<serde_json::Value>>,
    /// Root agent ids this process has already seeded repository knowledge
    /// for, so a client retry-storming a keyed `create_agent` does not repeat
    /// the tree walk and memory-gate work on every replay. Per process, not
    /// durable: a restart re-seeds once, same as a fresh agent would.
    pub(crate) seeded_repositories: parking_lot::Mutex<std::collections::HashSet<AgentId>>,
    /// Free space on the store's filesystem, as the disk guard last saw it.
    pub(crate) disk: parking_lot::Mutex<disk::DiskState>,
    workspace_gc_cursor: parking_lot::Mutex<workspace_gc::WorkspaceGcCursor>,
    workspace_gc_active: parking_lot::Mutex<Option<std::path::PathBuf>>,
}

impl std::fmt::Debug for Runtime {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Runtime")
            .field("config", &self.config)
            .finish()
    }
}

impl Runtime {
    /// Build the runtime, recover interrupted work, and start the scheduler.
    pub async fn start(
        config: RuntimeConfig,
        store: Arc<dyn Store>,
        models: Arc<Models>,
        tools: Arc<ToolRegistry>,
    ) -> Result<Arc<Self>> {
        Self::start_with(config, store, models, tools, None).await
    }

    /// Like [`Runtime::start`], with an optional client that replaces every
    /// provider (used by deterministic tests).
    pub async fn start_with(
        config: RuntimeConfig,
        store: Arc<dyn Store>,
        models: Arc<Models>,
        tools: Arc<ToolRegistry>,
        client_override: Option<Arc<dyn ModelClient>>,
    ) -> Result<Arc<Self>> {
        let (engines, engine_unavailable) =
            detect_engines(models.config().model.prompt_cache_ttl).await;
        Self::start_with_engines_inner(
            config,
            store,
            models,
            tools,
            client_override,
            engines,
            engine_unavailable,
        )
        .await
    }

    /// Like [`Runtime::start_with`], with the engines given instead of
    /// detected on this machine (used by deterministic tests). These engines
    /// carry no detection history, so a missing one falls back to the
    /// generic not-installed message rather than a stale preflight reason.
    pub async fn start_with_engines(
        config: RuntimeConfig,
        store: Arc<dyn Store>,
        models: Arc<Models>,
        tools: Arc<ToolRegistry>,
        client_override: Option<Arc<dyn ModelClient>>,
        engines: HashMap<aether_engine::EngineKind, Arc<dyn aether_engine::Engine>>,
    ) -> Result<Arc<Self>> {
        Self::start_with_engines_inner(
            config,
            store,
            models,
            tools,
            client_override,
            engines,
            HashMap::new(),
        )
        .await
    }

    async fn start_with_engines_inner(
        config: RuntimeConfig,
        store: Arc<dyn Store>,
        models: Arc<Models>,
        tools: Arc<ToolRegistry>,
        client_override: Option<Arc<dyn ModelClient>>,
        engines: HashMap<aether_engine::EngineKind, Arc<dyn aether_engine::Engine>>,
        engine_unavailable: HashMap<aether_engine::EngineKind, String>,
    ) -> Result<Arc<Self>> {
        attachments::check_attachment_dir(config.attachment_dir.as_deref())?;
        // One gate for the whole daemon: memory curation is a read-modify-write
        // against the same store a turn commits to, so both take this.
        let commit_lock = aether_store::WriteGate::new();
        let runtime = Arc::new(Self {
            memory: match config.memory_index.clone() {
                Some(index) => {
                    MemoryService::new(store.clone(), commit_lock.clone()).with_index(index)
                }
                None => MemoryService::new(store.clone(), commit_lock.clone()),
            },
            default_model: parking_lot::RwLock::new(config.default_model.clone()),
            client_override,
            engines,
            engine_unavailable,
            engine_turns: parking_lot::Mutex::new(HashMap::new()),
            store,
            models,
            tools,
            resources: Arc::new(resources::ResourceLeases::new()),
            tool_runtime: ToolRuntime::new(),
            context: ContextManager::new(),
            bus: EventBus::new(),
            config,
            sched: parking_lot::Mutex::new(SchedulerState::default()),
            notify: Notify::new(),
            engine_slot: Notify::new(),
            shutdown: CancellationToken::new(),
            started_at: now(),
            commit_lock,
            artifact_lock: tokio::sync::Mutex::new(()),
            workspace_gc_cursor: Default::default(),
            workspace_gc_active: Default::default(),
            storage_gc_lock: parking_lot::Mutex::new(()),
            storage_last_run: parking_lot::Mutex::new(None),
            seeded_repositories: parking_lot::Mutex::new(std::collections::HashSet::new()),
            disk: parking_lot::Mutex::new(disk::DiskState::default()),
        });
        runtime.apply(Commit::new().event(EventDraft::new(
            Actor::System,
            EventKind::DaemonStarted {
                version: env!("CARGO_PKG_VERSION").to_owned(),
                pid: std::process::id(),
            },
        )))?;
        recovery::recover(&runtime)?;
        // Preserve startup artifact cleanup. Periodic retention runs on a
        // separate blocking worker, never inside the scheduler tick loop.
        match runtime.sweep_artifacts() {
            Ok(0) => {}
            Ok(n) => tracing::info!(evicted = n, "retention dropped artifact bytes"),
            Err(e) => tracing::error!(error = %e, "artifact retention sweep failed"),
        }
        disk::spawn(runtime.clone());
        scheduler::spawn(runtime.clone());
        retention::spawn(runtime.clone());
        Ok(runtime)
    }

    /// Replay a run from another daemon's export, then publish. Like
    /// [`Runtime::apply`] but the events keep the timestamps they had where
    /// they ran, and the scheduler is not woken: imported agents are history
    /// and take no turns.
    pub fn apply_imported(
        &self,
        events: Vec<aether_store::ImportedEvent>,
        mutations: Vec<aether_store::Mutation>,
    ) -> Result<Vec<Event>> {
        if events.is_empty() && mutations.is_empty() {
            return Ok(Vec::new());
        }
        let hold = self.hold();
        let events = self.store.import(events, mutations)?;
        // Published under the hold, as `apply_held` does.
        for event in &events {
            self.bus.publish_event(event.clone());
        }
        drop(hold);
        Ok(events)
    }

    /// Commit atomically, then publish. The single write path.
    pub fn apply(&self, commit: Commit) -> Result<Vec<Event>> {
        self.apply_held(self.hold(), commit)
    }

    /// Take the commit lock before building a commit. A value read from the
    /// store that the same commit then writes - an inbox seq - must be read
    /// while this is held: read outside it, a concurrent commit can claim the
    /// same seq first and the store rejects the whole commit. `push_inbox`
    /// asks for one, so the compiler carries the requirement to its callers.
    pub(crate) fn hold(&self) -> CommitHold<'_> {
        self.commit_lock.hold()
    }

    /// Whether a commit is being built right now; stale once read. Race tests
    /// only, to tell a read under the hold from one outside it.
    #[doc(hidden)]
    pub fn commit_gate_held(&self) -> bool {
        self.commit_lock.is_held()
    }

    /// Apply a commit built under `hold`, then publish.
    pub(crate) fn apply_held(&self, hold: CommitHold<'_>, commit: Commit) -> Result<Vec<Event>> {
        if commit.is_empty() {
            return Ok(Vec::new());
        }
        let events = self.store.commit(commit)?;
        // Published before the next commit can land, so subscribers see the
        // log in seq order; the send never blocks, so the hold stays short.
        for event in &events {
            self.bus.publish_event(event.clone());
        }
        drop(hold);
        self.notify.notify_one();
        Ok(events)
    }

    pub fn store(&self) -> &Arc<dyn Store> {
        &self.store
    }

    /// Read the store while no commit is being built, passing the log position
    /// `read` sees. Keep `read` short; every writer waits for it.
    pub fn read_at_rest<T>(&self, read: impl FnOnce(&dyn Store, u64) -> Result<T>) -> Result<T> {
        let _hold = self.hold();
        let last_seq = self.store.last_seq()?;
        read(self.store.as_ref(), last_seq)
    }

    pub fn models(&self) -> &Arc<Models> {
        &self.models
    }

    pub fn tools(&self) -> &Arc<ToolRegistry> {
        &self.tools
    }

    pub fn memory(&self) -> &MemoryService {
        &self.memory
    }

    pub fn config(&self) -> &RuntimeConfig {
        &self.config
    }

    /// Resource keys mutating tool calls hold right now, across agents.
    pub fn resource_holdings(&self) -> Vec<aether_core::lease::ResourceHolding> {
        self.resources.holdings()
    }

    pub fn default_model(&self) -> Option<ModelRef> {
        self.default_model.read().clone()
    }

    pub fn set_default_model(&self, model: Option<ModelRef>) {
        *self.default_model.write() = model;
    }

    pub fn started_at(&self) -> Timestamp {
        self.started_at
    }

    /// The authored model profile for a resolved model, or `None` when the
    /// registry declares no family for it or that family's profile is off.
    /// Native harnesses own their coding guidance, even when API profiles
    /// are enabled globally.
    pub(crate) fn model_profile(&self, resolved: &aether_model::ResolvedModel) -> Option<String> {
        if resolved.provider.protocol.is_engine() {
            return None;
        }
        let family = resolved.family()?;
        if !self.models.config().model.profiles.enabled(family) {
            return None;
        }
        resolved.profile_text()
    }

    /// Health of every configured MCP server, empty when none are configured.
    pub fn mcp_health(&self) -> Vec<aether_core::mcp::McpServerHealth> {
        self.tools.mcp().map(|m| m.health()).unwrap_or_default()
    }

    /// Record an MCP server's transition. The manager reports every change on
    /// its channel; this is the only place those become durable events, so a
    /// server that dies leaves a trace instead of just missing tools.
    pub fn record_mcp_state(&self, health: &aether_core::mcp::McpServerHealth) -> Result<()> {
        let kind = match health.state {
            aether_core::mcp::McpServerState::Ready => EventKind::McpServerConnected {
                server: health.name.clone(),
                tools: health.tools.iter().map(|t| t.name.clone()).collect(),
            },
            aether_core::mcp::McpServerState::Failed => EventKind::McpServerFailed {
                server: health.name.clone(),
                error: health
                    .error
                    .clone()
                    .unwrap_or_else(|| "unavailable".to_owned()),
            },
            // Nothing happened yet; there is nothing to record.
            aether_core::mcp::McpServerState::Unknown => return Ok(()),
        };
        self.apply(Commit::new().event(EventDraft::new(Actor::System, kind)))?;
        Ok(())
    }

    pub fn subscribe(&self) -> tokio::sync::broadcast::Receiver<BusMessage> {
        self.bus.subscribe()
    }

    /// Stop scheduling, cancel running turns, and record the stop.
    pub async fn shutdown(&self) {
        self.shutdown.cancel();
        let tokens: Vec<CancellationToken> = self
            .sched
            .lock()
            .running
            .values()
            .map(|r| r.cancel.clone())
            .collect();
        for token in tokens {
            token.cancel();
        }
        let deadline = Instant::now() + Duration::from_secs(10);
        while !self.sched.lock().running.is_empty() && Instant::now() < deadline {
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
        // Nothing this daemon started may outlive it: after exit the registry
        // is empty, so a survivor would be unreachable forever.
        let background_processes_stopped = match self.tools.background() {
            Some(processes) => processes.stop_all().await,
            None => 0,
        };
        if let Err(e) = self.apply(Commit::new().event(EventDraft::new(
            Actor::System,
            EventKind::DaemonStopped {
                background_processes_stopped,
            },
        ))) {
            tracing::error!(error = %e, "failed to record daemon stop");
        }
    }
}

#[cfg(test)]
mod model_profile_tests;

/// A runtime over an in-memory store and an empty config directory, for the
/// crate's own unit tests. Integration tests use `tests/kernel.rs`'s harness;
/// this exists for the tests that reach into scheduler state directly. The
/// caller holds the config directory for as long as it holds the runtime.
#[cfg(test)]
pub(crate) async fn test_runtime(
    configure: impl FnOnce(&mut RuntimeConfig),
) -> (Arc<Runtime>, tempfile::TempDir) {
    let config_dir = tempfile::tempdir().expect("config dir");
    test_runtime_in(config_dir, configure).await
}

/// [`test_runtime`] over a config directory the caller has already written,
/// for a test whose models come from its own `providers.toml`.
#[cfg(test)]
pub(crate) async fn test_runtime_in(
    config_dir: tempfile::TempDir,
    configure: impl FnOnce(&mut RuntimeConfig),
) -> (Arc<Runtime>, tempfile::TempDir) {
    let mut config = RuntimeConfig {
        holder: "unit-test".into(),
        max_concurrent_turns: 4,
        default_model: Some(ModelRef::new("anthropic", "claude-sonnet-5-5")),
        ..Default::default()
    };
    configure(&mut config);
    let runtime = Runtime::start_with_engines(
        config,
        Arc::new(aether_store::SqliteStore::open_in_memory().expect("store")),
        Arc::new(
            aether_model::Models::load(Some(config_dir.path().to_path_buf())).expect("models"),
        ),
        Arc::new(ToolRegistry::with_builtins()),
        None,
        HashMap::new(),
    )
    .await
    .expect("runtime");
    (runtime, config_dir)
}
