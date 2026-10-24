//! Large strings in event bodies - tool output, long messages - live outside
//! the events table, compressed and content-addressed (schema V27). The body
//! keeps a short reference in their place, so its JSON shape, ids and every
//! `json_extract` over it stay as they were, and readers get the full event.

use crate::{Result, StoreError};
use aether_core::event::Event;
use rusqlite::{Connection, OptionalExtension, params};
use serde_json::Value;
use sha2::{Digest, Sha256};

/// A body shorter than this is stored as it is: none of its strings can
/// reach [`MIN_STRING`] by much, and most events never pay for the walk.
const MIN_BODY: usize = 8 * 1024;
/// A string at least this long moves into a blob.
const MIN_STRING: usize = 2 * 1024;
const ZSTD_LEVEL: i32 = 3;
/// Larger than any event the daemon accepts; a blob claiming more is corrupt.
const MAX_BLOB: usize = 1 << 30;
/// What a reference string starts with. A string of real content that starts
/// with it is moved into a blob too, so content can never pose as a reference.
const MARKER: &str = "\u{1}aether-blob:";
/// [`MARKER`] as serde_json spells it inside a JSON document.
const MARKER_JSON: &str = "\\u0001aether-blob:";

/// The body to store for `event` at `seq`, with its large strings moved into
/// blobs, and the bytes newly written to blobs. An output already stored is
/// referenced, not written again.
pub(super) fn encode(tx: &Connection, event: &Event) -> Result<(String, usize)> {
    let body = serde_json::to_string(event)?;
    if body.len() < MIN_BODY && !body.contains(MARKER_JSON) {
        return Ok((body, 0));
    }
    let mut value = serde_json::to_value(event)?;
    let mut written = 0;
    let mut stack = vec![&mut value];
    while let Some(node) = stack.pop() {
        match node {
            Value::String(text) if text.len() >= MIN_STRING || text.starts_with(MARKER) => {
                written += store(tx, event.seq, text)?;
            }
            Value::Array(items) => stack.extend(items.iter_mut()),
            Value::Object(fields) => stack.extend(fields.values_mut()),
            _ => {}
        }
    }
    let stored = serde_json::to_string(&value)?;
    debug_assert!(stored.len() <= body.len() || body.contains(MARKER_JSON));
    Ok((stored, written))
}

/// Move `text` into a blob referenced by event `seq` and leave the reference
/// in its place. Returns the bytes written, 0 when the blob already existed.
fn store(tx: &Connection, seq: u64, text: &mut String) -> Result<usize> {
    let digest = hex::encode(Sha256::digest(text.as_bytes()));
    let seq = i64::try_from(seq)
        .map_err(|_| StoreError::Corrupt(format!("event sequence {seq} is out of range")))?;
    tx.execute(
        "INSERT OR IGNORE INTO event_blob_refs(seq, digest) VALUES (?1, ?2)",
        params![seq, digest],
    )?;
    let exists: bool = tx.query_row(
        "SELECT EXISTS(SELECT 1 FROM event_blobs WHERE digest = ?1)",
        [&digest],
        |row| row.get(0),
    )?;
    let mut written = 0;
    if !exists {
        let data = zstd::bulk::compress(text.as_bytes(), ZSTD_LEVEL)?;
        tx.execute(
            "INSERT INTO event_blobs(digest, size, data) VALUES (?1, ?2, ?3)",
            params![digest, text.len() as i64, data],
        )?;
        written = data.len();
    }
    *text = format!("{MARKER}{digest}");
    Ok(written)
}

/// The event a stored body stands for, its blob references resolved. Run it
/// inside the read that fetched the body, so expiry cannot remove the blobs
/// in between.
///
/// A marker string is a reference only when the event recorded it in
/// `event_blob_refs`: a body written before v27 may hold such content inline.
pub(super) fn decode(conn: &Connection, body: &str) -> Result<Event> {
    if !body.contains(MARKER_JSON) {
        return Ok(serde_json::from_str(body)?);
    }
    let mut value: Value = serde_json::from_str(body)?;
    let seq = value
        .get("seq")
        .and_then(Value::as_i64)
        .ok_or_else(|| StoreError::Corrupt("an event body has no sequence".into()))?;
    let mut stack = vec![&mut value];
    while let Some(node) = stack.pop() {
        match node {
            Value::String(text) => {
                if let Some(digest) = text.strip_prefix(MARKER)
                    && references(conn, seq, digest)?
                {
                    *text = load(conn, digest)?;
                }
            }
            Value::Array(items) => stack.extend(items.iter_mut()),
            Value::Object(fields) => stack.extend(fields.values_mut()),
            _ => {}
        }
    }
    Ok(serde_json::from_value(value)?)
}

/// Whether event `seq` stored a string as blob `digest`.
fn references(conn: &Connection, seq: i64, digest: &str) -> Result<bool> {
    debug_assert!(seq > 0, "event sequences start at 1");
    Ok(conn.query_row(
        "SELECT EXISTS(SELECT 1 FROM event_blob_refs WHERE seq = ?1 AND digest = ?2)",
        params![seq, digest],
        |row| row.get(0),
    )?)
}

/// The text of blob `digest`, checked against its recorded size and digest.
fn load(conn: &Connection, digest: &str) -> Result<String> {
    let row: Option<(i64, Vec<u8>)> = conn
        .query_row(
            "SELECT size, data FROM event_blobs WHERE digest = ?1",
            [digest],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .optional()?;
    let corrupt = |what: &str| {
        StoreError::Corrupt(format!(
            "event blob {digest} {what}; run `aether doctor` and `aether diagnose`"
        ))
    };
    let (size, data) = row.ok_or_else(|| corrupt("is missing"))?;
    let size = usize::try_from(size)
        .ok()
        .filter(|size| *size <= MAX_BLOB)
        .ok_or_else(|| corrupt("has an impossible size"))?;
    let bytes = zstd::bulk::decompress(&data, size)?;
    if bytes.len() != size || hex::encode(Sha256::digest(&bytes)) != digest {
        return Err(corrupt("does not match its digest"));
    }
    String::from_utf8(bytes).map_err(|_| corrupt("is not UTF-8"))
}

/// Events whose bodies `sql` selects as its first column, in one read.
pub(super) fn read_events(
    conn: &Connection,
    sql: &str,
    params: &[&dyn rusqlite::ToSql],
) -> Result<Vec<Event>> {
    let tx = conn.unchecked_transaction()?;
    let mut events = Vec::new();
    {
        let mut statement = tx.prepare(sql)?;
        let mut rows = statement.query(params)?;
        while let Some(row) = rows.next()? {
            let body: String = row.get(0)?;
            events.push(decode(&tx, &body)?);
        }
    }
    tx.commit()?;
    Ok(events)
}

/// Bytes of blobs referenced by events of `run` and by no other event: what
/// expiring the run frees beyond its rows.
pub(super) fn run_blob_bytes(tx: &Connection, run: &str) -> Result<u64> {
    let bytes: i64 = tx.query_row(
        "SELECT COALESCE(SUM(length(b.data)), 0) FROM event_blobs AS b
         WHERE b.digest IN (SELECT r.digest FROM event_blob_refs AS r
                            JOIN events AS e ON e.seq = r.seq WHERE e.run = ?1)
           AND NOT EXISTS (SELECT 1 FROM event_blob_refs AS r
                           JOIN events AS e ON e.seq = r.seq
                           WHERE r.digest = b.digest AND (e.run IS NULL OR e.run <> ?1))",
        [run],
        |row| row.get(0),
    )?;
    debug_assert!(bytes >= 0, "summed lengths are nonnegative");
    Ok(bytes as u64)
}
