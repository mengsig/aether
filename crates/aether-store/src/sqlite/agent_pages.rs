//! Agent pages in id order, and the subsystem fixed at an agent's creation.

use aether_core::agent::Agent;
use aether_core::id::AgentId;
use rusqlite::{OptionalExtension, Transaction, params};

use super::SqliteStore;
use super::event_queries::Arguments;
use super::write_tally::WriteTally;
use crate::{AgentFilter, AgentPage, Result, StoreError};

impl SqliteStore {
    pub(super) fn page_agents(
        &self,
        filter: &AgentFilter,
        after: Option<AgentId>,
        limit: usize,
    ) -> Result<AgentPage> {
        assert!(limit > 0, "a page holds at least one agent");
        assert!(!filter.budgeted_only, "budget selection is not paged");
        let ids = self.page_ids(filter, after, limit)?;
        let next_after = (ids.len() > limit).then(|| ids[limit - 1].clone());
        let page = &ids[..ids.len().min(limit)];
        let agents = self.agents_by_id(page)?;
        debug_assert!(agents.windows(2).all(|pair| pair[0].id < pair[1].id));
        let next_after = next_after
            .map(|id| id.parse())
            .transpose()
            .map_err(|e| StoreError::Corrupt(format!("invalid agent id: {e}")))?;
        debug_assert!(next_after.is_none_or(|next| after.is_none_or(|after| next > after)));
        Ok(AgentPage { agents, next_after })
    }

    /// Up to `limit + 1` ids past `after`: the extra one says another page follows.
    fn page_ids(
        &self,
        filter: &AgentFilter,
        after: Option<AgentId>,
        limit: usize,
    ) -> Result<Vec<String>> {
        let mut args: Arguments = Vec::new();
        let mut sql = format!(
            "SELECT id FROM agents WHERE {}",
            super::store::agent_predicate(filter, &mut args)
        );
        // `archived` lives in the body; here it must narrow the page itself.
        if filter.exclude_archived {
            sql.push_str(" AND COALESCE(json_extract(body, '$.archived'), 0) = 0");
        }
        if let Some(after) = after {
            args.push(Box::new(after.to_string()));
            sql.push_str(&format!(" AND id > ?{}", args.len()));
        }
        args.push(Box::new((limit + 1) as i64));
        sql.push_str(&format!(" ORDER BY id ASC LIMIT ?{}", args.len()));
        debug_assert_eq!(sql.matches('?').count(), args.len());
        let refs: Vec<&dyn rusqlite::ToSql> = args.iter().map(|a| a.as_ref()).collect();
        let conn = self.read_conn()?;
        let mut statement = conn.prepare(&sql)?;
        let ids = statement
            .query_map(refs.as_slice(), |row| row.get::<_, String>(0))?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        Ok(ids)
    }

    fn agents_by_id(&self, ids: &[String]) -> Result<Vec<Agent>> {
        if ids.is_empty() {
            return Ok(Vec::new());
        }
        let placeholders: Vec<String> = (1..=ids.len()).map(|n| format!("?{n}")).collect();
        let sql = format!(
            "SELECT id, body FROM agents WHERE id IN ({}) ORDER BY id ASC",
            placeholders.join(",")
        );
        let refs: Vec<&dyn rusqlite::ToSql> =
            ids.iter().map(|id| id as &dyn rusqlite::ToSql).collect();
        self.record_list("agents", &sql, &refs, |agent: &Agent| agent.id.to_string())
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
            "INSERT INTO agents(id, parent, role, status, run, created_at, updated_at, body, subsystem)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
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
            ],
        )?;
        debug_assert_eq!(inserted, 1, "a plain insert writes its one row or fails");
        Ok(())
    }
}
