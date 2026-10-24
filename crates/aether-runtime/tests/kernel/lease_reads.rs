//! lease reads scenarios and fixtures.
use super::*;

/// MAJOR 3 of the merge gate: `unreleased_keys` walks an agent's whole
/// history on every daemon start, and an inspection is cleared only by a
/// person, so parked agents accumulate. It reads two event kinds, so the
/// store filter says so - and no agent is walked twice in one pass.
fn lease_event_types() -> Vec<String> {
    vec![
        "lease.resource_acquired".to_owned(),
        "lease.resource_released".to_owned(),
    ]
}

#[derive(Default)]
struct StoreReads {
    /// Event rows deserialized by every `events()` call.
    rows: usize,
    agent_filters: Vec<AgentFilter>,
    agent_reads: usize,
    /// Calls narrowed to the two lease kinds: one quarantine walk each.
    lease_walks: usize,
}

struct CountingStore {
    inner: Arc<dyn Store>,
    reads: std::sync::Mutex<StoreReads>,
}

impl CountingStore {
    fn new(inner: Arc<dyn Store>) -> Arc<Self> {
        Arc::new(Self {
            inner,
            reads: Default::default(),
        })
    }

    fn take(&self) -> StoreReads {
        std::mem::take(&mut self.reads.lock().expect("reads"))
    }
}

impl Store for CountingStore {
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
        self.inner.commit(commit)
    }
    fn import(
        &self,
        events: Vec<aether_store::ImportedEvent>,
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
        let page = self.inner.events(after, limit, filter)?;
        let mut reads = self.reads.lock().expect("reads");
        reads.rows += page.len();
        if filter.types == lease_event_types() {
            reads.lease_walks += 1;
        }
        Ok(page)
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
        self.reads.lock().expect("reads").agent_reads += 1;
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
        self.reads
            .lock()
            .expect("reads")
            .agent_filters
            .push(filter.clone());
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

#[tokio::test]
async fn the_quarantine_walk_reads_only_lease_events_and_only_once_per_agent() {
    use aether_core::tool::ResourceKey;

    fn seed_agent(id: AgentId, at: aether_core::time::Timestamp) -> aether_core::agent::Agent {
        aether_core::agent::Agent {
            archived: false,
            id,
            parent: None,
            role: "worker".into(),
            name: "w".into(),
            scope: Scope::default(),
            model: ModelRef::new("anthropic", "claude-sonnet-5-5"),
            fallbacks: Vec::new(),
            status: AgentStatus::Created,
            current_task: None,
            inbox_cursor: 0,
            checkpoint: None,
            engine_session: None,
            engine_usage: None,
            engine_sent: None,
            status_since: None,
            paused: false,
            paused_via: None,
            resume_wait: None,
            restarted_from: None,
            restarted_as: None,
            engine_alive: false,
            last_event_at: None,
            config: AgentConfig::default(),
            stats: Default::default(),
            run: Default::default(),
            created_at: at,
            updated_at: at,
        }
    }

    let parked = AgentId::new();
    let child = AgentId::new();
    let interrupted = AgentId::new();
    let store = CountingStore::new(Arc::new(SqliteStore::open_in_memory().expect("store")));

    let at = now();
    let blocked = |id: AgentId, parent: Option<AgentId>| {
        let mut a = seed_agent(id, at);
        a.parent = parent;
        a.status = AgentStatus::Blocked {
            reason: BlockReason::Inspection {
                detail: "a person has to look".into(),
            },
        };
        a
    };
    let key = |k: &str| ResourceKey(k.to_owned());
    store
        .commit(
            Commit::new()
                .event(
                    EventDraft::new(
                        Actor::System,
                        EventKind::ResourceAcquired {
                            keys: vec![key("parked-key")],
                        },
                    )
                    .agent(parked),
                )
                // The child's own unreleased key is not the parent's to hold.
                .event(
                    EventDraft::new(
                        Actor::System,
                        EventKind::ResourceAcquired {
                            keys: vec![key("child-key")],
                        },
                    )
                    .agent(child)
                    .parent(Some(parked)),
                )
                .mutate(Mutation::UpsertAgent(blocked(parked, None)))
                .mutate(Mutation::UpsertAgent(blocked(child, Some(parked)))),
        )
        .expect("seed the parked agents");
    // A long-lived store: the walk must not read any of this.
    for _ in 0..8 {
        let mut noise = Commit::new();
        for i in 0..500 {
            noise.push_event(
                EventDraft::new(
                    Actor::System,
                    EventKind::AgentMessage {
                        role: aether_core::model::MessageRole::Assistant,
                        content: vec![ContentBlock::text(format!("chatter {i}"))],
                        invocation: None,
                    },
                )
                .agent(parked),
            );
        }
        store.commit(noise).expect("history");
    }
    // And one agent the main recovery pass parks itself, which the second
    // pass then sees in `Blocked { Inspection }`.
    let mut running = seed_agent(interrupted, at);
    running.transition(AgentStatus::Ready, at).expect("ready");
    running
        .transition(AgentStatus::Running, at)
        .expect("running");
    let call = ToolCall {
        id: Default::default(),
        provider_call_id: "p".into(),
        name: "bash".into(),
        input: json!({}),
        agent_id: interrupted,
        task_id: None,
        invocation_id: None,
        resources: vec![key("interrupted-key")],
        idempotency_key: None,
        requested_at: at,
    };
    store
        .commit(
            Commit::new()
                .event(
                    EventDraft::new(Actor::System, EventKind::AgentStarted { turn: 1 })
                        .agent(interrupted),
                )
                .event(
                    EventDraft::new(
                        Actor::System,
                        EventKind::ResourceAcquired {
                            keys: vec![key("interrupted-key")],
                        },
                    )
                    .agent(interrupted),
                )
                .event(
                    EventDraft::new(
                        Actor::System,
                        EventKind::ToolRequested {
                            call: call.clone(),
                            side_effect: SideEffectClass::Mutation,
                        },
                    )
                    .agent(interrupted),
                )
                .event(
                    EventDraft::new(Actor::System, EventKind::ToolStarted { call: call.id })
                        .agent(interrupted),
                )
                .mutate(Mutation::UpsertAgent(running))
                .mutate(Mutation::SetLease(Lease::new(
                    interrupted,
                    "dead-daemon",
                    1,
                    at,
                    Duration::from_secs(1),
                ))),
        )
        .expect("seed the interrupted turn");
    let seeded = store.take().rows;
    assert_eq!(seeded, 0, "seeding reads no events");

    let h = harness_full(
        vec![],
        |_| {},
        |_| {},
        None,
        None,
        Some(store.clone()),
        |_| {},
    )
    .await;
    let reads = store.take();
    assert!(
        reads.rows < 500,
        "recovery read {} event rows over a 4000-event history",
        reads.rows
    );
    assert_eq!(
        reads.lease_walks, 3,
        "one type-filtered walk for each parked agent, and no agent twice"
    );

    // And it still finds the right keys: each agent's own, never its child's.
    let held: Vec<_> = h
        .runtime
        .resource_holdings()
        .into_iter()
        .map(|h| (h.agent, h.keys))
        .collect();
    assert!(
        held.contains(&(parked, vec![key("parked-key")])),
        "{held:?}"
    );
    assert!(held.contains(&(child, vec![key("child-key")])), "{held:?}");
    assert!(
        held.contains(&(interrupted, vec![key("interrupted-key")])),
        "{held:?}"
    );
}

#[tokio::test]
async fn an_idle_tick_reads_only_budgeted_roots_and_one_live_snapshot() {
    let store = CountingStore::new(Arc::new(SqliteStore::open_in_memory().expect("store")));
    let h = harness_full(
        vec![],
        |_| {},
        |_| {},
        None,
        None,
        Some(store.clone()),
        |_| {},
    )
    .await;
    h.runtime.shutdown().await;
    let (agent, _) = h
        .runtime
        .create_agent(idle_spec(), Actor::user())
        .expect("agent");
    let mut agent = agent;
    agent.status = AgentStatus::Waiting {
        condition: aether_core::agent::WaitCondition::Inbox,
    };
    store
        .commit(Commit::new().mutate(Mutation::UpsertAgent(agent)))
        .expect("waiting");
    store.take();

    aether_runtime::scheduler::tick(&h.runtime).expect("tick");
    let reads = store.take();
    assert!(!reads.agent_filters.contains(&AgentFilter::default()));
    assert_eq!(
        reads
            .agent_filters
            .iter()
            .filter(|f| f.live_only && !f.budgeted_only)
            .count(),
        1
    );
    assert_eq!(
        reads
            .agent_filters
            .iter()
            .filter(|f| f.budgeted_only)
            .count(),
        1
    );
    assert_eq!(
        reads.agent_reads, 0,
        "no deadlines means no individual re-reads"
    );
}

#[tokio::test]
async fn status_never_reads_the_whole_fleet() {
    let store = CountingStore::new(Arc::new(SqliteStore::open_in_memory().expect("store")));
    let h = harness_full(
        vec![],
        |_| {},
        |_| {},
        None,
        None,
        Some(store.clone()),
        |_| {},
    )
    .await;
    h.runtime.shutdown().await;
    let (agent, _) = h
        .runtime
        .create_agent(idle_spec(), Actor::user())
        .expect("agent");
    store.take();

    let status = h.runtime.status().expect("status");
    let reads = store.take();
    assert_eq!(status.agents_live, 1);
    assert!(
        reads
            .agent_filters
            .iter()
            .all(|f| f.live_only || f.run == Some(agent.run)),
        "status read the whole fleet: {:?}",
        reads.agent_filters
    );
}
