use super::*;
use crate::Hold;
use aether_core::retention::{RunRetentionDecision, RunRetentionPreview};
use aether_core::{agent::Agent, id::RunId, task::Task};
use std::sync::atomic::Ordering;

mod guards;
mod screen;
pub use screen::HistoryScreen;

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
        if let Some(kept) = expiry_blocker(&tx, &id, before, &self.guard_passes)? {
            return Ok(kept);
        }
        self.delete_run(tx, &id)
    }

    /// Delete `run`'s history in `tx` and commit, once its guards have passed.
    fn delete_run(&self, tx: Transaction<'_>, id: &str) -> Result<RunExpiry> {
        tx.execute("INSERT INTO expired_event_spans SELECT run,COALESCE(agent_id,''),COALESCE(parent_agent_id,''),COALESCE(task_id,''),type,family,MIN(seq),MAX(seq) FROM events WHERE run=?1 GROUP BY run,agent_id,parent_agent_id,task_id,type,family
            ON CONFLICT(run,agent_id,parent_agent_id,task_id,type) DO UPDATE SET first_seq=MIN(first_seq,excluded.first_seq),last_seq=MAX(last_seq,excluded.last_seq)", [id])?;
        let events = tx.execute("DELETE FROM events WHERE run=?1", [id])?;
        let mut rows = 0;
        for table in AGENT_HISTORY_TABLES {
            rows += tx.execute(
                &format!(
                    "DELETE FROM {table} WHERE agent_id IN (SELECT id FROM agents WHERE run=?1)"
                ),
                [id],
            )?;
        }
        rows += tx.execute("DELETE FROM tasks WHERE run=?1", [id])?;
        engine_sessions::preserve_expired_owners(&tx, id)?;
        rows += tx.execute("DELETE FROM agents WHERE run=?1", [id])?;
        debug_assert!(rows > 0, "an expired run had at least its agents");
        // last_seq is deliberately unchanged: expiry can never recycle an event cursor.
        tx.commit()?;
        self.commits.fetch_add(1, Ordering::Relaxed);
        Ok(RunExpiry::Expired { events, rows })
    }

    /// Whole-table guard passes made for one run at a time, screens and
    /// expiry alike. A sweep's own screen judges every run in one pass.
    #[doc(hidden)]
    pub fn history_guard_passes(&self) -> u64 {
        self.guard_passes.load(Ordering::Relaxed)
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
            let decision = match expiry_blocker(tx, &id, before, &self.guard_passes)? {
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
fn expiry_blocker(
    tx: &Transaction<'_>,
    run: &str,
    before: Timestamp,
    passes: &AtomicU64,
) -> Result<Option<RunExpiry>> {
    let (agents, tasks) = run_members(tx, run)?;
    if let Some(kept) = state_blocker(&agents, &tasks, before) {
        return Ok(Some(kept));
    }
    Ok(protection(tx, run, before, passes)?.map(RunExpiry::Protected))
}

fn run_members(tx: &Transaction<'_>, run: &str) -> Result<(Vec<Agent>, Vec<Task>)> {
    let agents = SqliteStore::read_many(tx, "SELECT body FROM agents WHERE run=?1", &[&run])?;
    let tasks = SqliteStore::read_many(tx, "SELECT body FROM tasks WHERE run=?1", &[&run])?;
    Ok((agents, tasks))
}

/// What the run's own agents and tasks say: missing, unfinished or recent.
fn state_blocker(agents: &[Agent], tasks: &[Task], before: Timestamp) -> Option<RunExpiry> {
    if agents.is_empty() {
        return Some(RunExpiry::Missing);
    }
    if agents.iter().any(|agent| !agent.status.is_terminal())
        || tasks.iter().any(|task| !task.status.is_terminal())
    {
        return Some(RunExpiry::Protected("unfinished work"));
    }
    if agents.iter().any(|agent| agent.updated_at >= before)
        || tasks.iter().any(|task| task.updated_at >= before)
    {
        return Some(RunExpiry::Protected("recent state change"));
    }
    None
}

fn recent_events(tx: &Transaction<'_>, run: &str, before: Timestamp) -> Result<bool> {
    Ok(tx.query_row("SELECT EXISTS(SELECT 1 FROM events WHERE run=?1 AND (unixepoch(at) IS NULL OR unixepoch(at)>=unixepoch(?2)))",
        params![run, before.to_rfc3339()], |row| row.get(0))?)
}

fn protection(
    tx: &Transaction<'_>,
    run: &str,
    before: Timestamp,
    passes: &AtomicU64,
) -> Result<Option<&'static str>> {
    if recent_events(tx, run, before)? {
        return Ok(Some("recent event or invalid event timestamp"));
    }
    passes.fetch_add(1, Ordering::Relaxed);
    for guard in guards::GUARDS {
        if tx.query_row(&guards::whole(guard), [run], |row| row.get::<_, bool>(0))? {
            return Ok(Some(guard.reason));
        }
    }
    Ok(None)
}
