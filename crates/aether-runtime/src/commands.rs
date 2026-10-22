//! State-changing operations available to clients. Each validates, builds one
//! atomic commit, and applies it. Nothing here calls a model.

use aether_core::agent::{Agent, AgentConfig, ApprovalPolicy, ExecutionBackend, Scope};
use aether_core::budget::RunSummary;
use aether_core::id::{AgentId, RunId, TaskId};
use aether_core::lease::ResourceHolding;
use aether_core::model::{Effort, ModelRef, Usage};
use aether_core::provider::ProviderHealth;
use aether_core::task::{Priority, Task};
use aether_core::tool::ToolExposure;
use aether_store::Commit;

mod approvals;
mod archive;
mod creation;
mod messaging;
mod pause;
mod recovery;
mod repository;
mod status;
mod tasks;
mod termination;
mod update;
mod validation;

pub(crate) use recovery::COMPACTION_WAKE;
pub(crate) use repository::{checkout_repository_id, repository_id};

/// How many transcript messages a restarted agent inherits: enough to see
/// what it was in the middle of, while the checkpoint carries everything
/// older.
pub const RESTART_TRANSCRIPT_TAIL: usize = 20;

#[derive(Debug, Clone, Default)]
pub struct CreateAgentSpec {
    pub role: String,
    pub name: Option<String>,
    pub objective: Option<String>,
    pub model: Option<ModelRef>,
    pub fallbacks: Vec<ModelRef>,
    pub scope: Option<Scope>,
    /// A checkout whose derived repository id becomes `scope.repository`;
    /// exclusive with naming `scope.repository` outright. Never stored.
    pub repository_root: Option<std::path::PathBuf>,
    pub parent: Option<AgentId>,
    pub config: Option<AgentConfig>,
    /// Approval policy asked for outright. `None` inherits the parent's, or
    /// the daemon's configured default for a root.
    pub approval: Option<ApprovalPolicy>,
    pub run: Option<RunId>,
    pub start: bool,
}

pub use aether_core::inbox::MessageDelivery;
pub use messaging::pending_question;
pub(crate) use messaging::{check_payload_size, check_text_size};

/// What a pause moved (§3.1). `changed` is empty when every target was
/// already paused; `mid_turn` names those still finishing a turn, so a caller
/// can say when the pause takes effect instead of implying it already has.
#[derive(Debug, Clone, Default, serde::Serialize, serde::Deserialize)]
pub struct PauseOutcome {
    pub changed: Vec<Agent>,
    pub mid_turn: Vec<AgentId>,
}

#[derive(Debug, Clone, Default)]
pub struct CreateTaskSpec {
    pub objective: String,
    pub owner: Option<AgentId>,
    pub parent_task: Option<TaskId>,
    pub dependencies: Vec<TaskId>,
    pub priority: Priority,
}

#[derive(Debug, Clone, Default)]
pub struct AgentUpdate {
    pub name: Option<String>,
    pub model: Option<ModelRef>,
    /// Replaces the fallback chain; an empty list clears it.
    pub fallbacks: Option<Vec<ModelRef>>,
    pub effort: Option<Effort>,
    pub instructions: Option<String>,
    pub max_turns: Option<u32>,
    /// Token limit over the agent and its subtree; `Some(0)` clears it.
    pub token_budget: Option<u64>,
    /// Spend limit in dollars over the agent and its subtree; `Some(0)` clears it.
    pub cost_budget: Option<f64>,
    pub tools: Option<ToolExposure>,
    pub approval: Option<ApprovalPolicy>,
    pub priority: Option<Priority>,
    /// Cap on descendants running at once; `Some(0)` clears it.
    pub concurrency: Option<usize>,
    /// Wall-clock stop: set or move it, or clear it.
    pub deadline: Option<aether_core::agent::DeadlineUpdate>,
    pub long_lived: Option<bool>,
    /// Scope knobs. Both must still narrow the parent's scope.
    pub network: Option<aether_core::agent::NetworkPolicy>,
    pub backend: Option<ExecutionBackend>,
}

#[derive(Debug, Clone, Default, PartialEq)]
pub struct RuntimeStatus {
    pub problems: Vec<String>,
    pub agents_live: u32,
    pub agents_running: u32,
    pub agents_waiting: u32,
    pub agents_blocked: u32,
    /// Blocked on provider exhaustion; also counted in `agents_blocked`.
    pub agents_suspended: u32,
    /// Live agents the operator has paused, whatever their status.
    pub agents_paused: u32,
    pub tasks_active: u32,
    pub pending_approvals: u32,
    pub usage_total: Usage,
    /// Approvals are off daemon-wide.
    pub yolo: bool,
    /// What a new agent's approval policy defaults to.
    pub approval_default: aether_core::agent::ApprovalDefault,
    pub network_default: aether_core::agent::NetworkPolicy,
    pub max_concurrent_turns: usize,
    /// Daemon-wide cap on simultaneous engine turns; `None` means unlimited.
    pub max_concurrent_engine_turns: Option<usize>,
    /// Turns running on an engine right now, against that cap.
    pub engine_turns_running: u32,
    /// Daemon-wide dollar budget every run is held to; `None` means none.
    pub run_budget_usd: Option<f64>,
    pub run_hard_stop_usd: Option<f64>,
    pub run_hard_stop_tokens: Option<u64>,
    /// Adapter labels of the engines found on this machine, e.g. `codex-app/0.153.4`.
    pub engines: Vec<String>,
    /// Per-provider concurrency and health, for anyone deciding where work
    /// can go right now.
    pub providers: Vec<ProviderHealth>,
    /// Resource keys mutating tool calls hold right now.
    pub resource_leases: Vec<ResourceHolding>,
    /// Spend and forecast for every run that still has a live agent.
    pub runs: Vec<RunSummary>,
    /// Configured MCP servers and their health.
    pub mcp: Vec<aether_core::mcp::McpServerHealth>,
    /// Set while the store's filesystem is below its floor and new turns wait.
    pub low_disk: Option<aether_core::disk::LowDisk>,
}

/// What `create_agent_with_preflight` must verify itself, under the hold the
/// create commits under. A caller that checked earlier cannot serialize its
/// check against a concurrent create; this can.
#[derive(Debug, Clone, Copy, Default)]
pub(crate) struct Preflight<'a> {
    /// Model and network policy the caller already resolved. A mismatch means
    /// they changed mid-flight, and the caller is told to retry.
    pub checked: Option<(&'a ModelRef, aether_core::agent::NetworkPolicy)>,
    /// Hold the new agent's run to the daemon's `run_budget_usd`. Set by
    /// `delegate`; a person creating an agent is never refused for it.
    pub run_budget: bool,
    /// Only the delegation path can authorize a managed worktree outside its
    /// parent's physical root. Ordinary create requests never set this.
    pub isolated_workspace: Option<&'a std::path::Path>,
}

/// A validated agent not yet written: `commit` holds its creation events and
/// task, and lacks only the agent's own `UpsertAgent`.
pub(crate) struct PreparedAgent {
    pub agent: Agent,
    pub task: Option<Task>,
    pub commit: Commit,
    /// The parent row `agent` was derived from, read unheld: creation
    /// re-checks it under its hold before committing.
    pub parent: Option<Agent>,
}

/// An agent's background processes, waiting to be ended. Held between the
/// terminal transition and its commit: the agent is reported terminal only
/// once, and its processes are stopped only after that is durable.
#[must_use = "an ended agent must be reaped after its terminal commit"]
pub(crate) struct PendingReap(AgentId);

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::panic)]
mod preflight_tests;
