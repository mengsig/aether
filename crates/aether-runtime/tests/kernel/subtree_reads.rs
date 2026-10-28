//! Per-turn, per-delegation and per-command agent reads stay inside the
//! subtree they act on instead of decoding every agent the store ever held.
use super::lease_reads::CountingStore;
use super::*;
use aether_core::agent::subtree_of;

struct Fleet {
    root: AgentId,
    /// Everything under `root`, root included.
    subtree: Vec<AgentId>,
    /// Stored agents outside the subtree.
    outside: usize,
}

fn spend(store: &CountingStore, id: AgentId, tokens: u64, change: impl FnOnce(&mut Agent)) {
    let mut row = store.agent(id).expect("read").expect("row");
    row.stats.usage.output_tokens = tokens;
    change(&mut row);
    store
        .commit(Commit::new().mutate(Mutation::UpsertAgent(row)))
        .expect("store the row");
}

fn child(h: &Harness, parent: AgentId) -> AgentId {
    let mut s = spec("unused");
    s.objective = None;
    s.parent = Some(parent);
    h.runtime
        .create_agent(s, Actor::user())
        .expect("child")
        .0
        .id
}

/// A root with completed, cancelled, paused, archived and live descendants
/// (one live grandchild under a finished child), and 25 finished agents
/// elsewhere. Its own and its descendants' spend sums to 900 tokens.
async fn fleet(h: &Harness, store: &CountingStore) -> Fleet {
    let mut s = idle_spec();
    s.config.as_mut().expect("config").token_budget = Some(1_000_000);
    let root = h.runtime.create_agent(s, Actor::user()).expect("root").0.id;
    let done = child(h, root);
    let stopped = child(h, root);
    let paused = child(h, root);
    let archived = child(h, root);
    let live = child(h, root);
    let grandchild = child(h, done);
    let failed = child(h, stopped);
    spend(store, root, 50, |_| {});
    spend(store, done, 200, |a| {
        a.status = AgentStatus::Completed { summary: None }
    });
    spend(store, stopped, 300, |a| {
        a.status = AgentStatus::Cancelled { reason: None }
    });
    spend(store, paused, 100, |a| a.paused = true);
    spend(store, archived, 150, |a| a.archived = true);
    spend(store, live, 0, |_| {});
    spend(store, grandchild, 0, |_| {});
    spend(store, failed, 100, |a| {
        a.status = AgentStatus::Failed { error: "x".into() }
    });
    let outside = 25;
    for _ in 0..outside {
        let id = h
            .runtime
            .create_agent(idle_spec(), Actor::user())
            .expect("other")
            .0
            .id;
        spend(store, id, 9_999, |a| {
            a.status = AgentStatus::Completed { summary: None };
            a.archived = true;
        });
    }
    let all = store.agents(&AgentFilter::default()).expect("fleet");
    let subtree = subtree_of(&all, root);
    assert_eq!(subtree.len(), 8, "{subtree:?}");
    Fleet {
        root,
        subtree,
        outside,
    }
}

fn counting_store() -> Arc<CountingStore> {
    CountingStore::new(Arc::new(SqliteStore::open_in_memory().expect("store")))
}

async fn quiet_harness(store: &Arc<CountingStore>) -> Harness {
    let h = harness_full(
        vec![],
        |_| {},
        |_| {},
        None,
        None,
        Some(store.clone()),
        |_| {},
    )
    .await;
    h.runtime.shutdown().await;
    h
}

fn only_subtree_decoded(reads: &super::lease_reads::StoreReads, fleet: &Fleet) {
    assert!(
        !reads.agent_filters.contains(&AgentFilter::default()),
        "the whole fleet was read: {:?}",
        reads.agent_filters
    );
    assert!(
        reads.agents_decoded <= reads.subtree_roots.len() * fleet.subtree.len(),
        "decoded {} agents for {} subtree reads of {} agents ({} live elsewhere)",
        reads.agents_decoded,
        reads.subtree_roots.len(),
        fleet.subtree.len(),
        fleet.outside
    );
}

#[tokio::test]
async fn pausing_and_resuming_a_subtree_reads_only_that_subtree() {
    let store = counting_store();
    let h = quiet_harness(&store).await;
    let fleet = fleet(&h, &store).await;
    let all = store.agents(&AgentFilter::default()).expect("fleet");
    let mut expected: Vec<AgentId> = all
        .iter()
        .filter(|a| fleet.subtree.contains(&a.id) && !a.status.is_terminal())
        .map(|a| a.id)
        .collect();
    expected.sort();
    assert_eq!(
        expected.len(),
        5,
        "root, paused, archived, live, grandchild"
    );

    store.take();
    h.runtime
        .pause_agent(fleet.root, true, Actor::user())
        .expect("pause");
    let reads = store.take();
    only_subtree_decoded(&reads, &fleet);
    assert!(!reads.subtree_roots.is_empty());
    assert!(reads.subtree_roots.iter().all(|r| *r == fleet.root));
    let mut paused: Vec<AgentId> = store
        .agents(&AgentFilter::default())
        .expect("fleet")
        .into_iter()
        .filter(|a| a.paused)
        .map(|a| a.id)
        .collect();
    paused.sort();
    assert_eq!(paused, expected, "exactly the live subtree is paused");

    store.take();
    h.runtime
        .resume_agent(fleet.root, true, Actor::user())
        .expect("resume");
    let reads = store.take();
    only_subtree_decoded(&reads, &fleet);
    let still_paused = store
        .agents(&AgentFilter::default())
        .expect("fleet")
        .into_iter()
        .filter(|a| a.paused)
        .count();
    assert_eq!(still_paused, 0, "resume lifts every pause it stamped");
}

#[tokio::test]
async fn waking_over_a_subtree_budget_reads_only_that_subtree() {
    let store = counting_store();
    let h = quiet_harness(&store).await;
    let fleet = fleet(&h, &store).await;
    spend(&store, fleet.root, 50, |a| {
        a.config.token_budget = Some(900);
        a.status = AgentStatus::Failed { error: "x".into() };
    });

    store.take();
    let error = h
        .runtime
        .wake_agent(fleet.root, Actor::user())
        .expect_err("the subtree's 900 tokens meet the 900 budget");
    let reads = store.take();
    assert!(
        error.to_string().contains("token budget of 900: 900 used"),
        "{error}"
    );
    only_subtree_decoded(&reads, &fleet);
    assert!(reads.subtree_roots.contains(&fleet.root));

    spend(&store, fleet.root, 50, |a| {
        a.config.token_budget = Some(901)
    });
    h.runtime
        .wake_agent(fleet.root, Actor::user())
        .expect("one token under the budget wakes");
}

#[tokio::test]
async fn a_budgeted_turn_and_a_delegation_never_read_the_whole_fleet() {
    let store = counting_store();
    let h = harness_full(
        vec![],
        |_| {},
        |_| {},
        None,
        None,
        Some(store.clone()),
        |_| {},
    )
    .await;
    let fleet = fleet(&h, &store).await;
    h.wait_until(fleet.root, |s| s.kind() == AgentStatusKind::Waiting)
        .await;
    h.client.push(ScriptedClient::tool_call(
        "delegate",
        json!({"model":"anthropic/claude-sonnet-5-5", "effort":"low", "role": "worker", "objective": "sub task", "wait": false}),
    ));
    h.client.push(ScriptedClient::text("delegated"));
    h.client.push(ScriptedClient::tool_call(
        "complete_task",
        json!({"summary": "child done"}),
    ));
    store.take();

    h.runtime
        .send_message(
            fleet.root,
            vec![aether_core::model::ContentBlock::text("go")],
            Actor::user(),
        )
        .expect("message");
    h.wait_for("the delegation", || {
        h.store
            .agents(&AgentFilter {
                parent: Some(fleet.root),
                ..Default::default()
            })
            .expect("children")
            .len()
            > 5
    })
    .await;
    let reads = store.take();
    assert!(
        !reads.agent_filters.contains(&AgentFilter::default()),
        "the whole fleet was read: {:?}",
        reads.agent_filters
    );
    assert!(
        reads.subtree_roots.contains(&fleet.root),
        "the budget check"
    );
    assert!(reads.ancestry_calls >= 1, "the delegation advisory");
}
