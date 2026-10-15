use super::SqliteStore;
use crate::{Result, StoreError};
use aether_core::id::{MemoryFeedbackId, MemoryId, TaskId};
use aether_core::memory_feedback::{MAX_FEEDBACK_REASON_CHARS, MemoryFeedback};
use aether_core::time::Timestamp;
use rusqlite::{Transaction, params};

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
            &[&memory.to_string(), &(limit as i64)],
        )?;
        debug_assert!(records.len() <= limit);
        debug_assert!(records.iter().all(|record| record.spec.memory == memory));
        Ok(records)
    }

    pub(super) fn insert_memory_feedback(
        tx: &Transaction<'_>,
        record: &MemoryFeedback,
    ) -> Result<()> {
        debug_assert!(record.retracted_at.is_none());
        debug_assert!(record.retraction_reason.is_none());
        tx.execute(
            "INSERT INTO memory_feedback(id, memory_id, created_at, body) VALUES (?1, ?2, ?3, ?4)",
            params![
                record.id.to_string(),
                record.spec.memory.to_string(),
                record.created_at.to_rfc3339(),
                serde_json::to_string(record)?
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
        debug_assert!(!reason.trim().is_empty());
        debug_assert!(reason.chars().count() <= MAX_FEEDBACK_REASON_CHARS);
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
