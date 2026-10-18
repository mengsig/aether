//! Read-only projections of authoritative state for clients.

use std::collections::{BTreeMap, HashMap};

use aether_core::id::{AgentId, RunId};
use aether_core::model::Usage;
use aether_core::replay::{self, ReplayFrame};
use aether_core::report::{self, RunReport};
use aether_protocol::{AgentTree, RunListing, TreeNode};
use aether_runtime::Runtime;
use aether_store::{AgentFilter, EventFilter, Store, TaskFilter};

/// Build the live agent graph for one run, or every run.
pub fn build_tree(
    store: &dyn Store,
    run: Option<RunId>,
    include_archived: bool,
) -> aether_store::Result<AgentTree> {
    // Always roll usage up over the unfiltered set: an archived child's spend
    // still counts against its live ancestors' budgets even once hidden from
    // the tree. `exclude_archived` below only trims what gets projected.
    let all_agents = store.agents(&AgentFilter {
        run,
        exclude_archived: false,
        ..Default::default()
    })?;
    let tasks = store.tasks(&TaskFilter {
        run,
        ..Default::default()
    })?;
    let leases: BTreeMap<AgentId, _> = store
        .leases()?
        .into_iter()
        .map(|l| (l.agent_id, l))
        .collect();

    // Roll each agent's usage into every live ancestor with a post-order
    // walk over the parent/child structure itself, so a child is always
    // folded in before its own parent is. Not a reverse creation-order walk
    // over the store's ASC `created_at` order:
    // verified `to_rfc3339`'s "+00:00" suffix keeps that order
    // chronological here (a digit always sorts after it, unlike a 'Z'
    // suffix), but nothing guarantees the wall clock itself never steps
    // backward between a parent's creation and its child's - the store
    // layer promises nothing about ordering either way, so this does not
    // depend on it.
    let mut all_children: BTreeMap<AgentId, Vec<AgentId>> = BTreeMap::new();
    let all_ids: std::collections::BTreeSet<AgentId> = all_agents.iter().map(|a| a.id).collect();
    for agent in &all_agents {
        if let Some(p) = agent.parent {
            all_children.entry(p).or_default().push(agent.id);
        }
    }
    let mut subtree_usage: BTreeMap<AgentId, Usage> =
        all_agents.iter().map(|a| (a.id, a.stats.usage)).collect();
    let roots: Vec<AgentId> = all_agents
        .iter()
        .filter(|a| a.parent.is_none_or(|p| !all_ids.contains(&p)))
        .map(|a| a.id)
        .collect();
    for root in roots {
        let mut stack = vec![(root, false)];
        while let Some((id, post)) = stack.pop() {
            let Some(kids) = all_children.get(&id) else {
                continue;
            };
            if post {
                let mut total = subtree_usage[&id];
                for child in kids {
                    total += subtree_usage[child];
                }
                subtree_usage.insert(id, total);
                continue;
            }
            stack.push((id, true));
            stack.extend(kids.iter().map(|c| (*c, false)));
        }
    }

    let agents: Vec<_> = if include_archived {
        all_agents
    } else {
        all_agents.into_iter().filter(|a| !a.archived).collect()
    };
    let visible: std::collections::BTreeSet<_> = agents.iter().map(|agent| agent.id).collect();

    let tasks: HashMap<_, _> = tasks.into_iter().map(|t| (t.id, t)).collect();
    let mut nodes: BTreeMap<AgentId, TreeNode> = BTreeMap::new();
    let mut children: BTreeMap<AgentId, Vec<AgentId>> = BTreeMap::new();
    let mut roots = Vec::new();
    for agent in &agents {
        match agent.parent {
            Some(p) if visible.contains(&p) => children.entry(p).or_default().push(agent.id),
            _ => roots.push(agent.id),
        }
    }
    for agent in agents {
        let task = agent.current_task.and_then(|id| tasks.get(&id).cloned());
        let waiting_on = Runtime::waiting_on(&agent).into_iter().collect();
        let subtree = subtree_usage.get(&agent.id).copied().unwrap_or_default();
        let hidden_usage = all_children.get(&agent.id).and_then(|children| {
            let mut hidden = children.iter().filter(|id| !visible.contains(*id));
            let mut total = subtree_usage[hidden.next()?];
            for child in hidden {
                total += subtree_usage[child];
            }
            Some(total)
        });
        nodes.insert(
            agent.id,
            TreeNode {
                children: children.remove(&agent.id).unwrap_or_default(),
                task,
                waiting_on,
                lease: leases.get(&agent.id).cloned(),
                subtree_usage: subtree,
                hidden_usage,
                agent,
            },
        );
    }
    Ok(AgentTree { roots, nodes })
}

/// Every run the store knows about, newest first. Projected from agents and
/// their tasks, so a finished run is listed as long as its agents are.
pub fn list_runs(store: &dyn Store, limit: usize) -> aether_store::Result<Vec<RunListing>> {
    let agents = store.agents(&AgentFilter::default())?;
    let tasks = store.tasks(&TaskFilter::default())?;
    let mut by_run: BTreeMap<RunId, RunListing> = BTreeMap::new();
    for agent in &agents {
        let listing = by_run.entry(agent.run).or_insert_with(|| RunListing {
            run: agent.run,
            objective: None,
            agents: 0,
            agents_live: 0,
            agents_imported: 0,
            usage: Usage::default(),
            started_at: agent.created_at,
            updated_at: agent.updated_at,
        });
        listing.agents += 1;
        listing.agents_live += u32::from(agent.is_live());
        listing.agents_imported +=
            u32::from(agent.status.kind() == aether_core::agent::AgentStatusKind::Imported);
        listing.usage += agent.stats.usage;
        listing.started_at = listing.started_at.min(agent.created_at);
        listing.updated_at = listing.updated_at.max(agent.updated_at);
    }
    // The run's objective is its oldest task; tasks come back creation-ordered.
    for task in tasks {
        if let Some(listing) = by_run.get_mut(&task.run)
            && listing.objective.is_none()
        {
            listing.objective = Some(task.objective);
        }
    }
    let mut runs: Vec<RunListing> = by_run.into_values().collect();
    runs.sort_by_key(|r| std::cmp::Reverse(r.started_at));
    runs.truncate(limit);
    Ok(runs)
}

/// Fold a run's whole event log into its report, paging so a long run never
/// has to fit in memory at once.
pub fn build_report(store: &dyn Store, run: RunId) -> aether_store::Result<RunReport> {
    const PAGE: usize = 2_000;
    let filter = EventFilter::for_run(run);
    let mut builder = report::Builder::new(run);
    let mut after = 0u64;
    loop {
        let page = store.events(after, PAGE, &filter)?;
        let Some(last) = page.last() else { break };
        after = last.seq;
        for event in &page {
            builder.push(event);
        }
        if page.len() < PAGE {
            break;
        }
    }
    Ok(builder.finish(aether_core::time::now()))
}

/// One agent's turns as they happened. Every event of the agent is folded,
/// and the frames are bounded by its turns rather than its events.
pub fn replay(store: &dyn Store, agent: AgentId) -> aether_store::Result<Vec<ReplayFrame>> {
    const PAGE: usize = 2_000;
    // The fold keeps only the agent's own events, so the children's are not
    // read at all; the `(agent_id, seq)` index serves this.
    let filter = EventFilter::own_events(agent);
    let mut events = Vec::new();
    let mut after = 0u64;
    loop {
        let page = store.events(after, PAGE, &filter)?;
        let Some(last) = page.last() else { break };
        after = last.seq;
        let count = page.len();
        events.extend(page);
        if count < PAGE {
            break;
        }
    }
    Ok(replay::build(agent, &events))
}

#[cfg(test)]
mod tests {
    use super::*;
    use aether_core::agent::{Agent, AgentConfig, AgentStats, AgentStatus, Role, Scope};
    use aether_core::event::{Actor, EventDraft, EventKind};
    use aether_core::model::{ModelRef, Usage};
    use aether_core::time::now;
    use aether_store::{Commit, Mutation};

    fn agent(parent: Option<AgentId>, run: RunId, archived: bool, input_tokens: u64) -> Agent {
        let at = now();
        Agent {
            id: AgentId::new(),
            parent,
            role: Role::new("worker"),
            name: "worker".into(),
            scope: Scope::default(),
            model: ModelRef::new("mock", "m"),
            fallbacks: Vec::new(),
            status: AgentStatus::Completed { summary: None },
            current_task: None,
            inbox_cursor: 0,
            checkpoint: None,
            engine_session: None,
            engine_usage: None,
            engine_sent: None,
            status_since: None,
            archived,
            paused: false,
            paused_via: None,
            resume_wait: None,
            restarted_from: None,
            restarted_as: None,
            config: AgentConfig::default(),
            stats: AgentStats {
                usage: Usage {
                    input_tokens,
                    ..Default::default()
                },
                ..Default::default()
            },
            run,
            created_at: at,
            updated_at: at,
        }
    }

    fn upsert(store: &dyn Store, a: &Agent) {
        aether_store::Store::commit(
            store,
            Commit::new()
                .event(EventDraft::new(Actor::System, EventKind::AgentReady).agent(a.id))
                .mutate(Mutation::UpsertAgent(a.clone())),
        )
        .expect("commit");
    }

    /// An archived child's cost must still count toward its live
    /// parent's subtree usage, even though the child itself is hidden.
    #[test]
    fn subtree_usage_rolls_up_archived_children() {
        let store = aether_store::SqliteStore::open_in_memory().expect("open");
        let run = RunId::new();
        let parent = agent(None, run, false, 100);
        let child = agent(Some(parent.id), run, true, 50);
        upsert(&store, &parent);
        upsert(&store, &child);

        let tree = build_tree(&store, Some(run), false).expect("tree");

        assert!(!tree.nodes.contains_key(&child.id), "archived child hidden");
        let parent_node = tree.nodes.get(&parent.id).expect("parent visible");
        assert_eq!(parent_node.hidden_usage, Some(child.stats.usage));
        assert_eq!(
            parent_node.subtree_usage.input_tokens, 150,
            "archived child's usage still rolls up into the live parent"
        );
    }

    /// 2,000 agents of which 100 are visible: every node resolves its
    /// children and task through lookups, not scans of the whole fleet.
    #[test]
    fn a_large_fleet_projects_only_its_visible_agents_with_their_children() {
        let store = aether_store::SqliteStore::open_in_memory().expect("open");
        let run = RunId::new();
        let root = agent(None, run, false, 1);
        upsert(&store, &root);
        let mut visible_children = Vec::new();
        for i in 0..1_999 {
            let child = agent(Some(root.id), run, i >= 99, 1);
            if !child.archived {
                visible_children.push(child.id);
            }
            upsert(&store, &child);
        }

        let tree = build_tree(&store, Some(run), false).expect("tree");

        assert_eq!(tree.nodes.len(), 100);
        assert_eq!(tree.roots, vec![root.id]);
        assert_eq!(tree.nodes[&root.id].children, visible_children);
        assert_eq!(tree.nodes[&root.id].subtree_usage.input_tokens, 2_000);
        assert_eq!(
            tree.nodes[&root.id].hidden_usage.map(|u| u.input_tokens),
            Some(1_900)
        );
    }

    #[test]
    fn include_archived_shows_the_child_too() {
        let store = aether_store::SqliteStore::open_in_memory().expect("open");
        let run = RunId::new();
        let parent = agent(None, run, false, 100);
        let child = agent(Some(parent.id), run, true, 50);
        upsert(&store, &parent);
        upsert(&store, &child);

        let tree = build_tree(&store, Some(run), true).expect("tree");

        assert!(tree.nodes.contains_key(&child.id));
        assert_eq!(tree.nodes[&parent.id].hidden_usage, None);
        assert_eq!(tree.nodes[&parent.id].subtree_usage.input_tokens, 150);
    }

    /// Nothing guarantees the store returns
    /// `agents` in creation order relative to parent/child. A clock that
    /// steps backward between a parent's creation and its child's is
    /// enough; `to_rfc3339`'s digit-count quirk turned out not to reproduce
    /// it with this codebase's actual `+00:00`-suffixed timestamps
    /// (verified with a throwaway chrono probe), but the store layer
    /// promises nothing about ordering either way. Set the middle agent's
    /// `created_at` later than its own child's, so the store's ASC order
    /// comes back grandparent, child, parent (inverted at the parent/child
    /// boundary), and check the grandparent's rollup still counts every
    /// generation.
    #[test]
    fn subtree_usage_rolls_up_correctly_even_when_the_store_returns_a_child_before_its_parent() {
        use chrono::{TimeZone, Utc};

        let store = aether_store::SqliteStore::open_in_memory().expect("open");
        let run = RunId::new();
        let base = Utc.timestamp_opt(1_700_000_000, 0).unwrap();
        let at = |mut a: Agent, when: aether_core::time::Timestamp| {
            a.created_at = when;
            a.updated_at = when;
            a
        };
        let grandparent = at(agent(None, run, false, 100), base);
        // Explicit timestamps, not `now()` twice in a row: the parent's is
        // deliberately later than its own child's, so nothing about the
        // ordering depends on how fast two clock reads happen to run.
        let parent = at(
            agent(Some(grandparent.id), run, false, 10),
            base + chrono::Duration::seconds(2),
        );
        let child = at(
            agent(Some(parent.id), run, false, 1),
            base + chrono::Duration::seconds(1),
        );
        upsert(&store, &grandparent);
        upsert(&store, &parent);
        upsert(&store, &child);

        let stored_order: Vec<_> = aether_store::Store::agents(&store, &AgentFilter::default())
            .expect("agents")
            .into_iter()
            .map(|a| a.id)
            .collect();
        assert_eq!(
            stored_order,
            vec![grandparent.id, child.id, parent.id],
            "test setup: the store must actually return the child before its parent"
        );

        let tree = build_tree(&store, Some(run), false).expect("tree");
        assert_eq!(
            tree.nodes[&grandparent.id].subtree_usage.input_tokens, 111,
            "every generation's usage must reach the grandparent regardless of store order"
        );
        assert_eq!(tree.nodes[&parent.id].subtree_usage.input_tokens, 11);
    }
}
