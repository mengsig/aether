//! Preserve malformed records in place while making list reads usable.

use rusqlite::params;
use serde::de::DeserializeOwned;

use super::SqliteStore;
use crate::{RecordQuarantine, Result};

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
            statement.query_map(params, |row| {
                Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
            })?.collect::<rusqlite::Result<Vec<_>>>()?
        };
        let quarantined: std::collections::BTreeSet<_> = self.list_quarantines()?.into_iter()
            .filter(|record| record.table == table).map(|record| record.id).collect();
        let mut records = Vec::with_capacity(rows.len());
        for (id, body) in rows {
            let decoded = serde_json::from_str::<T>(&body)
                .map_err(|error| error.to_string())
                .and_then(|record| {
                    if identity(&record) == id { Ok(record) }
                    else { Err("the stored identity disagrees with the row key".into()) }
                });
            match decoded {
                Ok(record) => {
                    if quarantined.contains(&id) {
                        self.conn.lock().execute("DELETE FROM meta WHERE key=?1", [format!("{PREFIX}{table}.{id}")])?;
                    }
                    records.push(record);
                }
                Err(reason) => self.quarantine_record(table, &id, &reason)?,
            }
        }
        Ok(records)
    }

    pub(super) fn quarantine_record(&self, table: &str, id: &str, reason: &str) -> Result<()> {
        let record = RecordQuarantine {
            table: table.into(),
            id: id.into(),
            reason: format!("Cannot read {table} record {id}: {reason}. Run `aether doctor` and `aether diagnose`. The original row is preserved."),
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
        Ok(())
    }

    pub(super) fn list_quarantines(&self) -> Result<Vec<RecordQuarantine>> {
        Self::read_many(&self.read_conn(), "SELECT value FROM meta WHERE key LIKE 'quarantine.%' ORDER BY key", &[])
    }
}
