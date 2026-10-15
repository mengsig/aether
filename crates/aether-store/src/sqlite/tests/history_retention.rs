use super::*;
use crate::{RunExpiry, WriteGate};
use aether_core::task::TaskStatus;

mod provenance;

struct Fixture {
    store: SqliteStore,
    agent: Agent,
    task: Task,
    cutoff: Timestamp,
    gate: WriteGate,
}

impl Fixture {
    fn new() -> Self {
        Self::on(SqliteStore::open_in_memory().expect("store"))
    }

    fn on(store: SqliteStore) -> Self {
        let old = now() - chrono::Duration::days(14);
        let mut agent = agent();
        agent.status = AgentStatus::Completed { summary: None };
        agent.created_at = old;
        agent.updated_at = old;
        let mut task = Task::new("old task", Actor::agent(agent.id), agent.run, old);
        task.owner = Some(agent.id);
        task.status = TaskStatus::Completed {
            result: TaskResult::summary("finished"),
        };
        store
            .import(
                vec![ImportedEvent {
                    id: EventId::new(),
                    at: old,
                    draft: EventDraft::new(
                        Actor::System,
                        EventKind::Custom {
                            name: "old".into(),
                            payload: serde_json::json!({"text":"expired payload"}),
                        },
                    )
                    .agent(agent.id)
                    .task(task.id)
                    .run(agent.run),
                }],
                vec![
                    Mutation::UpsertAgent(agent.clone()),
                    Mutation::UpsertTask(task.clone()),
                ],
            )
            .expect("seed history");
        Self {
            store,
            agent,
            task,
            cutoff: now() - chrono::Duration::days(7),
            gate: WriteGate::new(),
        }
    }

    /// Expire the run, checking first that the lock-free screen would only
    /// have passed over a run that expiry itself keeps.
    fn expire(&self) -> Result<RunExpiry> {
        let changes = self.store.conn.lock().total_changes();
        let last_seq = self.store.last_seq()?;
        let preview = self
            .store
            .preview_run_history(self.agent.run, self.cutoff)?;
        assert_eq!(
            self.store.conn.lock().total_changes(),
            changes,
            "preview wrote to the store"
        );
        assert_eq!(self.store.last_seq()?, last_seq);
        let screened = self.store.screen_run_expiry(self.agent.run, self.cutoff)?;
        match (&preview, &screened) {
            (None, Some(RunExpiry::Missing)) => {}
            (Some(p), Some(RunExpiry::Protected(reason))) => {
                assert_eq!(
                    p.decision,
                    aether_core::retention::RunRetentionDecision::Protected {
                        reason: (*reason).into()
                    }
                );
                assert_eq!(p.history_payload_bytes, 0);
            }
            (Some(p), None) => {
                assert_eq!(
                    p.decision,
                    aether_core::retention::RunRetentionDecision::Eligible
                );
                assert!(p.history_payload_bytes > 0);
                self.assert_preserved();
            }
            other => panic!("preview and screen disagree: {other:?}"),
        }
        let verdict =
            self.store
                .expire_run_history(self.agent.run, self.cutoff, &self.gate.hold())?;
        if let (Some(p), RunExpiry::Expired { events, rows }) = (&preview, &verdict) {
            assert_eq!(p.events, *events as u64);
            assert_eq!(p.rows, *rows as u64);
        }
        match &screened {
            Some(kept) => assert_eq!(kept, &verdict, "the screen and expiry disagree"),
            None => assert!(
                matches!(verdict, RunExpiry::Expired { .. }),
                "the screen let through a run expiry kept: {verdict:?}"
            ),
        }
        Ok(verdict)
    }

    fn mutate(&self, mutation: Mutation) {
        self.store
            .commit(Commit::new().mutate(mutation))
            .expect("mutation");
    }

    fn assert_preserved(&self) {
        assert!(self.store.agent(self.agent.id).expect("agent").is_some());
        assert!(self.store.task(self.task.id).expect("task").is_some());
        assert_eq!(
            self.store
                .events(0, 10, &EventFilter::for_run(self.agent.run))
                .expect("history")
                .len(),
            1
        );
    }
}

#[test]
fn external_snapshot_preserves_its_task_reference() {
    use aether_core::{
        id::SnapshotId,
        snapshot::{InvocationParams, InvocationSnapshot},
    };
    let f = Fixture::new();
    let observer = agent();
    f.mutate(Mutation::UpsertAgent(observer.clone()));
    let snapshot = InvocationSnapshot {
        id: SnapshotId::new(),
        agent_id: observer.id,
        task_id: Some(f.task.id),
        turn: 1,
        model: observer.model,
        adapter: None,
        capabilities: None,
        memory_ids: vec![],
        layers: vec![],
        messages: vec![],
        tools: vec![],
        params: InvocationParams::default(),
        token_estimate: 0,
        created_at: f.agent.created_at,
    };
    f.mutate(Mutation::PutSnapshot(snapshot));
    assert_eq!(
        f.expire().unwrap(),
        RunExpiry::Protected("external snapshot task reference")
    );
    f.assert_preserved();
}

#[test]
fn durable_memory_preserves_scope_and_provenance_targets() {
    for reference in 0..5 {
        let f = Fixture::new();
        let mut provenance = MemoryProvenance::default();
        let scope = match reference {
            0 => MemoryScope::Agent { id: f.agent.id },
            1 => MemoryScope::Subtree { root: f.agent.id },
            2 => {
                provenance.agent = Some(f.agent.id);
                MemoryScope::Global
            }
            3 => {
                provenance.task = Some(f.task.id);
                MemoryScope::Global
            }
            _ => {
                provenance.event = Some(f.store.event_by_seq(1).unwrap().unwrap().id);
                MemoryScope::Global
            }
        };
        let record = MemoryRecord::from_candidate(
            MemoryCandidate {
                scope,
                kind: MemoryKind::Fact,
                text: "Retain verifiable provenance".into(),
                structured: None,
                importance: 0.5,
                confidence: 1.0,
                tags: vec![],
                provenance,
                supersedes: None,
            },
            f.agent.created_at,
        );
        f.mutate(Mutation::PutMemory(record.clone()));
        assert_eq!(
            f.expire().unwrap(),
            RunExpiry::Protected("memory scope or provenance")
        );
        f.assert_preserved();
        assert_eq!(f.store.memory(record.id).unwrap(), Some(record));
    }
}

#[test]
fn approval_rules_preserve_scoped_runs_and_agents() {
    use aether_core::{
        approval::{ApprovalRule, RuleMatch, RuleScope},
        id::ApprovalRuleId,
        tool::ApprovalDecision,
    };
    for agent_scope in [true, false] {
        let f = Fixture::new();
        let rule = ApprovalRule {
            id: ApprovalRuleId::new(),
            decision: ApprovalDecision::Denied,
            matcher: RuleMatch::default(),
            scope: if agent_scope {
                RuleScope::Agent { id: f.agent.id }
            } else {
                RuleScope::Run { id: f.agent.run }
            },
            created_at: f.agent.created_at,
            created_by: Actor::System,
            note: None,
            revoked: None,
        };
        f.mutate(Mutation::PutApprovalRule(rule.clone()));
        assert_eq!(f.expire().unwrap(), RunExpiry::Protected("approval rule"));
        f.assert_preserved();
        assert_eq!(f.store.approval_rule(rule.id).unwrap(), Some(rule));
    }
}

#[test]
fn finished_old_run_expires_without_recycling_sequences() {
    let f = Fixture::new();
    assert_eq!(
        f.expire().unwrap(),
        RunExpiry::Expired { events: 1, rows: 2 }
    );
    assert!(f.store.agent(f.agent.id).unwrap().is_none());
    assert!(f.store.task(f.task.id).unwrap().is_none());
    assert!(matches!(
        f.store.events(0, 10, &EventFilter::for_run(f.agent.run)),
        Err(StoreError::HistoryExpired { .. })
    ));
    assert_eq!(f.store.last_seq().unwrap(), 1);
    assert_eq!(f.expire().unwrap(), RunExpiry::Missing);
    let next = f
        .store
        .commit(
            Commit::new().event(
                EventDraft::new(
                    Actor::System,
                    EventKind::Custom {
                        name: "new".into(),
                        payload: serde_json::json!({}),
                    },
                )
                .run(RunId::new()),
            ),
        )
        .unwrap();
    assert_eq!(next[0].seq, 2);
}

#[test]
fn unfinished_agents_and_tasks_protect_the_whole_run() {
    for agent_unfinished in [true, false] {
        let f = Fixture::new();
        if agent_unfinished {
            let mut agent = f.agent.clone();
            agent.status = AgentStatus::Ready;
            f.mutate(Mutation::UpsertAgent(agent));
        } else {
            let mut task = f.task.clone();
            task.status = TaskStatus::Queued;
            f.mutate(Mutation::UpsertTask(task));
        }
        assert_eq!(f.expire().unwrap(), RunExpiry::Protected("unfinished work"));
        f.assert_preserved();
    }
}

#[test]
fn cutoff_is_strict_and_recent_events_extend_retention() {
    let f = Fixture::new();
    let mut task = f.task.clone();
    task.updated_at = f.cutoff;
    f.mutate(Mutation::UpsertTask(task));
    assert_eq!(
        f.expire().unwrap(),
        RunExpiry::Protected("recent state change")
    );
    f.assert_preserved();
    let f = Fixture::new();
    f.store
        .commit(
            Commit::new().event(
                EventDraft::new(
                    Actor::System,
                    EventKind::Custom {
                        name: "recent".into(),
                        payload: serde_json::json!({}),
                    },
                )
                .run(f.agent.run),
            ),
        )
        .unwrap();
    assert_eq!(
        f.expire().unwrap(),
        RunExpiry::Protected("recent event or invalid event timestamp")
    );
}

#[test]
fn even_an_expired_lease_blocks_deletion_until_recovery_releases_it() {
    let f = Fixture::new();
    f.mutate(Mutation::SetLease(Lease::new(
        f.agent.id,
        "old",
        1,
        f.agent.created_at,
        std::time::Duration::from_secs(1),
    )));
    assert_eq!(f.expire().unwrap(), RunExpiry::Protected("lease"));
    f.assert_preserved();
}

#[test]
fn external_task_dependencies_and_artifacts_are_protected() {
    let f = Fixture::new();
    let mut dependent = Task::new("dependent", Actor::System, RunId::new(), now());
    dependent.dependencies.push(f.task.id);
    f.mutate(Mutation::UpsertTask(dependent));
    assert_eq!(
        f.expire().unwrap(),
        RunExpiry::Protected("external task reference")
    );
    f.assert_preserved();
    let f = Fixture::new();
    let mut artifact = artifact_named("keep");
    artifact.agent_id = Some(f.agent.id);
    f.mutate(Mutation::PutArtifact(artifact));
    assert_eq!(f.expire().unwrap(), RunExpiry::Protected("artifact"));
    f.assert_preserved();
    let f = Fixture::new();
    let mut artifact = artifact_named("creator only");
    artifact.agent_id = None;
    artifact.task_id = None;
    artifact.created_by = Actor::agent(f.agent.id);
    f.mutate(Mutation::PutArtifact(artifact));
    assert_eq!(f.expire().unwrap(), RunExpiry::Protected("artifact"));
    f.assert_preserved();
}

#[test]
fn deletion_failure_rolls_back_payloads_and_expiry_spans_together() {
    let f = Fixture::new();
    f.store.conn.lock().execute_batch("CREATE TRIGGER refuse_expiry BEFORE DELETE ON agents BEGIN SELECT RAISE(ABORT,'injected failure'); END;").unwrap();
    assert!(f.expire().is_err());
    f.assert_preserved();
    let spans: i64 = f
        .store
        .conn
        .lock()
        .query_row("SELECT COUNT(*) FROM expired_event_spans", [], |row| {
            row.get(0)
        })
        .unwrap();
    assert_eq!(spans, 0);
    assert_eq!(f.store.last_seq().unwrap(), 1);
}

#[test]
fn pending_cancellation_protects_its_target_and_invalid_journal_fails_closed() {
    let f = Fixture::new();
    f.mutate(Mutation::SetMeta {
        key: "runtime.pending_cancellations".into(),
        value: serde_json::json!({f.agent.id.to_string(): {"reason":"pending"}}).to_string(),
    });
    assert_eq!(
        f.expire().unwrap(),
        RunExpiry::Protected("pending cancellation")
    );
    f.assert_preserved();
    f.mutate(Mutation::SetMeta {
        key: "runtime.pending_cancellations".into(),
        value: "invalid json".into(),
    });
    assert!(f.store.preview_run_history(f.agent.run, f.cutoff).is_err());
    assert!(
        f.store
            .expire_run_history(f.agent.run, f.cutoff, &f.gate.hold())
            .is_err()
    );
    f.assert_preserved();
}

#[test]
fn external_agent_waits_are_references_even_without_task_dependencies() {
    let f = Fixture::new();
    let mut waiter = agent();
    waiter.status = AgentStatus::Waiting {
        condition: aether_core::agent::WaitCondition::Task { id: f.task.id },
    };
    f.mutate(Mutation::UpsertAgent(waiter));
    assert_eq!(
        f.expire().unwrap(),
        RunExpiry::Protected("external agent reference")
    );
    f.assert_preserved();
}

#[test]
fn pending_approval_preserves_its_terminal_agent() {
    use aether_core::{
        id::ApprovalId,
        tool::{ApprovalRequest, ApprovalStatus, SideEffectClass},
    };
    let f = Fixture::new();
    f.mutate(Mutation::PutApproval(ApprovalRequest {
        id: ApprovalId::new(),
        agent_id: f.agent.id,
        task_id: Some(f.task.id),
        call_id: ToolCallId::new(),
        tool: "shell".into(),
        description: "pending".into(),
        argument: None,
        resources: vec![],
        side_effect: SideEffectClass::External,
        requested_at: f.agent.created_at,
        status: ApprovalStatus::Pending,
    }));
    assert_eq!(
        f.expire().unwrap(),
        RunExpiry::Protected("pending approval")
    );
    f.assert_preserved();
}

#[test]
fn a_submission_replay_record_preserves_the_created_work() {
    use aether_core::idempotency::{RecordedSubmission, SubmissionOutcome};
    let f = Fixture::new();
    f.mutate(Mutation::RecordSubmission(RecordedSubmission {
        key: IdempotencyKey("retention-key".into()),
        fingerprint: "test".into(),
        scheme: 1,
        at: now(),
        outcome: SubmissionOutcome::Agent {
            agent: f.agent.id,
            task: Some(f.task.id),
        },
    }));
    assert_eq!(
        f.expire().unwrap(),
        RunExpiry::Protected("submission replay record")
    );
    f.assert_preserved();
}

#[test]
fn unscoped_events_are_not_orphaned_by_run_deletion() {
    let f = Fixture::new();
    f.store
        .import(
            vec![ImportedEvent {
                id: EventId::new(),
                at: f.agent.created_at,
                draft: EventDraft::new(Actor::System, EventKind::AgentReady).agent(f.agent.id),
            }],
            vec![],
        )
        .unwrap();
    assert_eq!(
        f.expire().unwrap(),
        RunExpiry::Protected("external or unscoped event reference")
    );
    f.assert_preserved();
}

#[test]
fn another_runs_inbox_preserves_referenced_task_and_sender() {
    use aether_core::{
        id::InboxItemId,
        inbox::{InboxItem, InboxKind},
    };

    for reference_task in [true, false] {
        let f = Fixture::new();
        let recipient = agent();
        f.mutate(Mutation::UpsertAgent(recipient.clone()));
        let kind = if reference_task {
            InboxKind::TaskCancelled {
                task: f.task.id,
                reason: None,
            }
        } else {
            InboxKind::ChildResumed { agent: f.agent.id }
        };
        f.mutate(Mutation::DeliverInbox(InboxItem {
            id: InboxItemId::new(),
            agent_id: recipient.id,
            seq: 1,
            kind,
            delivered_at: f.agent.created_at,
        }));
        assert_eq!(
            f.expire().unwrap(),
            RunExpiry::Protected("external inbox reference")
        );
        f.assert_preserved();
        assert_eq!(f.store.inbox(recipient.id, 0).unwrap().len(), 1);
    }
}

#[test]
fn unrelated_external_inbox_does_not_prevent_expiry() {
    use aether_core::{
        id::InboxItemId,
        inbox::{InboxItem, InboxKind},
    };

    let f = Fixture::new();
    let recipient = agent();
    f.mutate(Mutation::UpsertAgent(recipient.clone()));
    f.mutate(Mutation::DeliverInbox(InboxItem {
        id: InboxItemId::new(),
        agent_id: recipient.id,
        seq: 1,
        kind: InboxKind::Wake {
            reason: "unrelated".into(),
        },
        delivered_at: now(),
    }));
    assert!(matches!(f.expire().unwrap(), RunExpiry::Expired { .. }));
    assert_eq!(f.store.inbox(recipient.id, 0).unwrap().len(), 1);
}

#[test]
fn paused_delivery_schedules_protect_their_target_even_without_next_due() {
    use aether_core::{
        id::ScheduleId,
        schedule::{
            BusyPolicy, MissedRunPolicy, Recurrence, Schedule, ScheduleMode, ScheduleStatus,
        },
    };
    let f = Fixture::new();
    f.mutate(Mutation::UpsertSchedule(Schedule {
        id: ScheduleId::new(),
        name: "paused".into(),
        owner: Actor::System,
        mode: ScheduleMode::Deliver {
            target: f.agent.id,
            run: f.agent.run,
            last_inbox_seq: None,
        },
        recurrence: Recurrence::Interval {
            seconds: std::num::NonZeroU64::new(60).unwrap(),
        },
        missed: MissedRunPolicy::Skip,
        busy: BusyPolicy::Skip,
        payload: serde_json::Value::Null,
        status: ScheduleStatus::Paused {
            next_due: f.agent.created_at,
        },
        max_deliveries: None,
        deliveries: 0,
        last: None,
        created_at: f.agent.created_at,
        updated_at: f.agent.updated_at,
    }));
    assert_eq!(f.expire().unwrap(), RunExpiry::Protected("schedule"));
    f.assert_preserved();
}

/// A sweep screens every old run each hour. On the store's own connections
/// that queued every commit and read behind table scans.
#[test]
fn screening_a_file_store_takes_neither_of_its_connections() {
    let dir = tempfile::tempdir().expect("tmp");
    let f = Fixture::on(SqliteStore::open(dir.path().join("aether.db")).expect("open"));
    let mut agent = f.agent.clone();
    agent.status = AgentStatus::Ready;
    f.mutate(Mutation::UpsertAgent(agent));
    let writer = f.store.conn.lock();
    let reader = f
        .store
        .reader
        .as_ref()
        .expect("a file store reads apart")
        .lock();
    std::thread::scope(|scope| {
        let (tx, rx) = std::sync::mpsc::channel();
        let store = &f.store;
        let (run, cutoff) = (f.agent.run, f.cutoff);
        scope.spawn(move || tx.send(store.screen_run_expiry(run, cutoff)));
        let screened = rx.recv_timeout(std::time::Duration::from_secs(10));
        drop((writer, reader));
        assert_eq!(
            screened
                .expect("screened while both connections were held")
                .unwrap(),
            Some(RunExpiry::Protected("unfinished work"))
        );
    });
}

#[test]
fn schedule_occurrence_preserves_spawned_run_history() {
    use aether_core::{
        id::ScheduleId,
        schedule::{ScheduleOccurrence, ScheduleOutcome},
    };
    let f = Fixture::new();
    let observer = agent();
    let schedule = ScheduleId::new();
    use aether_core::schedule::{
        BusyPolicy, MissedRunPolicy, Recurrence, Schedule, ScheduleMode, ScheduleStatus,
    };
    f.mutate(Mutation::UpsertAgent(observer.clone()));
    f.mutate(Mutation::UpsertSchedule(Schedule {
        id: schedule,
        name: "unrelated delivery".into(),
        owner: Actor::System,
        mode: ScheduleMode::Deliver {
            target: observer.id,
            run: observer.run,
            last_inbox_seq: None,
        },
        recurrence: Recurrence::Interval {
            seconds: std::num::NonZeroU64::new(60).unwrap(),
        },
        missed: MissedRunPolicy::Skip,
        busy: BusyPolicy::Skip,
        payload: serde_json::Value::Null,
        status: ScheduleStatus::Paused {
            next_due: f.agent.created_at,
        },
        max_deliveries: None,
        deliveries: 0,
        last: None,
        created_at: f.agent.created_at,
        updated_at: f.agent.updated_at,
    }));
    f.mutate(Mutation::RecordScheduleOccurrence(ScheduleOccurrence {
        schedule,
        due: f.agent.created_at,
        recorded_at: f.agent.created_at,
        outcome: ScheduleOutcome::Spawned {
            agent: f.agent.id,
            task: f.task.id,
            run: f.agent.run,
        },
    }));
    assert_eq!(
        f.expire().unwrap(),
        RunExpiry::Protected("schedule occurrence")
    );
    f.assert_preserved();
}

#[test]
fn external_resolved_approval_preserves_its_task_reference() {
    use aether_core::{
        id::ApprovalId,
        tool::{ApprovalRequest, ApprovalStatus, SideEffectClass},
    };
    let f = Fixture::new();
    let observer = agent();
    f.mutate(Mutation::UpsertAgent(observer.clone()));
    f.mutate(Mutation::PutApproval(ApprovalRequest {
        id: ApprovalId::new(),
        agent_id: observer.id,
        task_id: Some(f.task.id),
        call_id: ToolCallId::new(),
        tool: "shell".into(),
        description: "resolved".into(),
        argument: None,
        resources: vec![],
        side_effect: SideEffectClass::External,
        requested_at: f.agent.created_at,
        status: ApprovalStatus::Resolved {
            decision: aether_core::tool::ApprovalDecision::Denied,
            by: Actor::System,
            at: f.agent.created_at,
            note: None,
        },
    }));
    assert_eq!(
        f.expire().unwrap(),
        RunExpiry::Protected("external approval reference")
    );
    f.assert_preserved();
}

#[test]
fn retention_snapshot_expiry_preserves_source_in_memory_and_committed_wal() {
    let dir = tempfile::tempdir().expect("directory");
    for store in [
        SqliteStore::open_in_memory().expect("memory store"),
        SqliteStore::open(dir.path().join("store.sqlite")).expect("file store"),
    ] {
        let f = Fixture::on(store);
        let changes = f.store.conn.lock().total_changes();
        let seq = f.store.last_seq().expect("cursor");
        let commits = f.store.commits();
        let spans: i64 = f
            .store
            .conn
            .lock()
            .query_row("SELECT COUNT(*) FROM expired_event_spans", [], |row| {
                row.get(0)
            })
            .expect("spans");
        let snapshot = f.store.retention_snapshot().expect("snapshot");
        assert_eq!(
            snapshot.agent(f.agent.id).expect("agent").as_ref(),
            Some(&f.agent)
        );
        assert_eq!(
            snapshot.task(f.task.id).expect("task").as_ref(),
            Some(&f.task)
        );
        assert_eq!(snapshot.last_seq().expect("cursor"), seq);
        let preview = snapshot
            .preview_run_history(f.agent.run, f.cutoff)
            .expect("preview")
            .expect("run");
        assert_eq!(
            snapshot
                .expire_run_history(f.agent.run, f.cutoff, &f.gate.hold())
                .expect("expire"),
            RunExpiry::Expired {
                events: preview.events as usize,
                rows: preview.rows as usize
            }
        );
        assert!(snapshot.agent(f.agent.id).expect("expired agent").is_none());
        f.assert_preserved();
        assert_eq!(f.store.conn.lock().total_changes(), changes);
        assert_eq!(f.store.last_seq().expect("cursor"), seq);
        assert_eq!(f.store.commits(), commits);
        assert_eq!(
            f.store
                .conn
                .lock()
                .query_row("SELECT COUNT(*) FROM expired_event_spans", [], |row| row
                    .get::<_, i64>(0))
                .expect("spans"),
            spans
        );
    }
}

#[test]
fn retention_snapshot_rejects_a_database_over_the_memory_limit() {
    let dir = tempfile::tempdir().unwrap();
    let store = SqliteStore::open(dir.path().join("history.db")).unwrap();
    {
        let conn = store.conn.lock();
        conn.execute("CREATE TABLE snapshot_size_fixture (body BLOB)", [])
            .unwrap();
        conn.execute(
            "INSERT INTO snapshot_size_fixture VALUES (zeroblob(?1))",
            [256 * 1024 * 1024],
        )
        .unwrap();
    }
    let error = match store.retention_snapshot() {
        Ok(_) => panic!("oversized snapshot accepted"),
        Err(error) => error,
    };
    assert!(
        matches!(error, StoreError::RetentionSnapshotTooLarge { bytes, limit } if bytes > limit && limit == 256 * 1024 * 1024),
        "{error}"
    );
    assert_eq!(
        store
            .conn
            .lock()
            .query_row(
                "SELECT length(body) FROM snapshot_size_fixture",
                [],
                |row| row.get::<_, i64>(0)
            )
            .unwrap(),
        256 * 1024 * 1024
    );
}

#[test]
fn feedback_rows_survive_expiring_their_task_history() {
    use aether_core::memory_feedback::{
        FeedbackTally, MemoryEvidence, MemoryFeedback, MemoryFeedbackSpec, MemoryVerdict,
    };
    let f = Fixture::new();
    let record = MemoryFeedback {
        id: aether_core::id::MemoryFeedbackId::new(),
        spec: MemoryFeedbackSpec {
            memory: MemoryId::new(),
            verdict: MemoryVerdict::Hurt,
            evidence: MemoryEvidence::Review,
            task: f.task.id,
            invocation: Some(InvocationId::new()),
            reason: "Review failed".into(),
        },
        actor: Actor::user(),
        created_at: now(),
        retracted_at: None,
        retraction_reason: None,
    };
    f.mutate(Mutation::RecordMemoryFeedback(record.clone()));
    assert!(matches!(f.expire().unwrap(), RunExpiry::Expired { .. }));
    assert!(f.store.task(f.task.id).unwrap().is_none());
    let snapshot = f.store.retention_snapshot().unwrap();
    assert_eq!(
        snapshot.memory_feedback_record(record.id).unwrap(),
        Some(record.clone())
    );
    assert_eq!(
        snapshot
            .memory_feedback_tallies(&[record.spec.memory])
            .unwrap()
            .get(&record.spec.memory),
        Some(&FeedbackTally { helped: 0, hurt: 1 })
    );
    let task_id: String = f
        .store
        .read_conn()
        .query_row(
            "SELECT task_id FROM memory_feedback WHERE id = ?1",
            [record.id.to_string()],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(task_id, record.spec.task.to_string());
    assert_eq!(
        f.store.memory_feedback_record(record.id).unwrap(),
        Some(record.clone())
    );
    assert_eq!(
        f.store.memory_feedback(record.spec.memory, 100).unwrap(),
        vec![record]
    );
}
