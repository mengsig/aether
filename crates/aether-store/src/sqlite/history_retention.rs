use super::*;
use crate::Hold;
use aether_core::retention::{RunRetentionDecision, RunRetentionPreview};
use aether_core::{agent::Agent, id::RunId, task::Task};

const MAX_RETENTION_SNAPSHOT_BYTES: u64 = 256 * 1024 * 1024;

const AGENT_HISTORY_TABLES: &[&str] = &[
    "inbox",
    "snapshots",
    "invocations",
    "checkpoints",
    "approvals",
    "tool_outcomes",
];

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RunExpiry {
    Missing,
    Protected(&'static str),
    Expired { events: usize, rows: usize },
}

impl SqliteStore {
    /// Copy SQLite state only; the shadow has no payload directories or recovery workers.
    pub fn retention_snapshot(&self) -> Result<Box<dyn crate::Store>> {
        let shadow = Self::open_in_memory()?;
        self.read_history(|source| {
            // The read transaction pins page_count and the backup to the same snapshot.
            let pages: i64 = source.query_row("PRAGMA page_count", [], |row| row.get(0))?;
            let page_size: i64 = source.query_row("PRAGMA page_size", [], |row| row.get(0))?;
            let bytes = pages.saturating_mul(page_size) as u64;
            if bytes > MAX_RETENTION_SNAPSHOT_BYTES {
                return Err(crate::StoreError::RetentionSnapshotTooLarge {
                    bytes,
                    limit: MAX_RETENTION_SNAPSHOT_BYTES,
                });
            }
            let mut destination = shadow.conn.lock();
            let backup = rusqlite::backup::Backup::new(source, &mut destination)?;
            // One step takes a consistent snapshot, including committed WAL data.
            // Refuse an incomplete copy rather than simulating on partial state.
            match backup.step(-1)? {
                rusqlite::backup::StepResult::Done => {}
                result => {
                    let code = match result {
                        rusqlite::backup::StepResult::Busy => rusqlite::ffi::SQLITE_BUSY,
                        rusqlite::backup::StepResult::Locked => rusqlite::ffi::SQLITE_LOCKED,
                        _ => rusqlite::ffi::SQLITE_ERROR,
                    };
                    return Err(rusqlite::Error::SqliteFailure(
                        rusqlite::ffi::Error::new(code),
                        Some(format!("retention snapshot incomplete: {result:?}")),
                    )
                    .into());
                }
            }
            Ok(())
        })?;
        Ok(Box::new(shadow))
    }

    /// Expire a whole finished run under the caller's runtime write hold.
    /// All guards, expiry spans, and deletions share one write transaction.
    /// Artifact-owning runs remain protected until blob cleanup is integrated.
    /// The caller must also exclude in-flight turns and held resources, which
    /// are not represented by durable projections alone.
    pub fn expire_run_history(
        &self,
        run: RunId,
        before: Timestamp,
        _hold: &Hold<'_>,
    ) -> Result<RunExpiry> {
        let mut conn = self.conn.lock();
        let tx = conn.transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)?;
        let id = run.to_string();
        if let Some(kept) = expiry_blocker(&tx, &id, before)? {
            return Ok(kept);
        }
        tx.execute("INSERT INTO expired_event_spans SELECT run,COALESCE(agent_id,''),COALESCE(parent_agent_id,''),COALESCE(task_id,''),type,family,MIN(seq),MAX(seq) FROM events WHERE run=?1 GROUP BY run,agent_id,parent_agent_id,task_id,type,family
            ON CONFLICT(run,agent_id,parent_agent_id,task_id,type) DO UPDATE SET first_seq=MIN(first_seq,excluded.first_seq),last_seq=MAX(last_seq,excluded.last_seq)", [&id])?;
        let events = tx.execute("DELETE FROM events WHERE run=?1", [&id])?;
        let mut rows = 0;
        for table in AGENT_HISTORY_TABLES {
            rows += tx.execute(
                &format!(
                    "DELETE FROM {table} WHERE agent_id IN (SELECT id FROM agents WHERE run=?1)"
                ),
                [&id],
            )?;
        }
        rows += tx.execute("DELETE FROM tasks WHERE run=?1", [&id])?;
        engine_sessions::preserve_expired_owners(&tx, &id)?;
        rows += tx.execute("DELETE FROM agents WHERE run=?1", [&id])?;
        // last_seq is deliberately unchanged: expiry can never recycle an event cursor.
        tx.commit()?;
        self.commits
            .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        Ok(RunExpiry::Expired { events, rows })
    }

    /// Inspect the exact durable expiry guards and payloads in one read transaction.
    /// This does not open/recover the store, write expiry spans, or delete rows.
    pub fn preview_run_history(
        &self,
        run: RunId,
        before: Timestamp,
    ) -> Result<Option<RunRetentionPreview>> {
        self.read_history(|tx| {
            let id = run.to_string();
            let decision = match expiry_blocker(tx, &id, before)? {
                Some(RunExpiry::Missing) => return Ok(None),
                Some(RunExpiry::Protected(reason)) => RunRetentionDecision::Protected {
                    reason: reason.into(),
                },
                None => RunRetentionDecision::Eligible,
                Some(RunExpiry::Expired { .. }) => unreachable!("expiry guards cannot delete"),
            };
            let mut preview = RunRetentionPreview {
                run,
                decision,
                events: 0,
                rows: 0,
                history_payload_bytes: 0,
            };
            if preview.decision != RunRetentionDecision::Eligible {
                return Ok(Some(preview));
            }
            let (events, bytes): (i64, i64) = tx.query_row(
                "SELECT COUNT(*), COALESCE(SUM(length(CAST(body AS BLOB))),0) \
                 FROM events WHERE run=?1",
                [&id],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )?;
            // COUNT and summed byte lengths are nonnegative SQLite integers.
            preview.events = events as u64;
            preview.history_payload_bytes =
                bytes as u64 + super::event_blobs::run_blob_bytes(tx, &id)?;
            for table in AGENT_HISTORY_TABLES
                .iter()
                .chain(["tasks", "agents"].iter())
            {
                let predicate = if *table == "tasks" || *table == "agents" {
                    "run=?1"
                } else {
                    "agent_id IN (SELECT id FROM agents WHERE run=?1)"
                };
                let (rows, bytes): (i64, i64) = tx.query_row(
                    &format!(
                        "SELECT COUNT(*), COALESCE(SUM(length(CAST(body AS BLOB))),0) \
                     FROM {table} WHERE {predicate}"
                    ),
                    [&id],
                    |row| Ok((row.get(0)?, row.get(1)?)),
                )?;
                preview.rows += rows as u64;
                preview.history_payload_bytes += bytes as u64;
            }
            Ok(Some(preview))
        })
    }

    /// [`Self::expire_run_history`]'s checks on a connection of its own, under
    /// no lock, so a run that cannot expire is passed over without stalling
    /// writers or readers. `None` promises nothing: expiry checks again.
    pub fn screen_run_expiry(&self, run: RunId, before: Timestamp) -> Result<Option<RunExpiry>> {
        self.read_history(|tx| expiry_blocker(tx, &run.to_string(), before))
    }

    // Guard scans must not queue normal readers or commits behind whole-table reads.
    fn read_history<T>(&self, read: impl FnOnce(&Transaction<'_>) -> Result<T>) -> Result<T> {
        let Some(path) = &self.path else {
            let mut conn = self.read_conn();
            return read(&conn.transaction()?);
        };
        let mut conn = Connection::open_with_flags(
            path,
            OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX,
        )?;
        conn.busy_timeout(std::time::Duration::from_secs(5))?;
        conn.execute_batch(super::KEEP_TEMP_IN_MEMORY)?;
        read(&conn.transaction()?)
    }
}

/// Why `run` cannot expire before `before`, or `None` when it can.
fn expiry_blocker(tx: &Transaction<'_>, run: &str, before: Timestamp) -> Result<Option<RunExpiry>> {
    let agents: Vec<Agent> =
        SqliteStore::read_many(tx, "SELECT body FROM agents WHERE run=?1", &[&run])?;
    if agents.is_empty() {
        return Ok(Some(RunExpiry::Missing));
    }
    let tasks: Vec<Task> =
        SqliteStore::read_many(tx, "SELECT body FROM tasks WHERE run=?1", &[&run])?;
    if agents.iter().any(|agent| !agent.status.is_terminal())
        || tasks.iter().any(|task| !task.status.is_terminal())
    {
        return Ok(Some(RunExpiry::Protected("unfinished work")));
    }
    if agents.iter().any(|agent| agent.updated_at >= before)
        || tasks.iter().any(|task| task.updated_at >= before)
    {
        return Ok(Some(RunExpiry::Protected("recent state change")));
    }
    Ok(protection(tx, run, before)?.map(RunExpiry::Protected))
}

fn protection(tx: &Transaction<'_>, run: &str, before: Timestamp) -> Result<Option<&'static str>> {
    let recent_events: bool = tx.query_row("SELECT EXISTS(SELECT 1 FROM events WHERE run=?1 AND (unixepoch(at) IS NULL OR unixepoch(at)>=unixepoch(?2)))",
        params![run, before.to_rfc3339()], |row| row.get(0))?;
    if recent_events {
        return Ok(Some("recent event or invalid event timestamp"));
    }
    for (reason, sql) in GUARDS {
        if tx.query_row(sql, [run], |row| row.get::<_, bool>(0))? {
            return Ok(Some(reason));
        }
    }
    Ok(None)
}

// Protect references before removing their target rows. Even resolved external
// references are retained here; batched dependency-closed expiry can relax this
// later without making one-run deletion leave dangling state.
const GUARDS: &[(&str, &str)] = &[
    (
        "memory scope or provenance",
        "SELECT EXISTS(SELECT 1 FROM memories WHERE json_extract(body,'$.scope.id') IN (SELECT id FROM agents WHERE run=?1) OR json_extract(body,'$.scope.root') IN (SELECT id FROM agents WHERE run=?1) OR json_extract(body,'$.provenance.agent') IN (SELECT id FROM agents WHERE run=?1) OR json_extract(body,'$.provenance.task') IN (SELECT id FROM tasks WHERE run=?1) OR json_extract(body,'$.provenance.event') IN (SELECT id FROM events WHERE run=?1))",
    ),
    (
        "approval rule",
        "SELECT EXISTS(SELECT 1 FROM approval_rules WHERE json_extract(body,'$.scope.id')=?1 OR json_extract(body,'$.scope.id') IN (SELECT id FROM agents WHERE run=?1) OR json_extract(body,'$.scope.root') IN (SELECT id FROM agents WHERE run=?1) OR json_extract(body,'$.created_by.id') IN (SELECT id FROM agents WHERE run=?1) OR json_extract(body,'$.revoked.by.id') IN (SELECT id FROM agents WHERE run=?1))",
    ),
    (
        "lease",
        "SELECT EXISTS(SELECT 1 FROM leases WHERE agent_id IN (SELECT id FROM agents WHERE run=?1))",
    ),
    (
        "pending approval",
        "SELECT EXISTS(SELECT 1 FROM approvals WHERE state='pending' AND agent_id IN (SELECT id FROM agents WHERE run=?1))",
    ),
    (
        "schedule",
        "SELECT EXISTS(SELECT 1 FROM schedules WHERE target IN (SELECT id FROM agents WHERE run=?1) OR json_extract(body,'$.run')=?1 OR json_extract(body,'$.owner.id') IN (SELECT id FROM agents WHERE run=?1) OR json_extract(body,'$.last_spawned') IN (SELECT id FROM agents WHERE run=?1))",
    ),
    (
        "schedule occurrence",
        "SELECT EXISTS(SELECT 1 FROM schedule_occurrences WHERE json_extract(body,'$.outcome.run')=?1)",
    ),
    (
        "artifact",
        "SELECT EXISTS(SELECT 1 FROM artifacts WHERE agent_id IN (SELECT id FROM agents WHERE run=?1) OR task_id IN (SELECT id FROM tasks WHERE run=?1) OR json_extract(body,'$.created_by.id') IN (SELECT id FROM agents WHERE run=?1))",
    ),
    (
        "external snapshot task reference",
        "SELECT EXISTS(SELECT 1 FROM snapshots WHERE agent_id NOT IN (SELECT id FROM agents WHERE run=?1) AND json_extract(body,'$.task_id') IN (SELECT id FROM tasks WHERE run=?1))",
    ),
    (
        "external invocation snapshot reference",
        "SELECT EXISTS(SELECT 1 FROM invocations WHERE agent_id NOT IN (SELECT id FROM agents WHERE run=?1) AND snapshot_id IN (SELECT id FROM snapshots WHERE agent_id IN (SELECT id FROM agents WHERE run=?1)))",
    ),
    (
        "external checkpoint provenance",
        "SELECT EXISTS(SELECT 1 FROM checkpoints WHERE agent_id NOT IN (SELECT id FROM agents WHERE run=?1) AND (json_extract(body,'$.provenance.source_snapshot') IN (SELECT id FROM snapshots WHERE agent_id IN (SELECT id FROM agents WHERE run=?1)) OR json_extract(body,'$.provenance.previous') IN (SELECT id FROM checkpoints WHERE agent_id IN (SELECT id FROM agents WHERE run=?1))))",
    ),
    (
        "external agent checkpoint reference",
        "SELECT EXISTS(SELECT 1 FROM agents WHERE run<>?1 AND json_extract(body,'$.checkpoint') IN (SELECT id FROM checkpoints WHERE agent_id IN (SELECT id FROM agents WHERE run=?1)))",
    ),
    (
        "external or unscoped event reference",
        "SELECT EXISTS(SELECT 1 FROM events WHERE (run IS NULL OR run<>?1) AND (agent_id IN (SELECT id FROM agents WHERE run=?1) OR parent_agent_id IN (SELECT id FROM agents WHERE run=?1) OR task_id IN (SELECT id FROM tasks WHERE run=?1)))",
    ),
    (
        "external agent reference",
        "SELECT EXISTS(SELECT 1 FROM agents AS other, json_tree(other.body) WHERE other.run<>?1 AND json_tree.type='text' AND (json_tree.value IN (SELECT id FROM agents WHERE run=?1) OR json_tree.value IN (SELECT id FROM tasks WHERE run=?1)))",
    ),
    (
        "external task reference",
        "SELECT EXISTS(SELECT 1 FROM tasks AS other WHERE run<>?1 AND (owner IN (SELECT id FROM agents WHERE run=?1) OR json_extract(body,'$.creator.id') IN (SELECT id FROM agents WHERE run=?1) OR json_extract(body,'$.parent_task') IN (SELECT id FROM tasks WHERE run=?1) OR EXISTS(SELECT 1 FROM json_each(other.body,'$.dependencies') WHERE value IN (SELECT id FROM tasks WHERE run=?1))))",
    ),
    (
        "external inbox reference",
        "SELECT EXISTS(SELECT 1 FROM inbox AS other, json_tree(other.body) WHERE other.agent_id NOT IN (SELECT id FROM agents WHERE run=?1) AND json_tree.type='text' AND (json_tree.value IN (SELECT id FROM agents WHERE run=?1) OR json_tree.value IN (SELECT id FROM tasks WHERE run=?1) OR json_tree.value IN (SELECT id FROM approvals WHERE agent_id IN (SELECT id FROM agents WHERE run=?1))))",
    ),
    (
        "external approval reference",
        "SELECT EXISTS(SELECT 1 FROM approvals WHERE agent_id NOT IN (SELECT id FROM agents WHERE run=?1) AND json_extract(body,'$.task_id') IN (SELECT id FROM tasks WHERE run=?1))",
    ),
    (
        "submission replay record",
        "SELECT EXISTS(SELECT 1 FROM submissions, json_tree(submissions.body) WHERE json_tree.type='text' AND (json_tree.value IN (SELECT id FROM agents WHERE run=?1) OR json_tree.value IN (SELECT id FROM tasks WHERE run=?1)))",
    ),
    (
        "pending cancellation",
        "SELECT EXISTS(SELECT 1 FROM meta, json_each(meta.value) WHERE meta.key='runtime.pending_cancellations' AND json_each.key IN (SELECT id FROM agents WHERE run=?1))",
    ),
];
