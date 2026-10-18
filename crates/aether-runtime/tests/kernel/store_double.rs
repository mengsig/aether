//! store double scenarios and fixtures.
use std::path::PathBuf;
use std::sync::Arc;

use aether_core::agent::{Agent, AgentStatusKind};
use aether_core::approval::ApprovalRule;
use aether_core::artifact::Artifact;
use aether_core::checkpoint::Checkpoint;
use aether_core::event::{Event, EventKind};
use aether_core::id::{
    AgentId, ApprovalId, ApprovalRuleId, ArtifactId, CheckpointId, EventId, InvocationId, MemoryId,
    SnapshotId, TaskId, ToolCallId,
};
use aether_core::idempotency::{IdempotencyKey, RecordedOutcome};
use aether_core::inbox::InboxItem;
use aether_core::lease::Lease;
use aether_core::memory::MemoryRecord;
use aether_core::snapshot::{Invocation, InvocationSnapshot};
use aether_core::task::Task;
use aether_core::tool::ApprovalRequest;
use aether_store::{
    AgentFilter, ArtifactFilter, Commit, EventFilter, MemoryQuery, Result, Store, TaskFilter,
    TranscriptEntry,
};

/// The read (or, for `ChildCreatedCommit`, the write) an armed action
/// lands on, naming it by what makes that moment the interesting one
/// rather than by its arguments.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Hook {
    /// The budgeted roots: the read `check_budgets` decides from.
    BudgetPassReadsAgents,
    /// This agent's inbox seq while it stands Ready: the scheduler asking
    /// whether it has work, after it took its snapshot of the fleet and
    /// before it dispatches. Ready is what separates that read from the
    /// same one a waiting agent's condition makes.
    SchedulerAsksIfReadyAgentHasWork,
    /// A commit that creates a child agent, fired just before it lands -
    /// so an armed write sees the fleet as it stood with no such child
    /// yet, the same view `cancel_agent`'s own children walk would get if
    /// it ran concurrently one instant earlier.
    ChildCreatedCommit,
    ArtifactPublicationCommit,
    ArtifactPromotion,
    /// The cap advisory's own fleet read (`turn::cap_status`), made just
    /// after `create_agent` returns: self-arming (no `arm` call needed) -
    /// a child-creation commit sets `fail_next_read`, so the very next
    /// matching read fails instead of an unrelated scheduler tick's,
    /// proving the advisory can never fail the turn.
    CapAdvisoryFleetRead,
    CancelDescendantRead,
    CancelMutationFailure,
    /// One agent row, fired after the read returns: a writer landing here
    /// changes the row after the caller read it and before it writes back.
    AgentRead,
    /// The dependency gate's first owner read, after scanning queued tasks.
    DependencyOwnerRead,
    /// The whole agent table, the same moment for a subtree's rows.
    FleetRead,
    /// One task row, fired after the read returns.
    TaskRead,
    /// The turn reads whether a context fold is pending, before it starts
    /// assembling the model request.
    CompactionPending,
    /// The completed task's transcript read, after the task-boundary pending
    /// check and before its request would be committed.
    TaskBoundaryTranscriptRead,
    MemoryRetrievalRead,
    /// The one read of counted feedback a retrieval with candidates makes.
    FeedbackTallyRead,
    FeedbackContextRead,
    /// A repository id resolved through the aliases, fired after the
    /// resolution returns: an alias landing here makes that answer stale.
    RepositoryResolved,
}

/// A write a concurrent actor makes, run against the underlying store.
pub type Armed = Box<dyn FnOnce(&dyn Store) -> Result<()> + Send>;

/// A store that runs one armed write inside a chosen read, standing in for
/// a concurrent writer - or, armed with `fail_next_read` instead, makes
/// that one read itself return an error. Every other call goes straight
/// through.
pub struct CommitsMidPass {
    inner: Arc<dyn Store>,
    hook: Hook,
    armed: std::sync::Mutex<Option<Armed>>,
    fail_next_read: std::sync::atomic::AtomicBool,
    read_failed: std::sync::atomic::AtomicBool,
    dependency_owner_read: std::sync::atomic::AtomicBool,
    skip: std::sync::atomic::AtomicUsize,
    tally_reads: std::sync::atomic::AtomicUsize,
}

impl CommitsMidPass {
    pub fn new(inner: Arc<dyn Store>, hook: Hook) -> Self {
        Self {
            inner,
            hook,
            armed: std::sync::Mutex::new(None),
            fail_next_read: std::sync::atomic::AtomicBool::new(false),
            read_failed: std::sync::atomic::AtomicBool::new(false),
            dependency_owner_read: std::sync::atomic::AtomicBool::new(false),
            skip: std::sync::atomic::AtomicUsize::new(0),
            tally_reads: std::sync::atomic::AtomicUsize::new(0),
        }
    }

    /// Arm `write` for the hook's `skip + 1`th moment, not its next one.
    pub fn arm_skipping(&self, skip: usize, write: Armed) {
        self.skip.store(skip, std::sync::atomic::Ordering::SeqCst);
        self.arm(write);
    }

    pub fn arm(&self, write: Armed) {
        *self.armed.lock().expect("armed") = Some(write);
    }

    /// Whether an armed read failure actually fired.
    pub fn read_failed(&self) -> bool {
        self.read_failed.load(std::sync::atomic::Ordering::SeqCst)
    }

    /// How many counted-feedback reads have reached the store.
    pub fn tally_reads(&self) -> usize {
        self.tally_reads.load(std::sync::atomic::Ordering::SeqCst)
    }

    /// Whether the armed write has run.
    pub fn fired(&self) -> bool {
        self.armed.lock().expect("armed").is_none()
    }

    fn fire(&self, hook: Hook) -> Result<()> {
        if hook != self.hook {
            return Ok(());
        }
        let skipped = self.skip.fetch_update(
            std::sync::atomic::Ordering::SeqCst,
            std::sync::atomic::Ordering::SeqCst,
            |n| n.checked_sub(1),
        );
        if skipped.is_ok() {
            return Ok(());
        }
        let write = self.armed.lock().expect("armed").take();
        match write {
            Some(write) => write(self.inner.as_ref()),
            None => Ok(()),
        }
    }
}

impl Store for CommitsMidPass {
    fn engine_session_ownerships(
        &self,
    ) -> aether_store::Result<Vec<aether_store::EngineSessionOwnership>> {
        self.inner.engine_session_ownerships()
    }

    fn expire_run_history(
        &self,
        run: aether_core::id::RunId,
        before: aether_core::time::Timestamp,
        hold: &aether_store::Hold<'_>,
    ) -> Result<aether_store::RunExpiry> {
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
    ) -> Result<Option<aether_store::RunExpiry>> {
        self.inner.screen_run_expiry(run, before)
    }

    fn is_descendant(&self, root: AgentId, id: AgentId) -> Result<bool> {
        self.inner.is_descendant(root, id)
    }
    fn subtree_ids(&self, root: AgentId) -> Result<std::collections::BTreeSet<AgentId>> {
        self.inner.subtree_ids(root)
    }
    fn any_agent_in_repository(&self, repository: &str) -> Result<bool> {
        self.inner.any_agent_in_repository(repository)
    }
    fn resolve_repository(&self, repository: &str) -> Result<String> {
        let resolved = self.inner.resolve_repository(repository)?;
        self.fire(Hook::RepositoryResolved)?;
        Ok(resolved)
    }
    fn repository_aliases(&self) -> Result<Vec<aether_core::repository::RepositoryAlias>> {
        self.inner.repository_aliases()
    }
    fn preview_repository_alias(
        &self,
        alias: &aether_core::repository::RepositoryAlias,
    ) -> Result<aether_store::RepositoryAliasMove> {
        self.inner.preview_repository_alias(alias)
    }
    fn alias_repository(
        &self,
        alias: &aether_core::repository::RepositoryAlias,
        by: aether_core::event::Actor,
        hold: &aether_store::Hold<'_>,
    ) -> Result<aether_store::RepositoryAliasMove> {
        self.inner.alias_repository(alias, by, hold)
    }
    fn agents(&self, filter: &AgentFilter) -> Result<Vec<Agent>> {
        if self.hook == Hook::CancelDescendantRead
            && filter.parent.is_some()
            && self
                .fail_next_read
                .swap(false, std::sync::atomic::Ordering::SeqCst)
        {
            self.read_failed
                .store(true, std::sync::atomic::Ordering::SeqCst);
            return Err(aether_store::StoreError::Corrupt(
                "simulated cancellation walk failure".into(),
            ));
        }
        if self.hook == Hook::CapAdvisoryFleetRead
            && *filter == AgentFilter::default()
            && self
                .fail_next_read
                .swap(false, std::sync::atomic::Ordering::SeqCst)
        {
            self.read_failed
                .store(true, std::sync::atomic::Ordering::SeqCst);
            return Err(aether_store::StoreError::Corrupt(
                "simulated fleet read failure".into(),
            ));
        }
        let agents = self.inner.agents(filter)?;
        if filter.budgeted_only {
            self.fire(Hook::BudgetPassReadsAgents)?;
        }
        if *filter == AgentFilter::default() {
            self.fire(Hook::FleetRead)?;
        }
        Ok(agents)
    }

    fn inbox_last_seq(&self, agent: AgentId) -> Result<u64> {
        // Ready is what separates the scheduler's has-work read from the
        // same one a waiting agent's condition makes. Fired before the
        // read, so the caller sees what the commit delivered.
        if self.inner.agent(agent)?.map(|a| a.status.kind()) == Some(AgentStatusKind::Ready) {
            self.fire(Hook::SchedulerAsksIfReadyAgentHasWork)?;
        }
        self.inner.inbox_last_seq(agent)
    }

    fn commit(&self, commit: Commit) -> Result<Vec<Event>> {
        if commit
            .mutations
            .iter()
            .any(|m| matches!(m, aether_store::Mutation::PutArtifact(_)))
        {
            self.fire(Hook::ArtifactPublicationCommit)?;
        }
        let cancels_agent = commit
            .events
            .iter()
            .any(|event| matches!(event.kind, EventKind::AgentCancelled { .. }));
        if cancels_agent
            && self.hook == Hook::CancelMutationFailure
            && !self
                .read_failed
                .swap(true, std::sync::atomic::Ordering::SeqCst)
        {
            return Err(aether_store::StoreError::Corrupt(
                "simulated cancellation commit failure".into(),
            ));
        }
        if cancels_agent && self.hook == Hook::CancelDescendantRead {
            self.fail_next_read
                .store(true, std::sync::atomic::Ordering::SeqCst);
        }
        let creates_child = commit.events.iter().any(|e| {
            matches!(
                &e.kind,
                EventKind::AgentCreated {
                    parent: Some(_),
                    ..
                }
            )
        });
        if self.hook == Hook::ChildCreatedCommit && creates_child {
            self.fire(Hook::ChildCreatedCommit)?;
        }
        if self.hook == Hook::CapAdvisoryFleetRead && creates_child {
            // Arm right as the child commits, synchronously, so the very
            // next fleet read - `turn::cap_status`'s own, made right
            // after `create_agent` returns in the same call stack - is
            // the one that fails, not an unrelated scheduler tick.
            self.fail_next_read
                .store(true, std::sync::atomic::Ordering::SeqCst);
        }
        self.inner.commit(commit)
    }
    fn import(
        &self,
        events: Vec<aether_store::ImportedEvent>,
        mutations: Vec<aether_store::Mutation>,
    ) -> Result<Vec<Event>> {
        self.inner.import(events, mutations)
    }
    fn last_seq(&self) -> Result<u64> {
        self.inner.last_seq()
    }
    fn commits(&self) -> u64 {
        self.inner.commits()
    }
    fn events(&self, after: u64, limit: usize, filter: &EventFilter) -> Result<Vec<Event>> {
        if self.hook == Hook::FeedbackContextRead && filter.types == ["invocation.failed"] {
            assert!(filter.agent.is_some());
            assert!(filter.exclude_children);
            assert!(filter.task.is_none());
            self.fire(Hook::FeedbackContextRead)?;
        }
        self.inner.events(after, limit, filter)
    }
    fn event_by_id(&self, id: EventId) -> Result<Option<Event>> {
        self.inner.event_by_id(id)
    }
    fn event_by_seq(&self, seq: u64) -> Result<Option<Event>> {
        self.inner.event_by_seq(seq)
    }
    fn events_before(&self, before: u64, limit: usize, filter: &EventFilter) -> Result<Vec<Event>> {
        self.inner.events_before(before, limit, filter)
    }

    fn latest_events(&self, limit: usize, filter: &EventFilter) -> Result<Vec<Event>> {
        if filter
            .types
            .iter()
            .any(|kind| kind == "compaction.requested")
            && let Some(id) = filter.agent
            && self
                .inner
                .agent(id)?
                .is_some_and(|agent| agent.status.kind() == AgentStatusKind::Running)
        {
            self.fire(Hook::CompactionPending)?;
        }
        self.inner.latest_events(limit, filter)
    }
    fn tool_events(
        &self,
        agent: aether_core::id::AgentId,
        scope: aether_store::ToolEventScope<'_>,
    ) -> Result<Vec<Event>> {
        self.inner.tool_events(agent, scope)
    }
    fn latest_exhaustion_incident(
        &self,
        agent: aether_core::id::AgentId,
        model: &aether_core::model::ModelRef,
    ) -> Result<Option<Event>> {
        self.inner.latest_exhaustion_incident(agent, model)
    }
    fn agent(&self, id: AgentId) -> Result<Option<Agent>> {
        let agent = self.inner.agent(id)?;
        self.fire(Hook::AgentRead)?;
        if self
            .dependency_owner_read
            .swap(false, std::sync::atomic::Ordering::SeqCst)
        {
            self.fire(Hook::DependencyOwnerRead)?;
        }
        Ok(agent)
    }
    fn task(&self, id: TaskId) -> Result<Option<Task>> {
        let task = self.inner.task(id)?;
        self.fire(Hook::TaskRead)?;
        Ok(task)
    }
    fn tasks(&self, filter: &TaskFilter) -> Result<Vec<Task>> {
        let tasks = self.inner.tasks(filter)?;
        if self.hook == Hook::DependencyOwnerRead
            && filter.statuses == [aether_core::task::TaskStatusKind::Queued]
            && tasks
                .iter()
                .any(|task| task.owner.is_some() && !task.dependencies.is_empty())
        {
            self.dependency_owner_read
                .store(true, std::sync::atomic::Ordering::SeqCst);
        }
        Ok(tasks)
    }
    fn inbox(&self, agent: AgentId, after: u64) -> Result<Vec<InboxItem>> {
        self.inner.inbox(agent, after)
    }
    fn lease(&self, agent: AgentId) -> Result<Option<Lease>> {
        self.inner.lease(agent)
    }
    fn leases(&self) -> Result<Vec<Lease>> {
        self.inner.leases()
    }
    fn snapshot(&self, id: SnapshotId) -> Result<Option<InvocationSnapshot>> {
        self.inner.snapshot(id)
    }
    fn snapshot_memories(
        &self,
        ids: &[SnapshotId],
    ) -> Result<Vec<aether_core::snapshot::SnapshotMemories>> {
        self.fire(Hook::FeedbackContextRead)?;
        let found = self.inner.snapshot_memories(ids)?;
        assert!(found.len() <= ids.len());
        assert!(found.iter().all(|entry| ids.contains(&entry.snapshot)));
        Ok(found)
    }
    fn invocation(&self, id: InvocationId) -> Result<Option<Invocation>> {
        self.inner.invocation(id)
    }
    fn invocations_for_agent(&self, agent: AgentId) -> Result<Vec<Invocation>> {
        self.inner.invocations_for_agent(agent)
    }
    fn checkpoint(&self, id: CheckpointId) -> Result<Option<Checkpoint>> {
        self.inner.checkpoint(id)
    }
    fn latest_checkpoint(&self, agent: AgentId) -> Result<Option<Checkpoint>> {
        self.inner.latest_checkpoint(agent)
    }
    fn approval(&self, id: ApprovalId) -> Result<Option<ApprovalRequest>> {
        self.inner.approval(id)
    }
    fn pending_approvals(&self) -> Result<Vec<ApprovalRequest>> {
        self.inner.pending_approvals()
    }
    fn memory_was_in_task_context(
        &self,
        memory: MemoryId,
        task: TaskId,
        through: Option<aether_core::time::Timestamp>,
    ) -> aether_store::Result<bool> {
        self.fire(Hook::FeedbackContextRead)?;
        self.inner.memory_was_in_task_context(memory, task, through)
    }

    fn memory_feedback_record(
        &self,
        id: aether_core::id::MemoryFeedbackId,
    ) -> aether_store::Result<Option<aether_core::memory_feedback::MemoryFeedback>> {
        self.inner.memory_feedback_record(id)
    }

    fn memory_feedback(
        &self,
        memory: MemoryId,
        limit: usize,
    ) -> aether_store::Result<Vec<aether_core::memory_feedback::MemoryFeedback>> {
        self.inner.memory_feedback(memory, limit)
    }

    fn memory_feedback_tallies(
        &self,
        memories: &[MemoryId],
    ) -> aether_store::Result<
        std::collections::BTreeMap<MemoryId, aether_core::memory_feedback::FeedbackTally>,
    > {
        self.tally_reads
            .fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        self.fire(Hook::FeedbackTallyRead)?;
        self.inner.memory_feedback_tallies(memories)
    }

    fn memory(&self, id: MemoryId) -> Result<Option<MemoryRecord>> {
        self.inner.memory(id)
    }
    fn memories(&self, query: &MemoryQuery) -> Result<Vec<MemoryRecord>> {
        self.fire(Hook::MemoryRetrievalRead)?;
        self.inner.memories(query)
    }
    fn artifact(&self, id: ArtifactId) -> Result<Option<Artifact>> {
        self.inner.artifact(id)
    }
    fn artifacts_for_task(&self, task: TaskId) -> Result<Vec<Artifact>> {
        self.inner.artifacts_for_task(task)
    }
    fn remove_unreferenced_artifact_bytes(&self, id: ArtifactId) -> Result<()> {
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
        self.fire(Hook::ArtifactPromotion)?;
        self.inner.finish_artifact_publication(publication)
    }

    fn write_artifact_bytes(&self, id: ArtifactId, bytes: &[u8]) -> Result<PathBuf> {
        self.inner.write_artifact_bytes(id, bytes)
    }
    fn import_artifact_file(
        &self,
        id: ArtifactId,
        source: &std::path::Path,
    ) -> Result<aether_store::ImportedArtifact> {
        self.inner.import_artifact_file(id, source)
    }
    fn read_artifact_bytes(&self, id: ArtifactId) -> Result<Option<Vec<u8>>> {
        self.inner.read_artifact_bytes(id)
    }
    fn recorded_outcome(
        &self,
        agent: AgentId,
        invocation: InvocationId,
        call: ToolCallId,
        key: &IdempotencyKey,
    ) -> Result<Option<RecordedOutcome>> {
        self.inner.recorded_outcome(agent, invocation, call, key)
    }
    fn submission(
        &self,
        key: &IdempotencyKey,
    ) -> Result<Option<aether_core::idempotency::RecordedSubmission>> {
        self.inner.submission(key)
    }
    fn submission_ledger(&self) -> Result<aether_core::idempotency::SubmissionLedger> {
        self.inner.submission_ledger()
    }
    fn approval_rule(&self, id: ApprovalRuleId) -> Result<Option<ApprovalRule>> {
        self.inner.approval_rule(id)
    }
    fn approval_rules(&self, include_revoked: bool) -> Result<Vec<ApprovalRule>> {
        self.inner.approval_rules(include_revoked)
    }
    fn device(&self, id: aether_core::id::DeviceId) -> Result<Option<aether_store::DeviceRow>> {
        self.inner.device(id)
    }
    fn devices(&self, include_revoked: bool) -> Result<Vec<aether_store::DeviceRow>> {
        self.inner.devices(include_revoked)
    }
    fn artifacts(&self, filter: &ArtifactFilter) -> Result<Vec<Artifact>> {
        self.inner.artifacts(filter)
    }
    fn artifact_by_digest(&self, digest: &str) -> Result<Option<Artifact>> {
        self.inner.artifact_by_digest(digest)
    }
    fn live_artifacts(&self) -> Result<Vec<Artifact>> {
        self.inner.live_artifacts()
    }
    fn evict_artifact_bytes(&self, id: ArtifactId) -> Result<()> {
        self.inner.evict_artifact_bytes(id)
    }
    fn artifact_bytes_exist(&self, id: ArtifactId) -> Result<bool> {
        self.inner.artifact_bytes_exist(id)
    }
    fn transcript(&self, agent: AgentId, after: u64) -> Result<Vec<TranscriptEntry>> {
        let entries = self.inner.transcript(agent, after)?;
        if self.hook == Hook::TaskBoundaryTranscriptRead {
            let task_done = match self.inner.agent(agent)?.and_then(|row| row.current_task) {
                Some(task) => self
                    .inner
                    .task(task)?
                    .is_some_and(|task| task.status.is_terminal()),
                None => false,
            };
            if task_done {
                self.fire(Hook::TaskBoundaryTranscriptRead)?;
            }
        }
        Ok(entries)
    }
    fn schedule(
        &self,
        id: aether_core::id::ScheduleId,
    ) -> Result<Option<aether_core::schedule::Schedule>> {
        self.inner.schedule(id)
    }
    fn schedules(&self, target: Option<AgentId>) -> Result<Vec<aether_core::schedule::Schedule>> {
        self.inner.schedules(target)
    }
    fn due_schedules(
        &self,
        at: aether_core::time::Timestamp,
        limit: usize,
    ) -> Result<Vec<aether_core::schedule::Schedule>> {
        self.inner.due_schedules(at, limit)
    }
    fn schedule_history(
        &self,
        id: aether_core::id::ScheduleId,
        limit: usize,
    ) -> Result<Vec<aether_core::schedule::ScheduleOccurrence>> {
        self.inner.schedule_history(id, limit)
    }
    fn meta(&self, key: &str) -> Result<Option<String>> {
        self.inner.meta(key)
    }
    fn instance_id(&self) -> Result<String> {
        self.inner.instance_id()
    }
}
