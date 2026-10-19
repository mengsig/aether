//! A disk that fills mid-turn and is freed later. Every write in between
//! fails the way SQLite fails one on a full disk; reads keep working.
use super::*;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};

#[derive(Default)]
struct Disk {
    full: AtomicBool,
    /// Refuses commits only, so a payload unlink would still succeed.
    commits_refused: AtomicBool,
    refused: AtomicU64,
}

impl Disk {
    fn fill(&self) {
        self.full.store(true, Ordering::SeqCst);
    }

    fn free(&self) {
        self.full.store(false, Ordering::SeqCst);
    }

    /// Writes refused since the disk filled.
    fn refused(&self) -> u64 {
        self.refused.load(Ordering::SeqCst)
    }

    fn refuse_commit(&self) -> StoreResult<()> {
        if self.commits_refused.load(Ordering::SeqCst) {
            return self.refusal();
        }
        self.refuse()
    }

    fn refuse(&self) -> StoreResult<()> {
        if !self.full.load(Ordering::SeqCst) {
            return Ok(());
        }
        self.refusal()
    }

    fn refusal(&self) -> StoreResult<()> {
        self.refused.fetch_add(1, Ordering::SeqCst);
        Err(aether_store::StoreError::Sqlite(
            rusqlite::Error::SqliteFailure(
                rusqlite::ffi::Error::new(rusqlite::ffi::SQLITE_FULL),
                Some("database or disk is full".into()),
            ),
        ))
    }
}

struct FullDisk {
    inner: Arc<dyn Store>,
    disk: Arc<Disk>,
}

fn over(disk: &Arc<Disk>) -> Arc<dyn Store> {
    Arc::new(FullDisk {
        inner: Arc::new(SqliteStore::open_in_memory().expect("store")),
        disk: disk.clone(),
    })
}

impl Store for FullDisk {
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
    ) -> StoreResult<aether_store::RunExpiry> {
        self.disk.refuse()?;
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
        self.disk.refuse_commit()?;
        self.inner.commit(commit)
    }
    fn import(
        &self,
        events: Vec<aether_store::ImportedEvent>,
        mutations: Vec<Mutation>,
    ) -> StoreResult<Vec<Event>> {
        self.disk.refuse()?;
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
    fn agent(&self, id: AgentId) -> StoreResult<Option<Agent>> {
        self.inner.agent(id)
    }
    fn is_descendant(&self, root: AgentId, id: AgentId) -> StoreResult<bool> {
        self.inner.is_descendant(root, id)
    }
    fn fleet_usage(&self) -> StoreResult<aether_core::model::Usage> {
        self.inner.fleet_usage()
    }
    fn subtree_ids(&self, root: AgentId) -> StoreResult<std::collections::BTreeSet<AgentId>> {
        self.inner.subtree_ids(root)
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
    fn submission(
        &self,
        key: &IdempotencyKey,
    ) -> StoreResult<Option<aether_core::idempotency::RecordedSubmission>> {
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
    fn notification(
        &self,
        id: aether_core::id::NotificationId,
    ) -> StoreResult<Option<aether_core::notification::Notification>> {
        self.inner.notification(id)
    }
    fn notification_by_key(
        &self,
        source: &str,
        key: &str,
    ) -> StoreResult<Option<aether_core::notification::Notification>> {
        self.inner.notification_by_key(source, key)
    }
    fn notifications(
        &self,
        filter: &aether_store::NotificationFilter,
    ) -> StoreResult<Vec<aether_core::notification::Notification>> {
        self.inner.notifications(filter)
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
    fn memory_was_in_task_context(
        &self,
        memory: MemoryId,
        task: TaskId,
        through: Option<aether_core::time::Timestamp>,
    ) -> aether_store::Result<bool> {
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
        self.inner.memory_feedback_tallies(memories)
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
        self.disk.refuse()?;
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
        self.disk.refuse()?;
        self.inner.stage_artifact_bytes(id, bytes)
    }
    fn finish_artifact_publication(
        &self,
        publication: &aether_store::StagedArtifact,
    ) -> aether_store::Result<()> {
        self.inner.finish_artifact_publication(publication)
    }

    fn write_artifact_bytes(&self, id: ArtifactId, bytes: &[u8]) -> StoreResult<PathBuf> {
        self.disk.refuse()?;
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
    fn schedule(
        &self,
        id: aether_core::id::ScheduleId,
    ) -> StoreResult<Option<aether_core::schedule::Schedule>> {
        self.inner.schedule(id)
    }
    fn schedules(
        &self,
        target: Option<AgentId>,
    ) -> StoreResult<Vec<aether_core::schedule::Schedule>> {
        self.inner.schedules(target)
    }
    fn due_schedules(
        &self,
        at: aether_core::time::Timestamp,
        limit: usize,
    ) -> StoreResult<Vec<aether_core::schedule::Schedule>> {
        self.inner.due_schedules(at, limit)
    }
    fn schedule_history(
        &self,
        id: aether_core::id::ScheduleId,
        limit: usize,
    ) -> StoreResult<Vec<aether_core::schedule::ScheduleOccurrence>> {
        self.inner.schedule_history(id, limit)
    }
    fn meta(&self, key: &str) -> StoreResult<Option<String>> {
        self.inner.meta(key)
    }
    fn instance_id(&self) -> StoreResult<String> {
        self.inner.instance_id()
    }
}

/// Fills the disk while it runs: the next write the turn makes - its own
/// result - is the first one refused.
struct FillDisk(Arc<Disk>);

#[async_trait::async_trait]
impl aether_tools::Tool for FillDisk {
    fn descriptor(&self) -> aether_core::tool::ToolDescriptor {
        aether_core::tool::ToolDescriptor {
            spec: aether_core::model::ToolSpec {
                name: "fill_disk".into(),
                description: "fills the disk".into(),
                input_schema: json!({"type": "object", "properties": {}}),
                strict: false,
            },
            side_effect: SideEffectClass::ReadOnly,
            needs_network: false,
        }
    }

    fn resources(
        &self,
        _input: &serde_json::Value,
        _ctx: &aether_tools::ToolContext,
    ) -> Vec<aether_core::tool::ResourceKey> {
        Vec::new()
    }

    async fn execute(
        &self,
        _input: serde_json::Value,
        _ctx: &aether_tools::ToolContext,
        _cancel: tokio_util::sync::CancellationToken,
    ) -> Result<aether_tools::ToolExecution, aether_tools::ToolError> {
        self.0.fill();
        Ok(aether_tools::ToolExecution::Outcome(ToolOutcome::ok(
            "the disk is full now",
        )))
    }
}

/// Holds the disk full for a while and frees it before returning, so the
/// turn itself writes nothing while it is full; only its heartbeat does.
struct FullForAWhile(Arc<Disk>);

#[async_trait::async_trait]
impl aether_tools::Tool for FullForAWhile {
    fn descriptor(&self) -> aether_core::tool::ToolDescriptor {
        aether_core::tool::ToolDescriptor {
            spec: aether_core::model::ToolSpec {
                name: "full_for_a_while".into(),
                description: "fills the disk, waits, frees it".into(),
                input_schema: json!({"type": "object", "properties": {}}),
                strict: false,
            },
            side_effect: SideEffectClass::ReadOnly,
            needs_network: false,
        }
    }

    fn resources(
        &self,
        _input: &serde_json::Value,
        _ctx: &aether_tools::ToolContext,
    ) -> Vec<aether_core::tool::ResourceKey> {
        Vec::new()
    }

    async fn execute(
        &self,
        _input: serde_json::Value,
        _ctx: &aether_tools::ToolContext,
        _cancel: tokio_util::sync::CancellationToken,
    ) -> Result<aether_tools::ToolExecution, aether_tools::ToolError> {
        self.0.fill();
        let deadline = std::time::Instant::now() + Duration::from_secs(10);
        while self.0.refused() < 2 && std::time::Instant::now() < deadline {
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        self.0.free();
        Ok(aether_tools::ToolExecution::Outcome(ToolOutcome::ok(
            "the disk was full for a while",
        )))
    }
}

/// Fills the disk, then panics on the turn's own task, so the failure the
/// turn has to record meets a disk that refuses it.
struct FillThenPanic(Arc<Disk>);

#[async_trait::async_trait]
impl aether_tools::Tool for FillThenPanic {
    fn descriptor(&self) -> aether_core::tool::ToolDescriptor {
        aether_core::tool::ToolDescriptor {
            spec: aether_core::model::ToolSpec {
                name: "fill_then_panic".into(),
                description: "fills the disk, then panics".into(),
                input_schema: json!({"type": "object", "properties": {}}),
                strict: false,
            },
            side_effect: SideEffectClass::Mutation,
            needs_network: false,
        }
    }

    fn resources(
        &self,
        _input: &serde_json::Value,
        _ctx: &aether_tools::ToolContext,
    ) -> Vec<aether_core::tool::ResourceKey> {
        Vec::new()
    }

    fn leases_resources(&self, _input: &serde_json::Value) -> bool {
        self.0.fill();
        panic!("panicked on a full disk");
    }

    async fn execute(
        &self,
        _input: serde_json::Value,
        _ctx: &aether_tools::ToolContext,
        _cancel: tokio_util::sync::CancellationToken,
    ) -> Result<aether_tools::ToolExecution, aether_tools::ToolError> {
        unreachable!("the turn never gets this far")
    }
}

/// What the outage left on the agent's log, once the disk took writes again.
fn outage_record(store: &dyn Store, id: AgentId) -> serde_json::Value {
    store
        .events(0, 10_000, &EventFilter::for_agent(id))
        .expect("events")
        .into_iter()
        .find_map(|e| match e.kind {
            EventKind::Custom { name, payload } if name == "store.write_failed" => Some(payload),
            _ => None,
        })
        .expect("the refused write left no trace on the agent")
}

/// 2026-10-06: the host disk filled mid-turn for four agents. Each turn's
/// next commit failed, the failure it then tried to record failed too, and
/// the agent stayed Running with a lease no one would ever clear: the
/// scheduler never revisits a Running agent and only a restart reaps leases.
/// Freeing the disk changed nothing until the daemon was restarted 84
/// minutes later. The turn must instead be recovered once writes land.
#[tokio::test]
async fn a_disk_that_fills_mid_turn_does_not_strand_the_agent_once_freed() {
    let disk = Arc::new(Disk::default());
    let h = harness_full(
        vec![
            ScriptedClient::tool_call("fill_disk", json!({})),
            ScriptedClient::tool_call("complete_task", json!({"summary": "carried on"})),
        ],
        |_| {},
        |_| {},
        None,
        None,
        Some(over(&disk)),
        |tools| tools.register(Arc::new(FillDisk(disk.clone()))),
    )
    .await;
    let (agent, _) = h
        .runtime
        .create_agent(spec("write through a full disk"), Actor::user())
        .expect("agent");
    h.wait_for(
        "the full disk to refuse the turn and what follows it",
        || disk.refused() >= 2,
    )
    .await;
    disk.free();

    let status = h.wait_until(agent.id, |s| s.is_terminal()).await;
    assert_eq!(
        status.kind(),
        AgentStatusKind::Completed,
        "the agent did not carry on once the disk was freed: {status:?}"
    );
    assert!(
        h.store.lease(agent.id).expect("lease").is_none(),
        "a lease outlived the turn"
    );
    let outage = outage_record(h.store.as_ref(), agent.id);
    assert!(
        outage["error"]
            .as_str()
            .is_some_and(|e| e.contains("database or disk is full")),
        "{outage}"
    );
    let events = h
        .store
        .events(0, 10_000, &EventFilter::for_agent(agent.id))
        .expect("events");
    assert!(
        events.iter().any(|e| matches!(
            &e.kind,
            EventKind::RecoveryClassified {
                class: aether_core::event::RecoveryClass::Retry,
                ..
            }
        )),
        "the read-only call in flight was not retried: {:?}",
        types(h.store.as_ref(), agent.id)
    );
}

/// The turn failed for its own reason, and the full disk then refused the
/// failure too. That reason is what the agent must end with once it frees,
/// not Running forever and not a store error in its place.
#[tokio::test]
async fn a_failure_the_full_disk_refused_is_recorded_once_freed() {
    let disk = Arc::new(Disk::default());
    let h = harness_full(
        vec![ScriptedClient::tool_call("fill_then_panic", json!({}))],
        |_| {},
        |_| {},
        None,
        None,
        Some(over(&disk)),
        |tools| tools.register(Arc::new(FillThenPanic(disk.clone()))),
    )
    .await;
    let (agent, _) = h
        .runtime
        .create_agent(spec("fail on a full disk"), Actor::user())
        .expect("agent");
    h.wait_for("the full disk to refuse the turn's failure", || {
        disk.refused() >= 1
    })
    .await;
    disk.free();

    let status = h.wait_until(agent.id, |s| s.is_terminal()).await;
    assert!(
        matches!(&status, AgentStatus::Failed { error } if error.contains("panicked on a full disk")),
        "the turn's own failure was lost: {status:?}"
    );
    assert!(
        h.store.lease(agent.id).expect("lease").is_none(),
        "a lease outlived the turn"
    );
    outage_record(h.store.as_ref(), agent.id);
}

/// Waiting out a full disk must not hold up a shutdown: the turn is left as
/// a crash leaves it, and the next daemon's recovery takes it from there.
#[tokio::test]
async fn shutdown_during_an_outage_leaves_the_turn_to_restart_recovery() {
    let disk = Arc::new(Disk::default());
    let h = harness_full(
        vec![ScriptedClient::tool_call("fill_disk", json!({}))],
        |_| {},
        |_| {},
        None,
        None,
        Some(over(&disk)),
        |tools| tools.register(Arc::new(FillDisk(disk.clone()))),
    )
    .await;
    let (agent, _) = h
        .runtime
        .create_agent(spec("stop during a full disk"), Actor::user())
        .expect("agent");
    h.wait_for("the full disk to refuse the turn", || disk.refused() >= 2)
        .await;
    tokio::time::timeout(Duration::from_secs(5), h.runtime.shutdown())
        .await
        .expect("shutdown waited on the full disk");
    assert_eq!(
        h.store
            .agent(agent.id)
            .expect("agent")
            .expect("row")
            .status
            .kind(),
        AgentStatusKind::Running,
        "the turn's state is the store's, as a crash leaves it"
    );

    disk.free();
    // The next daemon knows the tool too, so the call in flight reads as the
    // read-only call it was rather than an unknown one.
    let restarted = harness_full(
        vec![ScriptedClient::tool_call(
            "complete_task",
            json!({"summary": "recovered by the next daemon"}),
        )],
        |_| {},
        |_| {},
        None,
        None,
        Some(h.store.clone()),
        |tools| tools.register(Arc::new(FillDisk(disk.clone()))),
    )
    .await;
    let status = restarted.wait_until(agent.id, |s| s.is_terminal()).await;
    assert_eq!(status.kind(), AgentStatusKind::Completed, "{status:?}");
}

/// A turn that writes nothing while the disk is full lives through it. Its
/// heartbeat must keep trying rather than die, and the refused renewals must
/// be on the agent's log once one lands, not only in a log file on that disk.
#[tokio::test]
async fn refused_lease_renewals_are_retried_and_recorded_while_the_turn_lives() {
    let disk = Arc::new(Disk::default());
    let h = harness_full(
        vec![
            ScriptedClient::tool_call("full_for_a_while", json!({})),
            ScriptedClient::tool_call("complete_task", json!({"summary": "lived through it"})),
        ],
        |_| {},
        |c| c.lease_ttl = Duration::from_millis(100),
        None,
        None,
        Some(over(&disk)),
        |tools| tools.register(Arc::new(FullForAWhile(disk.clone()))),
    )
    .await;
    let (agent, _) = h
        .runtime
        .create_agent(spec("outlast a full disk"), Actor::user())
        .expect("agent");

    let status = h.wait_until(agent.id, |s| s.is_terminal()).await;
    assert_eq!(status.kind(), AgentStatusKind::Completed, "{status:?}");
    assert!(disk.refused() >= 2, "the heartbeat never met the full disk");
    let outage = outage_record(h.store.as_ref(), agent.id);
    assert_eq!(outage["during"], "lease renewal", "{outage}");
    assert!(
        outage["refused_writes"].as_u64().is_some_and(|n| n >= 2),
        "{outage}"
    );
}

/// Measured on the real filesystem at start, so even the first tick after a
/// restart onto a full disk starts nothing. A floor no disk can clear stands
/// in for a full one.
#[tokio::test]
async fn a_daemon_starting_below_its_disk_floor_holds_new_turns() {
    let workspace = tempfile::tempdir().expect("tempdir");
    let path = workspace.path().to_path_buf();
    let h = harness_configured(
        vec![ScriptedClient::text("never asked")],
        |_| {},
        |c| {
            c.disk_guard = Some(aether_runtime::DiskGuard {
                path,
                floor: aether_core::disk::DiskFloor::Bytes(u64::MAX),
            })
        },
    )
    .await;
    let low = h
        .runtime
        .status()
        .expect("status")
        .low_disk
        .expect("below the floor from the first check");
    assert_eq!(low.floor_bytes, u64::MAX);
    assert_eq!(low.path, workspace.path().display().to_string());

    let (agent, _) = h
        .runtime
        .create_agent(spec("wait for room"), Actor::user())
        .expect("agent");
    tokio::time::sleep(Duration::from_millis(1500)).await;
    assert!(
        h.client.requests().is_empty(),
        "a turn started on a low disk"
    );
    assert_eq!(
        h.store
            .agent(agent.id)
            .expect("read")
            .expect("agent")
            .status
            .kind(),
        AgentStatusKind::Ready
    );
    let recorded = h
        .store
        .events(0, 1000, &aether_store::EventFilter::default())
        .expect("events")
        .into_iter()
        .filter(|e| matches!(&e.kind, EventKind::Custom { name, .. } if name == "disk.low"))
        .count();
    assert_eq!(recorded, 1, "the low disk is on the event log, once");
}

/// A retention sweep whose commit is refused must leave the artifact live
/// with its bytes, so the next sweep evicts it and records the event once.
#[tokio::test]
async fn a_refused_eviction_commit_keeps_the_payload_for_the_next_sweep() {
    let disk = Arc::new(Disk::default());
    let mut artifact = Artifact {
        id: ArtifactId::new(),
        kind: aether_core::artifact::ArtifactKind::File,
        name: "old output".into(),
        summary: "old output".into(),
        path: None,
        size_bytes: 7,
        content_type: None,
        digest: Some("outage".into()),
        created_by: Actor::user(),
        agent_id: None,
        task_id: None,
        created_at: aether_core::time::now() - chrono::Duration::days(60),
        evicted_at: None,
    };
    let id = artifact.id;
    let h = harness_full(
        vec![],
        |_| {},
        |config| {
            config.retention = RetentionPolicy {
                max_age_days: Some(30),
                max_total_bytes: None,
            };
        },
        None,
        None,
        Some(over(&disk)),
        |_| {},
    )
    .await;
    // Seeded after startup, whose own sweep would otherwise evict it.
    let path = h.store.write_artifact_bytes(id, b"payload").expect("bytes");
    artifact.path = Some(path);
    h.store
        .commit(Commit::new().mutate(Mutation::PutArtifact(artifact)))
        .expect("record");
    let evictions = |h: &Harness| {
        h.store
            .events(0, 10_000, &EventFilter::default())
            .expect("events")
            .iter()
            .filter(|e| matches!(&e.kind, EventKind::ArtifactEvicted { artifact, .. } if *artifact == id))
            .count()
    };

    disk.commits_refused.store(true, Ordering::SeqCst);
    h.runtime.sweep_artifacts().expect_err("commit is refused");
    assert_eq!(evictions(&h), 0);
    assert_eq!(h.store.live_artifacts().expect("live").len(), 1);
    assert!(h.store.read_artifact_bytes(id).expect("read").is_some());

    disk.commits_refused.store(false, Ordering::SeqCst);
    assert_eq!(h.runtime.sweep_artifacts().expect("retry"), 1);
    assert_eq!(evictions(&h), 1);
    assert!(h.store.read_artifact_bytes(id).expect("read").is_none());
}
