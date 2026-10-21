//! Preserve malformed records in place while making list reads usable.

use rusqlite::{OptionalExtension, params};
use serde::de::DeserializeOwned;

use super::SqliteStore;
use crate::{RecordQuarantine, Result, StoreError};

const PREFIX: &str = "quarantine.";

impl SqliteStore {
    pub(super) fn record_list<T: DeserializeOwned>(
        &self,
        table: &str,
        sql: &str,
        params: &[&dyn rusqlite::ToSql],
        identity: impl Fn(&T) -> String,
    ) -> Result<Vec<T>> {
        let rows = {
            let conn = self.read_conn();
            let mut statement = conn.prepare(sql)?;
            statement
                .query_map(params, |row| {
                    Ok((
                        row.get::<_, String>(0)
                            .unwrap_or_else(|error| format!("unreadable-key:{error}")),
                        row.get::<_, String>(1).map_err(|error| error.to_string()),
                    ))
                })?
                .collect::<rusqlite::Result<Vec<_>>>()?
        };
        let quarantined: std::collections::BTreeSet<_> = self
            .list_quarantines()?
            .into_iter()
            .filter(|record| record.table == table)
            .map(|record| record.id)
            .collect();
        let mut records = Vec::with_capacity(rows.len());
        for (id, body) in rows {
            match decode_record(&id, &body, &identity) {
                Ok(record) => {
                    if quarantined.contains(&id) {
                        self.clear_quarantine(table, &id)?;
                    }
                    records.push(record);
                }
                Err(reason) => {
                    self.quarantine_record(table, &id, &reason)?;
                }
            }
        }
        Ok(records)
    }

    pub(super) fn record_one<T: DeserializeOwned>(
        &self,
        table: &str,
        sql: &str,
        id: &str,
        identity: impl Fn(&T) -> String,
    ) -> Result<Option<T>> {
        let body = self
            .read_conn()
            .query_row(sql, [id], |row| {
                Ok(row.get::<_, String>(0).map_err(|error| error.to_string()))
            })
            .optional()?;
        let Some(body) = body else { return Ok(None) };
        match decode_record(id, &body, &identity) {
            Ok(record) => {
                if self.meta_quarantine_exists(table, id)? {
                    self.clear_quarantine(table, id)?;
                }
                Ok(Some(record))
            }
            Err(reason) => Err(StoreError::Corrupt(
                self.quarantine_record(table, id, &reason)?,
            )),
        }
    }

    fn meta_quarantine_exists(&self, table: &str, id: &str) -> Result<bool> {
        Ok(self.read_conn().query_row(
            "SELECT EXISTS(SELECT 1 FROM meta WHERE key=?1)",
            [format!("{PREFIX}{table}.{id}")],
            |row| row.get(0),
        )?)
    }

    fn clear_quarantine(&self, table: &str, id: &str) -> Result<()> {
        self.conn.lock().execute(
            "DELETE FROM meta WHERE key=?1",
            [format!("{PREFIX}{table}.{id}")],
        )?;
        Ok(())
    }

    pub(super) fn quarantine_record(&self, table: &str, id: &str, reason: &str) -> Result<String> {
        let record = RecordQuarantine {
            table: table.into(),
            id: id.into(),
            reason: format!(
                "Cannot read {table} record {id}: {reason}. Run `aether doctor` and `aether diagnose`. The original row is preserved."
            ),
        };
        let body = serde_json::to_string(&record)?;
        let key = format!("{PREFIX}{table}.{id}");
        let changed = self.conn.lock().execute(
            "INSERT INTO meta(key,value) VALUES (?1,?2) ON CONFLICT(key) DO UPDATE SET value=excluded.value WHERE value<>excluded.value",
            params![key, body],
        )?;
        if changed > 0 {
            tracing::error!(table, id, %reason, "record quarantined; run `aether doctor`");
        }
        Ok(record.reason)
    }

    pub(super) fn list_quarantines(&self) -> Result<Vec<RecordQuarantine>> {
        let rows = {
            let conn = self.read_conn();
            let mut statement = conn.prepare(
                "SELECT key, value FROM meta WHERE key LIKE 'quarantine.%' ORDER BY key",
            )?;
            statement
                .query_map([], |row| {
                    let key: String = row.get(0)?;
                    let body = row.get::<_, String>(1).map_err(|error| error.to_string());
                    Ok((key, body))
                })?
                .collect::<rusqlite::Result<Vec<_>>>()?
        };
        let mut notices = Vec::new();
        for (key, body) in rows {
            if self.clear_removed_quarantine(&key)? {
                continue;
            }
            let decoded =
                body.and_then(|body| serde_json::from_str(&body).map_err(|e| e.to_string()));
            notices.push(decoded.unwrap_or_else(|reason| {
                let (table, id) = key.strip_prefix(PREFIX).unwrap_or(&key)
                    .split_once('.').unwrap_or(("meta", key.as_str()));
                RecordQuarantine {
                    table: table.into(), id: id.into(),
                    reason: format!("Quarantine notice {key} is unreadable: {reason}; run `aether doctor` and `aether diagnose`. The original record remains preserved."),
                }
            }));
        }
        Ok(notices)
    }

    fn clear_removed_quarantine(&self, key: &str) -> Result<bool> {
        use aether_core::id::{AgentId, TaskId};

        let Some((table, id)) = key.strip_prefix(PREFIX).and_then(|key| key.split_once('.')) else {
            return Ok(false);
        };
        let column = match table {
            "agents" if id.parse::<AgentId>().is_ok() => "id",
            "tasks" if id.parse::<TaskId>().is_ok() => "id",
            "leases" if id.parse::<AgentId>().is_ok() => "agent_id",
            _ => return Ok(false),
        };
        let changed = self.conn.lock().execute(
            &format!(
                "DELETE FROM meta WHERE key=?1 AND NOT EXISTS (SELECT 1 FROM {table} WHERE {column}=?2)"
            ),
            params![key, id],
        )?;
        Ok(changed > 0)
    }
}

fn decode_record<T: DeserializeOwned>(
    id: &str,
    body: &std::result::Result<String, String>,
    identity: &impl Fn(&T) -> String,
) -> std::result::Result<T, String> {
    let body = body.as_ref().map_err(Clone::clone)?;
    let record = serde_json::from_str(body).map_err(|error| error.to_string())?;
    if identity(&record) != id {
        return Err("the stored identity disagrees with the row key".into());
    }
    Ok(record)
}
