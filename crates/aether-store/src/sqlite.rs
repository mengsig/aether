//! SQLite implementation of [`Store`].

use crate::{EventFilter, Result, StoreError, schema};
use aether_core::{
    event::{Event, EventDraft},
    id::{ArtifactId, EventId},
    memory::MemoryScope,
    time::Timestamp,
    tool::ApprovalDecision,
};
use parking_lot::{Mutex, MutexGuard};
use rusqlite::{Connection, OpenFlags, OptionalExtension, Transaction, params};
use serde::de::DeserializeOwned;
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::AtomicU64;

mod blobs;
mod checkpoint;
mod devices;
mod event_queries;
mod history_retention;
pub use history_retention::RunExpiry;
mod ledger;
mod memories;
mod mutations;
mod publication;
mod push_subscriptions;
pub use push_subscriptions::PushSubscriptionRow;
mod retention;
mod store;

/// A SQLite-backed store. Writes share one connection, serialized by a mutex:
/// the daemon is the single writer. A file store reads on a second connection
/// and checkpoints its WAL on a third, so neither waits behind a commit.
pub struct SqliteStore {
    conn: Mutex<Connection>,
    /// Read-only; `None` in memory, where reads share `conn`.
    reader: Option<Mutex<Connection>>,
    checkpointer: Option<checkpoint::Checkpointer>,
    /// The database file, for work that opens a connection of its own; `None` in memory.
    path: Option<PathBuf>,
    artifact_dir: Option<PathBuf>,
    staging_dir: Option<PathBuf>,
    /// Artifact bytes for in-memory stores (tests, ephemeral runs).
    /// Payloads for a store with no directory, keyed by the same path
    /// string the record carries, so a shared blob resolves the same way.
    mem_artifacts: Mutex<BTreeMap<String, Vec<u8>>>,
    commits: AtomicU64,
    /// Submissions recorded since the last size pass. Finding the overflow
    /// costs a scan of the table, so it is paid every
    /// `SUBMISSION_SIZE_PASS_EVERY` writes rather than on every one.
    submissions_since_size_pass: AtomicU64,
    /// Submission records kept whatever their age. Fixed at construction; a
    /// test opens a store with a cap it can actually reach.
    max_submissions: usize,
    /// Recorded submissions between size passes.
    size_pass_every: u64,
    /// The clock receipts are aged against; a test moves it past the window.
    retention_clock: Box<dyn Fn() -> Timestamp + Send + Sync>,
}

impl std::fmt::Debug for SqliteStore {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SqliteStore")
            .field("artifact_dir", &self.artifact_dir)
            .finish()
    }
}

impl SqliteStore {
    /// Sync committed WAL data even when other tasks still hold this store.
    /// `false` when a reader or a concurrent checkpoint held frames back; the WAL keeps them.
    pub fn sync(&self) -> Result<bool> {
        let (busy, frames, copied): (i64, i64, i64) =
            self.conn
                .lock()
                .query_row("PRAGMA wal_checkpoint(FULL)", [], |row| {
                    Ok((row.get(0)?, row.get(1)?, row.get(2)?))
                })?;
        Ok(busy == 0 && copied >= frames)
    }

    /// Open or create the database at `path`, storing artifact bytes in a
    /// sibling `artifacts/` directory.
    pub fn open(path: impl AsRef<Path>) -> Result<Self> {
        Self::open_checkpointing(path.as_ref(), checkpoint::INTERVAL)
    }

    fn open_checkpointing(path: &Path, every: std::time::Duration) -> Result<Self> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let conn = Connection::open_with_flags(
            path,
            OpenFlags::SQLITE_OPEN_READ_WRITE
                | OpenFlags::SQLITE_OPEN_CREATE
                | OpenFlags::SQLITE_OPEN_NO_MUTEX,
        )?;
        // No automatic checkpoint: it runs inside COMMIT. The checkpointer
        // started below does it, and `open` fails if that cannot start.
        conn.execute_batch(
            "PRAGMA journal_mode = WAL;
             PRAGMA synchronous = NORMAL;
             PRAGMA foreign_keys = ON;
             PRAGMA busy_timeout = 5000;
             PRAGMA wal_autocheckpoint = 0;",
        )?;
        crate::migration_backup::before_migration(&conn, path).map_err(|source| {
            StoreError::MigrationBackup {
                database: path.to_path_buf(),
                source: Box::new(source),
            }
        })?;
        let canonical_path = std::fs::canonicalize(path)?;
        let path = canonical_path.as_path();
        let artifact_dir = path.parent().map(|p| p.join("artifacts"));
        if let Some(dir) = &artifact_dir {
            std::fs::create_dir_all(dir)?;
        }
        let mut store = Self::init(conn, artifact_dir)?;
        // Canonical database identity separates stores sharing an artifact directory.
        use sha2::Digest;
        store.staging_dir = store.artifact_dir.as_ref().map(|dir| {
            dir.join(".staging").join(hex::encode(sha2::Sha256::digest(
                path.as_os_str().as_encoded_bytes(),
            )))
        });
        if let Some(dir) = &store.staging_dir {
            let root = dir.parent().ok_or(StoreError::NoArtifactDir)?;
            for owned in [root, dir.as_path()] {
                std::fs::create_dir_all(owned)?;
                if !std::fs::symlink_metadata(owned)?.file_type().is_dir() {
                    return Err(StoreError::Corrupt(format!(
                        "artifact staging is not a directory: {}",
                        owned.display()
                    )));
                }
            }
        }
        store.recover_staged_blobs()?;
        store.retry_evicted_blobs()?;
        let reader = Connection::open_with_flags(
            path,
            OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX,
        )?;
        reader.busy_timeout(std::time::Duration::from_secs(5))?;
        store.reader = Some(Mutex::new(reader));
        store.path = Some(path.to_path_buf());
        store.checkpointer = Some(checkpoint::Checkpointer::start(path, every)?);
        Ok(store)
    }

    /// An ephemeral store for tests and dry runs.
    pub fn open_in_memory() -> Result<Self> {
        let conn = Connection::open_in_memory()?;
        conn.execute_batch("PRAGMA foreign_keys = ON;")?;
        Self::init(conn, None)
    }

    /// An ephemeral store whose submission cap a test can actually reach.
    /// The real cap is six figures; driving the size pass through `commit`
    /// otherwise means writing a hundred thousand records.
    #[cfg(test)]
    fn open_in_memory_capped(max_submissions: usize) -> Result<Self> {
        let mut store = Self::open_in_memory()?;
        store.max_submissions = max_submissions;
        Ok(store)
    }

    /// The same store with a receipt cap and a size-pass cadence a caller can
    /// reach: the defaults take a hundred thousand keyed commands to exercise.
    pub fn with_submission_limits(mut self, max_submissions: usize, size_pass_every: u64) -> Self {
        assert!(max_submissions > 0, "a store keeps at least one receipt");
        assert!(size_pass_every > 0, "the size pass runs on some write");
        self.max_submissions = max_submissions;
        self.size_pass_every = size_pass_every;
        self
    }

    /// The same store ageing receipts against `clock` instead of the wall
    /// clock, so a test can let the retention window pass without waiting.
    pub fn with_retention_clock(
        mut self,
        clock: impl Fn() -> Timestamp + Send + Sync + 'static,
    ) -> Self {
        self.retention_clock = Box::new(clock);
        self
    }

    fn init(conn: Connection, artifact_dir: Option<PathBuf>) -> Result<Self> {
        schema::migrate(&conn)?;
        conn.execute(
            "INSERT OR IGNORE INTO meta(key, value) VALUES ('instance_id', ?1)",
            [ulid::Ulid::generate().to_string()],
        )?;
        conn.execute(
            "INSERT OR IGNORE INTO meta(key, value) VALUES ('last_seq', '0')",
            [],
        )?;
        Self::ensure_submission_ledger(&conn)?;
        Ok(Self {
            conn: Mutex::new(conn),
            reader: None,
            checkpointer: None,
            path: None,
            artifact_dir,
            staging_dir: None,
            mem_artifacts: Mutex::new(BTreeMap::new()),
            commits: AtomicU64::new(0),
            submissions_since_size_pass: AtomicU64::new(0),
            max_submissions: aether_core::idempotency::MAX_SUBMISSIONS,
            size_pass_every: aether_core::idempotency::SUBMISSION_SIZE_PASS_EVERY,
            retention_clock: Box::new(aether_core::time::now),
        })
    }

    /// The connection for reads. Every commit is visible to it once `commit`
    /// returns; it never sees one in progress.
    pub(super) fn read_conn(&self) -> MutexGuard<'_, Connection> {
        self.reader.as_ref().unwrap_or(&self.conn).lock()
    }

    pub(super) fn read_one<T: DeserializeOwned>(&self, sql: &str, id: &str) -> Result<Option<T>> {
        let conn = self.read_conn();
        let body: Option<String> = conn.query_row(sql, [id], |r| r.get(0)).optional()?;
        body.map(|b| serde_json::from_str(&b).map_err(StoreError::from))
            .transpose()
    }

    pub(super) fn read_many<T: DeserializeOwned>(
        conn: &Connection,
        sql: &str,
        params: &[&dyn rusqlite::ToSql],
    ) -> Result<Vec<T>> {
        let mut stmt = conn.prepare(sql)?;
        let rows = stmt.query_map(params, |r| r.get::<_, String>(0))?;
        let mut out = Vec::new();
        for body in rows {
            out.push(serde_json::from_str(&body?)?);
        }
        Ok(out)
    }

    pub(super) fn artifact_path(&self, id: ArtifactId) -> Option<PathBuf> {
        self.artifact_dir.as_ref().map(|d| d.join(id.to_string()))
    }
}

/// Canonical text key for a memory scope, used for indexed lookups.
fn scope_key(scope: &MemoryScope) -> String {
    match scope {
        MemoryScope::Global => "global".into(),
        MemoryScope::Repository { id } => format!("repo:{id}"),
        MemoryScope::Role { role } => format!("role:{role}"),
        MemoryScope::Agent { id } => format!("agent:{id}"),
        // The id's length makes the id/role split unambiguous whatever the id
        // holds, and puts every role of one repository under one key prefix.
        MemoryScope::RepositoryRole { id, role } => {
            format!("{}{role}", repository_roles_prefix(id))
        }
        MemoryScope::Subtree { root } => format!("subtree:{root}"),
    }
}

/// The key prefix every `RepositoryRole` scope of repository `id` starts with.
fn repository_roles_prefix(id: &str) -> String {
    format!("repo_role:{}:{id}:", id.len())
}

/// The keys of the scopes nested in `scope`, as a half-open range: every key
/// starting with the prefix, and nothing else. `None` when nothing nests in it.
fn nested_scope_range(scope: &MemoryScope) -> Option<(String, String)> {
    let MemoryScope::Repository { id } = scope else {
        return None;
    };
    let prefix = repository_roles_prefix(id);
    // The prefix ends in `:`; the next byte up bounds everything under it.
    let upper = format!("{};", &prefix[..prefix.len() - 1]);
    Some((prefix, upper))
}

/// Escape a user query for FTS5 MATCH: quote each token so operators and
/// punctuation are treated as text.
fn fts_query(text: &str) -> String {
    text.split_whitespace()
        .map(|t| format!("\"{}\"", t.replace('"', "\"\"")))
        .collect::<Vec<_>>()
        .join(" OR ")
}

impl SqliteStore {
    /// Insert drafts under gapless sequence numbers. A commit mints each id
    /// and stamps the current time; an import brings both, so it can resolve
    /// an event's causation to another event of the same import in one pass.
    /// `events.id` is UNIQUE, so a brought id that collides fails loudly.
    pub(super) fn write_events(
        tx: &Transaction<'_>,
        seq: &mut u64,
        drafts: impl IntoIterator<Item = (EventId, EventDraft, Timestamp)>,
    ) -> Result<Vec<Event>> {
        let mut committed = Vec::new();
        for (id, draft, at) in drafts {
            *seq += 1;
            let event = Event::from_draft(draft, id, *seq, at);
            tx.execute(
                "INSERT INTO events(seq, id, at, type, family, agent_id, task_id, parent_agent_id, run, body)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)",
                params![
                    event.seq as i64,
                    event.id.to_string(),
                    event.at.to_rfc3339(),
                    event.kind.type_name(),
                    event.kind.family(),
                    event.agent_id.map(|a| a.to_string()),
                    event.task_id.map(|t| t.to_string()),
                    event.parent_agent_id.map(|p| p.to_string()),
                    event.run.map(|r| r.to_string()),
                    serde_json::to_string(&event)?,
                ],
            )?;
            committed.push(event);
        }
        Ok(committed)
    }

    pub(super) fn last_seq_in(tx: &Transaction<'_>) -> Result<u64> {
        tx.query_row("SELECT value FROM meta WHERE key = 'last_seq'", [], |r| {
            r.get::<_, String>(0)
        })?
        .parse()
        .map_err(|_| StoreError::Corrupt("meta.last_seq is not a number".into()))
    }
}

// Fixed precision keeps textual index ordering identical to UTC ordering,
// including exact seconds and subsecond due times.
/// Fixed width and always UTC, so the `at` column sorts and compares as text.
fn submission_time(at: Timestamp) -> String {
    at.to_rfc3339_opts(chrono::SecondsFormat::Nanos, true)
}

fn schedule_time(at: Timestamp) -> String {
    at.to_rfc3339_opts(chrono::SecondsFormat::Nanos, true)
}

/// How a decision is spelled in the `decision` column. Explicit, because a
/// `Debug` rendering is not a storage format: renaming the variant would
/// silently change what is on disk.
fn decision_key(decision: ApprovalDecision) -> &'static str {
    match decision {
        ApprovalDecision::Approved => "approved",
        ApprovalDecision::Denied => "denied",
    }
}

/// The in-memory payload key a path names, if it is one.
fn memory_key(path: &std::path::Path) -> Option<&str> {
    path.to_str().filter(|p| p.starts_with("memory://"))
}

#[cfg(test)]
mod tests;
