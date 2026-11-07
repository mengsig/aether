//! Agent pages in id order, and the subsystem fixed at an agent's creation.

use aether_core::agent::Agent;
use aether_core::id::AgentId;
use rusqlite::{OptionalExtension, Transaction, params};

use super::SqliteStore;
use super::event_queries::Arguments;
use super::quarantine::RecordRow;
use super::write_tally::WriteTally;
use crate::{AgentFilter, AgentPage, Result, StoreError};

impl SqliteStore {
    /// Ids and bodies come from one statement, and the cursor from the rows
    /// read, so a quarantined record shortens its page but never ends the walk.
    pub(super) fn page_agents(
        &self,
        filter: &AgentFilter,
        after: Option<AgentId>,
        limit: usize,
    ) -> Result<AgentPage> {
        assert!(limit > 0, "a page holds at least one agent");
        assert!(!filter.budgeted_only, "budget selection is not paged");
        let mut rows = self.page_rows(filter, after, limit)?;
        debug_assert!(rows.windows(2).all(|pair| pair[0].0 < pair[1].0));
        let next_after = if rows.len() > limit {
            Some(parse_id(&rows[limit - 1].0)?)
        } else {
            None
        };
        rows.truncate(limit);
        let agents: Vec<Agent> =
            self.decode_records("agents", rows, |agent: &Agent| agent.id.to_string())?;
        debug_assert!(agents.windows(2).all(|pair| pair[0].id < pair[1].id));
        debug_assert!(next_after.is_none_or(|next| after.is_none_or(|after| next > after)));
        debug_assert!(
            next_after.is_none_or(|next| agents.last().is_none_or(|last| last.id <= next)),
            "the cursor resumes after every agent returned"
        );
        Ok(AgentPage { agents, next_after })
    }

    /// Up to `limit + 1` rows past `after`: the extra one says another page follows.
    fn page_rows(
        &self,
        filter: &AgentFilter,
        after: Option<AgentId>,
        limit: usize,
    ) -> Result<Vec<RecordRow>> {
        let mut args: Arguments = Vec::new();
        let mut sql = format!(
            "SELECT id, body FROM agents WHERE {}",
            super::store::agent_predicate(filter, &mut args)
        );
        // `archived` lives in the body; here it must narrow the page itself. A
        // body that is not JSON stays in, so the decoder can quarantine it.
        if filter.exclude_archived {
            sql.push_str(
                " AND (CASE WHEN json_valid(body) THEN COALESCE(json_extract(body, '$.archived'), 0) ELSE 0 END) = 0",
            );
        }
        if let Some(after) = after {
            args.push(Box::new(after.to_string()));
            sql.push_str(&format!(" AND id > ?{}", args.len()));
        }
        args.push(Box::new((limit + 1) as i64));
        sql.push_str(&format!(" ORDER BY id ASC LIMIT ?{}", args.len()));
        debug_assert_eq!(sql.matches('?').count(), args.len());
        let refs: Vec<&dyn rusqlite::ToSql> = args.iter().map(|a| a.as_ref()).collect();
        let rows = self.record_rows(&sql, &refs)?;
        assert!(rows.len() <= limit + 1, "the statement is limited");
        Ok(rows)
    }

    pub(super) fn read_agent_subsystem(&self, id: AgentId) -> Result<Option<String>> {
        let subsystem = self
            .read_conn()?
            .query_row(
                "SELECT subsystem FROM agents WHERE id = ?1",
                [id.to_string()],
                |row| row.get::<_, Option<String>>(0),
            )
            .optional()?;
        Ok(subsystem.flatten())
    }

    pub(super) fn insert_agent(
        tx: &Transaction<'_>,
        agent: &Agent,
        subsystem: Option<&str>,
        tally: &mut WriteTally,
    ) -> Result<()> {
        assert!(
            subsystem.is_none_or(|name| !name.trim().is_empty()),
            "a subsystem is named or absent"
        );
        let inserted = tx.execute(
            "INSERT INTO agents(id, parent, role, status, run, created_at, updated_at, body, subsystem, archived)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)",
            params![
                agent.id.to_string(),
                agent.parent.map(|p| p.to_string()),
                agent.role.as_str(),
                agent.status.kind().as_str(),
                agent.run.to_string(),
                agent.created_at.to_rfc3339(),
                agent.updated_at.to_rfc3339(),
                tally.json("agents", agent)?,
                subsystem,
                agent.archived,
            ],
        )?;
        debug_assert_eq!(inserted, 1, "a plain insert writes its one row or fails");
        Ok(())
    }
}

fn parse_id(id: &str) -> Result<AgentId> {
    id.parse()
        .map_err(|e| StoreError::Corrupt(format!("invalid agent id {id}: {e}")))
}
