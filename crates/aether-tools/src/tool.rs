//! The tool contract: what a tool declares, what it receives, what it returns.

use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::Arc;

use aether_core::agent::{Scope, WaitCondition};
use aether_core::artifact::{Artifact, ArtifactKind};
use aether_core::id::{AgentId, ArtifactId, TaskId};
use aether_core::memory::MemoryCandidate;
use aether_core::model::ModelRef;
use aether_core::tool::{ResourceKey, ToolDescriptor, ToolOutcome};
use async_trait::async_trait;
use serde_json::Value;
use tokio_util::sync::CancellationToken;

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum WorkspaceMode {
    #[default]
    Shared,
    Isolated,
}

/// Everything a tool needs to know about the agent it runs for.
#[derive(Clone)]
pub struct ToolContext {
    pub agent_id: AgentId,
    pub task_id: Option<TaskId>,
    /// The daemon instance this call runs under. Labels the containers it
    /// starts, so a second daemon's orphan sweep cannot reap them.
    pub instance_id: Arc<str>,
    /// Workspace root, extra allowed paths and network policy.
    pub scope: Scope,
    /// Resolved working directory: the workspace root when set, else the process cwd.
    pub cwd: PathBuf,
    pub artifacts: Arc<dyn ArtifactSink>,
    /// Extra environment for subprocesses, merged over the daemon's own.
    pub env: BTreeMap<String, String>,
    /// Variable names no tool child may see, whatever the daemon inherited.
    /// The search credential lives here: an agent that can run a command
    /// must not be able to `printenv` it.
    pub hidden_env: Arc<[String]>,
    /// This agent's private temp dir, created on first use and exported to
    /// host commands as TMPDIR, TMP and TEMP. `None` leaves the daemon's own.
    pub temp_dir: Option<PathBuf>,
    /// Build cache variables and the directories they name, created on use
    /// and exported to host commands. Container commands never get them.
    pub build_env: Vec<(String, PathBuf)>,
}

impl std::fmt::Debug for ToolContext {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ToolContext")
            .field("agent_id", &self.agent_id)
            .field("task_id", &self.task_id)
            .field("instance_id", &self.instance_id)
            .field("scope", &self.scope)
            .field("cwd", &self.cwd)
            .field("env", &self.env.keys().collect::<Vec<_>>())
            .field("temp_dir", &self.temp_dir)
            .field("build_env", &self.build_env)
            .finish_non_exhaustive()
    }
}

/// Where oversized outputs go so the model context only carries a handle.
#[async_trait]
pub trait ArtifactSink: Send + Sync {
    /// Store bytes and return the artifact record; the sink mints the id.
    #[allow(clippy::too_many_arguments)]
    async fn store(
        &self,
        kind: ArtifactKind,
        name: &str,
        summary: &str,
        content_type: Option<&str>,
        bytes: Vec<u8>,
        ctx_agent: AgentId,
        ctx_task: Option<TaskId>,
    ) -> Result<Artifact, ToolError>;
    /// Import a completed output log. Production sinks override this to stream
    /// the file; the default keeps lightweight/custom sinks compatible.
    #[allow(clippy::too_many_arguments)]
    async fn store_file(
        &self,
        kind: ArtifactKind,
        name: &str,
        summary: &str,
        content_type: Option<&str>,
        path: &std::path::Path,
        ctx_agent: AgentId,
        ctx_task: Option<TaskId>,
    ) -> Result<Artifact, ToolError> {
        self.store(
            kind,
            name,
            summary,
            content_type,
            tokio::fs::read(path).await?,
            ctx_agent,
            ctx_task,
        )
        .await
    }
}

/// A `delegate` caller's key, with the arguments a repeat must match. The
/// runtime scopes the key to the calling agent.
#[derive(Debug, Clone, PartialEq)]
pub struct DelegateKey {
    pub key: String,
    pub request: Value,
}

/// A request to change authoritative harness state. The model proposes it
/// through a tool; the runtime validates, transitions and records it.
// Built per tool call and consumed at once, like [`ToolExecution`].
#[allow(clippy::large_enum_variant)]
#[derive(Debug, Clone, PartialEq)]
pub enum HarnessAction {
    CreateSchedule {
        spec: aether_core::schedule::CreateScheduleSpec,
    },
    ListSchedules {
        id: Option<aether_core::id::ScheduleId>,
    },
    UpdateSchedule {
        id: aether_core::id::ScheduleId,
        action: aether_core::schedule::ScheduleAction,
    },
    InspectState,
    ListAgents {
        after: Option<AgentId>,
        limit: usize,
    },
    /// Agents sharing the caller's parent, the caller excluded.
    ListPeers {
        after: Option<AgentId>,
        limit: usize,
    },
    ListTasks {
        after: Option<TaskId>,
        limit: usize,
    },
    ReadArtifact {
        id: ArtifactId,
        offset: u64,
        limit: usize,
    },
    ReadTask {
        task: TaskId,
        revision: Option<String>,
        offset: usize,
        max_chars: usize,
    },
    ProviderUsage,
    RefreshProviderUsage {
        provider: Option<String>,
    },
    ListModels {
        provider: Option<String>,
    },
    Delegate {
        role: String,
        name: Option<String>,
        objective: String,
        instructions: Option<String>,
        model: ModelRef,
        effort: aether_core::model::Effort,
        scope: Option<Scope>,
        workspace_mode: WorkspaceMode,
        /// Whether the caller wants to sleep until the child finishes.
        wait: bool,
        /// Whether the child is forbidden to create children of its own.
        leaf: bool,
        /// `None` inherits the parent's; not a scope knob, so it may widen.
        host_context: Option<aether_core::agent::HostContext>,
        /// Present when the caller keyed the call for a safe repeat.
        idempotency: Option<DelegateKey>,
    },
    IntegrateWorkspace {
        child: AgentId,
    },
    SendMessage {
        to: AgentId,
        content: String,
        /// Fan the message out to the recipient and every live descendant.
        subtree: bool,
    },
    CancelAgent {
        agent: AgentId,
        reason: String,
    },
    CompleteTask {
        summary: String,
        artifacts: Vec<ArtifactId>,
        structured: Option<Value>,
    },
    FailTask {
        error: String,
        structured: Option<Value>,
    },
    WaitFor {
        condition: WaitCondition,
    },
    Remember {
        candidate: MemoryCandidate,
    },
    RequestCheckpoint {
        reason: String,
    },
    AskUser {
        question: String,
    },
    /// Post to the notification center, or update what `post.key` names.
    Notify {
        post: aether_core::notification::NotificationPost,
    },
    /// Resolve the notification this agent posted under `key`.
    ResolveNotification {
        key: String,
    },
}

/// What a tool call produced: a world-facing outcome or a harness action.
// Variants differ in size, but one is built per tool call and consumed at
// once; boxing would buy an allocation, not a saving.
#[allow(clippy::large_enum_variant)]
#[derive(Debug, Clone, PartialEq)]
pub enum ToolExecution {
    Outcome(ToolOutcome),
    Action(HarnessAction),
}

#[derive(Debug, thiserror::Error)]
pub enum ToolError {
    #[error("invalid input: {0}")]
    InvalidInput(String),
    #[error("denied: {0}")]
    Denied(String),
    #[error("path is outside the agent's scope: {}", .0.display())]
    OutOfScope(PathBuf),
    #[error("io error: {0}")]
    Io(#[from] std::io::Error),
    #[error("cancelled")]
    Cancelled,
    #[error("timed out")]
    Timeout,
    #[error("internal error: {0}")]
    Internal(String),
}

impl ToolError {
    /// Whether the same call could reasonably be retried by the runtime.
    pub fn is_retryable(&self) -> bool {
        matches!(self, Self::Timeout | Self::Io(_))
    }
}

#[async_trait]
pub trait Tool: Send + Sync {
    fn descriptor(&self) -> ToolDescriptor;

    /// Resources the call will touch, derived from its input and filesystem aliases.
    /// May inspect paths, but must not mutate the filesystem.
    /// Put the primary declared resource first so remembered approvals stay narrow.
    fn resources(&self, input: &Value, ctx: &ToolContext) -> Vec<ResourceKey>;

    /// Whether this specific call takes a cross-agent lease on its resources.
    /// Only mutual exclusion turns on this; approval policy, retry and
    /// recovery keep using the descriptor's side-effect class.
    fn leases_resources(&self, _input: &Value) -> bool {
        self.descriptor().side_effect.is_mutating()
    }

    /// Whether [`Tool::resources`] is the whole truth about the call. False
    /// for a tool Aether hands off rather than runs, where a resource-scoped
    /// deny cannot be evaluated and the gate must fail closed.
    fn resources_known(&self, _input: &Value) -> bool {
        true
    }

    /// Whether the tool's work happens outside the agent's execution backend.
    /// True for a tool Aether hands to a process it does not contain, so a
    /// containerized agent must not be offered it at all.
    fn escapes_backend(&self) -> bool {
        false
    }

    /// Whether an interrupt or steer may cut this call short once the model
    /// has committed to it. A harness action (`delegate`, `complete_task`,
    /// ...) is a decided, in-memory state transition with no real-world
    /// progress to abort - racing it against a mere interrupt token risks
    /// dropping it entirely (a spawned child the model asked for, never
    /// created) even though the call itself never even looks at the token.
    /// A hard cancel of the whole agent still ends it.
    /// Real-world tools (`bash`, web, file writes) default to true so a
    /// person's Esc still lands promptly on work actually in flight.
    ///
    /// Not a hard guarantee: if this call shares a batch with a
    /// resource-contended one, an interrupt landing while the batch still
    /// waits for that resource cancels every call in the batch before this
    /// flag is ever consulted (`turn::hold_resources`'s `Held::Stopped`).
    /// This flag only protects a call already past that wait.
    ///
    /// Only `ToolRuntime::execute_batch` (the API-model turn path) consults
    /// this; `turn_engine::handle_request` (the engine path) runs every tool
    /// directly against a live, unbounded token regardless of this flag.
    fn interruptible(&self) -> bool {
        true
    }

    /// A key the tool declares for this call, when the call names its own
    /// effect (a job id, a request id). `None` derives one from the input.
    fn idempotency_key(&self, _input: &Value) -> Option<String> {
        None
    }

    /// The argument a person would write a remembered approval decision
    /// about: the command for a shell, the target for a network tool. Paths
    /// are matched through [`Tool::resources`] instead, so a tool whose whole
    /// argument is a path returns `None` here.
    fn rule_argument(&self, _input: &Value) -> Option<String> {
        None
    }

    async fn execute(
        &self,
        input: Value,
        ctx: &ToolContext,
        cancel: CancellationToken,
    ) -> Result<ToolExecution, ToolError>;
}

/// Parse a typed input, turning schema violations into `InvalidInput`.
pub fn parse_input<T: serde::de::DeserializeOwned>(input: Value) -> Result<T, ToolError> {
    serde_json::from_value(input).map_err(|e| ToolError::InvalidInput(e.to_string()))
}
