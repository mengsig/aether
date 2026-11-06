//! `get_tree`'s page reads: a page of agents chosen by their columns, the V33
//! indexes and `agent_tree`, so only the page's bodies are decoded. A read that
//! checks a filter row by row stops after [`TREE_SCAN_MAX`] rows and says where.

use std::collections::{HashMap, HashSet};

use aether_core::agent::Agent;
use aether_core::id::AgentId;
use rusqlite::Connection;

use super::SqliteStore;
use super::store::live_statuses;
use crate::{Result, StoreError, TREE_PAGE_MAX, TreePage, TreeQuery};

/// The most rows a walk, or the pin of one live status, reads when a filter is
/// checked row by row: a selective filter gives a short page and a continuation.
pub(super) const TREE_SCAN_MAX: usize = 8 * TREE_PAGE_MAX;

/// `query`'s conditions on `agents a`, each absent when it does not filter,
/// and their arguments: the root binds ?1, then the run, then the role prefix.
struct Scope {
    under_root: Option<String>,
    run: Option<String>,
    unarchived: Option<String>,
    role: Option<String>,
    args: Vec<String>,
}

impl Scope {
    fn of(query: &TreeQuery) -> Self {
        let mut args = Vec::new();
        let under_root = query.root.map(|root| {
            args.push(root.to_string());
            "EXISTS (SELECT 1 FROM agent_tree s WHERE s.ancestor = ?1 AND s.descendant = a.id)"
                .to_owned()
        });
        let run = query.run.map(|run| {
            args.push(run.to_string());
            format!("a.run = ?{}", args.len())
        });
        let unarchived = (!query.include_archived).then(|| "a.archived = 0".to_owned());
        let role = query.role_prefix.as_ref().map(|prefix| {
            args.push(prefix.clone());
            let n = args.len();
            format!("substr(a.role, 1, length(?{n})) = ?{n}")
        });
        assert!(args.len() <= 3, "at most a root, a run and a prefix");
        assert!(
            query
                .root
                .is_none_or(|root| args.first() == Some(&root.to_string())),
            "the root binds ?1"
        );
        Self {
            under_root,
            run,
            unarchived,
            role,
            args,
        }
    }
}

/// `parts` joined by `AND`; `1` when none of them filters.
fn all_of(parts: &[&Option<String>]) -> String {
    let parts: Vec<&str> = parts.iter().filter_map(|part| part.as_deref()).collect();
    if parts.is_empty() {
        return "1".to_owned();
    }
    parts.join(" AND ")
}

/// The walk's ids newest first, each beside whether it passes the conditions
/// its index leaves to be checked, and whether any is left: under a root, down `agent_tree`'s key.
pub(super) fn walk_sql(query: &TreeQuery) -> (String, bool) {
    let scope = Scope::of(query);
    let none = None;
    let (key, from, checks) = match query.root {
        Some(_) => (
            "t.descendant",
            "agent_tree t CROSS JOIN agents a ON a.id = t.descendant
             WHERE t.ancestor = ?1 AND t.descendant != ?1"
                .to_owned(),
            [&scope.run, &scope.unarchived, &scope.role],
        ),
        None => (
            "a.id",
            format!(
                "agents a WHERE {}",
                all_of(&[&scope.run, &scope.unarchived])
            ),
            [&scope.role, &none, &none],
        ),
    };
    let checked = checks.iter().any(|check| check.is_some());
    let mut last = scope.args.len();
    let mut sql = format!("SELECT {key}, {} FROM {from}", all_of(&checks));
    if query.after.is_some() {
        last += 1;
        sql.push_str(&format!(" AND {key} < ?{last}"));
    }
    sql.push_str(&format!(" ORDER BY {key} DESC LIMIT ?{}", last + 1));
    assert!(
        sql.matches(" LIMIT ").count() == 1,
        "one bound on the rows read"
    );
    (sql, checked)
}

/// [`walk_sql`]'s arguments, reading at most `budget` rows.
pub(super) fn walk_args(query: &TreeQuery, budget: usize) -> Vec<String> {
    let mut args = Scope::of(query).args;
    args.extend(query.after.map(|after| after.to_string()));
    args.push(budget.to_string());
    args
}

/// One live status's ids newest first, by the `(status, id)` index, each beside
/// whether it is in scope, and whether any condition is checked row by row.
pub(super) fn pin_sql(query: &TreeQuery) -> (String, bool) {
    let scope = Scope::of(query);
    let checks = [
        &scope.under_root,
        &scope.run,
        &scope.unarchived,
        &scope.role,
    ];
    let status = scope.args.len() + 1;
    let sql = format!(
        "SELECT a.id, {} FROM agents a WHERE a.status = ?{status}
         ORDER BY a.id DESC LIMIT ?{}",
        all_of(&checks),
        status + 1
    );
    (sql, checks.iter().any(|check| check.is_some()))
}

/// [`pin_sql`]'s arguments for `status`, reading at most `budget` rows.
pub(super) fn pin_args(query: &TreeQuery, status: &str, budget: usize) -> Vec<String> {
    let mut args = Scope::of(query).args;
    args.push(status.to_owned());
    args.push(budget.to_string());
    args
}

/// The root of a rooted query when it passes the run, archived and role filters.
fn root_sql(query: &TreeQuery) -> String {
    let scope = Scope::of(query);
    let checks = all_of(&[&scope.run, &scope.unarchived, &scope.role]);
    format!("SELECT a.id FROM agents a WHERE a.id = ?1 AND {checks}")
}

/// The agents of the JSON id list bound next to last and the chain of in-scope
/// parents above each, nearest first, stopping at the first parent out of
/// scope and at as many rows in all as bound last.
pub(super) fn ancestors_sql(query: &TreeQuery) -> (String, Vec<String>) {
    let scope = Scope::of(query);
    let filter = all_of(&[
        &scope.under_root,
        &scope.run,
        &scope.unarchived,
        &scope.role,
    ]);
    let ids = scope.args.len() + 1;
    // A LIMIT on the recursive step ends the recursion, the starting rows counted.
    let sql = format!(
        "WITH RECURSIVE up(id) AS (
             SELECT value FROM json_each(?{ids})
             UNION
             SELECT a.id FROM up JOIN agents c ON c.id = up.id
             JOIN agents a ON a.id = c.parent WHERE {filter} LIMIT ?{}
         )
         SELECT id FROM up",
        ids + 1
    );
    (sql, scope.args)
}

/// The ids `sql` selects under `args`, in its order.
fn ids(tx: &Connection, sql: &str, args: &[String]) -> Result<Vec<String>> {
    let mut statement = tx.prepare_cached(sql)?;
    let rows = statement
        .query_map(rusqlite::params_from_iter(args), |row| row.get(0))?
        .collect::<rusqlite::Result<Vec<String>>>()?;
    Ok(rows)
}

/// What a budgeted read found: the ids that passed, in its order, how many
/// rows it read and the last of them.
struct Scanned {
    kept: Vec<String>,
    read: usize,
    last: Option<String>,
}

/// The ids of `sql`'s rows whose check passes, stopping once `want` of them
/// pass or its rows run out: `sql` bounds how many it reads.
fn scan(tx: &Connection, sql: &str, args: &[String], want: usize) -> Result<Scanned> {
    let mut statement = tx.prepare_cached(sql)?;
    let mut rows = statement.query(rusqlite::params_from_iter(args))?;
    let mut scanned = Scanned {
        kept: Vec::new(),
        read: 0,
        last: None,
    };
    while scanned.kept.len() < want {
        let Some(row) = rows.next()? else {
            break;
        };
        let id: String = row.get(0)?;
        // A check that is NULL fails, as it would in a WHERE clause.
        if row.get::<_, Option<bool>>(1)? == Some(true) {
            scanned.kept.push(id.clone());
        }
        scanned.read += 1;
        scanned.last = Some(id);
    }
    assert!(scanned.kept.len() <= want, "no more than wanted");
    assert!(scanned.kept.len() <= scanned.read, "each kept row was read");
    Ok(scanned)
}

/// The first page's pinned ids: the newest `limit` live agents in scope,
/// read down the `(status, id)` index of each live status, then the root.
fn pinned_ids(tx: &Connection, query: &TreeQuery) -> Result<Vec<String>> {
    let (sql, checked) = pin_sql(query);
    let budget = if checked {
        TREE_SCAN_MAX
    } else {
        query.limit + 1
    };
    let root = query.root.map(|root| root.to_string());
    let mut live = Vec::new();
    for status in live_statuses() {
        let scanned = scan(tx, &sql, &pin_args(query, status, budget), query.limit + 1)?;
        live.extend(
            scanned
                .kept
                .into_iter()
                .filter(|id| Some(id) != root.as_ref()),
        );
    }
    live.sort_unstable_by(|a, b| b.cmp(a));
    live.truncate(query.limit);
    if let Some(root) = root {
        let found = ids(tx, &root_sql(query), &Scope::of(query).args)?;
        assert!(found.iter().all(|id| *id == root), "only the root");
        live.extend(found);
    }
    assert!(live.len() <= query.limit + 1, "the live cap and the root");
    Ok(live)
}

/// The page's pinned ids on the first page, then the walk's ids not among
/// them, and where the next page starts: below the walk's last shown agent,
/// or below its last row read when its budget ran out first.
fn page_ids(tx: &Connection, query: &TreeQuery) -> Result<(Vec<String>, Option<AgentId>)> {
    let mut wanted = match query.after {
        None => pinned_ids(tx, query)?,
        Some(_) => Vec::new(),
    };
    let (walk, checked) = walk_sql(query);
    let budget = if checked {
        TREE_SCAN_MAX
    } else {
        query.limit + 1
    };
    assert!(query.limit < budget, "a budget reads past a full page");
    let mut walked = scan(tx, &walk, &walk_args(query, budget), query.limit + 1)?;
    let next_after = if walked.kept.len() > query.limit {
        walked.kept.truncate(query.limit);
        Some(parse_id(&walked.kept[query.limit - 1])?)
    } else if walked.read == budget {
        walked.last.as_deref().map(parse_id).transpose()?
    } else {
        None
    };
    let pinned: HashSet<String> = wanted.iter().cloned().collect();
    wanted.extend(walked.kept.into_iter().filter(|id| !pinned.contains(id)));
    assert!(
        wanted.len() <= 2 * query.limit + 1,
        "a page is bounded by its limit"
    );
    Ok((wanted, next_after))
}

pub(super) fn parse_id(id: &str) -> Result<AgentId> {
    id.parse()
        .map_err(|e| StoreError::Corrupt(format!("invalid agent id {id}: {e}")))
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
            above_args.push((wanted.len() + query.limit).to_string());
            let above = ids(&tx, &above_sql, &above_args)?;
            let shown: HashSet<String> = wanted.iter().cloned().collect();
            assert_eq!(shown.len(), wanted.len(), "a page lists an agent once");
            wanted.extend(above.into_iter().filter(|id| !shown.contains(id)));
            assert!(
                wanted.len() <= shown.len() + query.limit,
                "at most `limit` ancestors"
            );
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
}
