//! `get_tree`'s reads: a page of agents chosen by their columns and the V33
//! indexes, so only the page's bodies are decoded, and a subtree's usage
//! read from `agent_usage`, so the hidden descendants' bodies never are.
//! A page is closed under its in-scope ancestors, so no shown agent's spend
//! is both under a shown ancestor and at a root of its own.

use aether_core::agent::Agent;
use aether_core::id::AgentId;

use super::SqliteStore;
use super::store::live_status_clause;
use super::usage_totals::{USAGE_COLUMNS, usage_from};
use crate::{Result, StoreError, TREE_PAGE_MAX, TreePage, TreeQuery, UsageNode};

/// The `WHERE` condition choosing `query`'s scope by columns alone, with its
/// arguments: no body is read to filter. `by_status` keeps the run and
/// archived indexes out of the plan, so the few live agents are sought by status.
fn scope_predicate(query: &TreeQuery, by_status: bool) -> (String, Vec<String>) {
    let unindexed = if by_status { "+" } else { "" };
    let mut sql = String::from("1 = 1");
    let mut args = Vec::new();
    if let Some(root) = query.root {
        args.push(root.to_string());
        sql.push_str(&format!(
            " AND a.id IN (WITH RECURSIVE down(id) AS (
                 SELECT id FROM agents WHERE id = ?{n}
                 UNION
                 SELECT c.id FROM agents c JOIN down ON c.parent = down.id
             ) SELECT id FROM down)",
            n = args.len()
        ));
    }
    if let Some(run) = query.run {
        args.push(run.to_string());
        sql.push_str(&format!(" AND {unindexed}a.run = ?{}", args.len()));
    }
    if !query.include_archived {
        sql.push_str(&format!(" AND {unindexed}a.archived = 0"));
    }
    if let Some(prefix) = &query.role_prefix {
        args.push(prefix.clone());
        let n = args.len();
        sql.push_str(&format!(" AND substr(a.role, 1, length(?{n})) = ?{n}"));
    }
    assert!(args.len() <= 3, "at most a root, a run and a prefix");
    (sql, args)
}

/// The ids of the page's pinned agents (first page only: the live ones, and
/// a rooted tree's root) and of its finished ones, newest first, one more than
/// `limit` to tell whether more remain.
pub(super) fn page_sql(query: &TreeQuery) -> (String, String, Vec<String>) {
    let (pinned_scope, _) = scope_predicate(query, true);
    let (scope, mut args) = scope_predicate(query, false);
    let live = live_status_clause();
    // The root is the first argument; it is pinned, so no page lists it as finished.
    let (pinned, unpinned) = if query.root.is_some() {
        (
            format!("(a.{live} OR a.id = ?1)"),
            format!("NOT a.{live} AND a.id != ?1"),
        )
    } else {
        (format!("a.{live}"), format!("NOT a.{live}"))
    };
    // Unordered: sorting here would walk an id index past every finished agent.
    let pinned_sql = format!("SELECT a.id FROM agents a WHERE {pinned} AND {pinned_scope}");
    let mut finished = format!("SELECT a.id FROM agents a WHERE {unpinned} AND {scope}");
    if let Some(after) = query.after {
        args.push(after.to_string());
        finished.push_str(&format!(" AND a.id < ?{}", args.len()));
    }
    finished.push_str(&format!(" ORDER BY a.id DESC LIMIT {}", query.limit + 1));
    (pinned_sql, finished, args)
}

/// The agents of the JSON id list bound last and the chain of in-scope
/// parents above each, stopping at the first parent out of scope.
pub(super) fn ancestors_sql(query: &TreeQuery) -> (String, Vec<String>) {
    let (scope, args) = scope_predicate(query, false);
    let ids = args.len() + 1;
    let sql = format!(
        "WITH RECURSIVE up(id) AS (
             SELECT value FROM json_each(?{ids})
             UNION
             SELECT a.id FROM up JOIN agents c ON c.id = up.id
             JOIN agents a ON a.id = c.parent WHERE {scope}
         )
         SELECT id FROM up"
    );
    (sql, args)
}

impl SqliteStore {
    pub(super) fn read_tree_page(&self, query: &TreeQuery) -> Result<TreePage> {
        assert!(
            (1..=TREE_PAGE_MAX).contains(&query.limit),
            "a page holds 1 to {TREE_PAGE_MAX} finished agents"
        );
        let (live_sql, finished_sql, args) = page_sql(query);
        let (wanted, next_after) = {
            let mut conn = self.read_conn()?;
            // One snapshot: an agent finishing between two reads would be both live and finished.
            let tx = conn.transaction()?;
            let ids = |sql: &str, args: &[String]| -> Result<Vec<String>> {
                let mut statement = tx.prepare_cached(sql)?;
                let rows = statement
                    .query_map(rusqlite::params_from_iter(args), |row| row.get(0))?
                    .collect::<rusqlite::Result<Vec<String>>>()?;
                Ok(rows)
            };
            // Only the first page, which has no `after`, holds the pinned agents.
            let mut wanted = if query.after.is_none() {
                ids(&live_sql, &args)?
            } else {
                Vec::new()
            };
            wanted.sort_unstable_by(|a, b| b.cmp(a));
            let mut finished = ids(&finished_sql, &args)?;
            let next_after = if finished.len() > query.limit {
                finished.truncate(query.limit);
                Some(parse_id(&finished[query.limit - 1])?)
            } else {
                None
            };
            wanted.extend(finished);
            let (above_sql, mut above_args) = ancestors_sql(query);
            above_args.push(serde_json::to_string(&wanted)?);
            let above = ids(&above_sql, &above_args)?;
            let shown: std::collections::HashSet<&String> = wanted.iter().collect();
            let above: Vec<String> = above.into_iter().filter(|id| !shown.contains(id)).collect();
            assert_eq!(
                shown.len(),
                wanted.len(),
                "no agent is both live and finished"
            );
            wanted.extend(above);
            tx.commit()?;
            (wanted, next_after)
        };
        let agents = self.agents_by_id(&wanted)?;
        assert!(agents.len() <= wanted.len(), "only the agents asked for");
        Ok(TreePage { agents, next_after })
    }

    /// The agents of `ids` that decode, in the order of `ids`.
    fn agents_by_id(&self, ids: &[String]) -> Result<Vec<Agent>> {
        if ids.is_empty() {
            return Ok(Vec::new());
        }
        let wanted = serde_json::to_string(ids)?;
        let mut agents = self.record_list(
            "agents",
            "SELECT a.id, a.body FROM json_each(?1) j CROSS JOIN agents a ON a.id = j.value",
            &[&wanted],
            |agent: &Agent| agent.id.to_string(),
        )?;
        let order: std::collections::HashMap<&str, usize> = ids
            .iter()
            .enumerate()
            .map(|(at, id)| (id.as_str(), at))
            .collect();
        agents.sort_by_key(|agent| order.get(agent.id.to_string().as_str()).copied());
        debug_assert!(
            agents
                .iter()
                .all(|a| order.contains_key(a.id.to_string().as_str()))
        );
        Ok(agents)
    }

    /// `roots` and their descendants, across runs, from columns and `agent_usage`.
    pub(super) fn read_subtree_usages(&self, roots: &[AgentId]) -> Result<Vec<UsageNode>> {
        if roots.is_empty() {
            return Ok(Vec::new());
        }
        let wanted =
            serde_json::to_string(&roots.iter().map(ToString::to_string).collect::<Vec<_>>())?;
        let rows: Vec<(String, Option<String>, String, aether_core::model::Usage)> = {
            let conn = self.read_conn()?;
            // CROSS JOIN keeps the walk the outer loop, as in `subtree_agents`;
            // UNION drops a repeated id, so a corrupt parent cycle ends it.
            let mut statement = conn.prepare_cached(&format!(
                "WITH RECURSIVE down(id) AS (
                     SELECT value FROM json_each(?1)
                     UNION
                     SELECT c.id FROM agents c JOIN down ON c.parent = down.id
                 )
                 SELECT a.id, a.parent, a.run, {USAGE_COLUMNS}
                 FROM down CROSS JOIN agents a ON a.id = down.id
                 JOIN agent_usage u ON u.agent_id = a.id"
            ))?;
            statement
                .query_map([wanted], |row| {
                    Ok((row.get(0)?, row.get(1)?, row.get(2)?, usage_from(row, 3)?))
                })?
                .collect::<rusqlite::Result<_>>()?
        };
        let mut nodes = Vec::with_capacity(rows.len());
        for (id, parent, run, usage) in rows {
            nodes.push(UsageNode {
                id: parse_id(&id)?,
                parent: parent.as_deref().map(parse_id).transpose()?,
                run: run
                    .parse()
                    .map_err(|_| StoreError::Corrupt(format!("agent {id} has an invalid run")))?,
                usage,
            });
        }
        debug_assert_eq!(
            nodes
                .iter()
                .map(|n| n.id)
                .collect::<std::collections::BTreeSet<_>>()
                .len(),
            nodes.len(),
            "the walk's UNION reads each agent once"
        );
        Ok(nodes)
    }
}

fn parse_id(id: &str) -> Result<AgentId> {
    id.parse()
        .map_err(|e| StoreError::Corrupt(format!("invalid agent id {id}: {e}")))
}
