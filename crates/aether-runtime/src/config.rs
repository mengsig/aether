use aether_core::model::ModelRef;
use aether_tools::ExecutionPolicy;
use std::{path::PathBuf, sync::Arc, time::Duration};

/// Model turns in flight at once, unless configured otherwise. A person's
/// message always gets a turn on top of this.
pub const DEFAULT_MAX_CONCURRENT_TURNS: usize = aether_model::config::DEFAULT_MAX_CONCURRENT_TURNS;

/// Maximum time to acquire a tool resource lock unless configured otherwise.
pub const DEFAULT_RESOURCE_WAIT_TIMEOUT: Duration = Duration::from_secs(30 * 60);

/// After a harness action ends an engine turn, how long the engine gets to
/// stop on its own before its process is stopped for it.
pub const DEFAULT_ENGINE_PARK_GRACE: Duration = Duration::from_secs(20);

/// Shortest wait before a suspended agent probes its exhausted provider.
pub const DEFAULT_EXHAUSTION_PROBE_FLOOR: Duration = Duration::from_secs(60);

/// How long a native engine turn keeps waiting out network, timeout, capacity
/// and server failures, from the first, before the agent fails.
pub const DEFAULT_ENGINE_TRANSIENT_RETRY_WINDOW: Duration = Duration::from_secs(2 * 60 * 60);

/// First wait after a transient native engine failure; each later one doubles.
pub const DEFAULT_ENGINE_TRANSIENT_RETRY_FLOOR: Duration = Duration::from_secs(15);

/// How long a native engine turn may stay silent, with no tool running,
/// before the stall watchdog interrupts it.
pub const DEFAULT_ENGINE_STALL_TIMEOUT: Duration = Duration::from_secs(15 * 60);

#[derive(Debug, Clone)]
pub struct RuntimeConfig {
    /// Identity recorded on leases; the daemon instance id.
    pub holder: String,
    /// The daemon instance id itself, as the store minted it. Stable across
    /// restarts on one data directory, so containers this daemon starts can
    /// be told from another daemon's.
    pub instance_id: Arc<str>,
    pub max_concurrent_turns: usize,
    /// Where message images are stored, one file per digest. `None` refuses
    /// a message that carries images.
    pub attachment_dir: Option<PathBuf>,
    /// Cap on simultaneous turns carried by an engine (Claude Code, Codex),
    /// within `max_concurrent_turns`. `None` leaves engine turns bounded only
    /// by the other limits, which is what the daemon did before the knob.
    pub max_concurrent_engine_turns: Option<usize>,
    /// Dollars one run may spend before `delegate` refuses to start new work
    /// in it. `None` means no run budget. Turns already running are never
    /// cancelled for it.
    pub run_budget_usd: Option<f64>,
    /// Spend at which a run's live agents are interrupted and no new turn is
    /// admitted. Unset means nothing changes.
    pub run_hard_stop: aether_core::budget::HardStop,
    pub lease_ttl: Duration,
    /// Model used when an agent is created without one.
    pub default_model: Option<ModelRef>,
    /// Workspace root for agents created without a scope.
    pub workspace_root: Option<PathBuf>,
    /// Dedicated checkout directory for automatic GitHub thread agents.
    pub github_checkout_root: Option<PathBuf>,
    /// Private location for detached worktrees made by isolated delegation.
    pub delegated_workspace_root: Option<PathBuf>,
    /// Real path of the per-agent temp dirs' root, already prepared and claimed
    /// for `instance_id`. `None` leaves agents the daemon's own temp dir.
    pub agent_temp_root: Option<PathBuf>,
    /// Root of the per-repository build cache partitions. `None` gives agents
    /// none, except a directory their client named.
    pub build_cache_root: Option<PathBuf>,
    pub network_default: aether_core::agent::NetworkPolicy,
    pub execution: ExecutionPolicy,
    /// Consecutive failed invocations before an agent is failed.
    pub max_consecutive_failures: u32,
    /// How external engines reach this daemon's harness tools.
    pub bridge: Option<BridgeConfig>,
    /// Optional operator-requested check-in for task/message waits.
    /// None leaves them asleep until their condition changes.
    pub wait_timeout: Option<Duration>,
    /// Bound on acquiring tool resource locks, independent of idle agents.
    pub resource_wait_timeout: Duration,
    /// Grace an engine gets to end a turn a harness action already ended,
    /// before the runtime stops the process itself.
    pub engine_park_grace: Duration,
    /// First interval an agent suspended on provider exhaustion waits before
    /// probing; each failed probe doubles it up to the 30 minute cap.
    pub exhaustion_probe_floor: Duration,
    /// Wall time, from its first transient failure (network, timeout,
    /// capacity, server), that a native engine turn keeps retrying before the
    /// agent fails. Progress does not extend it. Zero fails on the first one.
    pub engine_transient_retry_window: Duration,
    /// First backoff after a transient native failure; doubles per retry up
    /// to a five minute cap.
    pub engine_transient_retry_floor: Duration,
    /// Silence, with no tool running, after which a native engine turn is
    /// interrupted and resumed. Zero turns the watchdog off.
    pub engine_stall_timeout: Duration,
    /// `host:port` targets probed before relaunching an engine after a
    /// network failure. `None` probes the engine's provider host; empty skips
    /// the probe.
    pub engine_probe_targets: Option<Vec<String>>,
    /// Approvals off for every agent: tool calls run, engines run unsandboxed.
    /// Stronger than `approval_default`: no agent may turn approvals back on.
    pub yolo: bool,
    /// What an agent gets when it inherits no policy from a parent and the
    /// caller named none.
    pub approval_default: aether_core::agent::ApprovalDefault,
    /// How long artifact bytes are kept.
    pub retention: aether_core::artifact::RetentionPolicy,
    /// Store file and log root used by filesystem maintenance.
    pub storage_paths: Option<crate::storage::StoragePaths>,
    /// Optional semantic pass over memory retrieval. `None` retrieves
    /// structurally and lexically, exactly as before.
    pub memory_index: Option<Arc<dyn aether_memory::MemoryIndex>>,
    /// Hold new turns while the store's filesystem runs low. `None` never
    /// measures it.
    pub disk_guard: Option<DiskGuard>,
    /// Masks tool and engine output before it is recorded. The daemon seeds
    /// it with the secret values it knows; its `Debug` never prints them.
    pub redactor: aether_tools::Redactor,
}

/// Where free space is measured, and how much new turns wait for.
#[derive(Debug, Clone)]
pub struct DiskGuard {
    /// The store, or any path on its filesystem.
    pub path: PathBuf,
    pub floor: aether_core::disk::DiskFloor,
}

/// The MCP bridge an engine spawns: the `aether` binary plus the daemon socket.
#[derive(Debug, Clone)]
pub struct BridgeConfig {
    pub command: PathBuf,
    pub socket: PathBuf,
}

impl Default for RuntimeConfig {
    fn default() -> Self {
        Self {
            holder: format!("aether-{}", std::process::id()),
            instance_id: "unnamed-instance".into(),
            max_concurrent_turns: DEFAULT_MAX_CONCURRENT_TURNS,
            attachment_dir: None,
            max_concurrent_engine_turns: None,
            run_budget_usd: None,
            run_hard_stop: Default::default(),
            lease_ttl: Duration::from_secs(120),
            default_model: None,
            workspace_root: None,
            github_checkout_root: None,
            delegated_workspace_root: None,
            agent_temp_root: None,
            build_cache_root: None,
            network_default: aether_model::config::NetworkSection::default().default,
            execution: ExecutionPolicy::default(),
            max_consecutive_failures: 3,
            bridge: None,
            wait_timeout: None,
            resource_wait_timeout: DEFAULT_RESOURCE_WAIT_TIMEOUT,
            engine_park_grace: DEFAULT_ENGINE_PARK_GRACE,
            exhaustion_probe_floor: DEFAULT_EXHAUSTION_PROBE_FLOOR,
            engine_transient_retry_window: DEFAULT_ENGINE_TRANSIENT_RETRY_WINDOW,
            engine_transient_retry_floor: DEFAULT_ENGINE_TRANSIENT_RETRY_FLOOR,
            engine_stall_timeout: DEFAULT_ENGINE_STALL_TIMEOUT,
            engine_probe_targets: None,
            yolo: false,
            approval_default: aether_core::agent::ApprovalDefault::default(),
            retention: aether_core::artifact::RetentionPolicy::default(),
            storage_paths: None,
            memory_index: None,
            disk_guard: None,
            redactor: aether_tools::Redactor::default(),
        }
    }
}
