//! Read-only projections of authoritative state for clients.

use std::collections::{BTreeMap, BTreeSet, HashMap};

use aether_core::agent::Agent;

use aether_core::id::{AgentId, RunId};
use aether_core::model::Usage;
use aether_core::replay::{self, ReplayFrame};
use aether_core::report::{self, RunReport};
use aether_protocol::{AgentTree, RunListing, TranscriptEntry, TreeNode};
use aether_runtime::Runtime;
use aether_store::{AgentFilter, EventFilter, Store, TaskFilter};

/// Which agents `get_tree` projects. Spend is rolled up over every agent of
/// the run whatever is shown; a hidden child's lands in its parent's `hidden_usage`.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct TreeFilter {
    pub include_archived: bool,
    /// Only agents whose role starts with this text.
    pub role_prefix: Option<String>,
    /// Only this agent and its descendants.
    pub root: Option<AgentId>,
}

impl TreeFilter {
    fn shows(&self, agent: &Agent, under_root: Option<&BTreeSet<AgentId>>) -> bool {
        (self.include_archived || !agent.archived)
            && self
                .role_prefix
                .as_deref()
                .is_none_or(|prefix| agent.role.as_str().starts_with(prefix))
            && under_root.is_none_or(|under| under.contains(&agent.id))
    }
}

/// Build the live agent graph for one run, or every run.
pub fn build_tree(
    store: &dyn Store,
    run: Option<RunId>,
    filter: &TreeFilter,
) -> aether_store::Result<AgentTree> {
    // Always roll usage up over the unfiltered set: an archived child's spend
    // still counts against its live ancestors' budgets even once hidden.
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
    let mut all_children: BTreeMap<AgentId, Vec<AgentId>> = BTreeMap::new();
    for agent in &all_agents {
        if let Some(p) = agent.parent {
            all_children.entry(p).or_default().push(agent.id);
        }
    }
    let subtree_usage = roll_up_usage(&all_agents, &all_children);
    let under_root = filter.root.map(|root| subtree_of(root, &all_children));
    let agents: Vec<_> = all_agents
        .into_iter()
        .filter(|agent| filter.shows(agent, under_root.as_ref()))
        .collect();
    let visible: BTreeSet<_> = agents.iter().map(|agent| agent.id).collect();
    assert!(
        visible.len() <= subtree_usage.len(),
        "only agents of the run are shown"
    );

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

/// Each agent's own usage plus every descendant's, by a post-order walk over
/// the parent/child structure, so a child is always folded in before its
/// parent. Not by creation order: the wall clock may step backwards between
/// a parent's creation and its child's.
fn roll_up_usage(
    all_agents: &[Agent],
    all_children: &BTreeMap<AgentId, Vec<AgentId>>,
) -> BTreeMap<AgentId, Usage> {
    let all_ids: BTreeSet<AgentId> = all_agents.iter().map(|a| a.id).collect();
    let mut subtree_usage: BTreeMap<AgentId, Usage> =
        all_agents.iter().map(|a| (a.id, a.stats.usage)).collect();
    let roots = all_agents
        .iter()
        .filter(|a| a.parent.is_none_or(|p| !all_ids.contains(&p)))
        .map(|a| a.id);
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
    assert_eq!(subtree_usage.len(), all_ids.len(), "one total per agent");
    subtree_usage
}

/// `root` and every agent below it.
fn subtree_of(root: AgentId, all_children: &BTreeMap<AgentId, Vec<AgentId>>) -> BTreeSet<AgentId> {
    let mut under = BTreeSet::from([root]);
    let mut stack = vec![root];
    while let Some(id) = stack.pop() {
        for child in all_children.get(&id).into_iter().flatten() {
            // A parent cycle in a corrupt store must not loop forever.
            if under.insert(*child) {
                stack.push(*child);
            }
        }
    }
    assert!(under.contains(&root));
    under
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

/// One page of an agent's transcript, as `get_transcript_page` replies.
#[derive(Debug, Clone, PartialEq)]
pub struct TranscriptPage {
    pub entries: Vec<TranscriptEntry>,
    pub has_before: bool,
    pub has_after: bool,
    /// The agent's newest transcript entry, 0 before its first.
    pub last_seq: u64,
}

/// Up to `limit` transcript entries in seq order: past `after`, below
/// `before`, or the newest when neither is given.
pub fn transcript_page(
    store: &dyn Store,
    agent: AgentId,
    after: Option<u64>,
    before: Option<u64>,
    limit: usize,
) -> aether_store::Result<TranscriptPage> {
    assert!(limit > 0, "a page holds at least one entry");
    assert!(after.is_none() || before.is_none(), "one cursor at most");
    let filter = EventFilter {
        types: vec!["agent.message".into()],
        ..EventFilter::own_events(agent)
    };
    let last_seq = store
        .latest_events(1, &filter)?
        .first()
        .map_or(0, |event| event.seq);
    let (events, has_before, has_after) = match after {
        Some(after) => {
            let mut events = store.events(after, limit + 1, &filter)?;
            let has_after = events.len() > limit;
            events.truncate(limit);
            let has_before = !store
                .events_before(after.saturating_add(1), 1, &filter)?
                .is_empty();
            (events, has_before, has_after)
        }
        None => {
            let mut events = match before {
                Some(before) => store.events_before(before, limit + 1, &filter)?,
                None => store.latest_events(limit + 1, &filter)?,
            };
            let has_before = events.len() > limit;
            events.truncate(limit);
            events.reverse();
            let has_after = before.is_some_and(|before| last_seq >= before);
            (events, has_before, has_after)
        }
    };
    let entries: Vec<TranscriptEntry> = events.into_iter().filter_map(transcript_entry).collect();
    assert!(entries.windows(2).all(|pair| pair[0].seq < pair[1].seq));
    assert!(entries.last().is_none_or(|entry| entry.seq <= last_seq));
    Ok(TranscriptPage {
        entries,
        has_before,
        has_after,
        last_seq,
    })
}

fn transcript_entry(event: aether_core::event::Event) -> Option<TranscriptEntry> {
    match event.kind {
        aether_core::event::EventKind::AgentMessage {
            role,
            content,
            invocation,
        } => Some(TranscriptEntry {
            seq: event.seq,
            at: event.at,
            message: aether_core::model::Message { role, content },
            invocation,
        }),
        _ => None,
    }
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
            engine_alive: false,
            last_event_at: None,
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

        let tree = build_tree(&store, Some(run), &TreeFilter::default()).expect("tree");

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

        let tree = build_tree(&store, Some(run), &TreeFilter::default()).expect("tree");

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

        let tree = build_tree(
            &store,
            Some(run),
            &TreeFilter {
                include_archived: true,
                ..TreeFilter::default()
            },
        )
        .expect("tree");

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

        let tree = build_tree(&store, Some(run), &TreeFilter::default()).expect("tree");
        assert_eq!(
            tree.nodes[&grandparent.id].subtree_usage.input_tokens, 111,
            "every generation's usage must reach the grandparent regardless of store order"
        );
        assert_eq!(tree.nodes[&parent.id].subtree_usage.input_tokens, 11);
    }

    /// A root narrows the tree to its subtree, a role prefix to the roles it
    /// names; a filtered-out child's spend still reaches its shown parent.
    #[test]
    fn a_rooted_role_filtered_tree_shows_only_what_it_names() {
        let store = aether_store::SqliteStore::open_in_memory().expect("open");
        let run = RunId::new();
        let top = agent(None, run, false, 1000);
        let mut lead = agent(Some(top.id), run, false, 100);
        lead.role = Role::new("lead");
        let worker = agent(Some(lead.id), run, false, 10);
        let mut reviewer = agent(Some(lead.id), run, false, 1);
        reviewer.role = Role::new("reviewer");
        let sibling = agent(Some(top.id), run, false, 5);
        for a in [&top, &lead, &worker, &reviewer, &sibling] {
            upsert(&store, a);
        }

        let rooted = TreeFilter {
            root: Some(lead.id),
            ..TreeFilter::default()
        };
        let tree = build_tree(&store, Some(run), &rooted).expect("tree");
        assert_eq!(tree.roots, vec![lead.id]);
        let shown: BTreeSet<_> = tree.nodes.keys().copied().collect();
        assert_eq!(shown, BTreeSet::from([lead.id, worker.id, reviewer.id]));
        assert_eq!(tree.nodes[&lead.id].subtree_usage.input_tokens, 111);

        let workers = TreeFilter {
            root: Some(lead.id),
            role_prefix: Some("work".into()),
            ..TreeFilter::default()
        };
        let tree = build_tree(&store, Some(run), &workers).expect("tree");
        assert_eq!(
            tree.roots,
            vec![worker.id],
            "a shown agent without a shown parent is a root"
        );
        assert_eq!(tree.nodes.len(), 1);

        let leads = TreeFilter {
            role_prefix: Some("lead".into()),
            ..TreeFilter::default()
        };
        let tree = build_tree(&store, Some(run), &leads).expect("tree");
        assert_eq!(tree.roots, vec![lead.id]);
        assert_eq!(
            tree.nodes[&lead.id].hidden_usage.map(|u| u.input_tokens),
            Some(11),
            "the hidden worker's and reviewer's spend"
        );
    }
}
