use super::write_tally::WriteTally;
use super::{SqliteStore, fts_query, nested_scope_range, scope_key};
use crate::{MemoryQuery, Result, StoreError};
use aether_core::id::MemoryId;
use aether_core::memory::{MemoryRecord, MemoryRecorder};
use aether_core::memory_admission::{MemoryAdmission, MemoryState};
use rusqlite::{OptionalExtension, Transaction, params};

impl SqliteStore {
    pub(super) fn query_memories(&self, query: &MemoryQuery) -> Result<Vec<MemoryRecord>> {
        let mut args: Vec<Box<dyn rusqlite::ToSql>> = Vec::new();
        let mut sql = if let Some(text) = query.text.as_deref().filter(|t| !t.trim().is_empty()) {
            args.push(Box::new(fts_query(text)));
            String::from(
                "SELECT m.body FROM memories m JOIN memories_fts f ON f.rowid = m.rowid
                 WHERE memories_fts MATCH ?1",
            )
        } else {
            String::from("SELECT m.body FROM memories m WHERE 1 = 1")
        };
        scope_filters(query, &mut sql, &mut args);
        args.push(Box::new(query.proposed));
        sql.push_str(&format!(" AND m.proposed = ?{}", args.len()));
        if !query.include_superseded {
            sql.push_str(" AND m.superseded_by IS NULL");
        }
        if !query.include_forgotten {
            sql.push_str(" AND m.forgotten_at IS NULL");
        }
        if !query.include_candidates {
            sql.push_str(
                " AND NOT EXISTS (SELECT 1 FROM memory_admission a
                    WHERE a.memory_id = m.id AND a.state = 'candidate')",
            );
        }
        content_filters(query, &mut sql, &mut args)?;
        // Pinned first either way: a person put it there on purpose.
        if query.text.is_some() {
            sql.push_str(" ORDER BY m.pinned DESC, bm25(memories_fts) ASC, m.importance DESC");
        } else {
            sql.push_str(" ORDER BY m.pinned DESC, m.importance DESC, m.created_at DESC");
        }
        args.push(Box::new(memory_limit(query.limit)));
        sql.push_str(&format!(" LIMIT ?{}", args.len()));
        let conn = self.read_conn()?;
        let refs: Vec<&dyn rusqlite::ToSql> = args.iter().map(|a| a.as_ref()).collect();
        let records: Vec<MemoryRecord> = Self::read_many(&conn, &sql, &refs)?;
        if let Some(record) = records.iter().find(|record| {
            record.is_proposed() != query.proposed
                || (!query.include_forgotten && record.forgotten_at.is_some())
        }) {
            return Err(StoreError::Corrupt(format!(
                "memory {} body disagrees with its indexed state; run `aether doctor`",
                record.id
            )));
        }
        Ok(records)
    }

    pub(super) fn read_memory_admission(&self, id: MemoryId) -> Result<Option<MemoryAdmission>> {
        let conn = self.read_conn()?;
        let row: Option<(String, String)> = conn
            .query_row(
                "SELECT state, body FROM memory_admission WHERE memory_id = ?1",
                [id.to_string()],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .optional()?;
        let Some((state, body)) = row else {
            return Ok(None);
        };
        let admission: MemoryAdmission = serde_json::from_str(&body)?;
        if admission.memory != id || admission.state.as_str() != state {
            return Err(StoreError::Corrupt(format!(
                "memory {id} admission body disagrees with its indexed state; run `aether doctor`"
            )));
        }
        Ok(Some(admission))
    }
}

/// The store keeps only whether a memory is still a candidate: superseded
/// and retracted are the record's to say.
pub(super) fn put_memory_admission(
    tx: &Transaction<'_>,
    admission: &MemoryAdmission,
    tally: &mut WriteTally,
) -> Result<()> {
    if !matches!(
        admission.state,
        MemoryState::Candidate | MemoryState::Active
    ) {
        return Err(StoreError::Corrupt(format!(
            "memory {} admission cannot be stored as {}",
            admission.memory,
            admission.state.as_str()
        )));
    }
    let memory = admission.memory.to_string();
    let stored: bool = tx.query_row(
        "SELECT EXISTS(SELECT 1 FROM memories WHERE id = ?1)",
        [&memory],
        |row| row.get(0),
    )?;
    if !stored {
        return Err(StoreError::Corrupt(format!(
            "admission for memory {memory}, which is not stored"
        )));
    }
    let written = tx.execute(
        "INSERT INTO memory_admission(memory_id, state, tier, body) VALUES (?1, ?2, ?3, ?4)
         ON CONFLICT(memory_id) DO UPDATE SET state = excluded.state, tier = excluded.tier,
           body = excluded.body",
        params![
            memory,
            admission.state.as_str(),
            admission.tier.as_str(),
            tally.json("memory_admission", admission)?,
        ],
    )?;
    debug_assert_eq!(written, 1);
    Ok(())
}

fn scope_filters(query: &MemoryQuery, sql: &mut String, args: &mut Vec<Box<dyn rusqlite::ToSql>>) {
    if let Some(scope) = &query.scope {
        args.push(Box::new(scope_key(scope)));
        let mut matches = vec![format!("m.scope = ?{}", args.len())];
        if !query.exact_scope {
            matches.insert(0, "m.scope = 'global'".to_owned());
        }
        if query.nested_scopes
            && let Some((lower, upper)) = nested_scope_range(scope)
        {
            args.push(Box::new(lower));
            args.push(Box::new(upper));
            matches.push(format!(
                "(m.scope >= ?{} AND m.scope < ?{})",
                args.len() - 1,
                args.len()
            ));
        }
        sql.push_str(&format!(" AND ({})", matches.join(" OR ")));
    }
    if let Some(agent) = query.provenance_agent {
        args.push(Box::new(agent.to_string()));
        sql.push_str(&format!(
            " AND json_extract(m.body, '$.provenance.agent') = ?{}",
            args.len()
        ));
    }
    if let Some(recorder) = query.recorded_by {
        sql.push_str(recorded_by_clause(recorder));
    }
}

fn content_filters(
    query: &MemoryQuery,
    sql: &mut String,
    args: &mut Vec<Box<dyn rusqlite::ToSql>>,
) -> Result<()> {
    if let Some(id) = &query.id {
        args.push(Box::new(id.to_string()));
        sql.push_str(&format!(" AND m.id = ?{}", args.len()));
    }
    if let Some(text) = &query.exact_text {
        args.push(Box::new(text.clone()));
        sql.push_str(&format!(" AND m.text = ?{}", args.len()));
    }
    if !query.kinds.is_empty() {
        let placeholders: Vec<String> = query
            .kinds
            .iter()
            .map(|k| {
                let key = serde_json::to_value(k)?;
                let key = key
                    .as_str()
                    .ok_or_else(|| crate::StoreError::Corrupt("MemoryKind is not a string".into()))?
                    .to_owned();
                args.push(Box::new(key));
                Ok(format!("?{}", args.len()))
            })
            .collect::<Result<_>>()?;
        sql.push_str(&format!(" AND m.kind IN ({})", placeholders.join(",")));
    }
    for tag in &query.tags {
        args.push(Box::new(tag.clone()));
        sql.push_str(&format!(
            " AND EXISTS (SELECT 1 FROM json_each(m.body, '$.tags') WHERE value = ?{})",
            args.len()
        ));
    }
    Ok(())
}

/// Must match [`MemoryRecorder::recorded`]. `Actor` serializes with its
/// variant in an `actor` tag, so an imported operator's tag is `imported`.
fn recorded_by_clause(recorder: MemoryRecorder) -> &'static str {
    match recorder {
        MemoryRecorder::Operator => {
            " AND json_extract(m.body, '$.provenance.actor.actor') = 'user'"
        }
    }
}

fn memory_limit(limit: usize) -> i64 {
    i64::try_from(limit).unwrap_or(i64::MAX)
}

#[cfg(test)]
mod tests {
    use super::memory_limit;

    #[test]
    fn oversized_memory_limits_cannot_select_sqlites_unlimited_mode() {
        assert_eq!(memory_limit(0), 0);
        assert_eq!(memory_limit(5_001), 5_001);
        assert!(memory_limit(usize::MAX) >= 0);
        #[cfg(target_pointer_width = "64")]
        {
            assert_eq!(memory_limit(i64::MAX as usize), i64::MAX);
            assert_eq!(memory_limit(i64::MAX as usize + 1), i64::MAX);
            assert_eq!(memory_limit(usize::MAX), i64::MAX);
        }
    }
}
