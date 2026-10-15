#![allow(clippy::expect_used, clippy::unwrap_used, clippy::panic)]

//! A client that loses the answer to a submission retries it, and
//! the retry must not create the work a second time.
//!
//! Every test here drives the runtime directly with a `Submission`, which is
//! what the daemon builds from the client's frame.

use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use aether_core::agent::Agent;
use aether_core::approval::ApprovalRule;
use aether_core::artifact::Artifact;
use aether_core::checkpoint::Checkpoint;
use aether_core::event::{Actor, Event};
use aether_core::id::{
    AgentId, ApprovalId, ApprovalRuleId, ArtifactId, CheckpointId, EventId, InvocationId, MemoryId,
    MessageId, ScheduleId, SnapshotId, TaskId, ToolCallId,
};
use aether_core::idempotency::{IdempotencyKey, RecordedOutcome, RecordedSubmission, Submission};
use aether_core::inbox::{InboxItem, InboxKind};
use aether_core::lease::Lease;
use aether_core::memory::MemoryRecord;
use aether_core::model::{ContentBlock, ModelRef};
use aether_core::schedule::{
    CreateScheduleSpec, Recurrence, Schedule, ScheduleOccurrence, ScheduleTarget,
};
use aether_core::snapshot::{Invocation, InvocationSnapshot};
use aether_core::task::Task;
use aether_core::time::{Timestamp, now};
use aether_core::tool::ApprovalRequest;
use aether_model::Models;
use aether_model::testing::ScriptedClient;
use aether_runtime::{CreateAgentSpec, CreateTaskSpec, Runtime, RuntimeConfig};
use aether_store::{
    AgentFilter, ArtifactFilter, Commit, EventFilter, ImportedEvent, MemoryQuery, Mutation,
    Result as StoreResult, SqliteStore, Store, StoreError, TaskFilter, TranscriptEntry,
};
use aether_tools::ToolRegistry;
use serde_json::json;

struct Fixture {
    runtime: Arc<Runtime>,
    store: Arc<dyn Store>,
    _workspace: tempfile::TempDir,
    _config_dir: tempfile::TempDir,
}

async fn fixture() -> Fixture {
    let store: Arc<dyn Store> = Arc::new(SqliteStore::open_in_memory().expect("store"));
    over(store).await
}

/// A runtime over an existing store, so a test can stand a second daemon on
/// the state the first one left behind.
async fn over(store: Arc<dyn Store>) -> Fixture {
    let workspace = tempfile::tempdir().expect("workspace");
    let config_dir = tempfile::tempdir().expect("config dir");
    let models = Arc::new(Models::load(Some(config_dir.path().to_path_buf())).expect("models"));
    let config = RuntimeConfig {
        holder: "test".into(),
        max_concurrent_turns: 1,
        lease_ttl: Duration::from_secs(30),
        default_model: Some(ModelRef::new("anthropic", "claude-sonnet-5-5")),
        workspace_root: Some(workspace.path().to_path_buf()),
        ..Default::default()
    };
    let runtime = Runtime::start_with_engines(
        config,
        store.clone(),
        models,
        Arc::new(ToolRegistry::with_builtins()),
        Some(Arc::new(ScriptedClient::new())),
        Default::default(),
    )
    .await
    .expect("runtime");
    Fixture {
        runtime,
        store,
        _workspace: workspace,
        _config_dir: config_dir,
    }
}

/// The submission the daemon would build for a `send_message` frame.
fn message_submission(key: &str, to: &str, content: &str) -> Submission {
    Submission::new(
        key,
        &json!({"cmd": "send_message", "to": to, "content": content, "idempotency_key": key}),
    )
    .expect("a well-formed key")
}

fn agent_spec(role: &str) -> CreateAgentSpec {
    CreateAgentSpec {
        role: role.into(),
        config: Some(aether_core::agent::AgentConfig {
            effort: Some(aether_core::model::Effort::Low),
            ..Default::default()
        }),
        start: false,
        ..Default::default()
    }
}

fn agent_submission(key: &str, role: &str) -> Submission {
    Submission::new(key, &json!({"cmd": "create_agent", "role": role})).expect("key")
}

fn task_spec() -> CreateTaskSpec {
    CreateTaskSpec {
        objective: "ship it".into(),
        ..Default::default()
    }
}

fn schedule_spec(target: AgentId) -> CreateScheduleSpec {
    CreateScheduleSpec {
        name: "nightly".into(),
        target: ScheduleTarget::Agent(target),
        recurrence: Recurrence::Interval {
            seconds: std::num::NonZeroU64::new(3600).expect("nonzero"),
        },
        missed: Default::default(),
        busy: Default::default(),
        payload: json!({"source": "timer"}),
        max_deliveries: None,
    }
}

async fn an_agent(fx: &Fixture) -> aether_core::id::AgentId {
    fx.runtime
        .create_agent(agent_spec("worker"), Actor::user())
        .expect("create")
        .0
        .id
}

fn text(body: &str) -> Vec<ContentBlock> {
    vec![ContentBlock::text(body)]
}

// ---------------------------------------------------------------------------
// The same-transaction guarantee.
//
// The guarantee is that the key, its fingerprint and what it created
// become durable in the same transaction as the command's own mutations. A
// daemon that recorded the submission in a second commit would pass every test
// above and still lose the guarantee on a crash between the two, so the tests
// below read the commits themselves.
// ---------------------------------------------------------------------------

/// The mutation a keyed command exists to make, named so a test can look for
/// it in the same commit as the record.
#[derive(Debug, Clone, Copy)]
enum Work {
    Delivery(MessageId),
    Agent(AgentId),
    Task(TaskId),
    Schedule(ScheduleId),
    /// A trigger answers `Ok(())`, so the caller cannot name the item it made.
    Trigger(AgentId),
}

impl Work {
    fn is_in(self, mutations: &[Mutation]) -> bool {
        mutations.iter().any(|mutation| match (self, mutation) {
            (Work::Delivery(wanted), Mutation::DeliverInbox(item)) => {
                matches!(&item.kind, InboxKind::Message { message_id, .. } if *message_id == wanted)
            }
            (Work::Trigger(wanted), Mutation::DeliverInbox(item)) => {
                item.agent_id == wanted && matches!(&item.kind, InboxKind::ExternalTrigger { .. })
            }
            (Work::Agent(wanted), Mutation::UpsertAgent(agent)) => agent.id == wanted,
            (Work::Task(wanted), Mutation::UpsertTask(task)) => task.id == wanted,
            (Work::Schedule(wanted), Mutation::UpsertSchedule(job)) => job.id == wanted,
            _ => false,
        })
    }
}

fn records_submission(mutations: &[Mutation]) -> bool {
    mutations
        .iter()
        .any(|mutation| matches!(mutation, Mutation::RecordSubmission(_)))
}

/// The record and the work it describes became durable together, or the
/// command has no guarantee to offer.
fn assert_committed_together(commits: &[Vec<Mutation>], work: Work, command: &str) {
    let recording: Vec<&Vec<Mutation>> = commits
        .iter()
        .filter(|mutations| records_submission(mutations))
        .collect();
    assert_eq!(
        recording.len(),
        1,
        "{command}: one keyed command writes exactly one submission record, saw {} commits \
         carrying one",
        recording.len()
    );
    assert!(
        work.is_in(recording[0]),
        "{command}: the submission record committed without the {work:?} it describes"
    );
    assert_eq!(
        commits
            .iter()
            .filter(|mutations| work.is_in(mutations))
            .count(),
        1,
        "{command}: the {work:?} landed in a commit of its own, not the record's"
    );
}

/// What `CommitLog` does to a commit before passing it on.
#[derive(Debug, Clone, Copy, PartialEq)]
enum Refusal {
    None,
    /// Fail any commit carrying a submission record.
    Records,
}

/// A store that keeps the mutation vector of every commit it applies, so a
/// test can ask what became durable together, and counts the commits the inner
/// store rejected.
struct CommitLog {
    inner: SqliteStore,
    applied: std::sync::Mutex<Vec<Vec<Mutation>>>,
    rejected: std::sync::atomic::AtomicUsize,
    refusal: Refusal,
}

impl CommitLog {
    fn open(refusal: Refusal) -> Arc<Self> {
        Arc::new(Self {
            inner: SqliteStore::open_in_memory().expect("store"),
            applied: Default::default(),
            rejected: Default::default(),
            refusal,
        })
    }

    /// The commits applied since the last call, so one command's commits are
    /// read without the runtime's startup writes or a test's own setup.
    fn take(&self) -> Vec<Vec<Mutation>> {
        std::mem::take(&mut self.applied.lock().expect("applied"))
    }

    fn rejected(&self) -> usize {
        self.rejected.load(std::sync::atomic::Ordering::SeqCst)
    }
}

async fn logged() -> (Fixture, Arc<CommitLog>) {
    let log = CommitLog::open(Refusal::None);
    let fx = over(log.clone() as Arc<dyn Store>).await;
    (fx, log)
}

impl Store for CommitLog {
    fn expire_run_history(
        &self,
        run: aether_core::id::RunId,
        before: aether_core::time::Timestamp,
        hold: &aether_store::Hold<'_>,
    ) -> StoreResult<aether_store::RunExpiry> {
        self.inner.expire_run_history(run, before, hold)
    }

    fn retention_snapshot(&self) -> aether_store::Result<Box<dyn Store>> {
        self.inner.retention_snapshot()
    }

    fn preview_run_history(
        &self,
        run: aether_core::id::RunId,
        before: aether_core::time::Timestamp,
    ) -> aether_store::Result<Option<aether_core::retention::RunRetentionPreview>> {
        self.inner.preview_run_history(run, before)
    }

    fn pending_artifact_cleanup(
        &self,
    ) -> aether_store::Result<aether_core::retention::PendingArtifactCleanup> {
        self.inner.pending_artifact_cleanup()
    }

    fn screen_run_expiry(
        &self,
        run: aether_core::id::RunId,
        before: aether_core::time::Timestamp,
    ) -> StoreResult<Option<aether_store::RunExpiry>> {
        self.inner.screen_run_expiry(run, before)
    }

    fn commit(&self, commit: Commit) -> StoreResult<Vec<Event>> {
        let mutations = commit.mutations.clone();
        if self.refusal == Refusal::Records && records_submission(&mutations) {
            return Err(StoreError::Corrupt("the test refuses this commit".into()));
        }
        match self.inner.commit(commit) {
            Ok(events) => {
                self.applied.lock().expect("applied").push(mutations);
                Ok(events)
            }
            Err(err) => {
                self.rejected
                    .fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                Err(err)
            }
        }
    }
    fn import(
        &self,
        events: Vec<ImportedEvent>,
        mutations: Vec<Mutation>,
    ) -> StoreResult<Vec<Event>> {
        self.inner.import(events, mutations)
    }
    fn last_seq(&self) -> StoreResult<u64> {
        self.inner.last_seq()
    }
    fn commits(&self) -> u64 {
        self.inner.commits()
    }
    fn events(&self, after: u64, limit: usize, filter: &EventFilter) -> StoreResult<Vec<Event>> {
        self.inner.events(after, limit, filter)
    }
    fn event_by_id(&self, id: EventId) -> StoreResult<Option<Event>> {
        self.inner.event_by_id(id)
    }
    fn event_by_seq(&self, seq: u64) -> StoreResult<Option<Event>> {
        self.inner.event_by_seq(seq)
    }
    fn events_before(
        &self,
        before: u64,
        limit: usize,
        filter: &EventFilter,
    ) -> StoreResult<Vec<Event>> {
        self.inner.events_before(before, limit, filter)
    }

    fn latest_events(&self, limit: usize, filter: &EventFilter) -> StoreResult<Vec<Event>> {
        self.inner.latest_events(limit, filter)
    }
    fn tool_events(
        &self,
        agent: aether_core::id::AgentId,
        scope: aether_store::ToolEventScope<'_>,
    ) -> StoreResult<Vec<Event>> {
        self.inner.tool_events(agent, scope)
    }
    fn latest_exhaustion_incident(
        &self,
        agent: aether_core::id::AgentId,
        model: &aether_core::model::ModelRef,
    ) -> StoreResult<Option<Event>> {
        self.inner.latest_exhaustion_incident(agent, model)
    }
    fn schedule(&self, id: ScheduleId) -> StoreResult<Option<Schedule>> {
        self.inner.schedule(id)
    }
    fn schedules(&self, target: Option<AgentId>) -> StoreResult<Vec<Schedule>> {
        self.inner.schedules(target)
    }
    fn due_schedules(&self, at: Timestamp, limit: usize) -> StoreResult<Vec<Schedule>> {
        self.inner.due_schedules(at, limit)
    }
    fn schedule_history(
        &self,
        id: ScheduleId,
        limit: usize,
    ) -> StoreResult<Vec<ScheduleOccurrence>> {
        self.inner.schedule_history(id, limit)
    }
    fn agent(&self, id: AgentId) -> StoreResult<Option<Agent>> {
        self.inner.agent(id)
    }
    fn any_agent_in_repository(&self, repository: &str) -> StoreResult<bool> {
        self.inner.any_agent_in_repository(repository)
    }
    fn resolve_repository(&self, repository: &str) -> StoreResult<String> {
        self.inner.resolve_repository(repository)
    }
    fn repository_aliases(&self) -> StoreResult<Vec<aether_core::repository::RepositoryAlias>> {
        self.inner.repository_aliases()
    }
    fn preview_repository_alias(
        &self,
        alias: &aether_core::repository::RepositoryAlias,
    ) -> StoreResult<aether_store::RepositoryAliasMove> {
        self.inner.preview_repository_alias(alias)
    }
    fn alias_repository(
        &self,
        alias: &aether_core::repository::RepositoryAlias,
        by: aether_core::event::Actor,
        hold: &aether_store::Hold<'_>,
    ) -> StoreResult<aether_store::RepositoryAliasMove> {
        self.inner.alias_repository(alias, by, hold)
    }
    fn agents(&self, filter: &AgentFilter) -> StoreResult<Vec<Agent>> {
        self.inner.agents(filter)
    }
    fn task(&self, id: TaskId) -> StoreResult<Option<Task>> {
        self.inner.task(id)
    }
    fn tasks(&self, filter: &TaskFilter) -> StoreResult<Vec<Task>> {
        self.inner.tasks(filter)
    }
    fn inbox(&self, agent: AgentId, after: u64) -> StoreResult<Vec<InboxItem>> {
        self.inner.inbox(agent, after)
    }
    fn inbox_last_seq(&self, agent: AgentId) -> StoreResult<u64> {
        self.inner.inbox_last_seq(agent)
    }
    fn lease(&self, agent: AgentId) -> StoreResult<Option<Lease>> {
        self.inner.lease(agent)
    }
    fn leases(&self) -> StoreResult<Vec<Lease>> {
        self.inner.leases()
    }
    fn snapshot(&self, id: SnapshotId) -> StoreResult<Option<InvocationSnapshot>> {
        self.inner.snapshot(id)
    }
    fn invocation(&self, id: InvocationId) -> StoreResult<Option<Invocation>> {
        self.inner.invocation(id)
    }
    fn invocations_for_agent(&self, agent: AgentId) -> StoreResult<Vec<Invocation>> {
        self.inner.invocations_for_agent(agent)
    }
    fn checkpoint(&self, id: CheckpointId) -> StoreResult<Option<Checkpoint>> {
        self.inner.checkpoint(id)
    }
    fn latest_checkpoint(&self, agent: AgentId) -> StoreResult<Option<Checkpoint>> {
        self.inner.latest_checkpoint(agent)
    }
    fn approval(&self, id: ApprovalId) -> StoreResult<Option<ApprovalRequest>> {
        self.inner.approval(id)
    }
    fn pending_approvals(&self) -> StoreResult<Vec<ApprovalRequest>> {
        self.inner.pending_approvals()
    }
    fn recorded_outcome(
        &self,
        agent: AgentId,
        invocation: InvocationId,
        call: ToolCallId,
        key: &IdempotencyKey,
    ) -> StoreResult<Option<RecordedOutcome>> {
        self.inner.recorded_outcome(agent, invocation, call, key)
    }
    fn submission(&self, key: &IdempotencyKey) -> StoreResult<Option<RecordedSubmission>> {
        self.inner.submission(key)
    }
    fn submission_ledger(&self) -> StoreResult<aether_core::idempotency::SubmissionLedger> {
        self.inner.submission_ledger()
    }
    fn approval_rule(&self, id: ApprovalRuleId) -> StoreResult<Option<ApprovalRule>> {
        self.inner.approval_rule(id)
    }
    fn approval_rules(&self, include_revoked: bool) -> StoreResult<Vec<ApprovalRule>> {
        self.inner.approval_rules(include_revoked)
    }
    fn device(
        &self,
        id: aether_core::id::DeviceId,
    ) -> StoreResult<Option<aether_store::DeviceRow>> {
        self.inner.device(id)
    }
    fn devices(&self, include_revoked: bool) -> StoreResult<Vec<aether_store::DeviceRow>> {
        self.inner.devices(include_revoked)
    }
    fn memory(&self, id: MemoryId) -> StoreResult<Option<MemoryRecord>> {
        self.inner.memory(id)
    }
    fn memories(&self, query: &MemoryQuery) -> StoreResult<Vec<MemoryRecord>> {
        self.inner.memories(query)
    }
    fn artifact(&self, id: ArtifactId) -> StoreResult<Option<Artifact>> {
        self.inner.artifact(id)
    }
    fn artifacts_for_task(&self, task: TaskId) -> StoreResult<Vec<Artifact>> {
        self.inner.artifacts_for_task(task)
    }
    fn artifacts(&self, filter: &ArtifactFilter) -> StoreResult<Vec<Artifact>> {
        self.inner.artifacts(filter)
    }
    fn artifact_by_digest(&self, digest: &str) -> StoreResult<Option<Artifact>> {
        self.inner.artifact_by_digest(digest)
    }
    fn live_artifacts(&self) -> StoreResult<Vec<Artifact>> {
        self.inner.live_artifacts()
    }
    fn evict_artifact_bytes(&self, id: ArtifactId) -> StoreResult<()> {
        self.inner.evict_artifact_bytes(id)
    }
    fn remove_unreferenced_artifact_bytes(&self, id: ArtifactId) -> StoreResult<()> {
        self.inner.remove_unreferenced_artifact_bytes(id)
    }

    fn stage_artifact_bytes(
        &self,
        id: ArtifactId,
        bytes: &[u8],
    ) -> aether_store::Result<aether_store::StagedArtifact> {
        self.inner.stage_artifact_bytes(id, bytes)
    }
    fn finish_artifact_publication(
        &self,
        publication: &aether_store::StagedArtifact,
    ) -> aether_store::Result<()> {
        self.inner.finish_artifact_publication(publication)
    }

    fn write_artifact_bytes(&self, id: ArtifactId, bytes: &[u8]) -> StoreResult<PathBuf> {
        self.inner.write_artifact_bytes(id, bytes)
    }
    fn read_artifact_bytes(&self, id: ArtifactId) -> StoreResult<Option<Vec<u8>>> {
        self.inner.read_artifact_bytes(id)
    }
    fn artifact_bytes_exist(&self, id: ArtifactId) -> StoreResult<bool> {
        self.inner.artifact_bytes_exist(id)
    }
    fn transcript(&self, agent: AgentId, after: u64) -> StoreResult<Vec<TranscriptEntry>> {
        self.inner.transcript(agent, after)
    }
    fn meta(&self, key: &str) -> StoreResult<Option<String>> {
        self.inner.meta(key)
    }
    fn instance_id(&self) -> StoreResult<String> {
        self.inner.instance_id()
    }
}

#[path = "idempotency/question_replies.rs"]
mod question_replies;

#[path = "idempotency/replay.rs"]
mod replay;

#[path = "idempotency/validation.rs"]
mod validation;

#[path = "idempotency/concurrency.rs"]
mod concurrency;

#[path = "idempotency/atomicity.rs"]
mod atomicity;

#[path = "idempotency/state_changes.rs"]
mod state_changes;

#[path = "idempotency/resolution.rs"]
mod resolution;
