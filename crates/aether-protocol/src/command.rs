use super::remote::{
    AttentionKind, DeviceId, DeviceRole, PathCompletionKind, PushSubscription, RemoteSettings,
};
use super::requests::default_true;
#[expect(
    unused_imports,
    reason = "preserve intra-doc links and schema descriptions"
)]
use super::{Agent, Reply};
use super::{
    AgentConfig, AgentId, AgentPatch, AgentQuery, ApprovalDecision, ApprovalId, ApprovalPolicy,
    ApprovalRuleId, ArchiveTarget, ArtifactId, ArtifactQuery, CheckpointId, CreateAgent,
    CreateTask, Deserialize, EventQuery, Hello, InvocationId, MemoryId, MemoryQuerySpec, ModelRef,
    NewApprovalRule, Priority, PublishArtifact, RememberSpec, RunId, ScheduleTarget, Scope,
    Serialize, SnapshotId, TaskId, TaskQuery,
};

use aether_core::attachment::{AttachmentId, ImageAttachment};

/// Everything a client can ask the daemon to do.
// Wire types are built once and serialized; variant size differences are irrelevant.
#[allow(clippy::large_enum_variant)]
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(tag = "cmd", rename_all = "snake_case")]
pub enum Command {
    Hello(Hello),
    Status,
    /// Read-only retention decisions; never applies deletion or cleanup.
    PreviewRetention,
    /// Bounded filesystem inventory and eligibility preview.
    StorageStatus,
    /// Host-only filesystem maintenance. Preview never deletes files.
    StorageGc {
        dry_run: bool,
        /// Also prune declared build output from finished workspaces that
        /// are kept for another reason. A daemon without
        /// `features::PRUNE_BUILD_OUTPUT` ignores it, so check that first.
        #[serde(default)]
        prune_build_output: bool,
    },
    Shutdown,
    /// Keep an otherwise silent connection alive under an idle timeout.
    /// Answered with [`Reply::Ok`].
    Ping,

    CreateAgent(CreateAgent),
    GetAgent {
        id: AgentId,
    },
    ListAgents {
        query: AgentQuery,
    },
    /// The live graph: every agent in the run (or all runs) with relations.
    GetTree {
        run: Option<RunId>,
        /// Include agents cleared from the view.
        #[serde(default)]
        include_archived: bool,
    },
    UpdateAgent {
        id: AgentId,
        patch: AgentPatch,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        idempotency_key: Option<String>,
    },
    /// Operator-only reply to one unanswered, unsuperseded question event.
    /// A stale question is refused without delivering anything.
    ReplyToQuestion {
        to: AgentId,
        question_seq: u64,
        content: String,
        /// Makes the delivery retryable; see [`CreateAgent::idempotency_key`].
        #[serde(default, skip_serializing_if = "Option::is_none")]
        idempotency_key: Option<String>,
    },
    /// Deliver a user message to an agent's inbox; wakes it if waiting.
    /// With `subtree`, the message is fanned out to every live agent beneath
    /// it too, each with its own inbox item and events (§3.5).
    SendMessage {
        to: AgentId,
        content: String,
        #[serde(default)]
        subtree: bool,
        /// Images sent with the text: PNG, JPEG, GIF or WebP, base64. The
        /// daemon checks each (type by its bytes, size, count), stores it
        /// once, and the message then carries a reference to it.
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        attachments: Vec<ImageAttachment>,
        /// Makes the delivery retryable; see [`CreateAgent::idempotency_key`].
        #[serde(default, skip_serializing_if = "Option::is_none")]
        idempotency_key: Option<String>,
    },
    Cancel {
        agent: AgentId,
        reason: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        idempotency_key: Option<String>,
    },
    /// Stop the running turn only; the agent keeps its transcript and waits
    /// for the next message.
    Interrupt {
        agent: AgentId,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        idempotency_key: Option<String>,
    },
    /// Inspect the opt-in GitHub watcher and its per-repository health.
    GetGithubWatcher,
    /// Replace watcher settings only if no other operator changed them.
    ConfigureGithubWatcher {
        config: aether_core::github::GithubWatcherConfig,
        expected_revision: u64,
    },
    CreateSchedule {
        spec: aether_core::schedule::CreateScheduleSpec,
        /// Makes the creation retryable; see [`CreateAgent::idempotency_key`].
        #[serde(default, skip_serializing_if = "Option::is_none")]
        idempotency_key: Option<String>,
    },
    GetSchedule {
        id: aether_core::id::ScheduleId,
    },
    ListSchedules {
        #[serde(default)]
        target: Option<AgentId>,
    },
    UpdateSchedule {
        id: aether_core::id::ScheduleId,
        action: aether_core::schedule::ScheduleAction,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        idempotency_key: Option<String>,
    },
    ScheduleHistory {
        id: aether_core::id::ScheduleId,
        limit: usize,
    },
    /// Deliver an external trigger (a webhook, a timer, a CI result) to an
    /// agent's inbox. It wakes a waiting agent like a person's message.
    Trigger {
        agent: AgentId,
        source: String,
        #[serde(default)]
        payload: serde_json::Value,
        /// Makes the delivery retryable; see [`CreateAgent::idempotency_key`].
        #[serde(default, skip_serializing_if = "Option::is_none")]
        idempotency_key: Option<String>,
    },
    /// Recreate a failed or cancelled agent from its last checkpoint, as a
    /// new agent linked to it.
    Restart {
        agent: AgentId,
        /// Makes the restart retryable; see [`CreateAgent::idempotency_key`].
        #[serde(default, skip_serializing_if = "Option::is_none")]
        idempotency_key: Option<String>,
    },
    /// Bring a failed agent back in place, with its history and context:
    /// it becomes Ready and reachable again under the same id. Additive in
    /// protocol 8; older daemons refuse the tag as unknown.
    Wake {
        agent: AgentId,
    },
    /// Pause an agent: a durable flag, orthogonal to its status. With
    /// `subtree` it reaches every live agent beneath it too (§3.1).
    Pause {
        agent: AgentId,
        #[serde(default)]
        subtree: bool,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        idempotency_key: Option<String>,
    },
    /// Lift a pause, or clear a wait or an inspection the operator owns.
    /// With `subtree` it lifts the pause across the subtree, and only that.
    Resume {
        agent: AgentId,
        #[serde(default)]
        subtree: bool,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        idempotency_key: Option<String>,
    },
    /// Clear finished work out of the live view, or bring it back. The mark
    /// is durable and hides nothing from the store, reports or replay.
    /// Only a finished agent can be archived.
    Archive {
        target: ArchiveTarget,
        /// `false` un-archives.
        #[serde(default = "default_true")]
        archived: bool,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        idempotency_key: Option<String>,
    },
    /// Turn approvals off (yolo) or back on for an agent and, with
    /// `subtree`, every live agent beneath it. A separate command
    /// from `UpdateAgent` because it carries the subtree reach and the
    /// never-widen-past-the-parent rule.
    SetApprovals {
        agent: AgentId,
        policy: ApprovalPolicy,
        #[serde(default)]
        subtree: bool,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        idempotency_key: Option<String>,
    },
    /// Ask the runtime to compact the agent's context before its next turn.
    Compact {
        agent: AgentId,
    },
    /// A harness tool call or permission prompt from an external engine's
    /// MCP bridge, authenticated by the per-turn token.
    EngineTool {
        agent: AgentId,
        token: String,
        name: String,
        input: serde_json::Value,
    },
    /// The tools an engine's MCP bridge should advertise for this agent:
    /// Aether's harness tools plus the MCP servers' tools it may use.
    EngineTools {
        agent: AgentId,
        token: String,
    },

    CreateTask(CreateTask),
    GetTask {
        id: TaskId,
    },
    ListTasks {
        query: TaskQuery,
    },

    ListApprovals,
    ResolveApproval {
        id: ApprovalId,
        decision: ApprovalDecision,
        note: Option<String>,
        /// Turn this answer into a durable rule as well.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        remember: Option<RememberSpec>,
        /// Makes the answer retryable; see [`CreateAgent::idempotency_key`].
        #[serde(default, skip_serializing_if = "Option::is_none")]
        idempotency_key: Option<String>,
    },
    /// Remembered decisions, oldest first.
    ListApprovalRules {
        #[serde(default)]
        include_revoked: bool,
    },
    AddApprovalRule(NewApprovalRule),
    RevokeApprovalRule {
        id: ApprovalRuleId,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        reason: Option<String>,
    },

    /// Stream events with `seq > after` (0 replays everything, `None` starts live).
    Subscribe {
        after: Option<u64>,
        query: EventQuery,
        deltas: bool,
    },
    Unsubscribe {
        id: u64,
    },
    GetEvents {
        after: u64,
        limit: usize,
        query: EventQuery,
    },
    GetTranscript {
        agent: AgentId,
        after: u64,
    },
    GetInbox {
        agent: AgentId,
        after: u64,
    },

    GetSnapshot {
        id: SnapshotId,
    },
    GetInvocation {
        id: InvocationId,
    },
    ListInvocations {
        agent: AgentId,
    },
    GetCheckpoint {
        id: CheckpointId,
    },
    LatestCheckpoint {
        agent: AgentId,
    },
    PublishArtifact(PublishArtifact),
    GetArtifact {
        id: ArtifactId,
    },
    ListArtifacts {
        query: ArtifactQuery,
    },
    /// Bytes, base64-encoded, capped at `max_bytes`.
    ReadArtifact {
        id: ArtifactId,
        max_bytes: usize,
    },
    Memories {
        query: MemoryQuerySpec,
    },
    /// Admit an operator-authored fact. Rejections return an error without mutation.
    CreateMemory {
        spec: aether_core::memory::OperatorMemorySpec,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        idempotency_key: Option<String>,
    },
    /// Accept a pending proposal and apply its deferred replacement. Needs `memory_proposals`.
    AcceptMemory {
        id: MemoryId,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        reason: Option<String>,
    },
    /// Reject a pending proposal with an explanation. Needs `memory_proposals`.
    RejectMemory {
        id: MemoryId,
        reason: String,
    },
    /// Record host evidence on a memory a task saw. Needs `memory_feedback`.
    RecordMemoryFeedback {
        memory: MemoryId,
        verdict: aether_core::memory_feedback::MemoryVerdict,
        evidence: aether_core::memory_feedback::MemoryEvidence,
        task: TaskId,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        invocation: Option<InvocationId>,
        reason: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        idempotency_key: Option<String>,
    },
    /// Withdraw evidence, preserving the record. Needs `memory_feedback`.
    RetractMemoryFeedback {
        feedback: aether_core::id::MemoryFeedbackId,
        reason: String,
    },
    /// Newest evidence first, including retractions. Needs `memory_feedback`.
    MemoryFeedback {
        memory: MemoryId,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        limit: Option<usize>,
    },
    /// Curate a memory out. The record stays; it is never retrieved again.
    ForgetMemory {
        id: MemoryId,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        reason: Option<String>,
    },
    /// Put a forgotten memory back. The inverse of `ForgetMemory`, so a
    /// mis-keyed curation has a way back.
    RestoreMemory {
        id: MemoryId,
    },
    /// Keep a memory in front of the model, or stop.
    PinMemory {
        id: MemoryId,
        pinned: bool,
    },
    /// Read a repository's committed notes into memory scoped to it.
    ///
    /// The scope is derived from the root, never named by the caller: reach
    /// to the socket is the whole authorization boundary (ARCHITECTURE §16),
    /// and a caller that could name both would be able to read notes out of a
    /// directory it just wrote and admit them into any repository's memory,
    /// attributed to the person.
    SyncRepositoryKnowledge {
        /// Repository root. Must be the daemon's workspace root or a live
        /// agent's, or inside one. Defaults to the daemon's.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        root: Option<std::path::PathBuf>,
    },
    /// Make `alias`, an old repository id, resolve to the repository at
    /// `root` or to the id `repository`, and move every memory filed under
    /// the alias there. The move is one-way. Needs `repository_aliases`.
    AliasRepository {
        alias: String,
        /// A checkout whose derived id is the canonical one, derived as for
        /// `create_agent.repository_root`. Exclusive with `repository`.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        root: Option<std::path::PathBuf>,
        /// The canonical id, named outright. Exclusive with `root`.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        repository: Option<String>,
        /// Report what would move without moving it.
        #[serde(default)]
        dry_run: bool,
    },
    /// Registered repository aliases. Needs `repository_aliases`.
    ListRepositoryAliases,
    /// Which stored memories every model call of a task, or one invocation,
    /// was given. Name exactly one. Needs `memory_context`.
    MemoriesInContext {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        task: Option<TaskId>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        invocation: Option<InvocationId>,
    },

    /// Runs this store knows about, newest first.
    ListRuns {
        limit: usize,
    },
    /// Compose a run's report from its events.
    GetRunReport {
        run: RunId,
    },
    /// Write a finished run's events, snapshots, checkpoints and artifacts
    /// to one file on the daemon's machine.
    ExportRun {
        run: RunId,
        path: std::path::PathBuf,
    },
    /// Recreate a run from such a file, under fresh ids.
    ImportRun {
        path: std::path::PathBuf,
    },
    /// One agent's turns as they happened, folded from the log.
    Replay {
        agent: AgentId,
    },

    ListModels,
    AuthStatus,
    /// Every provider the registry knows, with its live health.
    Providers,
    /// Configured MCP servers and their tools. `refresh` reconnects first,
    /// so tools added since daemon start are discovered on demand.
    McpServers {
        #[serde(default)]
        refresh: bool,
        /// Refresh only this server; all of them when absent.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        server: Option<String>,
    },

    /// Everything waiting on the operator: pending approvals, unanswered
    /// questions, and failed or blocked agents, read at one sequence number.
    GetAttention,

    /// Complete a partial path on the daemon's machine. Bounded: at most
    /// `limit` entries (default 50, never more than 200).
    CompletePath {
        kind: PathCompletionKind,
        /// What was typed so far. `~/` is the daemon user's home.
        prefix: String,
        /// The workspace to stay inside. Required for `workspace_file`.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        workspace: Option<std::path::PathBuf>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        limit: Option<u32>,
    },
    /// Workspaces holding root agents, most recently active first.
    ListWorkspaces {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        limit: Option<u32>,
    },
    /// Whether an agent can be started in `path`, and its canonical form.
    ValidateWorkspace {
        path: String,
    },
    /// Branch and dirtiness of a workspace on the daemon's machine.
    GetGitSummary {
        workspace: std::path::PathBuf,
    },
    /// Files, diffstat, commits and a capped patch of an agent's work.
    WorkspaceDiff {
        agent: AgentId,
    },
    /// Land a finished child's isolated workspace in the source it came
    /// from, through the same conflict check as the `integrate_workspace` tool.
    WorkspaceIntegrate {
        agent: AgentId,
    },
    /// Throw away an agent's isolated workspace. Refused while an agent using
    /// it is live unless `force`, which does not stop that agent.
    WorkspaceDiscard {
        agent: AgentId,
        #[serde(default)]
        force: bool,
    },

    /// The remote listener's settings and state. Host only.
    GetRemote,
    /// Replace the remote settings only if nobody else changed them. Opens
    /// or closes the listener at once. Host only.
    ConfigureRemote {
        settings: RemoteSettings,
        expected_revision: u64,
    },
    /// Open a pairing and mint its single-use code. Host only.
    StartPairing {
        #[serde(default)]
        role: DeviceRole,
    },
    /// Read a pairing's state; the code is never repeated. Host only.
    GetPairing {
        id: String,
    },
    /// Enrol or reject the device that claimed a pairing. Host only.
    ConfirmPairing {
        id: String,
        accept: bool,
    },
    /// Close a pairing nobody confirmed. Host only.
    CancelPairing {
        id: String,
    },
    /// Enrolled devices, oldest first. Host only.
    ListDevices {
        #[serde(default)]
        include_revoked: bool,
    },
    /// Revoke a device and close its connections and subscriptions at once.
    /// Host only.
    RevokeDevice {
        id: DeviceId,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        reason: Option<String>,
    },
    /// Rename a device or change its role. A role change closes its live
    /// connections. Host only.
    UpdateDevice {
        id: DeviceId,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        name: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        role: Option<DeviceRole>,
    },

    /// Whether the daemon sends Web Push, and the key to subscribe with.
    GetPushConfig,
    /// Store the calling device's push subscription, replacing any earlier
    /// one. Only a paired device can call it.
    SubscribePush {
        subscription: PushSubscription,
        /// Notify about these kinds only; empty means all of them.
        #[serde(default)]
        kinds: Vec<AttentionKind>,
    },
    /// Forget the calling device's push subscription.
    UnsubscribePush,
    /// Send the calling device one `test` notification.
    TestPush,

    /// Ask what became of a keyed command whose acknowledgement was lost.
    /// Read-only, within the caller's own keys; needs `command`'s standing.
    ResolveSubmission {
        /// The keyed command exactly as first sent: its `cmd`, its fields and
        /// its `idempotency_key`.
        #[schemars(with = "serde_json::Map<String, serde_json::Value>")]
        command: Box<Command>,
        /// The `submission_horizon` the client held when it first sent
        /// `command`. Without one an absent receipt proves nothing.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        horizon: Option<String>,
    },
}

impl Command {
    /// The wire tag, for a message about a command that did not complete.
    pub fn name(&self) -> String {
        serde_json::to_value(self)
            .ok()
            .and_then(|v| {
                v.get("cmd")
                    .and_then(serde_json::Value::as_str)
                    .map(str::to_owned)
            })
            .unwrap_or_else(|| "command".into())
    }

    /// The client's submission key on this command, with the fields that
    /// decide whether two submissions are the same request. `None` when the
    /// command carries no key, or is not one that creates work.
    ///
    /// The field set is named per command rather than taken over the whole
    /// re-serialized command on purpose: a field added later - even one with
    /// `#[serde(default)]` - would otherwise change the digest of every record
    /// an already-running daemon wrote, and a client's correct retry across
    /// that upgrade would be refused as a different request. Adding a field
    /// here changes what two requests mean and is a
    /// [`FINGERPRINT_SCHEME`](aether_core::idempotency::FINGERPRINT_SCHEME)
    /// bump, not a silent edit - unless it is skipped at its old implied
    /// value, so every earlier request still digests as before.
    pub fn keyed_request(&self) -> Option<serde_json::Result<KeyedRequest<'_>>> {
        let (key, fields) = match self {
            Self::CreateAgent(CreateAgent {
                role,
                name,
                objective,
                model,
                fallbacks,
                scope,
                repository_root,
                parent,
                config,
                approval,
                run,
                start,
                idempotency_key,
            }) => (
                idempotency_key.as_deref()?,
                serde_json::to_value(CreateAgentFingerprint {
                    cmd: "create_agent",
                    role,
                    name,
                    objective,
                    model,
                    fallbacks,
                    scope,
                    repository_root,
                    parent,
                    config,
                    approval,
                    run,
                    start: *start,
                }),
            ),
            Self::CreateTask(CreateTask {
                objective,
                owner,
                parent_task,
                dependencies,
                priority,
                idempotency_key,
            }) => (
                idempotency_key.as_deref()?,
                serde_json::to_value(CreateTaskFingerprint {
                    cmd: "create_task",
                    objective,
                    owner,
                    parent_task,
                    dependencies,
                    priority: *priority,
                }),
            ),
            Self::ReplyToQuestion {
                to,
                question_seq,
                content,
                idempotency_key,
            } => (
                idempotency_key.as_deref()?,
                serde_json::to_value(ReplyToQuestionFingerprint {
                    cmd: "reply_to_question",
                    to: *to,
                    question_seq: *question_seq,
                    content,
                }),
            ),
            Self::SendMessage {
                to,
                content,
                subtree,
                attachments,
                idempotency_key,
            } => (
                idempotency_key.as_deref()?,
                serde_json::to_value(SendMessageFingerprint {
                    cmd: "send_message",
                    to: *to,
                    content,
                    subtree: *subtree,
                    attachments: attachments.iter().map(AttachmentFingerprint::of).collect(),
                }),
            ),
            Self::Trigger {
                agent,
                source,
                payload,
                idempotency_key,
            } => (
                idempotency_key.as_deref()?,
                serde_json::to_value(TriggerFingerprint {
                    cmd: "trigger",
                    agent: *agent,
                    source,
                    payload,
                }),
            ),
            Self::CreateSchedule {
                spec:
                    aether_core::schedule::CreateScheduleSpec {
                        name,
                        target,
                        recurrence,
                        missed,
                        busy,
                        payload,
                        max_deliveries,
                    },
                idempotency_key,
            } => {
                let (target, spawn) = match target {
                    ScheduleTarget::Agent(agent) => (Some(*agent), None),
                    ScheduleTarget::Spawn(template) => (None, Some(template)),
                };
                (
                    idempotency_key.as_deref()?,
                    serde_json::to_value(CreateScheduleFingerprint {
                        cmd: "create_schedule",
                        name,
                        target,
                        spawn,
                        recurrence,
                        missed: *missed,
                        busy: *busy,
                        payload,
                        max_deliveries: *max_deliveries,
                    }),
                )
            }
            Self::ResolveApproval {
                id,
                decision,
                note,
                remember,
                idempotency_key,
            } => (
                idempotency_key.as_deref()?,
                serde_json::to_value(ResolveApprovalFingerprint {
                    cmd: "resolve_approval",
                    id: *id,
                    decision,
                    note,
                    remember,
                }),
            ),
            Self::Restart {
                agent,
                idempotency_key,
            } => (
                idempotency_key.as_deref()?,
                serde_json::to_value(RestartFingerprint {
                    cmd: "restart",
                    agent: *agent,
                }),
            ),
            _ => return self.additional_keyed_request(),
        };
        Some(fields.map(|fields| KeyedRequest { key, fields }))
    }
    fn additional_keyed_request(&self) -> Option<serde_json::Result<KeyedRequest<'_>>> {
        let (key, fields) = match self {
            Self::Cancel {
                agent,
                reason,
                idempotency_key,
            } => (
                idempotency_key.as_deref()?,
                serde_json::json!({"cmd": "cancel", "agent": agent, "reason": reason}),
            ),
            Self::Interrupt {
                agent,
                idempotency_key,
            } => (
                idempotency_key.as_deref()?,
                serde_json::json!({"cmd": "interrupt", "agent": agent}),
            ),
            Self::Pause {
                agent,
                subtree,
                idempotency_key,
            } => (
                idempotency_key.as_deref()?,
                serde_json::json!({"cmd": "pause", "agent": agent, "subtree": subtree}),
            ),
            Self::Resume {
                agent,
                subtree,
                idempotency_key,
            } => (
                idempotency_key.as_deref()?,
                serde_json::json!({"cmd": "resume", "agent": agent, "subtree": subtree}),
            ),
            Self::Archive {
                target,
                archived,
                idempotency_key,
            } => (
                idempotency_key.as_deref()?,
                serde_json::json!({"cmd": "archive", "target": target, "archived": archived}),
            ),
            _ => return self.configuration_keyed_request(),
        };
        Some(Ok(KeyedRequest { key, fields }))
    }

    fn configuration_keyed_request(&self) -> Option<serde_json::Result<KeyedRequest<'_>>> {
        let (key, fields) = match self {
            Self::UpdateAgent {
                id,
                patch,
                idempotency_key,
            } => (
                idempotency_key.as_deref()?,
                serde_json::json!({"cmd": "update_agent", "id": id, "patch": patch}),
            ),
            Self::SetApprovals {
                agent,
                policy,
                subtree,
                idempotency_key,
            } => (
                idempotency_key.as_deref()?,
                serde_json::json!({"cmd": "set_approvals", "agent": agent, "policy": policy, "subtree": subtree}),
            ),
            Self::UpdateSchedule {
                id,
                action,
                idempotency_key,
            } => (
                idempotency_key.as_deref()?,
                serde_json::json!({"cmd": "update_schedule", "id": id, "action": action}),
            ),
            Self::CreateMemory {
                spec,
                idempotency_key,
            } => (
                idempotency_key.as_deref()?,
                serde_json::json!({"cmd": "create_memory", "spec": spec}),
            ),
            Self::RecordMemoryFeedback {
                memory,
                verdict,
                evidence,
                task,
                invocation,
                reason,
                idempotency_key,
            } => (
                idempotency_key.as_deref()?,
                serde_json::json!({"cmd":"record_memory_feedback", "memory":memory,
                    "verdict":verdict, "evidence":evidence, "task":task,
                    "invocation":invocation, "reason":reason}),
            ),
            Self::AddApprovalRule(rule) => (
                rule.idempotency_key.as_deref()?,
                serde_json::json!({
                    "cmd": "add_approval_rule", "decision": rule.decision, "matcher": rule.matcher,
                    "scope": rule.scope, "note": rule.note,
                }),
            ),
            _ => return None,
        };
        Some(Ok(KeyedRequest { key, fields }))
    }
}

/// A keyed command's key and the request the key is spent on.
#[derive(Debug, Clone, PartialEq)]
pub struct KeyedRequest<'a> {
    pub key: &'a str,
    pub fields: serde_json::Value,
}

// The fingerprint views. One per keyed command, listing every field that
// changes what the command does - `cmd` included, so one key cannot be
// replayed across two command shapes. Nothing here is skipped when absent:
// a stable digest needs a stable field set. The exception is fields added
// later, skipped at their old implied value so earlier digests still match.

#[derive(Serialize)]
struct CreateAgentFingerprint<'a> {
    cmd: &'static str,
    role: &'a str,
    name: &'a Option<String>,
    objective: &'a Option<String>,
    model: &'a Option<ModelRef>,
    fallbacks: &'a [ModelRef],
    scope: &'a Option<Scope>,
    // Absent without a checkout, so an earlier create keeps its old digest.
    #[serde(skip_serializing_if = "Option::is_none")]
    repository_root: &'a Option<std::path::PathBuf>,
    parent: &'a Option<AgentId>,
    config: &'a Option<AgentConfig>,
    approval: &'a Option<ApprovalPolicy>,
    run: &'a Option<RunId>,
    start: bool,
}

#[derive(Serialize)]
struct CreateTaskFingerprint<'a> {
    cmd: &'static str,
    objective: &'a str,
    owner: &'a Option<AgentId>,
    parent_task: &'a Option<TaskId>,
    dependencies: &'a [TaskId],
    priority: Priority,
}

#[derive(Serialize)]
struct ReplyToQuestionFingerprint<'a> {
    cmd: &'static str,
    to: AgentId,
    question_seq: u64,
    content: &'a str,
}

#[derive(Serialize)]
struct SendMessageFingerprint<'a> {
    cmd: &'static str,
    to: AgentId,
    content: &'a str,
    subtree: bool,
    // Absent without images, so a text-only message keeps its old digest.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    attachments: Vec<AttachmentFingerprint<'a>>,
}

/// One image as a retry must repeat it: a digest stands in for the payload.
#[derive(Serialize)]
struct AttachmentFingerprint<'a> {
    media_type: &'a str,
    name: Option<&'a str>,
    sha256: AttachmentId,
}

impl<'a> AttachmentFingerprint<'a> {
    fn of(image: &'a ImageAttachment) -> Self {
        Self {
            media_type: &image.media_type,
            name: image.name.as_deref(),
            sha256: AttachmentId::of(image.data.as_bytes()),
        }
    }
}

#[derive(Serialize)]
struct TriggerFingerprint<'a> {
    cmd: &'static str,
    agent: AgentId,
    source: &'a str,
    payload: &'a serde_json::Value,
}

#[derive(Serialize)]
struct CreateScheduleFingerprint<'a> {
    cmd: &'static str,
    name: &'a str,
    // Exactly one of `target` and `spawn` is set.
    #[serde(skip_serializing_if = "Option::is_none")]
    target: Option<AgentId>,
    #[serde(skip_serializing_if = "Option::is_none")]
    spawn: Option<&'a aether_core::schedule::SpawnSpec>,
    recurrence: &'a aether_core::schedule::Recurrence,
    missed: aether_core::schedule::MissedRunPolicy,
    #[serde(skip_serializing_if = "is_skip")]
    busy: aether_core::schedule::BusyPolicy,
    payload: &'a serde_json::Value,
    max_deliveries: Option<std::num::NonZeroU64>,
}

#[derive(Serialize)]
struct ResolveApprovalFingerprint<'a> {
    cmd: &'static str,
    id: ApprovalId,
    decision: &'a ApprovalDecision,
    note: &'a Option<String>,
    remember: &'a Option<RememberSpec>,
}

#[derive(Serialize)]
struct RestartFingerprint {
    cmd: &'static str,
    agent: AgentId,
}

fn is_skip(busy: &aether_core::schedule::BusyPolicy) -> bool {
    *busy == aether_core::schedule::BusyPolicy::Skip
}
