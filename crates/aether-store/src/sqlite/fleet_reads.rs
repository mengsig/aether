//! Reads `status` and the GitHub watcher make on every poll, kept off the
//! wide tables: the fleet's usage from `agent_usage` (V33), and several meta
//! keys from one snapshot instead of under the commit lock.

use aether_core::agent::AgentStatusKind;
use aether_core::budget::RunMember;
use aether_core::id::RunId;
use aether_core::model::Usage;
use rusqlite::OptionalExtension;

use super::SqliteStore;
use crate::{Result, StoreError};

/// A run's agents by `agents_run_id`, each joined to its `agent_usage` row. A
/// body unreadable at its last write has no row; the agent reads flag it.
pub(super) const RUN_MEMBERS_SQL: &str = "SELECT a.id, a.status, a.created_at,
        COALESCE(u.usage, CASE WHEN a.status = 'imported' AND json_valid(a.body)
                          THEN json_extract(a.body, '$.stats.usage') END)
     FROM agents a LEFT JOIN agent_usage u ON u.agent_id = a.id
     WHERE a.run = ?1 AND (u.agent_id IS NOT NULL OR a.status = 'imported')";

/// One row of [`RUN_MEMBERS_SQL`] as a member; a column the store never
/// writes is corrupt.
fn run_member(
    run: RunId,
    id: &str,
    status: &str,
    created_at: &str,
    usage: Usage,
) -> Result<RunMember> {
    let corrupt =
        |what: &str| StoreError::Corrupt(format!("agent {id} has {what}; run `aether doctor`"));
    let id = id.parse().map_err(|_| corrupt("an invalid id"))?;
    let status = AgentStatusKind::ALL
        .iter()
        .find(|kind| kind.as_str() == status)
        .ok_or_else(|| corrupt("an unknown status"))?;
    let created_at = chrono::DateTime::parse_from_rfc3339(created_at)
        .map_err(|_| corrupt("an unreadable created_at"))?
        .with_timezone(&chrono::Utc);
    Ok(RunMember {
        id,
        run,
        live: !status.is_terminal(),
        created_at,
        usage,
    })
}

impl SqliteStore {
    /// Every non-imported agent's usage, summed; only the usage is decoded. A bad
    /// usage quarantines its agent; a body bad elsewhere is the agent reads' to flag.
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

    /// `run`'s agents as a run summary reads them, from their columns and
    /// `agent_usage`; an imported agent, which has no row there, from its body.
    pub(super) fn read_run_members(&self, run: RunId) -> Result<Vec<RunMember>> {
        let rows: Vec<(String, String, String, Option<String>)> = {
            let conn = self.read_conn()?;
            let mut statement = conn.prepare_cached(RUN_MEMBERS_SQL)?;
            statement
                .query_map([run.to_string()], |row| {
                    Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?))
                })?
                .collect::<rusqlite::Result<_>>()?
        };
        let mut members = Vec::with_capacity(rows.len());
        for (id, status, created_at, usage) in rows {
            let usage = match usage.map(|usage| serde_json::from_str::<Usage>(&usage)) {
                None => Usage::default(),
                Some(Ok(usage)) => usage,
                Some(Err(error)) => {
                    self.quarantine_record("agents", &id, &format!("usage: {error}"))?;
                    continue;
                }
            };
            members.push(run_member(run, &id, &status, &created_at, usage)?);
        }
        assert!(
            members.iter().all(|m| m.run == run),
            "members of the run asked for"
        );
        Ok(members)
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
