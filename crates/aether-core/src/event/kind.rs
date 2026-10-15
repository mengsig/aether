use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::agent::{AgentConfig, AgentStatusKind, Role, Scope, WaitCondition};
use crate::approval::{ApprovalRule, Revocation};
use crate::artifact::Artifact;
use crate::checkpoint::Checkpoint;
use crate::id::{
    AgentId, ApprovalId, ApprovalRuleId, ArtifactId, CheckpointId, InboxItemId, InvocationId,
    MemoryId, MessageId, RunId, SnapshotId, TaskId, ToolCallId,
};
use crate::idempotency::IdempotencyKey;
use crate::inbox::InboxItem;
use crate::lease::Lease;
use crate::memory::MemoryCandidate;
use crate::model::{ContentBlock, MessageRole, ModelRef, StopReason, Usage};
use crate::provider::QuotaWindow;
use crate::snapshot::ContextLayer;
use crate::task::{Priority, TaskResult};
use crate::time::Timestamp;
use crate::tool::{ApprovalDecision, ResourceKey, SideEffectClass, ToolCall, ToolOutcome};

use super::{Actor, CompactionTrigger, RecoveryClass};

/// Typed event payloads, grouped by family. The serialized `type` tag uses
/// dotted names (`agent.created`) so filters can match by family prefix.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(tag = "type")]
pub enum EventKind {
    // ----- agent lifecycle -------------------------------------------------
    #[serde(rename = "agent.created")]
    AgentCreated {
        role: Role,
        name: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        parent: Option<AgentId>,
        model: ModelRef,
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        fallbacks: Vec<ModelRef>,
        scope: Scope,
    },
    #[serde(rename = "agent.ready")]
    AgentReady,
    #[serde(rename = "agent.started")]
    AgentStarted { turn: u32 },
    #[serde(rename = "agent.turn_completed")]
    AgentTurnCompleted {
        turn: u32,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        invocation: Option<InvocationId>,
        next: AgentStatusKind,
    },
    /// The operator stopped the running turn. The agent keeps its
    /// transcript and waits for the next message.
    #[serde(rename = "agent.interrupted")]
    AgentInterrupted { turn: u32 },
    #[serde(rename = "agent.waiting")]
    AgentWaiting { condition: WaitCondition },
    #[serde(rename = "agent.resumed")]
    AgentResumed {
        reason: String,
        /// Set when a subtree resume on that ancestor reached this agent.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        via: Option<AgentId>,
    },
    /// The operator paused the agent (VISION §3.1). Durable and orthogonal to
    /// the status: it takes no turns until resumed, and its inbox fills.
    #[serde(rename = "agent.paused")]
    AgentPaused {
        /// Set when a subtree pause on that ancestor reached this agent.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        via: Option<AgentId>,
    },
    /// The agent was cleared out of the live view, or brought back.
    /// A view mark: nothing is removed, and replay is unaffected.
    #[serde(rename = "agent.archived")]
    AgentArchived { archived: bool },
    #[serde(rename = "agent.blocked")]
    AgentBlocked {
        reason: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        dependency: Option<TaskId>,
    },
    #[serde(rename = "agent.completed")]
    AgentCompleted {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        summary: Option<String>,
    },
    #[serde(rename = "agent.failed")]
    AgentFailed { error: String },
    /// A failed or cancelled agent was recreated from its last checkpoint
    /// (§3.8). Recorded on both agents so either end of the link is visible
    /// from the other's event stream.
    #[serde(rename = "agent.restarted")]
    AgentRestarted {
        from: AgentId,
        to: AgentId,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        checkpoint: Option<CheckpointId>,
        /// Transcript messages carried over from the old agent.
        transcript_tail: usize,
        /// Messages after the copied checkpoint that did not come across: the
        /// copy's provenance covers them, but no summary does.
        #[serde(default)]
        transcript_dropped: usize,
        /// Inbox items the old agent never consumed, re-delivered here.
        #[serde(default)]
        inbox_carried: usize,
        /// A deadline the old agent carried that had already passed, so the
        /// successor was created without it.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        dropped_deadline: Option<Timestamp>,
    },
    #[serde(rename = "agent.cancelled")]
    AgentCancelled {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        reason: Option<String>,
    },
    /// An operator or orchestrator changed the agent's configuration mid-run
    /// (model, effort, tool exposure, approval policy, budget).
    #[serde(rename = "agent.config_updated")]
    AgentConfigUpdated {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        name: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        model: Option<ModelRef>,
        config: AgentConfig,
        /// The new scope, when the update changed one of its knobs.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        scope: Option<Scope>,
    },
    /// Absolute billing totals, including corrections from a completed engine run.
    #[serde(rename = "agent.usage_updated")]
    AgentUsageUpdated { total: Usage, turn_usage: Usage },
    /// A transcript entry: what the model said, or what it was shown.
    #[serde(rename = "agent.message")]
    AgentMessage {
        role: MessageRole,
        content: Vec<ContentBlock>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        invocation: Option<InvocationId>,
    },

    // ----- task lifecycle --------------------------------------------------
    #[serde(rename = "task.created")]
    TaskCreated {
        objective: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        parent_task: Option<TaskId>,
        #[serde(default)]
        dependencies: Vec<TaskId>,
        priority: Priority,
    },
    #[serde(rename = "task.assigned")]
    TaskAssigned { owner: AgentId },
    #[serde(rename = "task.started")]
    TaskStarted,
    #[serde(rename = "task.blocked")]
    TaskBlocked {
        reason: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        dependency: Option<TaskId>,
    },
    #[serde(rename = "task.unblocked")]
    TaskUnblocked,
    #[serde(rename = "task.requeued")]
    TaskRequeued { reason: String },
    #[serde(rename = "task.completed")]
    TaskCompleted { result: TaskResult },
    #[serde(rename = "task.failed")]
    TaskFailed {
        error: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        structured: Option<Value>,
    },
    #[serde(rename = "task.cancelled")]
    TaskCancelled {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        reason: Option<String>,
    },

    // ----- tooling ---------------------------------------------------------
    #[serde(rename = "tool.requested")]
    ToolRequested {
        call: ToolCall,
        side_effect: SideEffectClass,
    },
    #[serde(rename = "tool.started")]
    ToolStarted { call: ToolCallId },
    #[serde(rename = "tool.succeeded")]
    ToolSucceeded {
        call: ToolCallId,
        outcome: ToolOutcome,
    },
    #[serde(rename = "tool.failed")]
    ToolFailed {
        call: ToolCallId,
        error: String,
        retryable: bool,
        /// Real elapsed time of the failed attempt. `None` when the call
        /// never actually ran (an unresolvable tool name, or the executing
        /// task panicking before it started) - distinct from a genuine
        /// sub-millisecond failure. `default` documents the schema for
        /// clients (an `Option` already decodes a missing field as `None`);
        /// `skip_serializing_if` keeps the field absent on the wire for
        /// `None`, matching the generated `number | undefined` / `Optional[int]`
        /// client types instead of emitting a literal `null`.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        duration_ms: Option<u64>,
    },
    #[serde(rename = "tool.cancelled")]
    ToolCancelled { call: ToolCallId },
    #[serde(rename = "tool.denied")]
    ToolDenied { call: ToolCallId, reason: String },
    /// The call carried an idempotency key already committed for this agent,
    /// so the recorded outcome was returned instead of running again.
    #[serde(rename = "tool.replayed")]
    ToolReplayed {
        call: ToolCallId,
        key: IdempotencyKey,
        /// The call that actually ran.
        original: ToolCallId,
    },
    /// A retryable failure is being tried again, within the class's policy.
    #[serde(rename = "tool.retried")]
    ToolRetried {
        call: ToolCallId,
        attempt: u32,
        of: u32,
        error: String,
    },

    // ----- context ---------------------------------------------------------
    #[serde(rename = "snapshot.created")]
    SnapshotCreated {
        snapshot: SnapshotId,
        turn: u32,
        model: ModelRef,
        token_estimate: u32,
        layers: Vec<ContextLayer>,
    },
    #[serde(rename = "invocation.completed")]
    InvocationCompleted {
        invocation: InvocationId,
        snapshot: SnapshotId,
        model: ModelRef,
        /// Zero and unpriced for a native engine's message: engine usage does
        /// not line up with messages and arrives as `agent.usage_updated`.
        usage: Usage,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        stop_reason: Option<StopReason>,
        duration_ms: u64,
    },
    /// The turn did not complete. Streamed content accumulated before the
    /// failure or cancellation is kept so nothing the user watched is lost.
    #[serde(rename = "invocation.failed")]
    InvocationFailed {
        invocation: InvocationId,
        snapshot: SnapshotId,
        error: String,
        /// Authoritative adapter class; absent in older logs and runtime recovery.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        class: Option<crate::model::ModelErrorClass>,
        retryable: bool,
        /// Stopped on purpose (interrupt or cancel), not a provider failure.
        #[serde(default)]
        cancelled: bool,
        #[serde(default)]
        partial: Vec<ContentBlock>,
    },
    #[serde(rename = "compaction.requested")]
    CompactionRequested { trigger: CompactionTrigger },
    #[serde(rename = "checkpoint.created")]
    CheckpointCreated { checkpoint: Checkpoint },
    #[serde(rename = "compaction.completed")]
    CompactionCompleted {
        checkpoint: CheckpointId,
        version: u32,
        from_seq: u64,
        to_seq: u64,
        /// Estimated history tokens (checkpoint plus transcript) before and
        /// after the fold; absent in older logs.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        before_tokens: Option<u32>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        after_tokens: Option<u32>,
    },
    /// `models` names what was asked to fold, in the order it was tried: a
    /// transcript that reached a provider that then failed leaves no other
    /// durable trace of the egress.
    #[serde(rename = "compaction.failed")]
    CompactionFailed {
        error: String,
        #[serde(default)]
        models: Vec<ModelRef>,
    },
    /// An agent's context crossed the engine/API boundary. Names what carried
    /// over and, honestly, what did not.
    #[serde(rename = "context.handoff")]
    ContextHandoff {
        from: ModelRef,
        to: ModelRef,
        /// The checkpoint the next turn starts from, when one was built.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        checkpoint: Option<CheckpointId>,
        /// Transcript messages folded into that checkpoint.
        carried: u32,
        /// What could not cross, in plain words. Empty means nothing was lost.
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        lost: Vec<String>,
    },

    // ----- memory ----------------------------------------------------------
    #[serde(rename = "memory.candidate")]
    MemoryCandidate { candidate: MemoryCandidate },
    #[serde(rename = "memory.accepted")]
    MemoryAccepted {
        memory: MemoryId,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        reason: Option<String>,
    },
    #[serde(rename = "memory.rejected")]
    MemoryRejected { reason: String },
    /// Curated out by a person. The record stays; it is never retrieved again.
    #[serde(rename = "memory.forgotten")]
    MemoryForgotten {
        memory: MemoryId,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        reason: Option<String>,
    },
    /// Put back after a `memory.forgotten`, so a mis-keyed curation has a way
    /// back that leaves the same trail everything else does.
    #[serde(rename = "memory.restored")]
    MemoryRestored { memory: MemoryId },
    /// Pinned in front of the model, or unpinned.
    #[serde(rename = "memory.pinned")]
    MemoryPinned { memory: MemoryId, pinned: bool },
    #[serde(rename = "memory.superseded")]
    MemorySuperseded {
        old: MemoryId,
        new: MemoryId,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        reason: Option<String>,
    },

    // ----- messaging -------------------------------------------------------
    #[serde(rename = "message.sent")]
    MessageSent {
        message: MessageId,
        from: Actor,
        to: AgentId,
        content: Vec<ContentBlock>,
    },
    #[serde(rename = "inbox.delivered")]
    InboxDelivered { item: InboxItem },
    #[serde(rename = "inbox.consumed")]
    InboxConsumed { up_to: u64, items: Vec<InboxItemId> },
    /// `ask_user` durably recording what it asked, so a client can show it
    /// and a person answering later still has it in view (the reply itself
    /// is an ordinary `message.sent` back to this agent).
    #[serde(rename = "question.asked")]
    QuestionAsked { question: String },
    #[serde(rename = "approval.requested")]
    ApprovalRequested {
        approval: ApprovalId,
        call: ToolCallId,
        tool: String,
        description: String,
        /// The argument a remembered decision would be written about, so a
        /// client can offer "always allow this" without a second round trip.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        argument: Option<String>,
        side_effect: SideEffectClass,
    },
    /// A decision was remembered as a durable rule.
    #[serde(rename = "approval.rule_added")]
    ApprovalRuleAdded { rule: ApprovalRule },
    /// The rule stops deciding anything from here on.
    #[serde(rename = "approval.rule_revoked")]
    ApprovalRuleRevoked {
        rule: ApprovalRuleId,
        revocation: Revocation,
    },
    #[serde(rename = "approval.resolved")]
    ApprovalResolved {
        approval: ApprovalId,
        decision: ApprovalDecision,
        by: Actor,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        note: Option<String>,
    },

    // ----- artifacts -------------------------------------------------------
    #[serde(rename = "artifact.created")]
    ArtifactCreated { artifact: Artifact },
    /// An identical artifact already existed, so the bytes were not stored
    /// again and the call was given the existing one.
    #[serde(rename = "artifact.deduplicated")]
    ArtifactDeduplicated {
        /// The new record, owned by the call that produced these bytes.
        artifact: ArtifactId,
        /// The record it shares the stored bytes with.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        shared_with: Option<ArtifactId>,
        digest: String,
    },
    /// Retention dropped the bytes. The record stays; the payload is gone.
    #[serde(rename = "artifact.evicted")]
    ArtifactEvicted {
        artifact: ArtifactId,
        size_bytes: u64,
        reason: String,
    },

    // ----- mcp servers -----------------------------------------------------
    /// A configured MCP server answered and its tools are offered to agents.
    #[serde(rename = "mcp.server_connected")]
    McpServerConnected { server: String, tools: Vec<String> },
    /// A configured MCP server could not be reached or stopped answering. Its
    /// tools are withdrawn; a call against it fails rather than hanging.
    #[serde(rename = "mcp.server_failed")]
    McpServerFailed { server: String, error: String },

    // ----- runs ------------------------------------------------------------
    /// A run was replayed into this store from another daemon's export. It is
    /// the imported run's first event, so everything after it reads as
    /// history that happened elsewhere.
    #[serde(rename = "run.imported")]
    RunImported {
        /// The run's id in the daemon that exported it.
        source_run: RunId,
        /// `instance_id` of the store it was exported from.
        source_instance: String,
        exported_at: Timestamp,
        /// How many events the export carried.
        events: u64,
    },

    // ----- runtime ---------------------------------------------------------
    #[serde(rename = "lease.granted")]
    LeaseGranted { lease: Lease },
    #[serde(rename = "lease.renewed")]
    LeaseRenewed { expires_at: Timestamp },
    #[serde(rename = "lease.released")]
    LeaseReleased,
    #[serde(rename = "lease.expired")]
    LeaseExpired { holder: String },
    /// A turn is holding resource keys for its mutating tool calls.
    #[serde(rename = "lease.resource_acquired")]
    ResourceAcquired { keys: Vec<ResourceKey> },
    #[serde(rename = "lease.resource_released")]
    ResourceReleased { keys: Vec<ResourceKey> },
    /// Another agent holds a key this turn needs; it waits, visibly Blocked.
    #[serde(rename = "lease.resource_waiting")]
    ResourceWaiting {
        keys: Vec<ResourceKey>,
        holder: AgentId,
        /// The held key that is in the way.
        blocked_on: ResourceKey,
    },
    /// The scheduler is holding turns back from a provider: a 429, a
    /// repeated failure, or a subscription window near its limit.
    #[serde(rename = "provider.backoff")]
    ProviderBackoff {
        provider: String,
        reason: String,
        until: Timestamp,
    },
    #[serde(rename = "provider.recovered")]
    ProviderRecovered { provider: String },
    /// A subscription engine reported how much of its window is spent.
    /// A turn moved to the next link of its model chain (VISION §4.5).
    #[serde(rename = "provider.fallback")]
    ProviderFallback {
        from: ModelRef,
        to: ModelRef,
        /// Why the turn left `from`, in one line.
        reason: String,
        /// The normalized error class, when a model error caused the move.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        class: Option<crate::model::ModelErrorClass>,
    },
    #[serde(rename = "provider.quota")]
    ProviderQuota {
        provider: String,
        window: QuotaWindow,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        usage: Option<crate::provider::ProviderUsage>,
    },
    #[serde(rename = "recovery.classified")]
    RecoveryClassified {
        class: RecoveryClass,
        detail: String,
    },
    /// A budget is 80 % spent: the person is told before the stop.
    #[serde(rename = "budget.warning")]
    BudgetWarning { reason: String, usage: Usage },
    /// The agent's wall-clock deadline passed; it stops as Failed.
    #[serde(rename = "deadline.passed")]
    DeadlinePassed { reason: String },
    /// A budget is spent; the agent stops as Failed with this reason.
    #[serde(rename = "budget.exceeded")]
    BudgetExceeded { reason: String, usage: Usage },
    #[serde(rename = "daemon.started")]
    DaemonStarted { version: String, pid: u32 },
    #[serde(rename = "daemon.stopped")]
    DaemonStopped {
        /// Background processes the daemon ended on its way out, so the log
        /// accounts for everything it started.
        #[serde(default)]
        background_processes_stopped: usize,
    },
    /// Escape hatch for orchestration layers to record their own events.
    #[serde(rename = "custom")]
    Custom { name: String, payload: Value },
}

impl EventKind {
    /// The dotted event type, e.g. `agent.created`.
    pub fn type_name(&self) -> String {
        match serde_json::to_value(self) {
            Ok(Value::Object(map)) => map
                .get("type")
                .and_then(Value::as_str)
                .map(str::to_owned)
                .unwrap_or_else(|| "unknown".into()),
            _ => "unknown".into(),
        }
    }

    /// The family prefix before the dot.
    pub fn family(&self) -> String {
        let name = self.type_name();
        name.split('.').next().unwrap_or("unknown").to_owned()
    }

    /// Artifact id referenced by this event, when it introduces one.
    pub fn artifact_id(&self) -> Option<ArtifactId> {
        match self {
            Self::ArtifactCreated { artifact } => Some(artifact.id),
            _ => None,
        }
    }
}
