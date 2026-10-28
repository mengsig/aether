//! A run judged outside the commit hold, and the recheck that lets expiry
//! delete it under the hold by reading only the rows written since.

use super::*;
use crate::sqlite::change_watch::{ChangeLog, Changes, Watch};

/// One run's expiry checks, made under no lock, and the writes since.
#[derive(Debug)]
pub struct HistoryScreen {
    run: RunId,
    before: Timestamp,
    blocker: Option<RunExpiry>,
    /// The run's agent ids at the screen, sorted.
    agents: Vec<String>,
    watch: Watch,
    /// The write connection's data version before the screen; a commit by
    /// another connection moves it, and that commit's rows are not watched.
    data_version: i64,
}

impl HistoryScreen {
    pub fn run(&self) -> RunId {
        self.run
    }

    /// Why the run could not expire at the screen, or `None` when it could.
    /// `None` promises nothing: expiry checks the writes since.
    pub fn blocker(&self) -> Option<&RunExpiry> {
        self.blocker.as_ref()
    }
}

impl SqliteStore {
    /// [`Self::expire_run_history`]'s checks on a connection of its own, under
    /// no lock, so a run that cannot expire is passed over without stalling
    /// writers or readers.
    pub fn screen_run_expiry(&self, run: RunId, before: Timestamp) -> Result<HistoryScreen> {
        let (watch, data_version) = self.watch_writes()?;
        let id = run.to_string();
        let (blocker, agents) = self.read_history(|tx| {
            let blocker = expiry_blocker(tx, &id, before, &self.guard_passes)?;
            Ok((blocker, agent_ids(tx, &id)?))
        })?;
        debug_assert!(
            agents.is_empty() == (blocker == Some(RunExpiry::Missing)),
            "a run is missing exactly when it has no agents"
        );
        Ok(HistoryScreen {
            run,
            before,
            blocker,
            agents,
            watch,
            data_version,
        })
    }

    /// Start watching before the screen's snapshot: every later commit is then
    /// in the snapshot, in the watch, or (from another connection) moves the version.
    /// Never waits for the write connection; a busy one only widens the watch.
    fn watch_writes(&self) -> Result<(Watch, i64)> {
        let Some(idle) = self.conn.try_lock() else {
            let watch = ChangeLog::watch(&self.changes, true);
            return Ok((watch, self.data_version.load(Ordering::Acquire)));
        };
        let watch = ChangeLog::watch(&self.changes, false);
        let version: i64 = idle.query_row("PRAGMA data_version", [], |row| row.get(0))?;
        self.data_version.store(version, Ordering::Release);
        Ok((watch, version))
    }

    /// Expire a run `screen` found eligible, under the caller's runtime write
    /// hold. Only rows written since the screen are judged again, unless the
    /// run's own members changed, when every guard runs as in a full expiry.
    pub fn expire_screened_run(
        &self,
        screen: &HistoryScreen,
        _hold: &Hold<'_>,
    ) -> Result<RunExpiry> {
        assert!(
            screen.watch.of(&self.changes),
            "a screen expires on the store that made it"
        );
        if let Some(kept) = &screen.blocker {
            return Ok(kept.clone());
        }
        let mut conn = self.conn.lock();
        let tx = conn.transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)?;
        let id = screen.run.to_string();
        if let Some(kept) = self.recheck(&tx, screen, &id)? {
            return Ok(kept);
        }
        self.delete_run(tx, &id)
    }

    /// The recheck [`Self::expire_screened_run`] makes, without deleting.
    #[cfg(test)]
    pub(crate) fn recheck_screened(&self, screen: &HistoryScreen) -> Result<Option<RunExpiry>> {
        if let Some(kept) = &screen.blocker {
            return Ok(Some(kept.clone()));
        }
        let mut conn = self.conn.lock();
        let tx = conn.transaction()?;
        self.recheck(&tx, screen, &screen.run.to_string())
    }

    fn recheck(
        &self,
        tx: &Transaction<'_>,
        screen: &HistoryScreen,
        id: &str,
    ) -> Result<Option<RunExpiry>> {
        let (agents, tasks) = run_members(tx, id)?;
        if let Some(kept) = state_blocker(&agents, &tasks, screen.before) {
            return Ok(Some(kept));
        }
        let mut ids: Vec<String> = agents.iter().map(|agent| agent.id.to_string()).collect();
        ids.sort_unstable();
        let version: i64 = tx.query_row("PRAGMA data_version", [], |row| row.get(0))?;
        self.data_version.store(version, Ordering::Release);
        let reason = match screen.watch.changes() {
            Some(changes)
                if ids == screen.agents
                    && version == screen.data_version
                    && !member_written(tx, id, &changes)? =>
            {
                written_protection(tx, id, &changes)?
            }
            _ => protection(tx, id, screen.before, &self.guard_passes)?,
        };
        Ok(reason.map(RunExpiry::Protected))
    }
}

fn agent_ids(tx: &Transaction<'_>, run: &str) -> Result<Vec<String>> {
    let mut statement = tx.prepare("SELECT id FROM agents WHERE run=?1 ORDER BY id")?;
    let ids = statement
        .query_map([run], |row| row.get(0))?
        .collect::<rusqlite::Result<Vec<String>>>()?;
    Ok(ids)
}

fn rowids(rows: &[i64]) -> Result<String> {
    debug_assert!(!rows.is_empty(), "only written tables are judged again");
    Ok(serde_json::to_string(rows)?)
}

/// Whether a row written since the screen is now one of the run's members,
/// which every guard compares against: an unchanged row may then match.
fn member_written(tx: &Transaction<'_>, run: &str, changes: &Changes) -> Result<bool> {
    for (table, member) in guards::RUN_MEMBERS {
        let Some(rows) = changes.rows(table) else {
            continue;
        };
        let sql = format!(
            "SELECT EXISTS(SELECT 1 FROM {table} WHERE rowid IN (SELECT value FROM json_each(?2)) AND {member})"
        );
        if tx.query_row(&sql, params![run, rowids(rows)?], |row| row.get(0))? {
            return Ok(true);
        }
    }
    Ok(false)
}

/// The first guard, in the full check's order, that a row written since the
/// screen now trips. Rows the screen read unchanged still trip none.
fn written_protection(
    tx: &Transaction<'_>,
    run: &str,
    changes: &Changes,
) -> Result<Option<&'static str>> {
    for guard in guards::GUARDS {
        let Some(rows) = changes.rows(guard.table) else {
            continue;
        };
        let tripped: bool = tx.query_row(
            &guards::written(guard),
            params![run, rowids(rows)?],
            |row| row.get(0),
        )?;
        if tripped {
            return Ok(Some(guard.reason));
        }
    }
    Ok(None)
}
