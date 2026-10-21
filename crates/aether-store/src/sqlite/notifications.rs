//! The notification center's rows: the record in `body`, the columns its
//! lookups need beside it.

use aether_core::id::NotificationId;
use aether_core::notification::Notification;
use rusqlite::{Connection, OptionalExtension, Transaction, params};

use super::SqliteStore;
use crate::{NotificationFilter, Result, StoreError};

/// The most rows one read returns, whatever the caller asks for.
pub(crate) const MAX_NOTIFICATIONS_READ: usize = 1000;

pub(super) fn put(tx: &Transaction<'_>, notification: &Notification) -> Result<()> {
    if notification.title.trim().is_empty() {
        return Err(StoreError::Corrupt(format!(
            "Notification {} has an empty title; run `aether doctor`",
            notification.id
        )));
    }
    tx.execute(
        "INSERT INTO notifications(id, source, key, state, updated_at, body)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6)
         ON CONFLICT(id) DO UPDATE SET state = excluded.state,
           updated_at = excluded.updated_at, body = excluded.body",
        params![
            notification.id.to_string(),
            notification.source,
            notification.key,
            notification.state.as_str(),
            notification.updated_at.to_rfc3339(),
            serde_json::to_string(notification)?,
        ],
    )?;
    let stored: (String, Option<String>) = tx.query_row(
        "SELECT source, key FROM notifications WHERE id = ?1",
        [notification.id.to_string()],
        |row| Ok((row.get(0)?, row.get(1)?)),
    )?;
    if stored != (notification.source.clone(), notification.key.clone()) {
        return Err(StoreError::Corrupt(format!(
            "notification {} cannot change its source or key",
            notification.id
        )));
    }
    Ok(())
}

pub(super) fn prune_resolved(tx: &Transaction<'_>, keep: usize) -> Result<()> {
    let keep = i64::try_from(keep).unwrap_or(i64::MAX);
    tx.execute(
        "DELETE FROM notifications WHERE state = 'resolved' AND id NOT IN (
           SELECT id FROM notifications WHERE state = 'resolved'
           ORDER BY updated_at DESC, id DESC LIMIT ?1)",
        [keep],
    )?;
    Ok(())
}

fn decode(id: &str, body: &str) -> Result<Notification> {
    let notification: Notification = serde_json::from_str(body)?;
    if notification.id.to_string() != id {
        return Err(StoreError::Corrupt(format!(
            "notification row {id} holds the record of {}",
            notification.id
        )));
    }
    Ok(notification)
}

impl SqliteStore {
    pub(super) fn read_notification(&self, id: NotificationId) -> Result<Option<Notification>> {
        let conn = self.read_conn();
        let body: Option<String> = conn
            .query_row(
                "SELECT body FROM notifications WHERE id = ?1",
                [id.to_string()],
                |row| row.get(0),
            )
            .optional()?;
        body.map(|body| decode(&id.to_string(), &body)).transpose()
    }

    pub(super) fn read_notification_by_key(
        &self,
        source: &str,
        key: &str,
    ) -> Result<Option<Notification>> {
        let conn = self.read_conn();
        let row: Option<(String, String)> = conn
            .query_row(
                "SELECT id, body FROM notifications WHERE source = ?1 AND key = ?2",
                params![source, key],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .optional()?;
        row.map(|(id, body)| decode(&id, &body)).transpose()
    }

    pub(super) fn read_notifications(
        &self,
        filter: &NotificationFilter,
    ) -> Result<Vec<Notification>> {
        read_filtered(&self.read_conn(), filter)
    }
}

/// Newest first by insertion: an upsert keeps its row, and two ids minted in
/// one millisecond do not sort by creation.
fn read_filtered(conn: &Connection, filter: &NotificationFilter) -> Result<Vec<Notification>> {
    let limit = filter.limit.min(MAX_NOTIFICATIONS_READ);
    let states: Vec<&str> = filter.states.iter().map(|s| s.as_str()).collect();
    let states_json = serde_json::to_string(&states)?;
    let mut statement = conn.prepare(
        "SELECT id, body FROM notifications
         WHERE (?1 = '[]' OR state IN (SELECT value FROM json_each(?1)))
           AND (?2 IS NULL OR source = ?2)
         ORDER BY rowid DESC LIMIT ?3",
    )?;
    let rows = statement.query_map(
        params![
            states_json,
            filter.source,
            i64::try_from(limit).unwrap_or(i64::MAX)
        ],
        |row| Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?)),
    )?;
    let mut out = Vec::new();
    for row in rows {
        let (id, body) = row?;
        out.push(decode(&id, &body)?);
    }
    debug_assert!(out.len() <= limit);
    if let Some(record) = out
        .iter()
        .find(|n| !filter.states.is_empty() && !filter.states.contains(&n.state))
    {
        return Err(StoreError::Corrupt(format!(
            "notification {} body disagrees with its indexed state; run `aether doctor`",
            record.id
        )));
    }
    Ok(out)
}

/// Whether a row's state column agrees with its record; for tests of the
/// column the state filter reads.
#[cfg(test)]
fn state_column(
    conn: &Connection,
    id: NotificationId,
) -> Option<aether_core::notification::NotificationState> {
    let raw: String = conn
        .query_row(
            "SELECT state FROM notifications WHERE id = ?1",
            [id.to_string()],
            |row| row.get(0),
        )
        .ok()?;
    aether_core::notification::NotificationState::parse(&raw)
}

#[cfg(test)]
mod tests;
