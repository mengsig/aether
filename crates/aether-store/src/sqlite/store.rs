use std::path::{Path, PathBuf};
use std::sync::atomic::Ordering;

use aether_core::agent::{Agent, AgentStatusKind};
use aether_core::approval::ApprovalRule;
use aether_core::artifact::Artifact;
use aether_core::checkpoint::Checkpoint;
use aether_core::event::{Event, EventKind};
use aether_core::id::ScheduleId;
use aether_core::id::{
    AgentId, ApprovalId, ApprovalRuleId, ArtifactId, CheckpointId, DeviceId, EventId, InvocationId,
    MemoryId, SnapshotId, TaskId, ToolCallId,
};
use aether_core::idempotency::{
    IdempotencyKey, RecordedOutcome, RecordedSubmission, SubmissionLedger,
};
use aether_core::inbox::InboxItem;
use aether_core::lease::Lease;
use aether_core::memory::MemoryRecord;
use aether_core::model::Message;
use aether_core::repository::RepositoryAlias;
use aether_core::schedule::{Schedule, ScheduleOccurrence};
use aether_core::snapshot::{Invocation, InvocationSnapshot};
use aether_core::task::Task;
use aether_core::time::{Timestamp, now};
use aether_core::tool::ApprovalRequest;
use rusqlite::{OptionalExtension, params};

use crate::{
    AgentFilter, ArtifactFilter, Commit, DeviceRow, EventFilter, ImportedEvent, MemoryQuery,
    Mutation, RepositoryAliasMove, Result, Store, StoreError, TaskFilter, TranscriptEntry,
};

use super::{SqliteStore, schedule_time};

impl Store for SqliteStore {
    fn retention_snapshot(&self) -> Result<Box<dyn Store>> {
        SqliteStore::retention_snapshot(self)
    }

    fn preview_run_history(
        &self,
        run: aether_core::id::RunId,
        before: Timestamp,
    ) -> Result<Option<aether_core::retention::RunRetentionPreview>> {
        SqliteStore::preview_run_history(self, run, before)
    }

    fn pending_artifact_cleanup(&self) -> Result<aether_core::retention::PendingArtifactCleanup> {
        SqliteStore::pending_artifact_cleanup(self)
    }

    fn expire_run_history(
        &self,
        run: aether_core::id::RunId,
        before: Timestamp,
        hold: &crate::Hold<'_>,
    ) -> Result<crate::RunExpiry> {
        SqliteStore::expire_run_history(self, run, before, hold)
    }

    fn screen_run_expiry(
        &self,
        run: aether_core::id::RunId,
        before: Timestamp,
    ) -> Result<Option<crate::RunExpiry>> {
        SqliteStore::screen_run_expiry(self, run, before)
    }

    fn commit(&self, commit: Commit) -> Result<Vec<Event>> {
        let mut conn = self.conn.lock();
        let tx = conn.transaction()?;
        let mut seq = Self::last_seq_in(&tx)?;
        let at = now();
        let mut committed = Self::write_events(
            &tx,
            &mut seq,
            commit.events.into_iter().map(|d| (EventId::new(), d, at)),
        )?;
        let records_submission = commit
            .mutations
            .iter()
            .any(|m| matches!(m, Mutation::RecordSubmission(_)));
        for mutation in commit.mutations {
            Self::apply(&tx, mutation)?;
        }
        // Swept here rather than inside the mutation so the eviction can be
        // written to the ledger in the same transaction as the record that
        // caused it.
        if records_submission
            && let Some(draft) = self.sweep_submissions(&tx)?.report(self.max_submissions)
        {
            committed.extend(Self::write_events(
                &tx,
                &mut seq,
                std::iter::once((EventId::new(), draft, at)),
            )?);
        }
        tx.execute(
            "UPDATE meta SET value = ?1 WHERE key = 'last_seq'",
            [seq.to_string()],
        )?;
        tx.commit()?;
        self.commits.fetch_add(1, Ordering::Relaxed);
        Ok(committed)
    }

    fn commits(&self) -> u64 {
        self.commits.load(Ordering::Relaxed)
    }

    fn import(&self, events: Vec<ImportedEvent>, mutations: Vec<Mutation>) -> Result<Vec<Event>> {
        let mut conn = self.conn.lock();
        let tx = conn.transaction()?;
        let mut seq = Self::last_seq_in(&tx)?;
        let committed = Self::write_events(
            &tx,
            &mut seq,
            events.into_iter().map(|e| (e.id, e.draft, e.at)),
        )?;
        for mutation in mutations {
            Self::apply(&tx, mutation)?;
        }
        tx.execute(
            "UPDATE meta SET value = ?1 WHERE key = 'last_seq'",
            [seq.to_string()],
        )?;
        tx.commit()?;
        self.commits.fetch_add(1, Ordering::Relaxed);
        Ok(committed)
    }

    fn last_seq(&self) -> Result<u64> {
        let conn = self.read_conn();
        let v: String =
            conn.query_row("SELECT value FROM meta WHERE key = 'last_seq'", [], |r| {
                r.get(0)
            })?;
        v.parse()
            .map_err(|_| StoreError::Corrupt("meta.last_seq is not a number".into()))
    }

    fn events(&self, after: u64, limit: usize, filter: &EventFilter) -> Result<Vec<Event>> {
        self.query_events(Some(after), None, limit, filter, true)
    }

    fn latest_events(&self, limit: usize, filter: &EventFilter) -> Result<Vec<Event>> {
        self.query_events(None, None, limit, filter, false)
    }

    fn events_before(&self, before: u64, limit: usize, filter: &EventFilter) -> Result<Vec<Event>> {
        self.query_events(None, Some(before), limit, filter, false)
    }

    fn tool_events(&self, agent: AgentId, scope: crate::ToolEventScope<'_>) -> Result<Vec<Event>> {
        let (predicate, parameter) = match scope {
            crate::ToolEventScope::Invocation(invocation) => (
                "json_extract(body, '$.kind.call.invocation_id') = ?2",
                invocation.to_string(),
            ),
            crate::ToolEventScope::Calls(calls) => {
                if calls.is_empty() {
                    return Ok(Vec::new());
                }
                (
                    "json_extract(body, '$.kind.call.id') IN (SELECT value FROM json_each(?2))",
                    serde_json::to_string(calls)?,
                )
            }
        };
        let conn = self.read_conn();
        // API invocations commit their response before dispatching tools.
        let lower = if matches!(scope, crate::ToolEventScope::Invocation(_)) {
            let sql = "SELECT seq FROM events WHERE agent_id = ?1 \
                       AND type = 'invocation.completed' \
                       AND json_extract(body, '$.kind.invocation') = ?2 \
                       ORDER BY seq DESC LIMIT 1";
            conn.query_row(sql, params![agent.to_string(), parameter], |row| {
                row.get::<_, i64>(0)
            })
            .optional()?
            .unwrap_or(0)
        } else {
            0
        };
        let first_sql = format!(
            "SELECT MIN(seq) FROM events WHERE agent_id = ?1 AND seq >= ?3 \
             AND type = 'tool.requested' AND {predicate}"
        );
        let first: Option<i64> = conn.query_row(
            &first_sql,
            params![agent.to_string(), parameter, lower],
            |row| row.get(0),
        )?;
        let Some(first) = first else {
            return Ok(Vec::new());
        };
        let ids = format!(
            "SELECT json_extract(body, '$.kind.call.id') FROM events \
             WHERE agent_id = ?1 AND seq >= ?3 AND type = 'tool.requested' AND {predicate}"
        );
        let sql = format!(
            "SELECT body FROM events WHERE agent_id = ?1 AND seq >= ?3 AND \
            ((type = 'tool.requested' AND json_extract(body, '$.kind.call.id') IN ({ids})) OR \
             (type IN ('tool.succeeded', 'tool.failed', 'tool.cancelled', 'tool.denied') \
              AND json_extract(body, '$.kind.call') IN ({ids}))) ORDER BY seq DESC"
        );
        Self::read_many(&conn, &sql, &[&agent.to_string(), &parameter, &first])
    }

    fn event_by_id(&self, id: EventId) -> Result<Option<Event>> {
        self.read_one("SELECT body FROM events WHERE id = ?1", &id.to_string())
    }

    fn latest_exhaustion_incident(
        &self,
        agent: AgentId,
        model: &aether_core::model::ModelRef,
    ) -> Result<Option<Event>> {
        let conn = self.read_conn();
        let sql = "SELECT body FROM events WHERE agent_id = ?1 AND type = 'custom' \
                   AND json_extract(body, '$.kind.name') \
                       IN ('exhaustion.reported', 'exhaustion.provider_recovered') \
                   AND json_extract(body, '$.kind.payload.provider') = ?2 \
                   AND json_extract(body, '$.kind.payload.model') = ?3 \
                   ORDER BY seq DESC LIMIT 1";
        let body: Option<String> = conn
            .query_row(
                sql,
                params![agent.to_string(), model.provider, model.model],
                |row| row.get(0),
            )
            .optional()?;
        body.map(|body| serde_json::from_str(&body).map_err(StoreError::from))
            .transpose()
    }

    fn event_by_seq(&self, seq: u64) -> Result<Option<Event>> {
        if seq == 0 || seq > i64::MAX as u64 {
            return Ok(None);
        }
        let conn = self.read_conn();
        let body: Option<String> = conn
            .query_row(
                "SELECT body FROM events WHERE seq = ?1",
                [seq as i64],
                |r| r.get(0),
            )
            .optional()?;
        if body.is_none() {
            let expired: Option<(i64, i64)> = conn.query_row(
                "SELECT first_seq,last_seq FROM expired_event_spans WHERE first_seq <= ?1 AND last_seq >= ?1 LIMIT 1",
                [seq as i64], |row| Ok((row.get(0)?, row.get(1)?))
            ).optional()?;
            if let Some((first_seq, last_seq)) = expired {
                return Err(StoreError::HistoryExpired {
                    first_seq: first_seq as u64,
                    last_seq: last_seq as u64,
                });
            }
        }
        body.map(|b| serde_json::from_str(&b).map_err(StoreError::from))
            .transpose()
    }

    fn agent(&self, id: AgentId) -> Result<Option<Agent>> {
        self.read_one("SELECT body FROM agents WHERE id = ?1", &id.to_string())
    }

    fn any_agent_in_repository(&self, repository: &str) -> Result<bool> {
        // A miss scans every agent, so it must not hold the writer while it does.
        Ok(self.read_conn().query_row(
            "SELECT EXISTS(SELECT 1 FROM agents WHERE json_extract(body, '$.scope.repository') = ?1)",
            [repository],
            |row| row.get(0),
        )?)
    }

    fn resolve_repository(&self, repository: &str) -> Result<String> {
        self.resolve_repository_id(repository)
    }

    fn repository_aliases(&self) -> Result<Vec<RepositoryAlias>> {
        self.list_repository_aliases()
    }

    fn preview_repository_alias(&self, alias: &RepositoryAlias) -> Result<RepositoryAliasMove> {
        self.preview_alias(alias)
    }

    fn alias_repository(
        &self,
        alias: &RepositoryAlias,
        by: aether_core::event::Actor,
        _hold: &crate::Hold<'_>,
    ) -> Result<RepositoryAliasMove> {
        self.register_alias(alias, by)
    }

    fn agents(&self, filter: &AgentFilter) -> Result<Vec<Agent>> {
        let mut sql = String::from("SELECT body FROM agents WHERE 1 = 1");
        let mut args: Vec<Box<dyn rusqlite::ToSql>> = Vec::new();
        if let Some(p) = filter.parent {
            args.push(Box::new(p.to_string()));
            sql.push_str(&format!(" AND parent = ?{}", args.len()));
        }
        if let Some(r) = filter.run {
            args.push(Box::new(r.to_string()));
            sql.push_str(&format!(" AND run = ?{}", args.len()));
        }
        if filter.live_only {
            // Off `is_terminal`, not a list typed here: a new terminal status
            // must not silently start counting as live.
            let terminal: Vec<String> = AgentStatusKind::ALL
                .iter()
                .filter(|k| k.is_terminal())
                .map(|k| {
                    args.push(Box::new(k.as_str().to_owned()));
                    format!("?{}", args.len())
                })
                .collect();
            sql.push_str(&format!(" AND status NOT IN ({})", terminal.join(",")));
        }
        if !filter.statuses.is_empty() {
            let placeholders: Vec<String> = filter
                .statuses
                .iter()
                .map(|s| {
                    args.push(Box::new(s.as_str().to_owned()));
                    format!("?{}", args.len())
                })
                .collect();
            sql.push_str(&format!(" AND status IN ({})", placeholders.join(",")));
        }
        sql.push_str(" ORDER BY created_at ASC");
        let conn = self.read_conn();
        let refs: Vec<&dyn rusqlite::ToSql> = args.iter().map(|a| a.as_ref()).collect();
        let mut agents: Vec<Agent> = Self::read_many(&conn, &sql, &refs)?;
        // `archived` lives in the JSON body, not a column: it is a view
        // filter on a bounded set, so it is cheaper to drop here than to
        // migrate the table for it.
        if filter.exclude_archived {
            agents.retain(|a| !a.archived);
        }
        Ok(agents)
    }

    fn task(&self, id: TaskId) -> Result<Option<Task>> {
        self.read_one("SELECT body FROM tasks WHERE id = ?1", &id.to_string())
    }

    fn tasks(&self, filter: &TaskFilter) -> Result<Vec<Task>> {
        let mut sql = String::from("SELECT body FROM tasks WHERE 1 = 1");
        let mut args: Vec<Box<dyn rusqlite::ToSql>> = Vec::new();
        if let Some(o) = filter.owner {
            args.push(Box::new(o.to_string()));
            sql.push_str(&format!(" AND owner = ?{}", args.len()));
        }
        if let Some(p) = filter.parent_task {
            args.push(Box::new(p.to_string()));
            sql.push_str(&format!(" AND parent_task = ?{}", args.len()));
        }
        if let Some(r) = filter.run {
            args.push(Box::new(r.to_string()));
            sql.push_str(&format!(" AND run = ?{}", args.len()));
        }
        if !filter.statuses.is_empty() {
            let placeholders: Vec<String> = filter
                .statuses
                .iter()
                .map(|s| {
                    args.push(Box::new(s.as_str().to_owned()));
                    format!("?{}", args.len())
                })
                .collect();
            sql.push_str(&format!(" AND status IN ({})", placeholders.join(",")));
        }
        sql.push_str(" ORDER BY created_at ASC");
        let conn = self.read_conn();
        let refs: Vec<&dyn rusqlite::ToSql> = args.iter().map(|a| a.as_ref()).collect();
        Self::read_many(&conn, &sql, &refs)
    }

    fn inbox(&self, agent: AgentId, after: u64) -> Result<Vec<InboxItem>> {
        if after >= i64::MAX as u64 {
            return Ok(Vec::new());
        }
        let conn = self.read_conn();
        Self::read_many(
            &conn,
            "SELECT body FROM inbox WHERE agent_id = ?1 AND seq > ?2 ORDER BY seq ASC",
            &[&agent.to_string(), &(after as i64)],
        )
    }

    fn inbox_last_seq(&self, agent: AgentId) -> Result<u64> {
        let conn = self.read_conn();
        let v: i64 = conn.query_row(
            "SELECT COALESCE(MAX(seq), 0) FROM inbox WHERE agent_id = ?1",
            [agent.to_string()],
            |r| r.get(0),
        )?;
        Ok(v as u64)
    }

    fn lease(&self, agent: AgentId) -> Result<Option<Lease>> {
        self.read_one(
            "SELECT body FROM leases WHERE agent_id = ?1",
            &agent.to_string(),
        )
    }

    fn leases(&self) -> Result<Vec<Lease>> {
        let conn = self.read_conn();
        Self::read_many(
            &conn,
            "SELECT body FROM leases ORDER BY expires_at ASC",
            &[],
        )
    }

    fn snapshot(&self, id: SnapshotId) -> Result<Option<InvocationSnapshot>> {
        self.read_one("SELECT body FROM snapshots WHERE id = ?1", &id.to_string())
    }

    fn invocation(&self, id: InvocationId) -> Result<Option<Invocation>> {
        self.read_one(
            "SELECT body FROM invocations WHERE id = ?1",
            &id.to_string(),
        )
    }

    fn invocations_for_agent(&self, agent: AgentId) -> Result<Vec<Invocation>> {
        let conn = self.read_conn();
        Self::read_many(
            &conn,
            "SELECT body FROM invocations WHERE agent_id = ?1 ORDER BY turn ASC, started_at ASC",
            &[&agent.to_string()],
        )
    }

    fn checkpoint(&self, id: CheckpointId) -> Result<Option<Checkpoint>> {
        self.read_one(
            "SELECT body FROM checkpoints WHERE id = ?1",
            &id.to_string(),
        )
    }

    fn latest_checkpoint(&self, agent: AgentId) -> Result<Option<Checkpoint>> {
        self.read_one(
            "SELECT body FROM checkpoints WHERE agent_id = ?1 ORDER BY version DESC LIMIT 1",
            &agent.to_string(),
        )
    }

    fn approval(&self, id: ApprovalId) -> Result<Option<ApprovalRequest>> {
        self.read_one("SELECT body FROM approvals WHERE id = ?1", &id.to_string())
    }

    fn pending_approvals(&self) -> Result<Vec<ApprovalRequest>> {
        let conn = self.read_conn();
        Self::read_many(
            &conn,
            "SELECT body FROM approvals WHERE state = 'pending' ORDER BY requested_at ASC",
            &[],
        )
    }

    fn recorded_outcome(
        &self,
        agent: AgentId,
        invocation: InvocationId,
        call: ToolCallId,
        key: &IdempotencyKey,
    ) -> Result<Option<RecordedOutcome>> {
        let conn = self.read_conn();
        let mut stmt = conn.prepare(
            "SELECT body FROM tool_outcomes
             WHERE agent_id = ?1 AND invocation_id = ?2 AND call_id = ?3 AND key = ?4",
        )?;
        let body: Option<String> = stmt
            .query_row(
                params![
                    agent.to_string(),
                    invocation.to_string(),
                    call.to_string(),
                    key.0.as_str()
                ],
                |r| r.get(0),
            )
            .optional()?;
        body.map(|b| serde_json::from_str(&b).map_err(Into::into))
            .transpose()
    }

    fn submission(&self, key: &IdempotencyKey) -> Result<Option<RecordedSubmission>> {
        self.read_one(
            "SELECT body FROM submissions WHERE key = ?1",
            key.0.as_str(),
        )
    }

    fn submission_ledger(&self) -> Result<SubmissionLedger> {
        Self::ledger_in(&self.read_conn())
    }

    fn approval_rule(&self, id: ApprovalRuleId) -> Result<Option<ApprovalRule>> {
        self.read_one(
            "SELECT body FROM approval_rules WHERE id = ?1",
            &id.to_string(),
        )
    }

    fn approval_rules(&self, include_revoked: bool) -> Result<Vec<ApprovalRule>> {
        let conn = self.read_conn();
        let sql = if include_revoked {
            "SELECT body FROM approval_rules ORDER BY created_at ASC"
        } else {
            "SELECT body FROM approval_rules WHERE revoked = 0 ORDER BY created_at ASC"
        };
        Self::read_many(&conn, sql, &[])
    }

    fn device(&self, id: DeviceId) -> Result<Option<DeviceRow>> {
        Self::read_device(&self.read_conn(), id)
    }

    fn devices(&self, include_revoked: bool) -> Result<Vec<DeviceRow>> {
        Self::read_devices(&self.read_conn(), include_revoked)
    }

    fn memory(&self, id: MemoryId) -> Result<Option<MemoryRecord>> {
        self.read_one("SELECT body FROM memories WHERE id = ?1", &id.to_string())
    }

    fn memories(&self, query: &MemoryQuery) -> Result<Vec<MemoryRecord>> {
        self.query_memories(query)
    }

    fn artifact(&self, id: ArtifactId) -> Result<Option<Artifact>> {
        self.read_one("SELECT body FROM artifacts WHERE id = ?1", &id.to_string())
    }

    fn artifacts_for_task(&self, task: TaskId) -> Result<Vec<Artifact>> {
        let conn = self.read_conn();
        Self::read_many(
            &conn,
            "SELECT body FROM artifacts WHERE task_id = ?1 ORDER BY created_at ASC",
            &[&task.to_string()],
        )
    }

    fn artifacts(&self, filter: &ArtifactFilter) -> Result<Vec<Artifact>> {
        let mut sql = String::from("SELECT body FROM artifacts WHERE 1 = 1");
        let mut args: Vec<String> = Vec::new();
        if let Some(agent) = filter.agent {
            args.push(agent.to_string());
            sql.push_str(&format!(" AND agent_id = ?{}", args.len()));
        }
        if let Some(task) = filter.task {
            args.push(task.to_string());
            sql.push_str(&format!(" AND task_id = ?{}", args.len()));
        }
        if let Some(name) = &filter.name {
            args.push(name.clone());
            sql.push_str(&format!(
                " AND json_extract(body, '$.name') = ?{}",
                args.len()
            ));
        }
        if filter.with_bytes_only {
            sql.push_str(" AND evicted = 0");
        }
        sql.push_str(&format!(
            " ORDER BY created_at DESC, rowid DESC LIMIT {}",
            filter.limit.clamp(1, 5000)
        ));
        let conn = self.read_conn();
        let refs: Vec<&dyn rusqlite::ToSql> =
            args.iter().map(|a| a as &dyn rusqlite::ToSql).collect();
        Self::read_many(&conn, &sql, &refs)
    }

    fn artifact_by_digest(&self, digest: &str) -> Result<Option<Artifact>> {
        let conn = self.read_conn();
        let mut found: Vec<Artifact> = Self::read_many(
            &conn,
            "SELECT body FROM artifacts WHERE digest = ?1 AND evicted = 0
             ORDER BY created_at ASC LIMIT 1",
            &[&digest],
        )?;
        Ok(found.pop())
    }

    fn live_artifacts(&self) -> Result<Vec<Artifact>> {
        let conn = self.read_conn();
        Self::read_many(
            &conn,
            "SELECT body FROM artifacts WHERE evicted = 0 ORDER BY created_at ASC",
            &[],
        )
    }

    fn evict_artifact_bytes(&self, id: ArtifactId) -> Result<()> {
        self.evict_blob(id)
    }

    fn remove_unreferenced_artifact_bytes(&self, id: ArtifactId) -> Result<()> {
        self.remove_unreferenced_blob(id)
    }

    fn write_artifact_bytes(&self, id: ArtifactId, bytes: &[u8]) -> Result<PathBuf> {
        self.write_blob(id, bytes)
    }

    fn stage_artifact_bytes(&self, id: ArtifactId, bytes: &[u8]) -> Result<crate::StagedArtifact> {
        self.stage_blob(id, bytes)
    }

    fn finish_artifact_publication(&self, publication: &crate::StagedArtifact) -> Result<()> {
        self.finish_blob(publication)
    }

    fn read_artifact_bytes(&self, id: ArtifactId) -> Result<Option<Vec<u8>>> {
        self.read_blob(id)
    }

    fn import_artifact_file(
        &self,
        id: ArtifactId,
        source: &Path,
    ) -> Result<crate::ImportedArtifact> {
        self.import_blob(id, source)
    }

    fn read_artifact_range(
        &self,
        id: ArtifactId,
        offset: u64,
        limit: usize,
    ) -> Result<Option<Vec<u8>>> {
        self.read_blob_range(id, offset, limit)
    }

    fn artifact_bytes_exist(&self, id: ArtifactId) -> Result<bool> {
        self.blob_exists(id)
    }

    fn transcript(&self, agent: AgentId, after: u64) -> Result<Vec<TranscriptEntry>> {
        let events = self.query_events(
            Some(after),
            None,
            usize::MAX,
            &EventFilter {
                types: vec!["agent.message".into()],
                ..EventFilter::own_events(agent)
            },
            true,
        )?;
        Ok(events
            .into_iter()
            .filter_map(|e| match e.kind {
                EventKind::AgentMessage {
                    role,
                    content,
                    invocation,
                } => Some(TranscriptEntry {
                    seq: e.seq,
                    at: e.at,
                    message: Message { role, content },
                    invocation,
                }),
                _ => None,
            })
            .collect())
    }

    fn schedule(&self, id: ScheduleId) -> Result<Option<Schedule>> {
        self.read_one("SELECT body FROM schedules WHERE id=?1", &id.to_string())
    }

    fn schedules(&self, target: Option<AgentId>) -> Result<Vec<Schedule>> {
        let conn = self.read_conn();
        Self::read_many(
            &conn,
            "SELECT body FROM schedules WHERE (?1 IS NULL OR target=?1) ORDER BY id",
            &[&target.map(|id| id.to_string())],
        )
    }

    fn due_schedules(&self, at: Timestamp, limit: usize) -> Result<Vec<Schedule>> {
        let conn = self.read_conn();
        Self::read_many(
            &conn,
            "SELECT body FROM schedules WHERE next_due <= ?1 ORDER BY next_due, id LIMIT ?2",
            &[&schedule_time(at), &(limit.min(i64::MAX as usize) as i64)],
        )
    }

    fn schedule_history(&self, id: ScheduleId, limit: usize) -> Result<Vec<ScheduleOccurrence>> {
        let conn = self.read_conn();
        Self::read_many(
            &conn,
            "SELECT body FROM schedule_occurrences WHERE schedule=?1 ORDER BY due DESC LIMIT ?2",
            &[&id.to_string(), &(limit.min(i64::MAX as usize) as i64)],
        )
    }

    fn meta(&self, key: &str) -> Result<Option<String>> {
        let conn = self.read_conn();
        Ok(conn
            .query_row("SELECT value FROM meta WHERE key = ?1", [key], |r| r.get(0))
            .optional()?)
    }

    fn instance_id(&self) -> Result<String> {
        self.meta("instance_id")?
            .ok_or_else(|| StoreError::Corrupt("missing instance_id".into()))
    }
}
