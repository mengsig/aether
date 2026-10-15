//! Durable storage for Aether: the event log and the state projections it
//! drives, committed together.
//!
//! The one rule this crate exists to enforce: a state change and the event
//! that describes it land in the same transaction. Callers build a
//! [`Commit`] of event drafts plus state mutations and hand it to
//! [`Store::commit`]; the store assigns gapless sequence numbers, applies the
//! mutations, and either everything is durable or nothing is.

mod migration_backup;
#[cfg(test)]
mod schedule_tests;
mod schema;
mod sqlite;

use std::collections::BTreeMap;
use std::path::PathBuf;

use aether_core::agent::{Agent, AgentStatusKind};
use aether_core::approval::ApprovalRule;
use aether_core::artifact::Artifact;
use aether_core::checkpoint::Checkpoint;
use aether_core::event::{Event, EventDraft};
use aether_core::id::ScheduleId;
use aether_core::id::{
    AgentId, ApprovalId, ApprovalRuleId, ArtifactId, CheckpointId, DeviceId, EventId, InvocationId,
    MemoryFeedbackId, MemoryId, RunId, SnapshotId, TaskId, ToolCallId,
};
use aether_core::idempotency::{
    IdempotencyKey, RecordedOutcome, RecordedSubmission, SubmissionLedger,
};
use aether_core::inbox::InboxItem;
use aether_core::lease::Lease;
use aether_core::memory::{MemoryKind, MemoryRecord, MemoryRecorder, MemoryScope};
use aether_core::model::{Message, ModelRef};
use aether_core::repository::{RepositoryAlias, RepositoryAliasCounts};
use aether_core::schedule::{Schedule, ScheduleOccurrence};
use aether_core::snapshot::{Invocation, InvocationSnapshot, SnapshotMemories};
use aether_core::task::{Task, TaskStatusKind};
use aether_core::time::Timestamp;
use aether_core::tool::ApprovalRequest;

use aether_core::memory_feedback::{FeedbackTally, MemoryFeedback};

pub use schema::SCHEMA_VERSION;
pub use sqlite::{PushSubscriptionRow, RunExpiry, SqliteStore};

/// The single write path, shared by everything that commits to one store.
///
/// [`Store::commit`] is atomic on its own. What it cannot make atomic is the
/// *build*: a commit assembled from values read out of the store - the next
/// inbox sequence, a memory record about to have one field changed - is stale
/// the moment another writer lands between the read and the commit, and the
/// build then writes that writer's decision back away. Anything of that shape
/// takes a [`Hold`] across both.
#[derive(Clone, Default)]
pub struct WriteGate(std::sync::Arc<parking_lot::Mutex<()>>);

impl WriteGate {
    pub fn new() -> Self {
        Self::default()
    }

    /// Block until nothing else is building a commit. Held for the lifetime
    /// of the returned value, which is passed to whatever needs the proof.
    pub fn hold(&self) -> Hold<'_> {
        Hold(self.0.lock())
    }

    /// Whether anyone holds the gate right now; stale once read. For race
    /// tests that must know which side of the gate a read ran on.
    #[doc(hidden)]
    pub fn is_held(&self) -> bool {
        self.0.is_locked()
    }
}

impl std::fmt::Debug for WriteGate {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("WriteGate")
    }
}

/// Proof that the holder is the only one building a commit right now. Taking
/// one while already holding one deadlocks: the gate is not reentrant.
pub struct Hold<'a>(
    #[expect(dead_code, reason = "held for its lifetime")] parking_lot::MutexGuard<'a, ()>,
);

impl std::fmt::Debug for Hold<'_> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("Hold")
    }
}

#[derive(Debug, thiserror::Error)]
pub enum StoreError {
    #[error("sqlite: {0}")]
    Sqlite(#[from] rusqlite::Error),
    #[error("serialization: {0}")]
    Serde(#[from] serde_json::Error),
    #[error("io: {0}")]
    Io(#[from] std::io::Error),
    #[error("cannot back up {} before migration: {source}", database.display())]
    MigrationBackup {
        database: std::path::PathBuf,
        source: Box<StoreError>,
    },
    #[error("inbox sequence for {agent}: expected {expected}, got {got}")]
    InboxSequence {
        agent: AgentId,
        expected: u64,
        got: u64,
    },
    #[error("store schema version {found} is newer than supported {supported}")]
    SchemaTooNew { found: u32, supported: u32 },
    #[error("retention snapshot is {bytes} bytes, exceeding the {limit} byte memory limit")]
    RetentionSnapshotTooLarge { bytes: u64, limit: u64 },
    #[error("store is corrupt: {0}")]
    Corrupt(String),
    #[error(
        "event history expired within sequence range {first_seq}..={last_seq}; obtain a fresh state snapshot before resuming replay"
    )]
    HistoryExpired { first_seq: u64, last_seq: u64 },
    #[error("artifact bytes unavailable: no artifact directory configured")]
    NoArtifactDir,
    /// A repository alias that would break its invariants: a different
    /// canonical for a registered alias, or a chain of aliases.
    #[error("{0}")]
    AliasRefused(String),
}

impl StoreError {
    /// The disk refused the write: full, over quota, or failing. Nothing
    /// about the data is wrong, so the same commit can land once it recovers.
    pub fn is_unwritable(&self) -> bool {
        match self {
            Self::Sqlite(rusqlite::Error::SqliteFailure(e, _)) => matches!(
                e.code,
                rusqlite::ErrorCode::DiskFull | rusqlite::ErrorCode::SystemIoFailure
            ),
            Self::Io(e) => matches!(
                e.kind(),
                std::io::ErrorKind::StorageFull | std::io::ErrorKind::QuotaExceeded
            ),
            _ => false,
        }
    }
}

pub type Result<T> = std::result::Result<T, StoreError>;

/// A state mutation applied atomically with its events.
// Variants differ in size, but a mutation is built, moved once, and consumed.
#[allow(clippy::large_enum_variant)]
#[derive(Debug, Clone, PartialEq)]
pub enum Mutation {
    UpsertSchedule(Schedule),
    RecordScheduleOccurrence(ScheduleOccurrence),
    UpsertAgent(Agent),
    UpsertTask(Task),
    /// `item.seq` must equal the agent's current last inbox seq plus one.
    DeliverInbox(InboxItem),
    SetLease(Lease),
    ClearLease(AgentId),
    PutSnapshot(InvocationSnapshot),
    PutInvocation(Invocation),
    PutCheckpoint(Checkpoint),
    PutApproval(ApprovalRequest),
    /// Insert or replace a remembered approval decision. Revoking one is a
    /// put of the same rule carrying its revocation.
    PutApprovalRule(ApprovalRule),
    /// Keep a tool outcome under its idempotency key. The first write for a
    /// key wins: a repeat must replay, never overwrite.
    RecordToolOutcome(RecordedOutcome),
    /// Keep what a client's keyed submission created, in the same transaction
    /// as the creation. A second record under one key fails the whole commit:
    /// the duplicate creation it belongs to must not become durable.
    RecordSubmission(RecordedSubmission),
    PutMemory(MemoryRecord),
    RecordMemoryFeedback(MemoryFeedback),
    RetractMemoryFeedback {
        id: MemoryFeedbackId,
        at: Timestamp,
        reason: String,
    },
    PutArtifact(Artifact),
    SetMeta {
        key: String,
        value: String,
    },
    /// Insert a paired device, or replace its record and revocation. Never
    /// moves `last_seen_at`: that belongs to [`Mutation::TouchDevice`].
    PutDevice(DeviceRow),
    /// Stamp a device's last accepted hello. An unknown id fails the commit.
    TouchDevice {
        id: DeviceId,
        at: Timestamp,
    },
}

/// One paired remote device as the store keeps it. The store indexes the
/// lifecycle columns and carries `record` for the layer that enrolled it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DeviceRow {
    pub id: DeviceId,
    pub created_at: Timestamp,
    pub revoked_at: Option<Timestamp>,
    /// Read-only here: written by [`Mutation::TouchDevice`] alone.
    pub last_seen_at: Option<Timestamp>,
    /// Name, role and public key, as the enrolling layer serialized them.
    pub record: serde_json::Value,
}

/// An event replayed into this store from another daemon's export. It keeps
/// the timestamp it had where it happened, so an imported run reads as it
/// ran; its id and sequence number are still this store's own.
#[derive(Debug, Clone, PartialEq)]
pub struct ImportedEvent {
    /// Minted by the importer, not the store, so an event's causation can be
    /// resolved to another event of the same import before either is
    /// written. `events.id` is UNIQUE: a collision fails, never silently
    /// overwrites.
    pub id: EventId,
    pub draft: EventDraft,
    pub at: Timestamp,
}

/// Events and mutations that must become durable together.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Commit {
    pub events: Vec<EventDraft>,
    pub mutations: Vec<Mutation>,
}

impl Commit {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn event(mut self, event: EventDraft) -> Self {
        self.events.push(event);
        self
    }

    pub fn mutate(mut self, mutation: Mutation) -> Self {
        self.mutations.push(mutation);
        self
    }

    pub fn push_event(&mut self, event: EventDraft) {
        self.events.push(event);
    }

    pub fn push_mutation(&mut self, mutation: Mutation) {
        self.mutations.push(mutation);
    }

    pub fn is_empty(&self) -> bool {
        self.events.is_empty() && self.mutations.is_empty()
    }
}

#[derive(Debug)]
pub enum ToolEventScope<'a> {
    Invocation(InvocationId),
    Calls(&'a [ToolCallId]),
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct EventFilter {
    /// The agent's own events and, unless `exclude_children`, its children's
    /// (`parent_agent_id`): a view of an agent follows the work it delegated.
    pub agent: Option<AgentId>,
    /// With `agent`, only what it emitted itself: what decides the agent's own
    /// state (its transcript tail, its calls, its failure streak) needs this.
    pub exclude_children: bool,
    pub task: Option<TaskId>,
    pub run: Option<RunId>,
    /// Dotted families such as `agent`, `tool`.
    pub families: Vec<String>,
    /// Exact types such as `task.completed`.
    pub types: Vec<String>,
    /// With `agent` and children included, the only types taken from the
    /// children; the agent's own events are unaffected. Empty takes every type.
    pub child_types: Vec<String>,
}

impl EventFilter {
    pub fn all() -> Self {
        Self::default()
    }

    pub fn for_agent(agent: AgentId) -> Self {
        Self {
            agent: Some(agent),
            ..Default::default()
        }
    }

    /// Only the events `agent` emitted itself, none of its children's.
    pub fn own_events(agent: AgentId) -> Self {
        Self {
            agent: Some(agent),
            exclude_children: true,
            ..Default::default()
        }
    }

    pub fn for_run(run: RunId) -> Self {
        Self {
            run: Some(run),
            ..Default::default()
        }
    }

    /// Whether an already-loaded event matches; used by live subscriptions.
    /// Must agree with the store's query for the same filter.
    pub fn matches(&self, event: &Event) -> bool {
        if let Some(a) = self.agent {
            let own = event.agent_id == Some(a);
            let child = !self.exclude_children
                && event.parent_agent_id == Some(a)
                && (self.child_types.is_empty()
                    || self.child_types.contains(&event.kind.type_name()));
            if !own && !child {
                return false;
            }
        }
        if let Some(t) = self.task
            && event.task_id != Some(t)
        {
            return false;
        }
        if let Some(r) = self.run
            && event.run != Some(r)
        {
            return false;
        }
        if !self.families.is_empty() && !self.families.iter().any(|f| *f == event.kind.family()) {
            return false;
        }
        if !self.types.is_empty() && !self.types.iter().any(|t| *t == event.kind.type_name()) {
            return false;
        }
        true
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct AgentFilter {
    pub parent: Option<AgentId>,
    pub run: Option<RunId>,
    pub statuses: Vec<AgentStatusKind>,
    pub live_only: bool,
    /// Leave out agents cleared from the view. Off by default so
    /// every internal walk - subtree, budget rollup, recovery - still sees
    /// them: archiving is a view filter, never a deletion.
    pub exclude_archived: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ArtifactFilter {
    pub name: Option<String>,
    pub agent: Option<AgentId>,
    pub task: Option<TaskId>,
    /// Only artifacts that still have their bytes.
    pub with_bytes_only: bool,
    pub limit: usize,
}

impl Default for ArtifactFilter {
    fn default() -> Self {
        Self {
            name: None,
            agent: None,
            task: None,
            with_bytes_only: false,
            limit: 50,
        }
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct TaskFilter {
    pub owner: Option<AgentId>,
    pub parent_task: Option<TaskId>,
    pub run: Option<RunId>,
    pub statuses: Vec<TaskStatusKind>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct MemoryQuery {
    /// Restrict authorship, independently of applicability scope.
    pub provenance_agent: Option<AgentId>,
    /// Restrict to memories this kind of recorder authored.
    pub recorded_by: Option<MemoryRecorder>,
    /// Memories visible to this scope: the exact scope plus global ones.
    pub scope: Option<MemoryScope>,
    /// Match `scope` exactly, without the global memories every scope sees.
    /// For asking what one scope holds - seeding and its retraction sweep -
    /// rather than what an agent in it can retrieve.
    pub exact_scope: bool,
    /// Also match every scope nested in `scope`: for a repository, the role
    /// scopes inside it. For the retraction sweep, which must reach notes
    /// whose whole role directory is gone.
    pub nested_scopes: bool,
    pub kinds: Vec<MemoryKind>,
    pub tags: Vec<String>,
    /// Full-text query over memory text and tags.
    pub text: Option<String>,
    /// Exact text match. Unlike `text` this is a keyed lookup, for asking
    /// whether one specific fact is already stored rather than searching.
    pub exact_text: Option<String>,
    pub include_superseded: bool,
    /// Include memories a person curated out. Off for anything a model sees.
    pub include_forgotten: bool,
    /// Select pending proposals instead of admitted memories. Never enabled for recall.
    pub proposed: bool,
    /// One record by id, for a browser showing a supersession chain.
    pub id: Option<MemoryId>,
    pub limit: usize,
}

impl Default for MemoryQuery {
    fn default() -> Self {
        Self {
            scope: None,
            provenance_agent: None,
            recorded_by: None,
            exact_scope: false,
            nested_scopes: false,
            kinds: Vec::new(),
            tags: Vec::new(),
            text: None,
            exact_text: None,
            include_superseded: false,
            include_forgotten: false,
            proposed: false,
            id: None,
            limit: 20,
        }
    }
}

/// What registering a repository alias moved, or a preview of it would.
#[derive(Debug, Clone, PartialEq)]
pub struct RepositoryAliasMove {
    pub counts: RepositoryAliasCounts,
    /// The alias was already registered to the same canonical id.
    pub existing: bool,
    /// The committed event; empty for a preview, or when nothing changed.
    pub events: Vec<Event>,
}

/// One transcript entry rebuilt from the `agent.message` events.
#[derive(Debug, Clone, PartialEq)]
pub struct TranscriptEntry {
    pub seq: u64,
    pub at: Timestamp,
    pub message: Message,
    pub invocation: Option<InvocationId>,
}

/// A payload lease. Dropping it leaves staged bytes for the next open to recover.
#[derive(Debug)]
pub struct StagedArtifact {
    pub path: PathBuf,
    pub(crate) staged: Option<(PathBuf, std::fs::File)>,
}

impl StagedArtifact {
    /// A payload without disk staging, for ephemeral stores.
    pub fn ready(path: PathBuf) -> Self {
        Self { path, staged: None }
    }
}

/// A streamed payload, optionally reusing an existing artifact's bytes.
#[derive(Debug)]
pub struct ImportedArtifact {
    pub path: PathBuf,
    pub size_bytes: u64,
    pub digest: String,
    pub shared_with: Option<ArtifactId>,
    pub publication: Option<StagedArtifact>,
}

/// The storage contract. Synchronous by design: SQLite work is short and the
/// daemon wraps calls in blocking tasks where it matters.
pub trait Store: Send + Sync {
    /// Atomically expire an old completed run, preserving replay boundaries.
    /// The caller holds the runtime write gate and excludes in-flight work
    /// and resource holdings; the store checks durable dependencies itself.
    fn expire_run_history(
        &self,
        run: RunId,
        before: Timestamp,
        hold: &Hold<'_>,
    ) -> Result<RunExpiry>;

    /// The checks of [`Store::expire_run_history`] without the write gate or
    /// a write transaction: why `run` cannot expire yet, or `None` when it may.
    fn screen_run_expiry(&self, run: RunId, before: Timestamp) -> Result<Option<RunExpiry>>;

    /// Copy durable state into an isolated ephemeral store for ordered expiry simulation.
    /// Mutating the returned store must never change source rows or payload files.
    fn retention_snapshot(&self) -> Result<Box<dyn Store>>;

    /// Read the same durable guards and payload counts without modifying history.
    fn preview_run_history(
        &self,
        run: RunId,
        before: Timestamp,
    ) -> Result<Option<aether_core::retention::RunRetentionPreview>>;

    /// Read pending cleanup journals and staged publications without recovery.
    fn pending_artifact_cleanup(&self) -> Result<aether_core::retention::PendingArtifactCleanup>;

    /// Apply events and mutations atomically. Returns the committed events
    /// with their sequence numbers, ids and timestamps assigned.
    fn commit(&self, commit: Commit) -> Result<Vec<Event>>;

    /// Replay a run from another daemon's export, atomically. Unlike
    /// [`Store::commit`] the events keep their original timestamps, so an
    /// imported run reads as it happened rather than as it arrived.
    fn import(&self, events: Vec<ImportedEvent>, mutations: Vec<Mutation>) -> Result<Vec<Event>>;

    fn last_seq(&self) -> Result<u64>;
    /// Commits applied to this store since the process opened it. A change
    /// counter, not a sequence: it moves for a mutation-only commit too, so a
    /// reader can tell "nothing has changed" from "no event was written".
    /// Never persisted; comparing values across processes means nothing.
    fn commits(&self) -> u64;
    /// Events with `seq > after`, ascending, at most `limit`.
    fn events(&self, after: u64, limit: usize, filter: &EventFilter) -> Result<Vec<Event>>;
    fn event_by_id(&self, id: EventId) -> Result<Option<Event>>;
    fn event_by_seq(&self, seq: u64) -> Result<Option<Event>>;
    /// The most recent events matching the filter, newest first.
    fn latest_events(&self, limit: usize, filter: &EventFilter) -> Result<Vec<Event>>;
    /// Events with `seq < before`, newest first, at most `limit`.
    fn events_before(&self, before: u64, limit: usize, filter: &EventFilter) -> Result<Vec<Event>>;
    /// Requests and terminal outcomes for these calls, newest first, without a history window.
    fn tool_events(&self, agent: AgentId, scope: ToolEventScope<'_>) -> Result<Vec<Event>>;
    /// Latest notice or recovery for this exhausted model, regardless of intervening events.
    fn latest_exhaustion_incident(&self, agent: AgentId, model: &ModelRef)
    -> Result<Option<Event>>;

    fn schedule(&self, id: ScheduleId) -> Result<Option<Schedule>>;
    fn schedules(&self, target: Option<AgentId>) -> Result<Vec<Schedule>>;
    /// Active jobs due at or before the supplied UTC instant.
    fn due_schedules(&self, at: Timestamp, limit: usize) -> Result<Vec<Schedule>>;
    /// Latest delivery records first, bounded by the caller's page size.
    fn schedule_history(&self, id: ScheduleId, limit: usize) -> Result<Vec<ScheduleOccurrence>>;

    fn agent(&self, id: AgentId) -> Result<Option<Agent>>;
    fn agents(&self, filter: &AgentFilter) -> Result<Vec<Agent>>;
    /// Whether any agent, including archived and terminal agents, carries this repository scope.
    fn any_agent_in_repository(&self, repository: &str) -> Result<bool>;

    /// The id `repository`'s memory is kept under: the canonical id when it
    /// is a registered alias, else `repository` itself.
    fn resolve_repository(&self, repository: &str) -> Result<String>;
    /// Every registered repository alias, ordered by alias.
    fn repository_aliases(&self) -> Result<Vec<RepositoryAlias>>;
    /// What [`Store::alias_repository`] would move now, refused as it would
    /// be refused. Writes nothing.
    fn preview_repository_alias(&self, alias: &RepositoryAlias) -> Result<RepositoryAliasMove>;
    /// Register `alias` and rescope every memory filed under it to the
    /// canonical id, in one transaction with its event. Re-registering the
    /// same pair moves whatever was filed under the alias since; a different
    /// canonical, or a chain of aliases, is [`StoreError::AliasRefused`].
    fn alias_repository(
        &self,
        alias: &RepositoryAlias,
        by: aether_core::event::Actor,
        hold: &Hold<'_>,
    ) -> Result<RepositoryAliasMove>;

    fn task(&self, id: TaskId) -> Result<Option<Task>>;
    fn tasks(&self, filter: &TaskFilter) -> Result<Vec<Task>>;

    /// Inbox items with `seq > after`, ascending.
    fn inbox(&self, agent: AgentId, after: u64) -> Result<Vec<InboxItem>>;
    fn inbox_last_seq(&self, agent: AgentId) -> Result<u64>;

    fn lease(&self, agent: AgentId) -> Result<Option<Lease>>;
    fn leases(&self) -> Result<Vec<Lease>>;

    fn snapshot(&self, id: SnapshotId) -> Result<Option<InvocationSnapshot>>;
    /// The memories each stored snapshot rendered, in the order of `ids`;
    /// ids with no stored snapshot are left out. Disk stores skip the messages.
    fn snapshot_memories(&self, ids: &[SnapshotId]) -> Result<Vec<SnapshotMemories>> {
        let mut out = Vec::with_capacity(ids.len());
        for id in ids {
            if let Some(snapshot) = self.snapshot(*id)? {
                out.push(SnapshotMemories::of(&snapshot));
            }
        }
        Ok(out)
    }
    fn invocation(&self, id: InvocationId) -> Result<Option<Invocation>>;
    fn invocations_for_agent(&self, agent: AgentId) -> Result<Vec<Invocation>>;

    fn checkpoint(&self, id: CheckpointId) -> Result<Option<Checkpoint>>;
    fn latest_checkpoint(&self, agent: AgentId) -> Result<Option<Checkpoint>>;

    fn approval(&self, id: ApprovalId) -> Result<Option<ApprovalRequest>>;
    fn pending_approvals(&self) -> Result<Vec<ApprovalRequest>>;

    /// The outcome committed for `key` while driving `invocation`, if any.
    /// Scoped to the invocation and the call, so only a re-drive of that
    /// attempt replays: a re-drive reuses the recorded call id, while two
    /// identical calls in one response are two calls and both run.
    fn recorded_outcome(
        &self,
        agent: AgentId,
        invocation: InvocationId,
        call: ToolCallId,
        key: &IdempotencyKey,
    ) -> Result<Option<RecordedOutcome>>;

    /// What the client's keyed submission created, if this store has seen the
    /// key. Daemon-wide: the key is the whole identity.
    fn submission(&self, key: &IdempotencyKey) -> Result<Option<RecordedSubmission>>;

    /// How many receipts this store has recorded and which it has dropped.
    /// Read it *after* looking a key up, so a drop the lookup missed is in it.
    fn submission_ledger(&self) -> Result<SubmissionLedger>;

    fn approval_rule(&self, id: ApprovalRuleId) -> Result<Option<ApprovalRule>>;
    /// Remembered decisions, oldest first. Revoked ones only when asked for.
    fn approval_rules(&self, include_revoked: bool) -> Result<Vec<ApprovalRule>>;

    fn device(&self, id: DeviceId) -> Result<Option<DeviceRow>>;
    /// Paired devices, oldest first. Revoked ones only when asked for.
    fn devices(&self, include_revoked: bool) -> Result<Vec<DeviceRow>>;

    /// Uncapped membership in this task's announced, stored snapshots.
    /// When supplied, `through` includes only snapshots created at or before it.
    fn memory_was_in_task_context(
        &self,
        memory: MemoryId,
        task: TaskId,
        through: Option<Timestamp>,
    ) -> Result<bool>;
    fn memory_feedback_record(&self, id: MemoryFeedbackId) -> Result<Option<MemoryFeedback>>;
    fn memory_feedback(&self, memory: MemoryId, limit: usize) -> Result<Vec<MemoryFeedback>>;
    /// [`FeedbackTally::of`] for each of `memories` that has counted feedback,
    /// in one read. A memory with none is absent.
    fn memory_feedback_tallies(
        &self,
        memories: &[MemoryId],
    ) -> Result<BTreeMap<MemoryId, FeedbackTally>>;

    fn memory(&self, id: MemoryId) -> Result<Option<MemoryRecord>>;
    fn memories(&self, query: &MemoryQuery) -> Result<Vec<MemoryRecord>>;

    fn artifact(&self, id: ArtifactId) -> Result<Option<Artifact>>;
    fn artifacts_for_task(&self, task: TaskId) -> Result<Vec<Artifact>>;
    fn artifacts(&self, filter: &ArtifactFilter) -> Result<Vec<Artifact>>;
    /// An artifact with the same content that still has its bytes, so an
    /// identical output is stored once. The lookup does not reserve its bytes;
    /// callers coordinate eviction and publication through the reference commit.
    fn artifact_by_digest(&self, digest: &str) -> Result<Option<Artifact>>;
    /// Every artifact that still has bytes, oldest first. Retention reads it.
    fn live_artifacts(&self) -> Result<Vec<Artifact>>;
    /// Drop an artifact's bytes, keeping its record. Idempotent.
    /// Use the same external coordinator as shared-payload lookup/import and publication.
    fn evict_artifact_bytes(&self, id: ArtifactId) -> Result<()>;
    /// Remove an unpublished payload at this id's own path, only if no live row references it.
    /// The caller excludes publication and eviction while cleaning up.
    fn remove_unreferenced_artifact_bytes(&self, id: ArtifactId) -> Result<()>;
    /// Low-level direct write; publications use `stage_artifact_bytes` instead.
    fn write_artifact_bytes(&self, id: ArtifactId, bytes: &[u8]) -> Result<PathBuf>;
    /// Stage bytes and retain the lease through the row commit and promotion.
    fn stage_artifact_bytes(&self, id: ArtifactId, bytes: &[u8]) -> Result<StagedArtifact>;
    /// Promote only after the row commits. Failures leave the staged payload recoverable.
    fn finish_artifact_publication(&self, publication: &StagedArtifact) -> Result<()>;
    /// Import a file, preserving content deduplication. Disk stores stream the copy and hash.
    /// The caller excludes eviction through the subsequent artifact-record commit.
    fn import_artifact_file(
        &self,
        id: ArtifactId,
        source: &std::path::Path,
    ) -> Result<ImportedArtifact> {
        use sha2::Digest;
        let bytes = std::fs::read(source)?;
        let size_bytes = bytes.len() as u64;
        let digest = hex::encode(sha2::Sha256::digest(&bytes));
        if let Some(twin) = self.artifact_by_digest(&digest)?
            && let Some(path) = twin.path
            && self.artifact_bytes_exist(twin.id)?
        {
            return Ok(ImportedArtifact {
                path,
                size_bytes,
                digest,
                shared_with: Some(twin.id),
                publication: None,
            });
        }
        let publication = self.stage_artifact_bytes(id, &bytes)?;
        Ok(ImportedArtifact {
            path: publication.path.clone(),
            size_bytes,
            digest,
            shared_with: None,
            publication: Some(publication),
        })
    }
    fn read_artifact_bytes(&self, id: ArtifactId) -> Result<Option<Vec<u8>>>;
    /// Read a bounded byte range. Disk-backed stores should seek rather than load the payload.
    fn read_artifact_range(
        &self,
        id: ArtifactId,
        offset: u64,
        limit: usize,
    ) -> Result<Option<Vec<u8>>> {
        Ok(self.read_artifact_bytes(id)?.map(|bytes| {
            let start = usize::try_from(offset)
                .unwrap_or(usize::MAX)
                .min(bytes.len());
            bytes[start..start.saturating_add(limit).min(bytes.len())].to_vec()
        }))
    }
    /// Whether the payload is still there, without reading it. Records that
    /// share a digest share one payload, so a caller about to point a new
    /// record at it needs to know it has not just been swept.
    fn artifact_bytes_exist(&self, id: ArtifactId) -> Result<bool>;

    /// The agent's transcript after `after` seq, oldest first.
    fn transcript(&self, agent: AgentId, after: u64) -> Result<Vec<TranscriptEntry>>;

    fn meta(&self, key: &str) -> Result<Option<String>>;
    /// Stable id of this store, minted on first open.
    fn instance_id(&self) -> Result<String>;
}
