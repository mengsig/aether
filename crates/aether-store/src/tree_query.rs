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
/// then the in-scope ancestors of those not already shown. A later page may
/// repeat one.
#[derive(Debug, Clone, PartialEq)]
pub struct TreePage {
    pub agents: Vec<Agent>,
    /// The oldest agent the walk showed, when older ones were left for the
    /// next page: pass it as [`TreeQuery::after`].
    pub next_after: Option<AgentId>,
}

/// What a shown agent and every agent below it spent, and the part of that
/// spent at or below its children the page leaves out; `None` when none counts.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct TreeUsage {
    pub subtree: Usage,
    pub hidden: Option<Usage>,
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
    let above = ancestors_of(&agents, &by_id);
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
    let mut children: HashMap<AgentId, Vec<AgentId>> = HashMap::new();
    for node in &nodes {
        if let Some(parent) = node.parent {
            children.entry(parent).or_default().push(node.id);
        }
    }
    let rolled = roll_up(&nodes, &children, run);
    let mut usages = BTreeMap::new();
    for agent in shown {
        let (_, subtree) = rolled[&agent.id];
        let mut hidden = (0, Usage::default());
        let left_out = children.get(&agent.id).into_iter().flatten();
        for child in left_out.filter(|child| !visible.contains(*child)) {
            let (members, usage) = rolled[child];
            hidden.0 += members;
            hidden.1 += usage;
        }
        let hidden = (hidden.0 > 0).then_some(hidden.1);
        usages.insert(agent.id, TreeUsage { subtree, hidden });
    }
    assert_eq!(usages.len(), visible.len(), "one roll-up per shown agent");
    usages
}

/// Each node's counted members at or below it and their usage, folded from
/// the default by a post-order walk, so a child is in before its parent.
fn roll_up(
    nodes: &[UsageNode],
    children: &HashMap<AgentId, Vec<AgentId>>,
    run: Option<RunId>,
) -> HashMap<AgentId, (u64, Usage)> {
    let mut rolled: HashMap<AgentId, (u64, Usage)> = HashMap::with_capacity(nodes.len());
    for node in nodes {
        let counted = run.is_none_or(|run| node.run == run);
        let mut own = (0, Usage::default());
        if counted {
            own = (1, Usage::default());
            own.1 += node.usage;
        }
        rolled.insert(node.id, own);
    }
    let roots: Vec<AgentId> = nodes
        .iter()
        .filter(|n| n.parent.is_none_or(|p| !rolled.contains_key(&p)))
        .map(|n| n.id)
        .collect();
    let mut done: HashSet<AgentId> = HashSet::with_capacity(nodes.len());
    for root in roots {
        let mut stack = vec![(root, false)];
        while let Some((id, post)) = stack.pop() {
            let kids = children.get(&id).map(Vec::as_slice).unwrap_or_default();
            if !post {
                // A repeated id is a parent cycle in a corrupt store: walk it once.
                if done.insert(id) {
                    stack.push((id, true));
                    stack.extend(kids.iter().map(|kid| (*kid, false)));
                }
                continue;
            }
            let mut total = rolled[&id];
            for kid in kids {
                let (members, usage) = rolled[kid];
                total.0 += members;
                total.1 += usage;
            }
            rolled.insert(id, total);
        }
    }
    assert_eq!(rolled.len(), nodes.len(), "one total per node");
    rolled
}

/// The agents of `scope` above `shown` and not among them, nearest first.
fn ancestors_of(shown: &[Agent], scope: &HashMap<AgentId, Agent>) -> Vec<Agent> {
    let mut seen: HashSet<AgentId> = shown.iter().map(|agent| agent.id).collect();
    let mut above = Vec::new();
    for agent in shown {
        let mut parent = agent.parent;
        while let Some(id) = parent {
            let Some(ancestor) = scope.get(&id) else {
                break;
            };
            if !seen.insert(id) {
                break;
            }
            parent = ancestor.parent;
            above.push(ancestor.clone());
        }
    }
    debug_assert!(above.len() <= scope.len(), "each ancestor is added once");
    above
}
