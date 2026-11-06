//! `get_tree`'s reads: a page of agents chosen by their columns, the V33
//! indexes and `agent_tree`, so only the page's bodies are decoded, and each
//! shown agent's roll-up read from the subtree totals V33 keeps, by key.
//! A page is closed under its in-scope ancestors, so no shown agent's spend
//! is both under a shown ancestor and at a root of its own.

use std::collections::{BTreeMap, HashMap, HashSet};

use aether_core::agent::Agent;
use aether_core::id::{AgentId, RunId};
use aether_core::model::Usage;
use rusqlite::Connection;

use super::SqliteStore;
use super::store::live_status_clause;
use super::usage_totals::{TOTALS_COLUMNS, USAGE_COLUMNS, totals_from, usage_from};
use crate::{Result, StoreError, TREE_PAGE_MAX, TreePage, TreeQuery, TreeUsage, UsageNode};

/// How a scope's root is checked: a correlated lookup of `a` in its subtree,
/// or not at all, because the query walks `agent_tree t` itself.
#[derive(Clone, Copy, PartialEq)]
enum Root {
    Looked,
    Walked,
}

/// The `WHERE` condition choosing `query`'s scope by columns alone, with its
/// arguments, the root first: no body is read to filter. `by_status` keeps the
/// run and archived indexes out of the plan, so the few live agents are sought by status.
fn scope_predicate(query: &TreeQuery, by_status: bool, root: Root) -> (String, Vec<String>) {
    let unindexed = if by_status { "+" } else { "" };
    let mut sql = String::from("1 = 1");
    let mut args = Vec::new();
    if let Some(id) = query.root {
        args.push(id.to_string());
        if root == Root::Looked {
            sql.push_str(
                " AND EXISTS (SELECT 1 FROM agent_tree s WHERE s.ancestor = ?1 AND s.descendant = a.id)",
            );
        }
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
    assert_eq!(
        query.root.is_some(),
        args.first() == query.root.map(|r| r.to_string()).as_ref()
    );
    (sql, args)
}

/// The ids of the first page's pinned agents (the live ones, and a rooted
/// tree's root), and of the walk's agents newest first, one more than `limit`
/// to tell whether more remain. A rooted walk runs down `agent_tree`'s key.
pub(super) fn page_sql(query: &TreeQuery) -> (String, String, Vec<String>) {
    let (pinned_scope, _) = scope_predicate(query, true, Root::Looked);
    let live = live_status_clause();
    // Unordered: sorting here would walk an id index past every finished agent.
    let pinned = match query.root {
        Some(_) => format!("(a.{live} OR a.id = ?1)"),
        None => format!("a.{live}"),
    };
    let pinned_sql = format!("SELECT a.id FROM agents a WHERE {pinned} AND {pinned_scope}");
    let (mut walk, mut args) = match query.root {
        Some(_) => {
            let (scope, args) = scope_predicate(query, false, Root::Walked);
            let sql = format!(
                "SELECT a.id FROM agent_tree t CROSS JOIN agents a ON a.id = t.descendant
                 WHERE t.ancestor = ?1 AND t.descendant != ?1 AND {scope}"
            );
            (sql, args)
        }
        None => {
            let (scope, args) = scope_predicate(query, false, Root::Looked);
            (format!("SELECT a.id FROM agents a WHERE {scope}"), args)
        }
    };
    let key = if query.root.is_some() {
        "t.descendant"
    } else {
        "a.id"
    };
    if let Some(after) = query.after {
        args.push(after.to_string());
        walk.push_str(&format!(" AND {key} < ?{}", args.len()));
    }
    walk.push_str(&format!(" ORDER BY {key} DESC LIMIT {}", query.limit + 1));
    (pinned_sql, walk, args)
}

/// The agents of the JSON id list bound last and the chain of in-scope
/// parents above each, stopping at the first parent out of scope.
pub(super) fn ancestors_sql(query: &TreeQuery) -> (String, Vec<String>) {
    let (scope, args) = scope_predicate(query, false, Root::Looked);
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

/// The ids `sql` selects under `args`, in its order.
fn ids(tx: &Connection, sql: &str, args: &[String]) -> Result<Vec<String>> {
    let mut statement = tx.prepare_cached(sql)?;
    let rows = statement
        .query_map(rusqlite::params_from_iter(args), |row| row.get(0))?
        .collect::<rusqlite::Result<Vec<String>>>()?;
    Ok(rows)
}

impl SqliteStore {
    pub(super) fn read_tree_page(&self, query: &TreeQuery) -> Result<TreePage> {
        assert!(
            (1..=TREE_PAGE_MAX).contains(&query.limit),
            "a page holds 1 to {TREE_PAGE_MAX} agents"
        );
        let (wanted, next_after) = {
            let mut conn = self.read_conn()?;
            // One snapshot: an agent finishing between two reads could be shown twice or not at all.
            let tx = conn.transaction()?;
            let (mut wanted, next_after) = page_ids(&tx, query)?;
            let (above_sql, mut above_args) = ancestors_sql(query);
            above_args.push(serde_json::to_string(&wanted)?);
            let above = ids(&tx, &above_sql, &above_args)?;
            let shown: HashSet<String> = wanted.iter().cloned().collect();
            assert_eq!(shown.len(), wanted.len(), "a page lists an agent once");
            wanted.extend(above.into_iter().filter(|id| !shown.contains(id)));
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
        let order: HashMap<&str, usize> = ids
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
        let rows: Vec<(String, Option<String>, String, Usage)> = {
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

    /// Each shown agent's roll-up from the subtree totals, all from one
    /// snapshot: a few rows by key per agent, however many are below it.
    pub(super) fn read_tree_usages(
        &self,
        shown: &[Agent],
        run: Option<RunId>,
    ) -> Result<BTreeMap<AgentId, TreeUsage>> {
        let visible: HashSet<AgentId> = shown.iter().map(|agent| agent.id).collect();
        let mut children: HashMap<AgentId, Vec<String>> = HashMap::new();
        for agent in shown {
            if let Some(parent) = agent.parent.filter(|parent| visible.contains(parent)) {
                children
                    .entry(parent)
                    .or_default()
                    .push(agent.id.to_string());
            }
        }
        let mut conn = self.read_conn()?;
        let tx = conn.transaction()?;
        let mut usages = BTreeMap::new();
        for agent in shown {
            let below = children
                .get(&agent.id)
                .map(Vec::as_slice)
                .unwrap_or_default();
            usages.insert(agent.id, tree_usage(&tx, agent, below, run)?);
        }
        tx.commit()?;
        assert_eq!(usages.len(), visible.len(), "one roll-up per shown agent");
        Ok(usages)
    }
}

/// `agent`'s subtree usage, and what of it its shown children `below` leave
/// out: its subtree less itself and theirs, none when no agent counts there.
fn tree_usage(
    tx: &Connection,
    agent: &Agent,
    below: &[String],
    run: Option<RunId>,
) -> Result<TreeUsage> {
    let run_name = run.map(|run| run.to_string());
    let (members, subtree) = subtree_totals(tx, agent.id, None, run_name.as_deref())?;
    let (hidden_members, hidden) = subtree_totals(tx, agent.id, Some(below), run_name.as_deref())?;
    if members < 0 || hidden_members < 0 {
        return Err(StoreError::Corrupt(format!(
            "agent {agent}'s subtree totals count fewer agents than its own and its shown children's",
            agent = agent.id
        )));
    }
    // Retention may delete a shown agent before this read: it counts as it was shown.
    let subtree = match members {
        0 if run.is_none_or(|run| run == agent.run) => UsageNode::of(agent).usage,
        _ => subtree,
    };
    let hidden = (hidden_members > 0).then_some(hidden);
    debug_assert!(
        hidden_members <= members,
        "the part left out is within the subtree"
    );
    Ok(TreeUsage { subtree, hidden })
}

/// The scopes of one subtree's totals: the one run's row, or every run's.
fn subtree_rows(id: &str, run: bool) -> String {
    if run {
        format!("scope = 'tree:' || {id} || ':' || ?3")
    } else {
        format!("scope >= 'tree:' || {id} || ':' AND scope < 'tree:' || {id} || ';'")
    }
}

/// The signed sums of `agent`'s subtree totals, less, with `less`, its own
/// usage row and the subtrees of the children in `less`.
pub(super) fn subtree_totals_sql(less: bool, run: bool) -> String {
    let sums: Vec<String> = TOTALS_COLUMNS
        .split(',')
        .map(|column| format!("COALESCE(SUM(sign * {}), 0)", column.trim()))
        .collect();
    let mut parts = vec![format!(
        "SELECT 1 AS sign, {TOTALS_COLUMNS} FROM usage_totals WHERE {}",
        subtree_rows("?1", run)
    )];
    if less {
        let own_run = if run { " AND u.run = ?3" } else { "" };
        parts.push(format!(
            "SELECT -1, {OWN_AS_TOTALS} FROM agent_usage u WHERE u.agent_id = ?1{own_run}"
        ));
        parts.push(format!(
            "SELECT -1, {TOTALS_COLUMNS} FROM json_each(?2) j CROSS JOIN usage_totals WHERE {}",
            subtree_rows("j.value", run)
        ));
    }
    assert_eq!(sums.len(), 20, "one sum per `usage_totals` column");
    assert!(
        less || parts.len() == 1,
        "only the subtree's own rows when nothing is taken off"
    );
    format!(
        "SELECT {} FROM ({})",
        sums.join(", "),
        parts.join(" UNION ALL ")
    )
}

/// The members and usage of [`subtree_totals_sql`], the members signed: a
/// count below none is a corrupt store the caller names.
fn subtree_totals(
    tx: &Connection,
    agent: AgentId,
    less: Option<&[String]>,
    run: Option<&str>,
) -> Result<(i64, Usage)> {
    let mut statement = tx.prepare_cached(&subtree_totals_sql(less.is_some(), run.is_some()))?;
    // Bound by number: ?2 is the children taken off, ?3 the run, so a run binds ?2 too.
    let mut args = vec![agent.to_string()];
    if less.is_some() || run.is_some() {
        args.push(serde_json::to_string(less.unwrap_or_default())?);
    }
    args.extend(run.map(str::to_string));
    assert_eq!(
        args.len(),
        statement.parameter_count(),
        "every parameter bound"
    );
    let read = |row: &rusqlite::Row<'_>| Ok((row.get::<_, i64>(0)?, totals_from(row)?.1));
    let totals = statement.query_row(rusqlite::params_from_iter(&args), read);
    totals.map_err(|error| match error {
        rusqlite::Error::FromSqlConversionFailure(..) => {
            StoreError::Corrupt(format!("agent {agent}'s subtree totals: {error}"))
        }
        error => error.into(),
    })
}

/// An `agent_usage u` row as one member's `usage_totals` columns, in their
/// order, as the `usage_totals_added` trigger adds it.
const OWN_AS_TOTALS: &str = "1, u.input_tokens, u.output_tokens, u.cache_read_tokens,
    u.cache_write_tokens, u.cache_write_1h_tokens, u.reasoning_tokens IS NOT NULL,
    COALESCE(u.reasoning_tokens, 0), u.cost_usd, u.estimated_cost_usd IS NOT NULL,
    COALESCE(u.estimated_cost_usd, 0.0), u.cost_gap IS 'unreported', u.cost_gap IS 'unpriced',
    u.cost_gap IS 'interrupted', u.cost_gap IS 'mixed', u.cost_gap IS 'unknown',
    u.figure AND u.cost_basis = 'unknown', u.figure AND u.cost_basis = 'list_price',
    u.figure AND u.cost_basis = 'provider_reported', u.figure AND u.cost_basis = 'mixed'";

fn parse_id(id: &str) -> Result<AgentId> {
    id.parse()
        .map_err(|e| StoreError::Corrupt(format!("invalid agent id {id}: {e}")))
}

/// The page's pinned ids on the first page, newest first and at most
/// `limit` live ones, then the walk's ids not among them, and where the walk ends.
fn page_ids(tx: &Connection, query: &TreeQuery) -> Result<(Vec<String>, Option<AgentId>)> {
    let (pinned_sql, walk_sql, args) = page_sql(query);
    let mut wanted = Vec::new();
    // Only the first page pins, and only a later one binds `after`: the two share `args`.
    if query.after.is_none() {
        let root = query.root.map(|root| root.to_string());
        let (roots, mut live): (Vec<String>, Vec<String>) = ids(tx, &pinned_sql, &args)?
            .into_iter()
            .partition(|id| Some(id) == root.as_ref());
        live.sort_unstable_by(|a, b| b.cmp(a));
        live.truncate(query.limit);
        wanted.extend(live);
        wanted.extend(roots);
    }
    let mut walked = ids(tx, &walk_sql, &args)?;
    let next_after = if walked.len() > query.limit {
        walked.truncate(query.limit);
        Some(parse_id(&walked[query.limit - 1])?)
    } else {
        None
    };
    let pinned: HashSet<String> = wanted.iter().cloned().collect();
    wanted.extend(walked.into_iter().filter(|id| !pinned.contains(id)));
    assert!(
        wanted.len() <= 2 * query.limit + 1,
        "a page is bounded by its limit"
    );
    Ok((wanted, next_after))
}

#[cfg(test)]
#[path = "tree_totals_tests.rs"]
mod tests;
