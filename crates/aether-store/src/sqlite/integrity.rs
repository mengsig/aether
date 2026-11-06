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
        let problems =
            quick_check(conn, table).map_err(|error| corrupt(table, &error.to_string()))?;
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
