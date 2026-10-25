use super::SqliteStore;
use super::write_tally::WriteTally;
use crate::{Result, StoreError};
use aether_core::id::{MemoryFeedbackId, MemoryId, TaskId};
use aether_core::memory_feedback::{
    FeedbackTally, MAX_FEEDBACK_REASON_CHARS, MemoryEvidence, MemoryFeedback, MemoryVerdict,
    RANKING_WINDOW,
};
use aether_core::time::Timestamp;
use rusqlite::{Transaction, params};
use std::collections::BTreeMap;

/// [`FeedbackTally::of`] in one pass over `memory_feedback_ranked`:
/// each task's latest counted verdict, then each memory's most recent tasks.
pub(super) const TALLIES: &str = "WITH counted AS (
        SELECT memory_id, created_at, id, verdict,
               ROW_NUMBER() OVER (PARTITION BY memory_id, task_id
                                  ORDER BY created_at DESC, id DESC) AS task_rank
        FROM memory_feedback
        WHERE memory_id IN (SELECT value FROM json_each(?1))
          AND retracted_at IS NULL
          AND evidence IN ('operator','review')
    ), latest AS (
        SELECT memory_id, verdict,
               ROW_NUMBER() OVER (PARTITION BY memory_id
                                  ORDER BY created_at DESC, id DESC) AS recency
        FROM counted WHERE task_rank = 1
    )
    SELECT memory_id, SUM(verdict = 'helped'), SUM(verdict = 'hurt')
    FROM latest WHERE recency <= ?2 GROUP BY memory_id";

impl SqliteStore {
    pub(super) fn task_context_contains(
        &self,
        memory: MemoryId,
        task: TaskId,
        through: Option<Timestamp>,
    ) -> Result<bool> {
        let conn = self.read_conn();
        let found: bool = conn.query_row(
            "SELECT EXISTS(SELECT 1 FROM events AS e INDEXED BY events_task
             JOIN snapshots AS s ON s.id = json_extract(e.body, '$.kind.snapshot')
             JOIN json_each(e.body, '$.kind.layers') AS layer
             WHERE e.task_id = ?1 AND e.type = 'snapshot.created'
               AND (?3 IS NULL OR s.created_at <= ?3)
               AND json_extract(layer.value, '$.kind') = 'retrieved_memory'
               AND CASE WHEN json_type(layer.value, '$.memories') IS NULL
                         AND json_extract(layer.value, '$.items') > 0
                   THEN EXISTS(SELECT 1 FROM json_each(s.body, '$.memory_ids') AS m
                               WHERE m.value = ?2)
                   ELSE EXISTS(SELECT 1 FROM json_each(layer.value, '$.memories') AS m
                               WHERE m.value = ?2)
                   END)",
            params![
                task.to_string(),
                memory.to_string(),
                through.map(|at| at.to_rfc3339())
            ],
            |row| row.get(0),
        )?;
        Ok(found)
    }

    pub(super) fn latest_memory_feedback(
        &self,
        memory: MemoryId,
        limit: usize,
    ) -> Result<Vec<MemoryFeedback>> {
        let conn = self.read_conn();
        let records: Vec<MemoryFeedback> = Self::read_many(
            &conn,
            "SELECT body FROM memory_feedback WHERE memory_id = ?1
             ORDER BY created_at DESC, id DESC LIMIT ?2",
            &[
                &memory.to_string(),
                &i64::try_from(limit).unwrap_or(i64::MAX),
            ],
        )?;
        debug_assert!(records.len() <= limit);
        if let Some(record) = records.iter().find(|record| record.spec.memory != memory) {
            return Err(StoreError::Corrupt(format!(
                "feedback {} body disagrees with its indexed memory; run `aether doctor`",
                record.id
            )));
        }
        Ok(records)
    }

    pub(super) fn memory_feedback_tallies_for(
        &self,
        memories: &[MemoryId],
    ) -> Result<BTreeMap<MemoryId, FeedbackTally>> {
        if memories.is_empty() {
            return Ok(BTreeMap::new());
        }
        let ids: Vec<String> = memories.iter().map(ToString::to_string).collect();
        let params = params![serde_json::to_string(&ids)?, RANKING_WINDOW as i64];
        let conn = self.read_conn();
        let mut stmt = conn.prepare_cached(TALLIES)?;
        let rows = stmt.query_map(params, |row| {
            Ok((row.get::<_, String>(0)?, row.get(1)?, row.get(2)?))
        })?;
        let mut tallies = BTreeMap::new();
        for row in rows {
            let (id, helped, hurt) = row?;
            let id: MemoryId = id
                .parse()
                .map_err(|e| StoreError::Corrupt(format!("invalid feedback memory id: {e}")))?;
            debug_assert!(memories.contains(&id), "{id} was not asked for");
            tallies.insert(id, FeedbackTally { helped, hurt });
        }
        debug_assert!(tallies.len() <= memories.len());
        debug_assert!(
            tallies
                .values()
                .all(|t| (t.helped + t.hurt) as usize <= RANKING_WINDOW)
        );
        Ok(tallies)
    }

    pub(super) fn insert_memory_feedback(
        tx: &Transaction<'_>,
        record: &MemoryFeedback,
        tally: &mut WriteTally,
    ) -> Result<()> {
        if record.retracted_at.is_some() || record.retraction_reason.is_some() {
            return Err(StoreError::Corrupt(format!(
                "cannot insert already retracted feedback {}; run `aether doctor`",
                record.id
            )));
        }
        let task_id = record.spec.task.to_string();
        let verdict = match record.spec.verdict {
            MemoryVerdict::Helped => "helped",
            MemoryVerdict::Hurt => "hurt",
            MemoryVerdict::Neutral => "neutral",
        };
        let evidence = match record.spec.evidence {
            MemoryEvidence::Operator => "operator",
            MemoryEvidence::Review => "review",
            MemoryEvidence::Outcome => "outcome",
        };
        let body = tally.json("memory_feedback", record)?;
        #[cfg(debug_assertions)]
        {
            let fields: serde_json::Value = serde_json::from_str(&body)?;
            debug_assert_eq!(fields["task"], task_id);
            debug_assert_eq!(fields["verdict"], verdict);
            debug_assert_eq!(fields["evidence"], evidence);
        }
        tx.execute(
            "INSERT INTO memory_feedback(id, memory_id, task_id, verdict, evidence, created_at, body)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
            params![
                record.id.to_string(),
                record.spec.memory.to_string(),
                task_id,
                verdict,
                evidence,
                record.created_at.to_rfc3339(),
                body
            ],
        )?;
        Ok(())
    }

    pub(super) fn retract_memory_feedback(
        tx: &Transaction<'_>,
        id: MemoryFeedbackId,
        at: Timestamp,
        reason: &str,
    ) -> Result<()> {
        if reason.trim().is_empty() || reason.chars().count() > MAX_FEEDBACK_REASON_CHARS {
            return Err(StoreError::Corrupt(format!(
                "feedback {id} retraction needs a nonempty reason of at most {MAX_FEEDBACK_REASON_CHARS} characters; run `aether doctor`"
            )));
        }
        let changed = tx.execute(
            "UPDATE memory_feedback SET retracted_at = ?1,
             body = json_set(body, '$.retracted_at', ?1, '$.retraction_reason', ?2)
             WHERE id = ?3 AND retracted_at IS NULL",
            params![at.to_rfc3339(), reason, id.to_string()],
        )?;
        if changed != 1 {
            return Err(StoreError::Corrupt(format!(
                "feedback {id} missing or already retracted"
            )));
        }
        Ok(())
    }
}
