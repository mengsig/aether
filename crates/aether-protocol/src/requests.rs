use super::{
    AgentConfig, AgentId, AgentStatusKind, ApprovalDecision, ApprovalPolicy, DeadlineUpdate,
    Deserialize, Effort, ExecutionBackend, MemoryId, MemoryKind, MemoryRecorder, MemoryScope,
    ModelRef, NetworkPolicy, Priority, RuleMatch, RuleScope, RunId, Scope, Serialize, TaskId,
    TaskStatusKind, ToolExposure,
};

/// Request to create an agent, optionally with an initial task.
/// A child requires explicit `config.effort`: low, medium, high, xhigh or max.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, schemars::JsonSchema)]
pub struct CreateAgent {
    pub role: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    /// When set, a task with this objective is created and assigned.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub objective: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model: Option<ModelRef>,
    /// Models to try after `model`, in order (VISION §4.5).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub fallbacks: Vec<ModelRef>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub scope: Option<Scope>,
    /// An absolute path to a checkout whose repository id the daemon derives
    /// and stores as `scope.repository`, exactly as for an agent working
    /// there. Input only; exclusive with `scope.repository`. A daemon without
    /// `features::REPOSITORY_ROOT` ignores it, so check that first.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub repository_root: Option<std::path::PathBuf>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub parent: Option<AgentId>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub config: Option<AgentConfig>,
    /// Approval policy asked for outright. Absent inherits the parent's, or
    /// the daemon's `[approval] default` for a root. Wins over
    /// `config.approval`, which older clients set.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub approval: Option<ApprovalPolicy>,
    /// Attach to an existing run instead of starting a new one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub run: Option<RunId>,
    /// Move to Ready immediately (default true).
    #[serde(default = "default_true")]
    pub start: bool,
    /// Makes this submission retryable: a second identical request under the
    /// same key returns what the first created instead of creating it again.
    /// A key reused for a different request is refused (§13.6).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub idempotency_key: Option<String>,
}

pub(super) fn default_true() -> bool {
    true
}

impl Default for CreateAgent {
    fn default() -> Self {
        Self {
            role: String::new(),
            name: None,
            objective: None,
            model: None,
            fallbacks: Vec::new(),
            scope: None,
            repository_root: None,
            parent: None,
            config: None,
            approval: None,
            run: None,
            start: true,
            idempotency_key: None,
        }
    }
}

/// Partial update to a live agent. Only set fields change.
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize, schemars::JsonSchema)]
pub struct AgentPatch {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model: Option<ModelRef>,
    /// Replaces the fallback chain; an empty list clears it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub fallbacks: Option<Vec<ModelRef>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub effort: Option<Effort>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub instructions: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_turns: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub token_budget: Option<u64>,
    /// Spend limit in US dollars over the agent and its subtree.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cost_budget: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tools: Option<ToolExposure>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub approval: Option<ApprovalPolicy>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub priority: Option<Priority>,
    /// Cap on descendants running at once; 0 clears it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub concurrency: Option<usize>,
    /// Wall-clock stop for the agent: `{"set": <rfc3339>}` moves it,
    /// `"clear"` removes it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub deadline: Option<DeadlineUpdate>,
    /// Stay alive after the current task and wait for more work.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub long_lived: Option<bool>,
    /// Scope knobs; both must still narrow the parent's scope.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub network: Option<NetworkPolicy>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub backend: Option<ExecutionBackend>,
}

#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize, schemars::JsonSchema)]
pub struct CreateTask {
    pub objective: String,
    /// Assign to this agent; when absent the task waits for an owner.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub owner: Option<AgentId>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub parent_task: Option<TaskId>,
    #[serde(default)]
    pub dependencies: Vec<TaskId>,
    #[serde(default)]
    pub priority: Priority,
    /// Makes this submission retryable: a second identical request under the
    /// same key returns what the first created instead of creating it again.
    /// A key reused for a different request is refused (§13.6).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub idempotency_key: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize, schemars::JsonSchema)]
pub struct AgentQuery {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub parent: Option<AgentId>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub run: Option<RunId>,
    #[serde(default)]
    pub statuses: Vec<AgentStatusKind>,
    #[serde(default)]
    pub live_only: bool,
    /// Include agents cleared from the view. Off by default, so a client
    /// that has not heard of archiving sees the cleared view.
    #[serde(default)]
    pub include_archived: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize, schemars::JsonSchema)]
pub struct TaskQuery {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub owner: Option<AgentId>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub run: Option<RunId>,
    #[serde(default)]
    pub statuses: Vec<TaskStatusKind>,
}

#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize, schemars::JsonSchema)]
pub struct EventQuery {
    /// GetEvents only: newest first, with seq strictly below this cursor.
    /// Use u64::MAX for the latest page; omit for ascending events after `after`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub before: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub agent: Option<AgentId>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub task: Option<TaskId>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub run: Option<RunId>,
    #[serde(default)]
    pub families: Vec<String>,
    #[serde(default)]
    pub types: Vec<String>,
    /// With `agent`: the only event types taken from its children, so a
    /// history page is not spent on child activity the client never shows.
    /// The agent's own events are unaffected. Empty takes every type.
    /// A daemon older than this field ignores it and sends every type.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub child_types: Vec<String>,
}

impl From<EventQuery> for aether_store::EventFilter {
    fn from(q: EventQuery) -> Self {
        Self {
            agent: q.agent,
            // A client watching an agent follows the work it delegated.
            exclude_children: false,
            task: q.task,
            run: q.run,
            families: q.families,
            types: q.types,
            child_types: q.child_types,
        }
    }
}

impl From<AgentQuery> for aether_store::AgentFilter {
    fn from(q: AgentQuery) -> Self {
        Self {
            parent: q.parent,
            run: q.run,
            statuses: q.statuses,
            live_only: q.live_only,
            budgeted_only: false,
            exclude_archived: !q.include_archived,
        }
    }
}

impl From<TaskQuery> for aether_store::TaskFilter {
    fn from(q: TaskQuery) -> Self {
        Self {
            owner: q.owner,
            parent_task: None,
            run: q.run,
            statuses: q.statuses,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, schemars::JsonSchema)]
pub struct MemoryQuerySpec {
    /// Who recorded the memory, not which agents may retrieve it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub provenance_agent: Option<AgentId>,
    /// Who recorded the memory, by kind: `operator` is a person on this
    /// machine. A daemon without `features::MEMORY_RECORDED_BY` ignores it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub recorded_by: Option<MemoryRecorder>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub scope: Option<MemoryScope>,
    #[serde(default)]
    pub kinds: Vec<MemoryKind>,
    #[serde(default)]
    pub tags: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub text: Option<String>,
    #[serde(default)]
    pub include_superseded: bool,
    /// Include memories a person curated out. A browser wants them; nothing
    /// a model sees ever does.
    #[serde(default)]
    pub include_forgotten: bool,
    /// Select pending proposals instead of admitted memories. Needs `memory_proposals`.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub proposed: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub id: Option<MemoryId>,
    #[serde(default = "default_limit")]
    pub limit: usize,
}

fn default_limit() -> usize {
    20
}

impl From<MemoryQuerySpec> for aether_store::MemoryQuery {
    fn from(q: MemoryQuerySpec) -> Self {
        Self {
            scope: q.scope,
            provenance_agent: q.provenance_agent,
            recorded_by: q.recorded_by,
            // A browser asks what an agent in the scope can see, which
            // includes the global memories every scope sees.
            exact_scope: false,
            // Role notes are listed by naming their own scope, as an agent
            // in the repository without that role does not see them.
            nested_scopes: false,
            kinds: q.kinds,
            tags: q.tags,
            text: q.text,
            // Not reachable over the socket: an exact-text lookup is how
            // seeding asks whether one note is stored, not a browse filter.
            exact_text: None,
            include_superseded: q.include_superseded,
            include_forgotten: q.include_forgotten,
            proposed: q.proposed,
            id: q.id,
            limit: q.limit,
        }
    }
}

/// Publish UTF-8 evidence associated with a durable task. The daemon owns
/// blob storage and derives the agent/run association from that task.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
pub struct PublishArtifact {
    pub task: TaskId,
    pub name: String,
    pub summary: String,
    pub content_type: String,
    pub text: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
pub struct ArtifactQuery {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub agent: Option<AgentId>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub task: Option<TaskId>,
    /// Only artifacts whose bytes retention has not dropped.
    #[serde(default)]
    pub with_bytes_only: bool,
    #[serde(default = "default_limit")]
    pub limit: usize,
}

impl Default for ArtifactQuery {
    fn default() -> Self {
        Self {
            name: None,
            agent: None,
            task: None,
            with_bytes_only: false,
            limit: default_limit(),
        }
    }
}

impl From<ArtifactQuery> for aether_store::ArtifactFilter {
    fn from(q: ArtifactQuery) -> Self {
        Self {
            name: q.name,
            agent: q.agent,
            task: q.task,
            with_bytes_only: q.with_bytes_only,
            limit: q.limit,
        }
    }
}

/// How to remember the answer a person just gave. The defaults are the
/// narrowest reading of it: the answered agent, this tool, and what this
/// call touched. Anything wider is asked for explicitly.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize, schemars::JsonSchema)]
pub struct RememberSpec {
    /// Where it applies; absent means the answered agent alone.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub scope: Option<RuleScope>,
    /// Cover every call of the tool instead of the answered call's argument
    /// and resources.
    #[serde(default)]
    pub tool_wide: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub note: Option<String>,
}

/// A remembered decision written directly, without answering a call first.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
pub struct NewApprovalRule {
    pub decision: ApprovalDecision,
    #[serde(rename = "match")]
    pub matcher: RuleMatch,
    pub scope: RuleScope,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub note: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub idempotency_key: Option<String>,
}
