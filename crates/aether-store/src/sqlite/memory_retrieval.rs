//! What retrieval keeps beside the records (V31): stale marks, read once per
//! retrieval for the whole pool, and the log of withheld injections.

use super::SqliteStore;
use super::write_tally::WriteTally;
use crate::{MemoryHoldout, MemoryStaleMark, Result, StoreError};
use aether_core::id::MemoryId;
use aether_core::time::Timestamp;
use rusqlite::{Transaction, params};
use std::collections::BTreeMap;

const STALE_MARKS: &str = "SELECT memory_id, reason, since, revision FROM memory_stale
     WHERE memory_id IN (SELECT value FROM json_each(?1))";

const HOLDOUTS: &str = "SELECT memory_id, agent_id, task_id, at, rate, score
     FROM memory_holdouts WHERE memory_id = ?1 ORDER BY seq";

fn timestamp(raw: &str, what: &str) -> Result<Timestamp> {
    chrono::DateTime::parse_from_rfc3339(raw)
        .map(|at| at.with_timezone(&chrono::Utc))
        .map_err(|e| StoreError::Corrupt(format!("{what} timestamp `{raw}`: {e}")))
}

fn parsed<T: std::str::FromStr>(raw: &str, what: &str) -> Result<T>
where
    T::Err: std::fmt::Display,
{
    raw.parse()
        .map_err(|e| StoreError::Corrupt(format!("invalid {what} `{raw}`: {e}")))
}

impl SqliteStore {
    pub(super) fn memory_stale_marks_for(
        &self,
        memories: &[MemoryId],
    ) -> Result<BTreeMap<MemoryId, MemoryStaleMark>> {
        if memories.is_empty() {
            return Ok(BTreeMap::new());
        }
        let ids: Vec<String> = memories.iter().map(ToString::to_string).collect();
        let conn = self.read_conn()?;
        let mut stmt = conn.prepare_cached(STALE_MARKS)?;
        let rows = stmt.query_map(params![serde_json::to_string(&ids)?], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get(1)?,
                row.get::<_, String>(2)?,
                row.get::<_, Option<String>>(3)?,
            ))
        })?;
        let mut marks = BTreeMap::new();
        for row in rows {
            let (id, reason, since, revision) = row?;
            let memory: MemoryId = parsed(&id, "stale memory id")?;
            debug_assert!(memories.contains(&memory), "{memory} was not asked for");
            let since = timestamp(&since, "stale mark")?;
            marks.insert(
                memory,
                MemoryStaleMark {
                    memory,
                    reason,
                    since,
                    revision,
                },
            );
        }
        debug_assert!(marks.len() <= memories.len());
        Ok(marks)
    }

    pub(super) fn memory_holdouts_of(&self, memory: MemoryId) -> Result<Vec<MemoryHoldout>> {
        let conn = self.read_conn()?;
        let mut stmt = conn.prepare_cached(HOLDOUTS)?;
        type Raw = (String, String, Option<String>, String, f64, f64);
        let rows = stmt.query_map(params![memory.to_string()], |row| {
            Ok::<Raw, _>((
                row.get(0)?,
                row.get(1)?,
                row.get(2)?,
                row.get(3)?,
                row.get(4)?,
                row.get(5)?,
            ))
        })?;
        let mut logged = Vec::new();
        for row in rows {
            let (id, agent, task, at, rate, score) = row?;
            let holdout = MemoryHoldout {
                memory: parsed(&id, "holdout memory id")?,
                agent: parsed(&agent, "holdout agent id")?,
                task: task
                    .as_deref()
                    .map(|t| parsed(t, "holdout task id"))
                    .transpose()?,
                at: timestamp(&at, "holdout")?,
                rate: rate as f32,
                score: score as f32,
            };
            if holdout.memory != memory {
                return Err(StoreError::Corrupt(format!(
                    "holdout of {} filed under {memory}; run `aether doctor`",
                    holdout.memory
                )));
            }
            logged.push(holdout);
        }
        Ok(logged)
    }
}

fn require_stored(tx: &Transaction<'_>, memory: &str, what: &str) -> Result<()> {
    let stored: bool = tx.query_row(
        "SELECT EXISTS(SELECT 1 FROM memories WHERE id = ?1)",
        [memory],
        |row| row.get(0),
    )?;
    if !stored {
        return Err(StoreError::Corrupt(format!(
            "{what} for memory {memory}, which is not stored"
        )));
    }
    Ok(())
}

pub(super) fn mark_memory_stale(
    tx: &Transaction<'_>,
    mark: &MemoryStaleMark,
    tally: &mut WriteTally,
) -> Result<()> {
    if mark.reason.trim().is_empty() {
        return Err(StoreError::Corrupt(format!(
            "stale mark for memory {} has no reason",
            mark.memory
        )));
    }
    if mark
        .revision
        .as_deref()
        .is_some_and(|r| r.trim().is_empty())
    {
        return Err(StoreError::Corrupt(format!(
            "stale mark for memory {} has a blank revision",
            mark.memory
        )));
    }
    let memory = mark.memory.to_string();
    require_stored(tx, &memory, "stale mark")?;
    let written = tx.execute(
        "INSERT INTO memory_stale(memory_id, reason, since, revision) VALUES (?1, ?2, ?3, ?4)
         ON CONFLICT(memory_id) DO UPDATE SET reason = excluded.reason,
             since = excluded.since, revision = excluded.revision",
        params![memory, mark.reason, mark.since.to_rfc3339(), mark.revision],
    )?;
    debug_assert_eq!(written, 1);
    tally.row("memory_stale", mark.reason.len());
    Ok(())
}

pub(super) fn clear_memory_stale(tx: &Transaction<'_>, memory: MemoryId) -> Result<()> {
    let removed = tx.execute(
        "DELETE FROM memory_stale WHERE memory_id = ?1",
        [memory.to_string()],
    )?;
    debug_assert!(removed <= 1, "memory_id is the key");
    Ok(())
}

pub(super) fn log_memory_holdout(
    tx: &Transaction<'_>,
    holdout: &MemoryHoldout,
    tally: &mut WriteTally,
) -> Result<()> {
    let in_range = (0.0..=1.0).contains(&holdout.rate);
    if !in_range || !holdout.score.is_finite() {
        return Err(StoreError::Corrupt(format!(
            "holdout of memory {} at rate {} and score {}",
            holdout.memory, holdout.rate, holdout.score
        )));
    }
    let memory = holdout.memory.to_string();
    require_stored(tx, &memory, "holdout")?;
    let unit = holdout.unit();
    let written = tx.execute(
        "INSERT INTO memory_holdouts(memory_id, agent_id, task_id, unit, at, rate, score)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7) ON CONFLICT(unit, memory_id) DO NOTHING",
        params![
            memory,
            holdout.agent.to_string(),
            holdout.task.map(|t| t.to_string()),
            unit,
            holdout.at.to_rfc3339(),
            f64::from(holdout.rate),
            f64::from(holdout.score),
        ],
    )?;
    debug_assert!(written <= 1);
    tally.row("memory_holdouts", memory.len() + unit.len());
    Ok(())
}
