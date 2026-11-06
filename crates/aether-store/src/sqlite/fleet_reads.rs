//! Reads `status` and the GitHub watcher make on every poll, kept off the
//! wide tables: the fleet's usage from `agent_usage` (V33), and several meta
//! keys from one snapshot instead of under the commit lock.

use aether_core::model::Usage;
use rusqlite::OptionalExtension;

use super::SqliteStore;
use crate::Result;

impl SqliteStore {
    /// Every non-imported agent's usage, summed. A usage that does not decode
    /// quarantines its agent and is left out, as the whole-row listing did.
    pub(super) fn read_fleet_usage(&self) -> Result<Usage> {
        let rows: Vec<(String, Option<String>)> = {
            let conn = self.read_conn()?;
            let mut statement = conn.prepare_cached("SELECT agent_id, usage FROM agent_usage")?;
            statement
                .query_map([], |row| Ok((row.get(0)?, row.get(1)?)))?
                .collect::<rusqlite::Result<_>>()?
        };
        let mut total = Usage::default();
        for (agent, usage) in rows {
            assert!(!agent.is_empty(), "agent_usage is keyed by agent id");
            let Some(usage) = usage else {
                total += Usage::default();
                continue;
            };
            match serde_json::from_str::<Usage>(&usage) {
                Ok(usage) => total += usage,
                Err(error) => {
                    self.quarantine_record("agents", &agent, &format!("usage: {error}"))?;
                }
            }
        }
        debug_assert!(total.cost_usd.is_finite(), "a sum of finite costs");
        Ok(total)
    }

    /// The value of each of `keys`, in order, all read from one snapshot.
    pub(super) fn read_metas(&self, keys: &[String]) -> Result<Vec<Option<String>>> {
        let mut conn = self.read_conn()?;
        let tx = conn.transaction()?;
        let mut values = Vec::with_capacity(keys.len());
        {
            let mut statement = tx.prepare_cached("SELECT value FROM meta WHERE key = ?1")?;
            for key in keys {
                values.push(statement.query_row([key], |row| row.get(0)).optional()?);
            }
        }
        tx.commit()?;
        assert_eq!(values.len(), keys.len(), "one value per key");
        Ok(values)
    }
}
