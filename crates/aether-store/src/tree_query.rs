//! What one `get_tree` page asks the store for, and what it gets back.

use std::collections::{BTreeMap, HashMap, HashSet};

use aether_core::agent::Agent;
use aether_core::id::{AgentId, RunId};
use aether_core::model::Usage;

use crate::Result;

/// The most agents one tree page walks; the first page pins as many live ones on top.
pub const TREE_PAGE_MAX: usize = 500;

/// Which agents one tree page shows: filters the SQLite store applies in SQL,
/// before any body is decoded, and where the page starts.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TreeQuery {
    pub run: Option<RunId>,
    /// Only this agent and its descendants, across runs.
    pub root: Option<AgentId>,
    pub include_archived: bool,
    /// Only agents whose role starts with this text.
    pub role_prefix: Option<String>,
    /// Continue with the agents older than this one; `None` is the first
    /// page, which also pins the newest `limit` live agents in scope and the root.
    pub after: Option<AgentId>,
    /// At most this many agents walked, and live ones pinned, 1 to [`TREE_PAGE_MAX`].
    pub limit: usize,
}

impl TreeQuery {
    /// Whether `agent`, already in the run or root's scope, passes the filters.
    pub fn selects(&self, agent: &Agent) -> bool {
        self.run.is_none_or(|run| agent.run == run)
            && (self.include_archived || !agent.archived)
            && self
                .role_prefix
                .as_deref()
                .is_none_or(|prefix| agent.role.as_str().starts_with(prefix))
    }
}

/// One page of a tree's agents: on the first, the newest live ones and a
/// rooted tree's root; then the walk's agents, live or finished, newest first;
/// then up to `limit` in-scope ancestors of those, nearest first, not already
/// shown. A later page may repeat one.
#[derive(Debug, Clone, PartialEq)]
pub struct TreePage {
    pub agents: Vec<Agent>,
    /// Where the walk stopped, when older agents were left for the next page:
    /// pass it as [`TreeQuery::after`]. A walk checking a filter row by row
    /// stops after a bounded number of rows, so a short page may have one.
    pub next_after: Option<AgentId>,
}

/// A shown agent's spend with that of every agent below it up to the next shown
/// ones, plus its shown children's subtrees; `hidden` is the agents below it
/// alone, `None` when none counts. Shown roots never share an agent.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct TreeUsage {
    pub subtree: Usage,
    pub hidden: Option<Usage>,
    /// The shown agent whose subtree counts this one's, `None` for a root:
    /// list it there, as the roll-up's snapshot links it, never by an older read.
    pub parent: Option<AgentId>,
}

/// One agent as a usage roll-up reads it: its links and usage, no body.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct UsageNode {
    pub id: AgentId,
    pub parent: Option<AgentId>,
    pub run: RunId,
    pub usage: Usage,
}

impl UsageNode {
    pub fn of(agent: &Agent) -> Self {
        Self {
            id: agent.id,
            parent: agent.parent,
            run: agent.run,
            usage: agent.stats.usage,
        }
    }
}

/// [`TreeQuery`]'s page of `scope`, every agent of which it selects: what a
/// store without a paged read of its own answers.
pub fn tree_page_of(mut scope: Vec<Agent>, query: &TreeQuery) -> Result<TreePage> {
    assert!(
        (1..=TREE_PAGE_MAX).contains(&query.limit),
        "a page holds 1 to {TREE_PAGE_MAX} agents"
    );
    debug_assert!(scope.iter().all(|agent| query.selects(agent)));
    scope.sort_by(|a, b| b.id.cmp(&a.id));
    let by_id: HashMap<AgentId, Agent> = scope.iter().map(|a| (a.id, a.clone())).collect();
    let is_root = |agent: &Agent| query.root == Some(agent.id);
    let mut agents: Vec<Agent> = Vec::new();
    if query.after.is_none() {
        let live = scope.iter().filter(|a| a.is_live() && !is_root(a));
        agents.extend(live.take(query.limit).cloned());
        agents.extend(scope.iter().filter(|a| is_root(a)).cloned());
    }
    let mut older: Vec<Agent> = scope
        .into_iter()
        .filter(|agent| !is_root(agent) && query.after.is_none_or(|after| agent.id < after))
        .collect();
    let next_after = (older.len() > query.limit).then(|| older[query.limit - 1].id);
    older.truncate(query.limit);
    let pinned: HashSet<AgentId> = agents.iter().map(|agent| agent.id).collect();
    agents.extend(
        older
            .into_iter()
            .filter(|agent| !pinned.contains(&agent.id)),
    );
    let above = ancestors_of(&agents, &by_id, query.limit);
    agents.extend(above);
    assert!(
        agents.len() <= by_id.len(),
        "a page shows only its scope, once each"
    );
    Ok(TreePage { agents, next_after })
}

/// [`TreeUsage`] for each of `shown`, folded from `nodes`: the shown agents
/// and every agent below them, as [`crate::Store::subtree_usages`] reads them.
/// With `run`, only the agents of that run are counted.
pub fn tree_usages_of(
    mut nodes: Vec<UsageNode>,
    shown: &[Agent],
    run: Option<RunId>,
) -> BTreeMap<AgentId, TreeUsage> {
    let read: HashSet<AgentId> = nodes.iter().map(|node| node.id).collect();
    // A shown agent written after the roll-up's read began counts as it was shown.
    nodes.extend(
        shown
            .iter()
            .filter(|a| !read.contains(&a.id))
            .map(UsageNode::of),
    );
    let visible: HashSet<AgentId> = shown.iter().map(|agent| agent.id).collect();
    let owners = owners_of(&nodes, &visible);
    let mut owned: HashMap<AgentId, Usage> = HashMap::with_capacity(visible.len());
    let mut hidden: HashMap<AgentId, (u64, Usage)> = HashMap::new();
    for node in nodes.iter().filter(|n| run.is_none_or(|run| n.run == run)) {
        let Some(owner) = owners.get(&node.id).copied().flatten() else {
            continue;
        };
        *owned.entry(owner).or_default() += node.usage;
        if owner != node.id {
            let left_out = hidden.entry(owner).or_default();
            left_out.0 += 1;
            left_out.1 += node.usage;
        }
    }
    let subtrees = fold_shown(shown, &visible, &owned);
    let mut usages = BTreeMap::new();
    for agent in shown {
        let hidden = hidden.get(&agent.id).map(|(_, usage)| *usage);
        let subtree = subtrees.get(&agent.id).copied().unwrap_or_default();
        // `fold_shown` folds each shown agent into its shown parent as `shown` links it.
        let parent = agent.parent.filter(|parent| visible.contains(parent));
        let usage = TreeUsage {
            subtree,
            hidden,
            parent,
        };
        usages.insert(agent.id, usage);
    }
    assert_eq!(usages.len(), visible.len(), "one roll-up per shown agent");
    debug_assert!(hidden.values().all(|(members, _)| *members > 0));
    usages
}

/// Each node's owner: the nearest shown agent at or above it among `nodes`,
/// none when its walk up leaves them first.
fn owners_of(nodes: &[UsageNode], visible: &HashSet<AgentId>) -> HashMap<AgentId, Option<AgentId>> {
    let parents: HashMap<AgentId, Option<AgentId>> =
        nodes.iter().map(|node| (node.id, node.parent)).collect();
    let mut owners: HashMap<AgentId, Option<AgentId>> = HashMap::with_capacity(nodes.len());
    for node in nodes {
        let mut chain = Vec::new();
        let mut at = Some(node.id);
        let owner = loop {
            let Some(id) = at else { break None };
            if let Some(known) = owners.get(&id) {
                break *known;
            }
            if visible.contains(&id) {
                break Some(id);
            }
            // Off the nodes read, or round a corrupt parent cycle: no shown agent above.
            let Some(parent) = parents.get(&id).filter(|_| chain.len() < nodes.len()) else {
                break None;
            };
            chain.push(id);
            at = *parent;
        };
        owners.extend(chain.into_iter().map(|id| (id, owner)));
        owners.entry(node.id).or_insert(owner);
    }
    assert_eq!(owners.len(), parents.len(), "an owner, or none, per node");
    debug_assert!(
        visible
            .iter()
            .all(|id| owners.get(id).is_none_or(|o| *o == Some(*id)))
    );
    owners
}

/// Each shown agent's subtree: what it owns and its shown children's
/// subtrees, folded children first by a post-order walk from the shown roots.
fn fold_shown(
    shown: &[Agent],
    visible: &HashSet<AgentId>,
    owned: &HashMap<AgentId, Usage>,
) -> HashMap<AgentId, Usage> {
    let mut children: HashMap<AgentId, Vec<AgentId>> = HashMap::new();
    let mut roots = Vec::new();
    for agent in shown {
        match agent.parent.filter(|parent| visible.contains(parent)) {
            Some(parent) => children.entry(parent).or_default().push(agent.id),
            None => roots.push(agent.id),
        }
    }
    let mut subtrees: HashMap<AgentId, Usage> = HashMap::with_capacity(shown.len());
    for root in roots {
        let mut stack = vec![(root, false)];
        while let Some((id, post)) = stack.pop() {
            let kids = children.get(&id).map(Vec::as_slice).unwrap_or_default();
            if !post {
                stack.push((id, true));
                stack.extend(kids.iter().map(|kid| (*kid, false)));
                continue;
            }
            let mut total = owned.get(&id).copied().unwrap_or_default();
            for kid in kids {
                total += subtrees.get(kid).copied().unwrap_or_default();
            }
            subtrees.insert(id, total);
        }
    }
    assert!(subtrees.len() <= shown.len(), "a subtree per shown agent");
    debug_assert!(subtrees.keys().all(|id| visible.contains(id)));
    subtrees
}

/// Up to `cap` agents of `scope` above `shown` and not among them, level by
/// level in `shown`'s order, as SQLite's recursive walk queues them.
fn ancestors_of(shown: &[Agent], scope: &HashMap<AgentId, Agent>, cap: usize) -> Vec<Agent> {
    let mut seen: HashSet<AgentId> = shown.iter().map(|agent| agent.id).collect();
    let mut above: Vec<Agent> = Vec::new();
    let mut level: Vec<&Agent> = shown.iter().collect();
    while !level.is_empty() && above.len() < cap {
        let mut next = Vec::new();
        for agent in level {
            let Some(parent) = agent.parent.and_then(|id| scope.get(&id)) else {
                continue;
            };
            if above.len() < cap && seen.insert(parent.id) {
                above.push(parent.clone());
                next.push(parent);
            }
        }
        level = next;
    }
    assert!(above.len() <= cap, "at most `cap` ancestors");
    debug_assert!(above.len() <= scope.len(), "each ancestor is added once");
    above
}
