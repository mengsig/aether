//! Reads `status` and the GitHub watcher make on every poll, kept off the
//! wide tables: the fleet's and a run's usage from the totals V33 keeps,
//! counts by index, and meta keys from one snapshot, not under the commit lock.

use std::collections::BTreeMap;

use aether_core::budget::RunTotals;
use aether_core::id::{AgentId, RunId};
use aether_core::model::Usage;
use aether_core::task::TaskStatusKind;
use rusqlite::OptionalExtension;

use super::SqliteStore;
use super::usage_totals::{FLEET, read_agent_usages, read_run_start, read_totals};
use crate::{Result, StoreError};

/// Tasks of the statuses bound as a JSON list, by the status index. One
/// quarantined as unreadable is left out, as listing the tasks leaves it out.
pub(super) const COUNT_TASKS_SQL: &str = "SELECT COUNT(*) FROM tasks t
     WHERE t.status IN (SELECT value FROM json_each(?1))
     AND NOT EXISTS (SELECT 1 FROM meta m WHERE m.key = 'quarantine.tasks.' || t.id)";

pub(super) const COUNT_PENDING_APPROVALS_SQL: &str =
    "SELECT COUNT(*) FROM approvals WHERE state = 'pending'";

impl SqliteStore {
    pub(super) fn count_tasks_of(&self, statuses: &[TaskStatusKind]) -> Result<u64> {
        assert!(!statuses.is_empty(), "a count names the statuses it counts");
        let wanted: Vec<&str> = statuses.iter().map(|status| status.as_str()).collect();
        let wanted = serde_json::to_string(&wanted)?;
        let count: i64 = self
            .read_conn()?
            .query_row(COUNT_TASKS_SQL, [wanted], |row| row.get(0))?;
        assert!(count >= 0, "a count is never negative");
        Ok(count as u64)
    }

    pub(super) fn count_pending(&self) -> Result<u64> {
        let count: i64 = self
            .read_conn()?
            .query_row(COUNT_PENDING_APPROVALS_SQL, [], |row| row.get(0))?;
        assert!(count >= 0, "a count is never negative");
        Ok(count as u64)
    }

    /// Every non-imported agent's usage, from the fleet's one totals row.
    pub(super) fn read_fleet_usage(&self) -> Result<Usage> {
        let (members, usage) = read_totals(&*self.read_conn()?, FLEET)?;
        if members == 0 && usage != Usage::default() {
            return Err(StoreError::Corrupt(
                "the fleet's usage totals bill an empty fleet".into(),
            ));
        }
        debug_assert!(usage.cost_usd.is_finite(), "a sum of finite costs");
        Ok(usage)
    }

    /// `run`'s kept totals and its first agent's creation, both by index and
    /// from one snapshot, so a commit between the two reads cannot split them.
    pub(super) fn read_run_totals(&self, run: RunId) -> Result<Option<RunTotals>> {
        let key = run.to_string();
        let mut conn = self.read_conn()?;
        let tx = conn.transaction()?;
        let (members, usage) = read_totals(&tx, &key)?;
        let started = read_run_start(&tx, &key)?;
        tx.commit()?;
        let started = match (members, started) {
            (0, None) => return Ok(None),
            (1.., Some(started)) => started,
            (members, started) => {
                return Err(StoreError::Corrupt(format!(
                    "run {run}'s totals count {members} agents but its first is {started:?}"
                )));
            }
        };
        let started_at = chrono::DateTime::parse_from_rfc3339(&started)
            .map_err(|_| StoreError::Corrupt(format!("run {run} has an unreadable created_at")))?
            .with_timezone(&chrono::Utc);
        Ok(Some(RunTotals {
            agents: u32::try_from(members).unwrap_or(u32::MAX),
            usage,
            started_at,
        }))
    }

    pub(super) fn read_agent_usages_by_id(
        &self,
        agents: &[AgentId],
    ) -> Result<BTreeMap<AgentId, Usage>> {
        read_agent_usages(&*self.read_conn()?, agents)
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
