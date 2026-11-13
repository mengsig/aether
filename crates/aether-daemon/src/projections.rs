//! Read-only projections of authoritative state for clients.

use std::collections::{BTreeMap, BTreeSet, HashMap};

use aether_core::agent::Agent;

use aether_core::id::{AgentId, RunId, TaskId};
use aether_core::lease::Lease;
use aether_core::model::Usage;
use aether_core::replay::{self, ReplayFrame};
use aether_core::report::{self, RunReport};
use aether_core::task::Task;
use aether_protocol::{AgentTree, RunListing, TranscriptEntry, TreeNode};
use aether_runtime::Runtime;
use aether_store::{AgentFilter, EventFilter, Store, TREE_PAGE_MAX, TaskFilter, TreeQuery};

/// Which agents `get_tree` projects. Spend is rolled up over every agent
/// below a shown one up to the next shown ones; what is not shown lands in the
/// `hidden_usage` of the nearest shown agent above it. Within a run, only that run's agents count.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct TreeFilter {
    pub include_archived: bool,
    /// Only agents whose role starts with this text.
    pub role_prefix: Option<String>,
    /// Only this agent and its descendants.
    pub root: Option<AgentId>,
    /// The page of agents older than this one.
    pub after: Option<AgentId>,
    /// At most this many agents walked; [`TREE_PAGE_MAX`] when absent.
    pub limit: Option<usize>,
}

impl TreeFilter {
    fn query(&self, run: Option<RunId>) -> TreeQuery {
        let limit = self.limit.unwrap_or(TREE_PAGE_MAX);
        assert!(
            (1..=TREE_PAGE_MAX).contains(&limit),
            "a page limit is validated before the read"
        );
        TreeQuery {
            run,
            root: self.root,
            include_archived: self.include_archived,
            role_prefix: self.role_prefix.clone(),
            after: self.after,
            limit,
        }
    }
}

/// Build one page of the live agent graph for one run, or every run.
pub fn build_tree(
    store: &dyn Store,
    run: Option<RunId>,
    filter: &TreeFilter,
) -> aether_store::Result<AgentTree> {
    let page = store.tree_page(&filter.query(run))?;
    let mut agents = page.agents;
    // A page comes newest first; the reply lists roots and children oldest first.
    agents.sort_unstable_by_key(|agent| agent.id);
    let visible: BTreeSet<AgentId> = agents.iter().map(|agent| agent.id).collect();
    assert_eq!(visible.len(), agents.len(), "a page shows an agent once");
    let usages = store.tree_usages(&agents, run)?;
    assert_eq!(usages.len(), visible.len(), "every shown agent has a total");
    let leases = shown_leases(store, &agents)?;
    let tasks = current_tasks(store, &agents)?;
    let mut nodes: BTreeMap<AgentId, TreeNode> = BTreeMap::new();
    let (roots, mut children) = link(&agents, &visible);
    for agent in agents {
        let task = agent.current_task.and_then(|id| tasks.get(&id).cloned());
        let waiting_on = Runtime::waiting_on(&agent).into_iter().collect();
        let usage = usages.get(&agent.id).copied();
        nodes.insert(
            agent.id,
            TreeNode {
                children: children.remove(&agent.id).unwrap_or_default(),
                task,
                waiting_on,
                lease: leases.get(&agent.id).cloned(),
                subtree_usage: usage.map(|usage| usage.subtree).unwrap_or_default(),
                hidden_usage: usage.and_then(|usage| usage.hidden),
                agent,
            },
        );
    }
    Ok(AgentTree {
        roots,
        nodes,
        stale: None,
        next_after: page.next_after,
    })
}

/// The lease each shown agent holds, read by its key: the fleet's leases are
/// never all decoded for the few the tree shows.
fn shown_leases(
    store: &dyn Store,
    agents: &[Agent],
) -> aether_store::Result<BTreeMap<AgentId, Lease>> {
    let mut leases = BTreeMap::new();
    for agent in agents {
        if let Some(lease) = store.lease(agent.id)? {
            assert_eq!(lease.agent_id, agent.id, "a lease read by agent is its own");
            leases.insert(agent.id, lease);
        }
    }
    assert!(leases.len() <= agents.len(), "at most one lease per agent");
    Ok(leases)
}

/// The task each shown agent is on, read by id: a long-lived store's task
/// history is never decoded for the few the tree shows.
fn current_tasks(
    store: &dyn Store,
    agents: &[Agent],
) -> aether_store::Result<HashMap<TaskId, Task>> {
    let ids: BTreeSet<TaskId> = agents
        .iter()
        .filter_map(|agent| agent.current_task)
        .collect();
    let mut tasks = HashMap::with_capacity(ids.len());
    for id in ids {
        if let Some(task) = store.task(id)? {
            assert_eq!(task.id, id, "a task read by id is that task");
            tasks.insert(id, task);
        }
    }
    assert!(
        tasks.len() <= agents.len(),
        "at most one current task per agent"
    );
    Ok(tasks)
}

/// The shown agents without a shown parent, and each shown agent's shown children.
fn link(
    agents: &[Agent],
    visible: &BTreeSet<AgentId>,
) -> (Vec<AgentId>, BTreeMap<AgentId, Vec<AgentId>>) {
    let mut children: BTreeMap<AgentId, Vec<AgentId>> = BTreeMap::new();
    let mut roots = Vec::new();
    for agent in agents {
        match agent.parent {
            Some(p) if visible.contains(&p) => children.entry(p).or_default().push(agent.id),
            _ => roots.push(agent.id),
        }
    }
    debug_assert_eq!(
        roots.len() + children.values().map(Vec::len).sum::<usize>(),
        agents.len()
    );
    (roots, children)
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
    let filter = EventFilter {
        types: vec!["agent.message".into()],
        ..EventFilter::own_events(agent)
    };
    let page = event_page(store, &filter, after, before, limit)?;
    let entries: Vec<TranscriptEntry> = page
        .events
        .into_iter()
        .filter_map(transcript_entry)
        .collect();
    assert!(entries.len() <= limit, "a page holds at most its limit");
    Ok(TranscriptPage {
        entries,
        has_before: page.has_before,
        has_after: page.has_after,
        last_seq: page.last_seq,
    })
}

/// One page of an agent's conversation, as `get_conversation` replies.
#[derive(Debug, Clone, PartialEq)]
pub struct ConversationPage {
    pub entries: Vec<aether_core::conversation::Entry>,
    /// The seq range of the events the page read, 0 and 0 when it read none.
    pub from_seq: u64,
    pub to_seq: u64,
    pub has_before: bool,
    pub has_after: bool,
    pub last_seq: u64,
}

/// The conversation entries of up to `limit` of the agent's events, paged
/// like [`transcript_page`]; `important_only` drops the rest.
pub fn conversation_page(
    store: &dyn Store,
    agent: AgentId,
    after: Option<u64>,
    before: Option<u64>,
    limit: usize,
    important_only: bool,
) -> aether_store::Result<ConversationPage> {
    let filter = EventFilter {
        types: aether_core::conversation::EVENT_TYPES
            .iter()
            .map(|name| (*name).to_owned())
            .collect(),
        ..EventFilter::own_events(agent)
    };
    let page = event_page(store, &filter, after, before, limit)?;
    let from_seq = page.events.first().map_or(0, |event| event.seq);
    let to_seq = page.events.last().map_or(0, |event| event.seq);
    let entries: Vec<_> = page
        .events
        .iter()
        .flat_map(aether_core::conversation::entries)
        .filter(|entry| entry.important || !important_only)
        .collect();
    assert!(from_seq <= to_seq, "a page's events are in seq order");
    assert!(
        entries
            .iter()
            .all(|entry| (from_seq..=to_seq).contains(&entry.seq)),
        "every entry comes from an event the page read"
    );
    Ok(ConversationPage {
        entries,
        from_seq,
        to_seq,
        has_before: page.has_before,
        has_after: page.has_after,
        last_seq: page.last_seq,
    })
}

/// Up to `limit` events matching `filter`, in seq order, with whether more
/// lie on either side and the newest matching seq.
struct EventPage {
    events: Vec<aether_core::event::Event>,
    has_before: bool,
    has_after: bool,
    last_seq: u64,
}

fn event_page(
    store: &dyn Store,
    filter: &EventFilter,
    after: Option<u64>,
    before: Option<u64>,
    limit: usize,
) -> aether_store::Result<EventPage> {
    assert!(limit > 0, "a page holds at least one entry");
    assert!(after.is_none() || before.is_none(), "one cursor at most");
    let (events, has_before, has_after) = match after {
        Some(after) => {
            let mut events = store.events(after, limit + 1, filter)?;
            let has_after = events.len() > limit;
            events.truncate(limit);
            let has_before = !store
                .events_before(after.saturating_add(1), 1, filter)?
                .is_empty();
            (events, has_before, has_after)
        }
        None => {
            let mut events = match before {
                Some(before) => store.events_before(before, limit + 1, filter)?,
                None => store.latest_events(limit + 1, filter)?,
            };
            let has_before = events.len() > limit;
            events.truncate(limit);
            events.reverse();
            (events, has_before, false)
        }
    };
    // Read after the page and from its own transaction: an entry committed
    // in between raises it, so it is never below the page's newest entry.
    let newest = store
        .latest_events(1, filter)?
        .first()
        .map(|event| event.seq);
    let page_end = events
        .last()
        .map(|event| event.seq)
        .or(after)
        .or(before.map(|before| before.saturating_sub(1)))
        .unwrap_or(0);
    let has_after = has_after || newest_is_past(page_end, newest);
    let last_seq = newest
        .into_iter()
        .chain(events.last().map(|event| event.seq))
        .max()
        .unwrap_or(0);
    assert!(events.windows(2).all(|pair| pair[0].seq < pair[1].seq));
    assert!(events.len() <= limit, "a page holds at most its limit");
    Ok(EventPage {
        events,
        has_before,
        has_after,
        last_seq,
    })
}

/// Whether the newest entry lies beyond where the page ends, whatever the
/// page's own read saw: a commit between the two reads still shows.
fn newest_is_past(page_end: u64, newest: Option<u64>) -> bool {
    newest.is_some_and(|seq| seq > page_end)
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

    fn say(store: &dyn Store, agent: AgentId, text: &str) -> u64 {
        let kind = EventKind::AgentMessage {
            role: aether_core::model::MessageRole::User,
            content: vec![aether_core::model::ContentBlock::text(text)],
            invocation: None,
        };
        let noise =
            EventDraft::new(Actor::System, EventKind::AgentStarted { turn: 1 }).agent(agent);
        let events = aether_store::Store::commit(
            store,
            Commit::new()
                .event(noise)
                .event(EventDraft::new(Actor::System, kind).agent(agent)),
        )
        .expect("commit");
        events[1].seq
    }

    /// Five messages among other events: the newest page, then each cursor
    /// both ways, report the entries beyond them.
    #[test]
    fn a_transcript_pages_by_seq_both_ways() {
        let store = aether_store::SqliteStore::open_in_memory().expect("open");
        let agent = AgentId::new();
        let empty = transcript_page(&store, agent, None, None, 2).expect("empty");
        assert_eq!((empty.entries.len(), empty.last_seq), (0, 0));
        assert!(!empty.has_before && !empty.has_after);
        let empty = transcript_page(&store, agent, None, Some(0), 2).expect("before 0");
        assert!(!empty.has_after, "no entry lies at or past the cursor");
        let seqs: Vec<u64> = (0..5)
            .map(|i| say(&store, agent, &format!("m{i}")))
            .collect();
        let page_seqs =
            |page: &TranscriptPage| page.entries.iter().map(|e| e.seq).collect::<Vec<_>>();

        let newest = transcript_page(&store, agent, None, None, 2).expect("newest");
        assert_eq!(page_seqs(&newest), &seqs[3..]);
        assert!(newest.has_before && !newest.has_after);
        assert_eq!(newest.last_seq, seqs[4]);

        let older = transcript_page(&store, agent, None, Some(seqs[3]), 2).expect("before");
        assert_eq!(page_seqs(&older), &seqs[1..3]);
        assert!(older.has_before && older.has_after);
        let oldest = transcript_page(&store, agent, None, Some(seqs[1]), 2).expect("oldest");
        assert_eq!(page_seqs(&oldest), &seqs[..1]);
        assert!(!oldest.has_before && oldest.has_after);

        let from_start = transcript_page(&store, agent, Some(0), None, 3).expect("after 0");
        assert_eq!(page_seqs(&from_start), &seqs[..3]);
        assert!(!from_start.has_before && from_start.has_after);
        let rest = transcript_page(&store, agent, Some(seqs[2]), None, 3).expect("after");
        assert_eq!(page_seqs(&rest), &seqs[3..]);
        assert!(rest.has_before && !rest.has_after);
        assert_eq!(rest.last_seq, seqs[4]);
    }

    #[test]
    fn a_commit_after_the_page_read_still_reports_has_after() {
        assert!(newest_is_past(4, Some(5)));
        assert!(!newest_is_past(5, Some(5)));
        assert!(
            newest_is_past(0, Some(5)),
            "the first entry of an empty page"
        );
        assert!(!newest_is_past(0, None));
    }

    /// Delegation may cross runs: a run filter keeps a same-run grandchild
    /// whose intermediate parent ran elsewhere, counted at its own root only.
    #[test]
    fn a_rooted_run_filtered_tree_follows_a_cross_run_intermediate() {
        let store = aether_store::SqliteStore::open_in_memory().expect("open");
        let (run_a, run_b) = (RunId::new(), RunId::new());
        let root = agent(None, run_a, false, 1);
        let middle = agent(Some(root.id), run_b, false, 10);
        let leaf = agent(Some(middle.id), run_a, false, 100);
        for a in [&root, &middle, &leaf] {
            upsert(&store, a);
        }
        let rooted = TreeFilter {
            root: Some(root.id),
            ..TreeFilter::default()
        };
        let tree = build_tree(&store, Some(run_a), &rooted).expect("tree");
        let shown: BTreeSet<_> = tree.nodes.keys().copied().collect();
        assert_eq!(shown, BTreeSet::from([root.id, leaf.id]));
        assert_eq!(tree.roots.len(), 2, "the leaf's parent is not shown");
        let top = &tree.nodes[&root.id];
        assert_eq!(
            top.subtree_usage.input_tokens, 1,
            "the leaf is a root of its own, and b's spend is not counted"
        );
        assert_eq!(top.hidden_usage, None, "only b's agent is left out");
        assert_eq!(tree.nodes[&leaf.id].subtree_usage.input_tokens, 100);
        assert_eq!(roots_total(&tree), 101, "each agent of run a once");
    }

    /// What the shown roots sum to, as a client's total adds them up.
    fn roots_total(tree: &AgentTree) -> u64 {
        let total: u64 = tree
            .roots
            .iter()
            .map(|root| tree.nodes[root].subtree_usage.input_tokens)
            .sum();
        assert!(tree.roots.iter().all(|root| tree.nodes.contains_key(root)));
        total
    }

    /// A live agent under an archived one is shown apart from its shown
    /// grandparent: its spend is counted at its own root, not hidden above it too.
    #[test]
    fn a_shown_agent_under_a_hidden_one_is_counted_once() {
        let store = aether_store::SqliteStore::open_in_memory().expect("open");
        let run = RunId::new();
        let top = agent(None, run, false, 1);
        let middle = agent(Some(top.id), run, true, 10);
        let leaf = agent(Some(middle.id), run, false, 100);
        for a in [&top, &middle, &leaf] {
            upsert(&store, a);
        }
        let tree = build_tree(&store, Some(run), &TreeFilter::default()).expect("tree");
        assert_eq!(tree.roots, vec![top.id, leaf.id]);
        let shown = &tree.nodes[&top.id];
        assert_eq!(shown.hidden_usage.map(|u| u.input_tokens), Some(10));
        assert_eq!(shown.subtree_usage.input_tokens, 11);
        assert_eq!(roots_total(&tree), 111, "each agent once");
    }
}
