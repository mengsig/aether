//! Reads for Memory 2.0's wire commands: the retrieval trace a snapshot
//! recorded, and the counts `memory_migration_status` reports.

use super::SqliteStore;
use crate::{MemoryV2Counts, Result, StoreError};
use aether_core::id::SnapshotId;
use aether_core::memory_v2::RetrievalTrace;

impl SqliteStore {
    pub(super) fn read_snapshot_retrievals(
        &self,
        ids: &[SnapshotId],
    ) -> Result<Vec<(SnapshotId, Option<RetrievalTrace>)>> {
        if ids.is_empty() {
            return Ok(Vec::new());
        }
        // Only the trace is extracted: the body also holds the message tail.
        let sql = "SELECT s.id, json_extract(s.body, '$.memory_retrieval') \
                   FROM json_each(?1) AS wanted JOIN snapshots AS s ON s.id = wanted.value \
                   ORDER BY wanted.key";
        let conn = self.read_conn()?;
        let mut stmt = conn.prepare(sql)?;
        let rows = stmt.query_map([serde_json::to_string(ids)?], |row| {
            Ok((row.get::<_, String>(0)?, row.get::<_, Option<String>>(1)?))
        })?;
        let mut found = Vec::with_capacity(ids.len());
        for row in rows {
            let (id, trace) = row?;
            let id: SnapshotId = id
                .parse()
                .map_err(|_| StoreError::Corrupt(format!("snapshot id `{id}` does not parse")))?;
            let trace = trace
                .map(|body| serde_json::from_str::<RetrievalTrace>(&body))
                .transpose()?;
            found.push((id, trace));
        }
        debug_assert!(found.len() <= ids.len());
        debug_assert!(found.iter().all(|(id, _)| ids.contains(id)));
        Ok(found)
    }

    pub(super) fn count_memory_v2(&self) -> Result<MemoryV2Counts> {
        let conn = self.read_conn()?;
        let count = |sql: &str| -> Result<u64> {
            let n: i64 = conn.query_row(sql, [], |row| row.get(0))?;
            u64::try_from(n).map_err(|_| StoreError::Corrupt(format!("negative count from `{sql}`")))
        };
        let total = count("SELECT COUNT(*) FROM memories")?;
        let admitted = count(
            "SELECT COUNT(*) FROM memory_admission a WHERE EXISTS \
             (SELECT 1 FROM memories m WHERE m.id = a.memory_id)",
        )?;
        let counts = MemoryV2Counts {
            total,
            admitted,
            implicit: total.saturating_sub(admitted),
            candidates: count("SELECT COUNT(*) FROM memory_admission WHERE state = 'candidate'")?,
            stale: count("SELECT COUNT(*) FROM memory_stale")?,
            held_out: count("SELECT COUNT(*) FROM memory_holdouts")?,
        };
        debug_assert!(counts.admitted <= counts.total, "an admission row names its record");
        debug_assert!(counts.candidates <= counts.admitted);
        Ok(counts)
    }
}
