//! What one `get_tree` page asks the store for, and what it gets back.

use std::collections::{HashMap, HashSet};

use aether_core::agent::Agent;
use aether_core::id::{AgentId, RunId};
use aether_core::model::Usage;

use crate::Result;

/// The most finished agents one tree page decodes; live ones come on top.
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
    /// Continue with the finished agents older than this one; `None` is the
    /// first page, which also holds every live agent in scope and the root.
    pub after: Option<AgentId>,
    /// At most this many finished agents, 1 to [`TREE_PAGE_MAX`].
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

/// One page of a tree's agents: on the first, the live ones and a rooted
/// tree's root, newest first; then finished ones, newest first; then the
/// in-scope ancestors of those not already shown, which a later page may repeat.
#[derive(Debug, Clone, PartialEq)]
pub struct TreePage {
    pub agents: Vec<Agent>,
    /// The oldest finished agent shown, when older ones were left for the
    /// next page: pass it as [`TreeQuery::after`].
    pub next_after: Option<AgentId>,
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
        "a page holds 1 to {TREE_PAGE_MAX} finished agents"
    );
    debug_assert!(scope.iter().all(|agent| query.selects(agent)));
    scope.sort_by(|a, b| b.id.cmp(&a.id));
    let by_id: HashMap<AgentId, Agent> = scope.iter().map(|a| (a.id, a.clone())).collect();
    let (pinned, finished): (Vec<Agent>, Vec<Agent>) = scope
        .into_iter()
        .partition(|agent| agent.is_live() || query.root == Some(agent.id));
    let mut older: Vec<Agent> = finished
        .into_iter()
        .filter(|agent| query.after.is_none_or(|after| agent.id < after))
        .collect();
    let next_after = (older.len() > query.limit).then(|| older[query.limit - 1].id);
    older.truncate(query.limit);
    let mut agents = if query.after.is_none() {
        pinned
    } else {
        Vec::new()
    };
    agents.extend(older);
    let above = ancestors_of(&agents, &by_id);
    agents.extend(above);
    Ok(TreePage { agents, next_after })
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
