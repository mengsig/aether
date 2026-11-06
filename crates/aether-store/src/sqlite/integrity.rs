//! A file store checks its memory tables when it opens, so damage there is
//! refused at open instead of failing every later turn's memory retrieval.

use crate::{Result, StoreError};
use rusqlite::Connection;

/// Every table memory retrieval and admission read, each checked with its
/// indexes; `memories_fts` checks the full-text index's own tables too.
const MEMORY_TABLES: [&str; 7] = [
    "memories",
    "memories_fts",
    "memory_feedback",
    "memory_stale",
    "memory_admission",
    "memory_holdouts",
    "repository_aliases",
];

/// Refuse a store whose memory tables SQLite's `quick_check` finds damaged,
/// naming the table and SQLite's first finding. Linear in those tables only.
pub(super) fn verify_memory_tables(conn: &Connection) -> Result<()> {
    for table in MEMORY_TABLES {
        let problems = quick_check(conn, table).map_err(|error| check_failed(table, error))?;
        if let Some(first) = problems.first() {
            return Err(corrupt(table, first));
        }
    }
    Ok(())
}

/// What `quick_check` reports for `table`, without its "ok" row.
fn quick_check(conn: &Connection, table: &str) -> rusqlite::Result<Vec<String>> {
    debug_assert!(
        table.bytes().all(|b| b.is_ascii_lowercase() || b == b'_'),
        "a pragma argument is spliced in, so only a plain table name is allowed"
    );
    let mut statement = conn.prepare(&format!("PRAGMA quick_check(\"{table}\")"))?;
    let rows = statement
        .query_map([], |row| row.get::<_, String>(0))?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    debug_assert!(
        !rows.is_empty(),
        "quick_check always answers at least one row"
    );
    Ok(rows.into_iter().filter(|row| row != "ok").collect())
}

/// A check that could not run is damage only when SQLite says so; a lock or
/// I/O failure keeps its own error, so `is_contended` and `is_unwritable` still see it.
fn check_failed(table: &str, error: rusqlite::Error) -> StoreError {
    match error.sqlite_error_code() {
        Some(rusqlite::ErrorCode::DatabaseCorrupt | rusqlite::ErrorCode::NotADatabase) => {
            corrupt(table, &error.to_string())
        }
        _ => StoreError::Sqlite(error),
    }
}

fn corrupt(table: &str, finding: &str) -> StoreError {
    let first_line = finding
        .lines()
        .find(|l| !l.starts_with("***"))
        .unwrap_or(finding);
    StoreError::Corrupt(format!("memory table {table} is damaged: {first_line}"))
}

#[cfg(test)]
mod tests {
    use crate::{SqliteStore, StoreError};
    use std::io::{Seek, SeekFrom, Write};
    use std::path::Path;

    /// A closed store file holding every commit, its WAL folded in.
    fn closed_store(path: &Path) {
        let store = SqliteStore::open(path).unwrap();
        assert!(
            (0..100).any(|_| store.sync().unwrap()),
            "the WAL folded into the file"
        );
    }

    /// Overwrite the root page of `table` with a byte pattern.
    fn garble_root_page(path: &Path, table: &str) {
        let conn = rusqlite::Connection::open(path).unwrap();
        let page_size: i64 = conn
            .query_row("PRAGMA page_size", [], |r| r.get(0))
            .unwrap();
        let root: i64 = conn
            .query_row(
                "SELECT rootpage FROM sqlite_master WHERE name = ?1",
                [table],
                |r| r.get(0),
            )
            .unwrap();
        drop(conn);
        assert!(root > 1, "page 1 is the header, not {table}'s root");
        let mut file = std::fs::OpenOptions::new().write(true).open(path).unwrap();
        file.seek(SeekFrom::Start(((root - 1) * page_size) as u64))
            .unwrap();
        file.write_all(&vec![0x5a; page_size as usize]).unwrap();
        file.sync_all().unwrap();
    }

    #[test]
    fn a_store_with_a_damaged_memory_table_refuses_to_open_and_names_it() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("aether.db");
        closed_store(&path);
        garble_root_page(&path, "memories");
        let Err(error) = SqliteStore::open(&path) else {
            panic!("a damaged memories table opened");
        };
        assert!(matches!(error, StoreError::Corrupt(_)), "{error:?}");
        assert!(
            error.to_string().contains("memory table memories"),
            "{error}"
        );
    }

    #[test]
    fn every_checked_memory_table_exists_in_the_schema() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("aether.db");
        closed_store(&path);
        let conn = rusqlite::Connection::open(&path).unwrap();
        for table in super::MEMORY_TABLES {
            let found: i64 = conn
                .query_row(
                    "SELECT count(*) FROM sqlite_master WHERE type = 'table' AND name = ?1",
                    [table],
                    |r| r.get(0),
                )
                .unwrap();
            // quick_check of a missing table answers "ok", so a stale name would go unchecked.
            assert_eq!(found, 1, "{table} is checked at open but is not a table");
        }
    }

    fn sqlite_failure(code: i32) -> rusqlite::Error {
        rusqlite::Error::SqliteFailure(rusqlite::ffi::Error::new(code), None)
    }

    #[test]
    fn a_check_that_meets_a_held_lock_is_contention_not_damage() {
        let busy = super::check_failed("memories", sqlite_failure(rusqlite::ffi::SQLITE_BUSY));
        assert!(busy.is_contended(), "{busy:?}");
        assert!(!matches!(busy, StoreError::Corrupt(_)), "{busy:?}");
        let damaged =
            super::check_failed("memories", sqlite_failure(rusqlite::ffi::SQLITE_CORRUPT));
        assert!(matches!(damaged, StoreError::Corrupt(_)), "{damaged:?}");
        assert!(
            damaged.to_string().contains("memory table memories"),
            "{damaged}"
        );
    }

    #[test]
    fn damage_outside_the_memory_tables_does_not_stop_a_store_opening() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("aether.db");
        closed_store(&path);
        assert!(SqliteStore::open(&path).is_ok(), "an intact store opens");
        garble_root_page(&path, "devices");
        assert!(
            SqliteStore::open(&path).is_ok(),
            "only memory tables are checked at open"
        );
    }
}
