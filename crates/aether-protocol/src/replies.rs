use super::remote::{
    Attention, Device, GitSummary, Pairing, PathCompletions, PushConfig, RemoteStatus, Workspace,
    WorkspaceValidation,
};
use super::{
    Agent, AgentId, ApprovalDefault, ApprovalRequest, ApprovalRule, Artifact, BTreeMap,
    CapabilityProfile, Checkpoint, Deserialize, Event, ExportSummary, ImportSummary, InboxItem,
    Invocation, InvocationId, InvocationSnapshot, Lease, LowDisk, McpServerHealth, MemoryRecord,
    Message, MessageId, ModelRef, NetworkPolicy, ProviderHealth, ReplayFrame, ResourceHolding,
    RunId, RunReport, RunSummary, Serialize, Task, TaskId, Timestamp, UnprovenAbsence, Usage,
    Welcome,
};

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, schemars::JsonSchema)]
pub struct DaemonStatus {
    pub instance_id: String,
    pub version: String,
    pub pid: u32,
    pub started_at: Timestamp,
    pub last_seq: u64,
    pub agents_live: u32,
    pub agents_running: u32,
    pub agents_waiting: u32,
    pub agents_blocked: u32,
    /// Blocked on provider exhaustion; also counted in `agents_blocked`.
    #[serde(default)]
    pub agents_suspended: u32,
    /// Live agents the operator has paused, whatever their status.
    #[serde(default)]
    pub agents_paused: u32,
    pub tasks_active: u32,
    pub pending_approvals: u32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub default_model: Option<ModelRef>,
    pub usage_total: Usage,
    /// Approvals are off daemon-wide.
    #[serde(default)]
    pub yolo: bool,
    /// Engines found on the daemon's machine, as adapter labels.
    #[serde(default)]
    pub engines: Vec<String>,
    /// Per-provider concurrency and health as the scheduler sees it.
    #[serde(default)]
    pub providers: Vec<ProviderHealth>,
    /// Resource keys mutating tool calls hold right now, across agents.
    #[serde(default)]
    pub resource_leases: Vec<ResourceHolding>,
    /// Spend and forecast for every run that still has a live agent.
    #[serde(default)]
    pub runs: Vec<RunSummary>,
    /// What a new agent's approval policy defaults to.
    #[serde(default)]
    pub approval_default: ApprovalDefault,
    /// Network scope used for new roots without an explicit scope.
    #[serde(default)]
    pub network_default: NetworkPolicy,
    /// Effective daemon-wide model-turn limit; absent on older daemons.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_concurrent_turns: Option<usize>,
    /// Daemon-wide cap on simultaneous engine (Claude Code, Codex) turns.
    /// Absent when no cap is configured, and on older daemons.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_concurrent_engine_turns: Option<usize>,
    /// Turns running on an engine right now, against that cap.
    #[serde(default)]
    pub engine_turns_running: u32,
    /// Dollars any one run may spend before new delegations are refused.
    /// Absent when no budget is configured, and on older daemons.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub run_budget_usd: Option<f64>,
    /// Configured MCP servers and their health.
    #[serde(default)]
    pub mcp: Vec<McpServerHealth>,
    /// Set while the store's filesystem is below its floor: new turns wait.
    /// Absent otherwise, and on older daemons.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub low_disk: Option<LowDisk>,
    /// The executable engines launch as the MCP bridge: the daemon's own
    /// build. Absent when engines have no bridge, and on older daemons.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub bridge: Option<String>,
}

/// One agent's copy of a fanned-out message.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
pub struct MessageDelivery {
    pub agent: AgentId,
    pub message: MessageId,
}

/// One node of the live graph.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, schemars::JsonSchema)]
pub struct TreeNode {
    pub agent: Agent,
    pub children: Vec<AgentId>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub task: Option<Task>,
    /// Tasks this agent is waiting on, resolved from its wait condition.
    #[serde(default)]
    pub waiting_on: Vec<TaskId>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub lease: Option<Lease>,
    /// Usage rolled up over this agent and its descendants.
    pub subtree_usage: Usage,
    /// Usage of child subtrees omitted from this tree (for example archived children).
    /// Kept separately so live clients can replace visible usage without losing it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub hidden_usage: Option<Usage>,
}

#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize, schemars::JsonSchema)]
pub struct AgentTree {
    pub roots: Vec<AgentId>,
    pub nodes: BTreeMap<AgentId, TreeNode>,
}

/// One row of `aether runs list`, projected from a run's agents and tasks.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, schemars::JsonSchema)]
pub struct RunListing {
    pub run: RunId,
    /// The root agent's task objective, when the run has one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub objective: Option<String>,
    pub agents: u32,
    pub agents_live: u32,
    /// How many of `agents` came out of an export. Non-zero means this run
    /// is an archive of another daemon's work, which the listing is the
    /// first place a person would look for.
    #[serde(default)]
    pub agents_imported: u32,
    pub usage: Usage,
    pub started_at: Timestamp,
    /// When anything in the run last changed.
    pub updated_at: Timestamp,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, schemars::JsonSchema)]
pub struct TranscriptEntry {
    pub seq: u64,
    pub at: Timestamp,
    pub message: Message,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub invocation: Option<InvocationId>,
}

impl From<aether_store::TranscriptEntry> for TranscriptEntry {
    fn from(e: aether_store::TranscriptEntry) -> Self {
        Self {
            seq: e.seq,
            at: e.at,
            message: e.message,
            invocation: e.invocation,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, schemars::JsonSchema)]
pub struct ModelInfo {
    /// Capability band within the provider; null when unranked.
    #[serde(default)]
    pub tier: Option<String>,
    pub model: ModelRef,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub aliases: Vec<String>,
    pub name: String,
    pub provider_name: String,
    pub capabilities: CapabilityProfile,
    /// Whether credentials resolve for the provider right now.
    pub configured: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub auth_source: Option<String>,
    pub is_default: bool,
}

/// One provider as a client sees it: what it is, whether it can be used, and
/// how it is behaving right now. `health` is live scheduler state, so a
/// provider that has done nothing since the daemon started reads as idle.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
pub struct ProviderInfo {
    pub provider: String,
    pub name: String,
    /// Wire protocol or engine, e.g. `anthropic-messages`, `claude-code`.
    pub protocol: String,
    /// Declared model family, which chooses the authored profile every model
    /// here is sent: `claude`, `gpt`, `codex`. `None` means no profile at
    /// all. A name, like `protocol`, so the wire type stays free of the
    /// registry. A model may declare its own; this is the provider's.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub family: Option<String>,
    /// Endpoint for an HTTP provider; empty for an engine, which owns its own.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub base_url: String,
    /// Inference runs through an official CLI under the person's own login.
    pub engine: bool,
    /// Models the registry lists; an open-list provider also takes others.
    pub models: u32,
    /// Whether the provider accepts any model id, not only the listed ones.
    pub open_model_list: bool,
    pub enabled: bool,
    /// Whether credentials resolve right now.
    pub configured: bool,
    /// Where the credential came from, e.g. `ANTHROPIC_API_KEY`, `claude login`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub auth_source: Option<String>,
    pub health: ProviderHealth,
    #[serde(default)]
    pub headroom: aether_core::provider::ProviderHeadroom,
}

impl ProviderInfo {
    /// Whether this provider can be used, in one word. Shared so `aether
    /// providers` and the TUI panel cannot drift apart.
    pub fn status_label(&self) -> &'static str {
        if !self.enabled {
            "off"
        } else if self.configured {
            "ready"
        } else {
            "not set"
        }
    }

    /// Models the registry lists; `+` marks a provider that also takes ids it
    /// does not list.
    pub fn model_count(&self) -> String {
        if self.open_model_list {
            format!("{}+", self.models)
        } else {
            self.models.to_string()
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
pub struct AuthInfo {
    pub provider: String,
    pub provider_name: String,
    pub configured: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source: Option<String>,
    /// Login methods the provider supports, e.g. `api_key`, `oauth`.
    #[serde(default)]
    pub methods: Vec<String>,
    #[serde(default)]
    pub subscription: bool,
    /// Why the provider is not ready, e.g. `claude not found`, `Not logged in`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub detail: Option<String>,
    /// The CLI that owns the login, when one does (`claude`, `codex`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub program: Option<String>,
}

// Adjacently tagged: internal tagging cannot carry sequence payloads.
#[allow(clippy::large_enum_variant)]
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(tag = "reply", content = "data", rename_all = "snake_case")]
pub enum Reply {
    StorageReport(serde_json::Value),
    RetentionPreview(aether_core::retention::RetentionPreview),
    GithubWatcher(aether_core::github::GithubWatcherStatus),
    Schedule(aether_core::schedule::Schedule),
    Schedules(Vec<aether_core::schedule::Schedule>),
    ScheduleHistory(Vec<aether_core::schedule::ScheduleOccurrence>),
    Welcome(Welcome),
    Status(DaemonStatus),
    Ok,
    /// Reply to `Interrupt`: whether a turn was running to stop.
    Interrupted {
        was_running: bool,
    },
    /// Reply to `Pause`: the agents it moved, root first, empty when they
    /// were already paused; `mid_turn` names those whose current turn must
    /// finish before the pause takes effect.
    Paused {
        agents: Vec<Agent>,
        mid_turn: Vec<AgentId>,
    },
    Agent(Agent),
    Agents(Vec<Agent>),
    Tree(AgentTree),
    Task(Task),
    Tasks(Vec<Task>),
    MessageSent {
        message: MessageId,
    },
    /// The note's event, by its sequence number in the log.
    NoteAppended {
        seq: u64,
    },
    Notification(aether_core::notification::Notification),
    /// Newest first.
    Notifications(Vec<aether_core::notification::Notification>),
    /// The answered notification, and the posting agent whose inbox the
    /// answer reached; `None` for a service, which reads the event.
    NotificationAnswered {
        notification: aether_core::notification::Notification,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        delivered_to: Option<AgentId>,
    },
    /// One entry per agent a fanned-out message reached, in delivery order.
    MessagesSent {
        deliveries: Vec<MessageDelivery>,
    },
    Approvals(Vec<ApprovalRequest>),
    ApprovalRule(ApprovalRule),
    ApprovalRules(Vec<ApprovalRule>),
    Subscribed {
        id: u64,
        last_seq: u64,
    },
    Events(Vec<Event>),
    Transcript(Vec<TranscriptEntry>),
    Inbox(Vec<InboxItem>),
    Snapshot(InvocationSnapshot),
    Invocation(Invocation),
    Invocations(Vec<Invocation>),
    Checkpoint(Option<Checkpoint>),
    Artifact(Artifact),
    Artifacts(Vec<Artifact>),
    ArtifactBytes {
        artifact: Artifact,
        base64: String,
        truncated: bool,
    },
    Memories(Vec<MemoryRecord>),
    RepositoryAliased(aether_core::repository::RepositoryAliasReport),
    RepositoryAliases(Vec<aether_core::repository::RepositoryAlias>),
    MemoriesInContext(aether_core::snapshot::MemoriesInContext),
    Memory(MemoryRecord),
    MemoryFeedback(aether_core::memory_feedback::MemoryFeedback),
    MemoryFeedbackList(aether_core::memory_feedback::MemoryFeedbackList),
    MemorySeeded {
        repository: String,
        report: aether_core::memory::SeedReport,
    },
    Runs(Vec<RunListing>),
    RunReport(Box<RunReport>),
    Replay(Vec<ReplayFrame>),
    RunExported(ExportSummary),
    RunImported(ImportSummary),
    McpServers(Vec<McpServerHealth>),
    Models(Vec<ModelInfo>),
    Providers(Vec<ProviderInfo>),
    ProviderUsage(serde_json::Value),
    Auth(Vec<AuthInfo>),
    EngineTool {
        content: String,
        is_error: bool,
    },
    EngineTools {
        tools: Vec<aether_core::model::ToolSpec>,
    },
    Attention(Attention),
    PathCompletions(PathCompletions),
    Workspaces(Vec<Workspace>),
    WorkspaceValidation(WorkspaceValidation),
    GitSummary(GitSummary),
    WorkspaceDiff(Box<aether_core::workspace_review::WorkspaceDiff>),
    WorkspaceIntegration(aether_core::workspace_review::WorkspaceIntegration),
    WorkspaceDiscarded(aether_core::workspace_review::WorkspaceDiscard),
    Remote(RemoteStatus),
    Pairing(Pairing),
    Device(Device),
    Devices(Vec<Device>),
    PushConfig(PushConfig),
    SubmissionResolved(SubmissionResolution),
}

/// What became of a keyed command, as far as the daemon can prove. Each
/// carries the daemon's `horizon` now; `retained_since` is for people to read.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(tag = "outcome", rename_all = "snake_case")]
pub enum SubmissionResolution {
    /// It ran. `reply` is what a duplicate under the same key is answered.
    Executed { reply: Box<Reply>, horizon: String },
    /// It never ran: no receipt, and no receipt recorded since the request's
    /// horizon was ever dropped. Sending it again under the same key is safe.
    AbsentSafe {
        horizon: String,
        retained_since: Timestamp,
    },
    /// No receipt, and nothing proves there never was one. Only a person can
    /// decide whether to send it again.
    Unknown {
        reason: UnprovenAbsence,
        horizon: String,
        retained_since: Timestamp,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum ErrorCode {
    /// Protocol version or handshake mismatch.
    Incompatible,
    /// The connection presented no token, or the wrong one. Only the
    /// transports that need one can answer this.
    Unauthorized,
    NotFound,
    InvalidTransition,
    InvalidArgument,
    /// The provider or model is not configured.
    Unavailable,
    Internal,
    /// The connection is authenticated, but its transport or role may not
    /// run this command.
    Forbidden,
    /// The command is part of the protocol and this build does not serve it.
    Unimplemented,
    /// Nothing named a model and the daemon has no default: no provider is
    /// signed in. A model named but not usable is refused with another code.
    NoModel,
}

#[derive(
    Debug, Clone, PartialEq, Eq, Serialize, Deserialize, thiserror::Error, schemars::JsonSchema,
)]
#[error("{code:?}: {message}")]
pub struct ErrorReply {
    pub code: ErrorCode,
    pub message: String,
}

impl ErrorReply {
    pub fn new(code: ErrorCode, message: impl Into<String>) -> Self {
        Self {
            code,
            message: message.into(),
        }
    }

    pub fn not_found(what: impl std::fmt::Display) -> Self {
        Self::new(ErrorCode::NotFound, format!("{what} not found"))
    }

    pub fn internal(err: impl std::fmt::Display) -> Self {
        Self::new(ErrorCode::Internal, err.to_string())
    }

    pub fn invalid(msg: impl Into<String>) -> Self {
        Self::new(ErrorCode::InvalidArgument, msg)
    }

    pub fn forbidden(msg: impl Into<String>) -> Self {
        Self::new(ErrorCode::Forbidden, msg)
    }

    pub fn unimplemented(what: impl std::fmt::Display) -> Self {
        Self::new(
            ErrorCode::Unimplemented,
            format!("{what} is not implemented in this build"),
        )
    }
}
