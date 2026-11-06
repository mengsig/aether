//! `get_tree`'s roll-ups, read by key from the subtree totals V33 keeps. Every
//! agent counts once, under the nearest shown agent at or above it.

use std::cmp::Reverse;
use std::collections::{BTreeMap, HashMap};

use aether_core::agent::Agent;
use aether_core::id::{AgentId, RunId};
use aether_core::model::Usage;
use rusqlite::Connection;

use super::SqliteStore;
use super::tree_pages::parse_id;
use super::usage_totals::{TOTALS_COLUMNS, USAGE_COLUMNS, totals_from, usage_from};
use crate::{Result, StoreError, TreeUsage, UsageNode};

/// An agent's ancestors read one by one past this many are sought among the
/// shown agents instead, so a deep chain costs at most the page per agent.
const ANCESTORS_READ_MAX: usize = 64;

/// Each agent of the JSON list ?1 beside its parent, when that parent is stored.
pub(super) const SHOWN_PARENTS_SQL: &str = "SELECT a.id, p.id FROM json_each(?1) j
    CROSS JOIN agents a ON a.id = j.value JOIN agents p ON p.id = a.parent";

/// The agents of the JSON list ?2 above the agent ?1, each sought by key.
pub(super) const SEEK_ANCESTORS_SQL: &str = "SELECT j.value FROM json_each(?2) j
    WHERE j.value != ?1
    AND EXISTS (SELECT 1 FROM agent_tree t WHERE t.ancestor = j.value AND t.descendant = ?1)";

/// The ancestors of the agent ?1 by the descendant index, one past the most read.
pub(super) fn ancestors_read_sql() -> String {
    format!(
        "SELECT ancestor FROM agent_tree WHERE descendant = ?1 AND ancestor != ?1 LIMIT {}",
        ANCESTORS_READ_MAX + 1
    )
}

impl SqliteStore {
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

    /// Each shown agent's roll-up, all from one snapshot, the links between
    /// the shown agents included: a few rows by key per agent, however many are below it.
    pub(super) fn read_tree_usages(
        &self,
        shown: &[Agent],
        run: Option<RunId>,
    ) -> Result<BTreeMap<AgentId, TreeUsage>> {
        let mut conn = self.read_conn()?;
        let tx = conn.transaction()?;
        let links = shown_links(&tx, shown)?;
        let mut taken: HashMap<AgentId, Vec<String>> = HashMap::new();
        for (agent, above) in &links.nearest {
            taken.entry(*above).or_default().push(agent.to_string());
        }
        let mut owned = HashMap::with_capacity(shown.len());
        for agent in shown {
            let below = taken.get(&agent.id).map(Vec::as_slice).unwrap_or_default();
            owned.insert(agent.id, owned_usage(&tx, agent, below, run)?);
        }
        tx.commit()?;
        let usages = fold_subtrees(shown, &links, owned);
        assert_eq!(
            usages.len(),
            links.depth.len(),
            "one roll-up per shown agent"
        );
        Ok(usages)
    }
}

/// How the shown agents nest, as the store links them: the nearest shown
/// agent above each that has one, and how many shown agents are above each.
struct Links {
    nearest: HashMap<AgentId, AgentId>,
    depth: HashMap<AgentId, usize>,
}

/// [`Links`] for `shown`: a shown parent is the nearest; above any other, the
/// shown ancestor in `agent_tree` with the most shown agents above it.
fn shown_links(tx: &Connection, shown: &[Agent]) -> Result<Links> {
    let by_text: HashMap<String, AgentId> = shown
        .iter()
        .map(|agent| (agent.id.to_string(), agent.id))
        .collect();
    assert_eq!(by_text.len(), shown.len(), "each agent is shown once");
    let ids = serde_json::to_string(&by_text.keys().collect::<Vec<_>>())?;
    let parents = shown_parents(tx, &ids, &by_text)?;
    let mut above: HashMap<AgentId, Vec<AgentId>> = HashMap::new();
    for agent in shown
        .iter()
        .filter(|agent| !parents.contains_key(&agent.id))
    {
        above.insert(agent.id, shown_ancestors(tx, agent.id, &ids, &by_text)?);
    }
    let depth = shown_depths(shown, &parents, &above)?;
    let mut nearest = parents;
    for (agent, candidates) in above {
        // On one path the deeper ancestor has more shown above it; the id breaks a corrupt tie.
        let pick = candidates.into_iter().max_by_key(|id| (depth.get(id), *id));
        nearest.extend(pick.map(|pick| (agent, pick)));
    }
    assert!(
        nearest.len() <= shown.len(),
        "at most one nearest per agent"
    );
    Ok(Links { nearest, depth })
}

/// The shown parent of each shown agent whose parent is shown, as stored now:
/// a parent deleted since the page was read links nothing.
fn shown_parents(
    tx: &Connection,
    ids: &str,
    by_text: &HashMap<String, AgentId>,
) -> Result<HashMap<AgentId, AgentId>> {
    let mut statement = tx.prepare_cached(SHOWN_PARENTS_SQL)?;
    let rows = statement
        .query_map([ids], |row| {
            Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
        })?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    let mut parents = HashMap::with_capacity(rows.len());
    for (id, parent) in rows {
        let (Some(id), Some(parent)) = (by_text.get(&id), by_text.get(&parent)) else {
            continue;
        };
        parents.insert(*id, *parent);
    }
    assert!(parents.len() <= by_text.len(), "only shown agents");
    assert!(
        parents.iter().all(|(id, parent)| id != parent),
        "no agent is its own parent"
    );
    Ok(parents)
}

/// The shown agents strictly above `agent` in `agent_tree`: its ancestors read
/// by the descendant index when few, else each shown agent sought as one.
fn shown_ancestors(
    tx: &Connection,
    agent: AgentId,
    ids: &str,
    by_text: &HashMap<String, AgentId>,
) -> Result<Vec<AgentId>> {
    let agent_text = agent.to_string();
    let mut statement = tx.prepare_cached(&ancestors_read_sql())?;
    let mut ancestors = statement
        .query_map([&agent_text], |row| row.get::<_, String>(0))?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    if ancestors.len() > ANCESTORS_READ_MAX {
        let mut statement = tx.prepare_cached(SEEK_ANCESTORS_SQL)?;
        ancestors = statement
            .query_map(rusqlite::params![agent_text, ids], |row| {
                row.get::<_, String>(0)
            })?
            .collect::<rusqlite::Result<Vec<_>>>()?;
    }
    let shown: Vec<AgentId> = ancestors
        .iter()
        .filter_map(|id| by_text.get(id).copied())
        .collect();
    assert!(shown.len() <= by_text.len(), "only shown agents");
    assert!(!shown.contains(&agent), "strictly above");
    Ok(shown)
}

/// How many shown agents are above each shown one: one more than above its
/// shown parent, or as many as `agent_tree` lists above it when its parent is not shown.
fn shown_depths(
    shown: &[Agent],
    parents: &HashMap<AgentId, AgentId>,
    above: &HashMap<AgentId, Vec<AgentId>>,
) -> Result<HashMap<AgentId, usize>> {
    let mut depth: HashMap<AgentId, usize> = above
        .iter()
        .map(|(id, ancestors)| (*id, ancestors.len()))
        .collect();
    for agent in shown {
        let mut chain = Vec::new();
        let mut at = agent.id;
        while !depth.contains_key(&at) {
            let Some(parent) = parents.get(&at).filter(|_| chain.len() < shown.len()) else {
                return Err(StoreError::Corrupt(format!(
                    "agent {at}'s shown parents form a cycle"
                )));
            };
            chain.push(at);
            at = *parent;
        }
        let mut below = depth[&at];
        for id in chain.into_iter().rev() {
            below += 1;
            depth.insert(id, below);
        }
    }
    assert_eq!(depth.len(), shown.len(), "a depth per shown agent");
    debug_assert!(
        parents
            .iter()
            .all(|(id, parent)| depth[id] == depth[parent] + 1)
    );
    Ok(depth)
}

/// What `agent` owns, itself and what is below it up to the shown agents in
/// `below`, and that less itself, none when no agent counts there.
fn owned_usage(
    tx: &Connection,
    agent: &Agent,
    below: &[String],
    run: Option<RunId>,
) -> Result<(Usage, Option<Usage>)> {
    let run_name = run.map(|run| run.to_string());
    let (members, owned) = subtree_totals(tx, agent.id, below, false, run_name.as_deref())?;
    let (hidden_members, hidden) = subtree_totals(tx, agent.id, below, true, run_name.as_deref())?;
    if members < 0 || hidden_members < 0 {
        return Err(StoreError::Corrupt(format!(
            "agent {agent}'s subtree totals count fewer agents than the shown subtrees below it",
            agent = agent.id
        )));
    }
    // Retention may delete a shown agent before this read: it counts as it was shown.
    let owned = match members {
        0 if run.is_none_or(|run| run == agent.run) => UsageNode::of(agent).usage,
        _ => owned,
    };
    debug_assert!(hidden_members <= members, "the part hidden is owned");
    Ok((owned, (hidden_members > 0).then_some(hidden)))
}

/// Each shown agent's roll-up: what it owns and the subtrees of the shown
/// children listed under it, children first, so no two shown roots share a member.
fn fold_subtrees(
    shown: &[Agent],
    links: &Links,
    owned: HashMap<AgentId, (Usage, Option<Usage>)>,
) -> BTreeMap<AgentId, TreeUsage> {
    let mut usages: BTreeMap<AgentId, TreeUsage> = owned
        .into_iter()
        .map(|(id, (subtree, hidden))| (id, TreeUsage { subtree, hidden }))
        .collect();
    let mut deepest_first: Vec<&Agent> = shown.iter().collect();
    deepest_first.sort_by_key(|agent| Reverse(links.depth[&agent.id]));
    for agent in deepest_first {
        // Listed under its parent only when the store still links it there.
        let Some(parent) = agent
            .parent
            .filter(|p| links.nearest.get(&agent.id) == Some(p))
        else {
            continue;
        };
        let child = usages[&agent.id].subtree;
        if let Some(usage) = usages.get_mut(&parent) {
            usage.subtree += child;
        }
    }
    assert_eq!(usages.len(), shown.len(), "one roll-up per shown agent");
    usages
}

/// The scope of one subtree's totals: within the run bound as ?3, or over every run.
fn subtree_rows(id: &str, run: bool) -> String {
    if run {
        format!("scope = 'tree:' || {id} || ':' || ?3")
    } else {
        format!("scope = 'tree:' || {id}")
    }
}

/// The signed sums of the subtree totals of the agent bound as ?1 less those
/// of the agents in the JSON list ?2, and with `own` less its own usage row too.
pub(super) fn subtree_totals_sql(own: bool, run: bool) -> String {
    let sums: Vec<String> = TOTALS_COLUMNS
        .split(',')
        .map(|column| format!("COALESCE(SUM(sign * {}), 0)", column.trim()))
        .collect();
    let mut parts = vec![
        format!(
            "SELECT 1 AS sign, {TOTALS_COLUMNS} FROM usage_totals WHERE {}",
            subtree_rows("?1", run)
        ),
        format!(
            "SELECT -1, {TOTALS_COLUMNS} FROM json_each(?2) j CROSS JOIN usage_totals WHERE {}",
            subtree_rows("j.value", run)
        ),
    ];
    if own {
        let own_run = if run { " AND u.run = ?3" } else { "" };
        parts.push(format!(
            "SELECT -1, {OWN_AS_TOTALS} FROM agent_usage u WHERE u.agent_id = ?1{own_run}"
        ));
    }
    assert_eq!(sums.len(), 20, "one sum per `usage_totals` column");
    assert_eq!(
        parts.len(),
        2 + usize::from(own),
        "the subtree, those below, its own row"
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
    below: &[String],
    own: bool,
    run: Option<&str>,
) -> Result<(i64, Usage)> {
    let mut statement = tx.prepare_cached(&subtree_totals_sql(own, run.is_some()))?;
    let mut args = vec![agent.to_string(), serde_json::to_string(below)?];
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

#[cfg(test)]
#[path = "tree_totals_tests.rs"]
mod tests;
