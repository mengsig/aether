//! Give freed pages back to the filesystem. Without incremental auto-vacuum a
//! SQLite file never shrinks: pages history expiry frees are reused, but the
//! file stays at its peak size for good.

use super::SqliteStore;
use crate::Result;
use rusqlite::Connection;
use std::path::Path;

/// `PRAGMA auto_vacuum` value of incremental mode.
const INCREMENTAL: i64 = 2;

/// Pages one reclaim pass frees: 8 MiB at 4 KiB pages, a short write.
const PASS_PAGES: i64 = 2048;

/// Passes one sweep runs, so a large backlog is returned over a few sweeps
/// instead of holding the writer for one long stretch.
const MAX_PASSES: u32 = 64;

/// Set before the first table exists, so a new store starts in incremental
/// mode; on an existing store it only takes effect at its next `VACUUM`.
pub(super) const REQUEST_INCREMENTAL: &str = "PRAGMA auto_vacuum = INCREMENTAL;";

fn auto_vacuum(conn: &Connection) -> Result<i64> {
    Ok(conn.query_row("PRAGMA auto_vacuum", [], |row| row.get(0))?)
}

fn freelist(conn: &Connection) -> Result<i64> {
    Ok(conn.query_row("PRAGMA freelist_count", [], |row| row.get(0))?)
}

/// One pass of up to [`PASS_PAGES`]. SQLite frees a page per step of the
/// statement, so it is stepped to the end rather than run once.
fn incremental_vacuum(conn: &Connection) -> Result<()> {
    let mut statement = conn.prepare(&format!("PRAGMA incremental_vacuum({PASS_PAGES})"))?;
    let mut rows = statement.query([])?;
    while rows.next()?.is_some() {}
    Ok(())
}

/// Whether the filesystem holding `path` has room for a `VACUUM` of it: the
/// temporary copy and the write-ahead log each take up to the store's size.
fn has_room_to_vacuum(path: &Path) -> Result<bool> {
    let size = std::fs::metadata(path)?.len();
    let directory = path.parent().ok_or_else(|| {
        crate::StoreError::Corrupt(format!("store path has no directory: {}", path.display()))
    })?;
    let available = fs4::available_space(directory)?;
    Ok(available / 2 > size)
}

/// Rewrite a store created before incremental mode so it can shrink. Runs
/// once, at open, before the reader and checkpointer connections exist:
/// `VACUUM` holds the writer for as long as the copy takes.
///
/// The copy goes through a temporary file, not memory: the store keeps temp
/// tables in memory, and a copy of a multi-gigabyte store would not fit.
/// Skipped while the disk lacks room for it; the next open tries again.
/// Returns whether the store was converted.
pub(super) fn convert_to_incremental(conn: &Connection, path: &Path) -> Result<bool> {
    if auto_vacuum(conn)? == INCREMENTAL {
        return Ok(false);
    }
    if !has_room_to_vacuum(path)? {
        tracing::warn!(
            path = %path.display(),
            "not enough free disk space to convert the store to incremental vacuum; it keeps its size until a later start"
        );
        return Ok(false);
    }
    let started = std::time::Instant::now();
    tracing::info!(
        "converting the store to incremental vacuum; this runs once and may take a while"
    );
    conn.execute_batch("PRAGMA temp_store = FILE;")?;
    let converted = conn.execute_batch(&format!("{REQUEST_INCREMENTAL} VACUUM;"));
    // Restored whatever the outcome: a failed VACUUM leaves the store as it was.
    conn.execute_batch(super::KEEP_TEMP_IN_MEMORY)?;
    converted?;
    // External-content full-text rows are keyed by rowid, which VACUUM may
    // renumber on tables without an INTEGER PRIMARY KEY.
    conn.execute_batch("INSERT INTO memories_fts(memories_fts) VALUES ('rebuild');")?;
    conn.query_row("PRAGMA wal_checkpoint(TRUNCATE)", [], |_| Ok(()))?;
    let mode = auto_vacuum(conn)?;
    debug_assert_eq!(mode, INCREMENTAL, "VACUUM applies the requested mode");
    tracing::info!(
        elapsed_ms = started.elapsed().as_millis() as u64,
        "store converted to incremental vacuum"
    );
    Ok(mode == INCREMENTAL)
}

impl SqliteStore {
    /// Return free pages to the filesystem in short passes. Each pass is its
    /// own write, so commits interleave with a long reclaim. Returns the
    /// pages freed; 0 for a store that is not in incremental mode.
    pub fn reclaim_free_pages(&self) -> Result<u64> {
        let mut reclaimed = 0;
        for _ in 0..MAX_PASSES {
            let conn = self.conn.lock();
            if auto_vacuum(&conn)? != INCREMENTAL {
                return Ok(0);
            }
            let before = freelist(&conn)?;
            if before == 0 {
                break;
            }
            incremental_vacuum(&conn)?;
            let after = freelist(&conn)?;
            debug_assert!(after <= before, "a reclaim pass never adds free pages");
            if after >= before {
                break;
            }
            reclaimed += (before - after) as u64;
        }
        Ok(reclaimed)
    }
}
