//! The daemon wire protocol.
//!
//! Newline-delimited JSON over a Unix domain socket. The client speaks first
//! with [`Command::Hello`]; the daemon answers with a [`Welcome`] or refuses.
//! Every later client frame carries a request id and the daemon answers it
//! exactly once. Subscriptions push [`ServerFrame::Event`] and
//! [`ServerFrame::Delta`] frames tagged with the subscription id.
//!
//! Correlation uses nonzero `req` on client frames and `id` on replies. Command
//! decoding errors echo `req` with `invalid_argument`. An unreadable or reserved
//! correlation key receives an ID-zero diagnostic followed by connection closure;
//! clients fail all pending requests on closure. See docs/protocol.md.
//!
//! Eighteen commands take an optional `idempotency_key` so a lost acknowledgement
//! can be retried safely (docs/remote.md).
//!
//! This crate is types and paths only. The client and daemon crates own the
//! sockets.

pub mod auth;
pub mod command_hint;
pub mod compat;
pub mod paths;
pub mod remote;
pub mod schema;

use std::collections::BTreeMap;

pub use aether_core::agent::ArchiveTarget;
use aether_core::agent::{
    Agent, AgentConfig, AgentStatusKind, ApprovalDefault, ApprovalPolicy, DeadlineUpdate,
    ExecutionBackend, NetworkPolicy, Scope,
};
pub use aether_core::attachment::{AttachmentId, AttachmentRef, ImageAttachment};

use aether_core::approval::{ApprovalRule, RuleMatch, RuleScope};
use aether_core::artifact::Artifact;
use aether_core::budget::RunSummary;
use aether_core::checkpoint::Checkpoint;
use aether_core::disk::LowDisk;
use aether_core::event::Event;
use aether_core::id::{
    AgentId, ApprovalId, ApprovalRuleId, ArtifactId, CheckpointId, InvocationId, MemoryId,
    MessageId, RunId, SnapshotId, TaskId,
};
pub use aether_core::idempotency::UnprovenAbsence;
use aether_core::inbox::InboxItem;
use aether_core::lease::{Lease, ResourceHolding};
use aether_core::mcp::McpServerHealth;
use aether_core::memory::{MemoryKind, MemoryRecord, MemoryRecorder, MemoryScope};
use aether_core::model::{CapabilityProfile, Effort, Message, ModelRef, Usage};
use aether_core::provider::ProviderHealth;
use aether_core::replay::ReplayFrame;
use aether_core::report::RunReport;
use aether_core::schedule::ScheduleTarget;
use aether_core::snapshot::{Invocation, InvocationSnapshot};
use aether_core::task::{Priority, Task, TaskStatusKind};
use aether_core::time::Timestamp;
use aether_core::tool::{ApprovalDecision, ApprovalRequest, ToolExposure};
use aether_core::transfer::{ExportSummary, ImportSummary};
use serde::{Deserialize, Serialize};

/// Bumped on any incompatible change to frames, commands or replies. The
/// daemon serves this version and at least the one before it, translating the
/// changed shapes at the edge ([`compat`]); anything outside that window is
/// refused at hello rather than failing to decode a reply later. Bumping this
/// constant therefore also means writing the rewrite that keeps the new N-1
/// served.
///
/// 2: `Reply::Paused` carries `agents`/`mid_turn` instead of
/// `after_current_turn`. `AgentPatch.deadline` is new in this version and is
/// a `DeadlineUpdate`, so a bare timestamp is not a shape any v1 client sent.
///
/// 3: run reports, run export/import, replay and doctor - five commands and
/// five replies. A v2 client talking to a v3 daemon would connect and then
/// fail on an unknown command, so the version says the command set changed.
///
/// 4: `[approval] default` ships as `"yolo"` instead of `"ask"`.
/// `CreateAgent.approval` is new and is the only reliable way to ask for a
/// gated agent under the new default; a v3 client cannot set it and its only
/// channel, `CreateAgent.config.approval`, is indistinguishable from "said
/// nothing" once the server's own default stops being the gated policy. So a
/// v3-or-older hello is refused rather than silently handed a yolo agent it
/// asked to be gated.
///
/// 5: memory curation - `ForgetMemory`, `PinMemory`, `RestoreMemory`,
/// `SyncRepositoryKnowledge` and the `Memory` / `MemorySeeded` replies.
/// Additive for a v5 daemon reading a v4 client, but not the other way: a v4
/// daemon cannot decode `ForgetMemory` at all, and `SyncRepositoryKnowledge`
/// takes no `repository`, which it would otherwise drop in silence.
///
/// 6: the handshake stops being an exact match - this daemon serves v5 as
/// well, and `Welcome` reports the window it negotiated. The bump is what
/// makes N-1 servable: v4 cannot decode the memory events v5 added, so the
/// window has to start at v5. It also carries the TCP transport: `Hello.token`,
/// `Welcome.idle_timeout_secs`, `Command::Ping` and `ErrorCode::Unauthorized`.
/// Only the last is a shape a v5 client cannot read, and `compat` folds it
/// into the refusal v5 knows.
///
/// 7: usage corrections have their own event variant. v6 clients receive
/// those corrections as `Custom` events with the same payload instead.
///
/// 8: `idempotency_key` on the five commands that create work.
/// Nothing about a reply changed, so a v7 client is served unrewritten - but
/// a v7 *daemon* ignores the field, and silently doing the work twice is the
/// exact failure the key exists to prevent. The bump is what refuses that
/// pairing at hello instead of leaving the client to find out from a
/// duplicate agent.
///
/// 9: spawn schedules, a shape v8 cannot decode; `compat` hides or
/// refuses them for v8. `busy` and `Schedule.last` are additive.
///
/// 10: exhaustion suspension: `BlockReason::Exhausted` and
/// `InboxKind::ChildSuspended`/`ChildResumed`, tags v9 cannot decode; `compat`
/// rewrites them into an inspection block and system messages.
///
/// 11: operator memory creation and `AgentPatch.name`. A v10 daemon cannot
/// decode the command and silently ignores the rename field; v10 clients
/// remain served by v11 because existing replies and events remain decodable.
///
/// 12: `ReplyToQuestion` atomically answers one still-pending question. v11
/// clients remain served because its reply is the existing `MessageSent`.
///
/// 13: live GitHub watcher configuration and status. v12 clients continue
/// using their existing commands and receive unchanged replies/events.
///
/// 14: automatic GitHub dispatch adds a watcher mode v13 clients cannot
/// decode. v13 watcher status returns a clear incompatible error when that
/// mode is present; other v13 replies and existing watcher modes remain valid.
///
/// 15: optional per-repository `allowed_users` on the GitHub watcher. A v14
/// Rust client cannot decode it, and a lenient one would drop it on save and
/// widen who may dispatch. v14 watcher replies carrying an allowlist become a
/// clear incompatible error, and v14 saves are refused while one is set.
///
/// 16: `PreviewRetention` and `Reply::RetentionPreview`. v15 clients retain
/// their existing commands, replies and events unchanged.
///
/// 17: optional watcher-level `operator_users`, and `operator` on an external
/// trigger. A v16 Rust client cannot decode the list, and a lenient one would
/// drop it on save. v16 watcher replies carrying it become a clear
/// incompatible error, and v16 saves are refused while it is set. The trigger
/// field is additive: an older peer ignores it and reads the trigger as external.
///
/// 18: remote control (docs/protocol.md). Device origins and the `forbidden` and
/// `unimplemented` codes are folded for v17 by `compat`; the rest is additive.
///
/// 19: `SendMessage.attachments` and the `image_ref` content block a message
/// with images then carries. A v18 daemon would drop the images in silence,
/// so it is refused at hello. An older client cannot decode the block; `compat`
/// hands it the same line of text a newer client prints for the image. This
/// build also serves 17 and 18.
///
/// 20: transient `accounting` deltas carry absolute live usage and context.
/// Tree nodes retain omitted descendants separately in optional `hidden_usage`.
/// Older clients receive durable accounting events and the existing stream deltas.
pub const PROTOCOL_VERSION: u32 = 20;

/// Optional features a peer may advertise. Unknown ones are ignored.
pub mod features {
    pub const DELTAS: &str = "deltas";
    pub const PUBLISH_ARTIFACT: &str = "publish_artifact";
    pub const ARTIFACT_BYTES: &str = "artifact_bytes";
    /// The daemon accepts `AgentConfig.host_context`.
    pub const HOST_CONTEXT: &str = "host_context";
    /// The daemon accepts role and repository-role operator memories,
    /// including explicit repository names on both repository scopes.
    pub const OPERATOR_MEMORY_ROLES: &str = "operator_memory_roles";
    /// The daemon applies `MemoryQuerySpec.recorded_by`. A daemon without it
    /// ignores the field and returns every memory, so check this first.
    pub const MEMORY_RECORDED_BY: &str = "memory_recorded_by";
    /// The daemon derives `scope.repository` from `CreateAgent.repository_root`.
    /// A daemon without it ignores the field, so check this first.
    pub const REPOSITORY_ROOT: &str = "repository_root";
    /// The daemon serves `alias_repository` and `list_repository_aliases`, and
    /// resolves a registered alias wherever a repository becomes a memory scope.
    pub const REPOSITORY_ALIASES: &str = "repository_aliases";
    /// Operator review of stored memories before they can enter recall.
    pub const MEMORY_PROPOSALS: &str = "memory_proposals";
    /// The daemon serves `memories_in_context`, and the retrieved-memory layer
    /// of `snapshot.created` names the memories it rendered.
    pub const MEMORY_CONTEXT: &str = "memory_context";
    /// Storage inventory and host-only filesystem garbage collection.
    pub const STORAGE: &str = "storage";
    /// The daemon applies `Command::StorageGc.prune_build_output`.
    pub const PRUNE_BUILD_OUTPUT: &str = "prune_build_output";
    /// The daemon accepts `AgentConfig.build_cache`. A daemon without it
    /// ignores the field, so check this first.
    pub const BUILD_CACHE: &str = "build_cache";
    /// Host evidence about memories a task saw, with append-only records and retractions.
    pub const MEMORY_FEEDBACK: &str = "memory_feedback";
}

mod handshake;
pub use handshake::{Hello, Welcome};
mod requests;
pub use requests::{
    AgentPatch, AgentQuery, ArtifactQuery, CreateAgent, CreateTask, EventQuery, MemoryQuerySpec,
    NewApprovalRule, PublishArtifact, RememberSpec, TaskQuery,
};
mod command;
pub use command::{Command, KeyedRequest};
mod replies;
pub use replies::{
    AgentTree, AuthInfo, DaemonStatus, ErrorCode, ErrorReply, MessageDelivery, ModelInfo,
    ProviderInfo, Reply, RunListing, SubmissionResolution, TranscriptEntry, TreeNode,
};
mod frames;
pub use frames::{ClientFrame, Delta, ServerFrame, decode, encode};
mod access;
pub use access::Access;
mod introduced;
pub use introduced::Introduced;

#[cfg(test)]
mod tests;
