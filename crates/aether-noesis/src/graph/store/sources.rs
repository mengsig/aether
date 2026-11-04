//! Imported sources (parity X8, X9): a stable identity from the text, and a
//! deletion that either preserves the graph or drops only what no other
//! session or source supports.

use rusqlite::{Connection, OptionalExtension, params};

use super::GraphError;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SourceRow {
    pub id: String,
    pub name: String,
    pub artifact: String,
    pub imported_at: String,
}

/// Record `row`, or keep the source already recorded under its id (the same
/// text imported again). Returns the row as stored.
pub fn insert_source(conn: &Connection, row: &SourceRow) -> Result<SourceRow, GraphError> {
    assert!(row.id.starts_with("s_"), "a source id is content-derived");
    assert!(!row.name.trim().is_empty(), "the protocol requires a name");
    conn.execute(
        "INSERT INTO sources (id, name, artifact, imported_at) VALUES (?1, ?2, ?3, ?4)
         ON CONFLICT(id) DO NOTHING",
        params![row.id, row.name, row.artifact, row.imported_at],
    )?;
    let stored = source(conn, &row.id)?;
    stored.ok_or_else(|| GraphError::Corrupt(format!("source {} vanished on insert", row.id)))
}

pub fn source(conn: &Connection, id: &str) -> Result<Option<SourceRow>, GraphError> {
    let row = conn
        .query_row(
            "SELECT id, name, artifact, imported_at FROM sources WHERE id = ?1",
            [id],
            |row| {
                Ok(SourceRow {
                    id: row.get(0)?,
                    name: row.get(1)?,
                    artifact: row.get(2)?,
                    imported_at: row.get(3)?,
                })
            },
        )
        .optional()?;
    Ok(row)
}

/// Delete `id` and its coverage. With `delete_unsupported`, its provenance
/// goes too, and so does every proposition or chain nothing else supports.
/// Whether the source existed. The caller holds a write transaction.
pub fn delete_source(
    conn: &Connection,
    id: &str,
    delete_unsupported: bool,
) -> Result<bool, GraphError> {
    if source(conn, id)?.is_none() {
        return Ok(false);
    }
    if delete_unsupported {
        drop_unsupported(conn, id)?;
    }
    conn.execute("DELETE FROM coverage WHERE source = ?1", [id])?;
    let deleted = conn.execute("DELETE FROM sources WHERE id = ?1", [id])?;
    assert_eq!(deleted, 1, "the source was read in this transaction");
    Ok(true)
}

/// Withdraw `source`'s support, then delete what only it supported.
fn drop_unsupported(conn: &Connection, source: &str) -> Result<(), GraphError> {
    conn.execute_batch(
        "CREATE TEMP TABLE IF NOT EXISTS withdrawn_nodes (id TEXT PRIMARY KEY);
         CREATE TEMP TABLE IF NOT EXISTS withdrawn_chains (id TEXT PRIMARY KEY);
         DELETE FROM withdrawn_nodes; DELETE FROM withdrawn_chains;",
    )?;
    conn.execute(
        "INSERT OR IGNORE INTO withdrawn_nodes
         SELECT proposition FROM proposition_provenance WHERE source = ?1",
        [source],
    )?;
    conn.execute(
        "INSERT OR IGNORE INTO withdrawn_chains SELECT chain FROM chain_provenance WHERE source = ?1",
        [source],
    )?;
    conn.execute(
        "DELETE FROM proposition_provenance WHERE source = ?1",
        [source],
    )?;
    conn.execute("DELETE FROM chain_provenance WHERE source = ?1", [source])?;
    conn.execute_batch(
        "DELETE FROM propositions WHERE id IN (SELECT id FROM withdrawn_nodes) AND NOT EXISTS
            (SELECT 1 FROM proposition_provenance p WHERE p.proposition = propositions.id);
         DELETE FROM chains WHERE id IN (SELECT id FROM withdrawn_chains) AND NOT EXISTS
            (SELECT 1 FROM chain_provenance p WHERE p.chain = chains.id);
         DELETE FROM withdrawn_nodes; DELETE FROM withdrawn_chains;",
    )?;
    let left: u32 = conn.query_row(
        "SELECT count(*) FROM proposition_provenance WHERE source = ?1",
        [source],
        |row| row.get(0),
    )?;
    assert_eq!(left, 0, "the source supports nothing any more");
    Ok(())
}
