//! A session's evidence bundle in `noesis.db` (N3): `evidence_bundles` and
//! `evidence_items`, deleted with the session by the schema's cascade.

use aether_core::noesis::{EvidenceBundle, EvidenceItem, EvidenceRef};
use rusqlite::{Connection, OptionalExtension, params};

use super::{EvidenceError, kind_name, kind_of, status_name, status_of, validate};

/// Store `bundle` with `session`, which must exist and have none yet. Run it
/// in the transaction that inserts the session, so both land or neither.
pub fn save(
    conn: &Connection,
    session: &str,
    bundle: &EvidenceBundle,
    at: &str,
) -> Result<(), EvidenceError> {
    assert!(session.starts_with("ns_"), "a bundle belongs to a session");
    let bytes = validate(bundle)?;
    conn.execute(
        "INSERT INTO evidence_bundles(session, title, excerpt_bytes, created_at)
         VALUES (?1, ?2, ?3, ?4)",
        params![session, bundle.title.trim(), bytes as i64, at],
    )?;
    let mut insert = conn.prepare(
        "INSERT INTO evidence_items(session, ord, ref_kind, ref_id, ref_url, excerpt, status)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
    )?;
    for (ord, item) in bundle.items.iter().enumerate() {
        insert.execute(params![
            session,
            ord as i64,
            kind_name(item.reference.kind),
            item.reference.id,
            item.reference.url,
            item.excerpt,
            status_name(item.status),
        ])?;
    }
    debug_assert_eq!(item_count(conn, session)?, bundle.items.len());
    Ok(())
}

/// The bundle stored with `session`, items in their original order.
pub fn load(conn: &Connection, session: &str) -> Result<Option<EvidenceBundle>, EvidenceError> {
    let head: Option<(String, i64)> = conn
        .query_row(
            "SELECT title, excerpt_bytes FROM evidence_bundles WHERE session = ?1",
            [session],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .optional()?;
    let Some((title, bytes)) = head else {
        return Ok(None);
    };
    let mut statement = conn.prepare(
        "SELECT ref_kind, ref_id, ref_url, excerpt, status FROM evidence_items
         WHERE session = ?1 ORDER BY ord",
    )?;
    let rows = statement.query_map([session], |row| {
        Ok((
            row.get::<_, String>(0)?,
            row.get::<_, String>(1)?,
            row.get::<_, Option<String>>(2)?,
            row.get::<_, String>(3)?,
            row.get::<_, String>(4)?,
        ))
    })?;
    let mut items = Vec::new();
    for row in rows {
        let (kind, id, url, excerpt, status) = row?;
        let corrupt = |what: &str| EvidenceError::Corrupt(format!("{what} in {session}"));
        let kind = kind_of(&kind).ok_or_else(|| corrupt("a reference kind"))?;
        let status = status_of(&status).ok_or_else(|| corrupt("a status"))?;
        let reference = EvidenceRef { kind, id, url };
        items.push(EvidenceItem {
            reference,
            excerpt,
            status,
        });
    }
    let bundle = EvidenceBundle { title, items };
    if super::excerpt_bytes(&bundle) as i64 != bytes || bundle.items.is_empty() {
        return Err(EvidenceError::Corrupt(format!(
            "the bundle of {session} does not match its recorded size"
        )));
    }
    Ok(Some(bundle))
}

fn item_count(conn: &Connection, session: &str) -> Result<usize, EvidenceError> {
    let count: i64 = conn.query_row(
        "SELECT count(*) FROM evidence_items WHERE session = ?1",
        [session],
        |row| row.get(0),
    )?;
    usize::try_from(count).map_err(|_| EvidenceError::Corrupt("a negative count".into()))
}
