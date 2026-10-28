use super::*;
use aether_core::agent::subtree_of;
use std::collections::BTreeSet;

/// A deterministic forest: each agent hangs under an earlier one or starts a new root.
fn forest(size: usize) -> Vec<Agent> {
    let mut agents: Vec<Agent> = Vec::new();
    let mut state: u64 = 0x2545_F491_4F6C_DD1D;
    for _ in 0..size {
        state ^= state << 13;
        state ^= state >> 7;
        state ^= state << 17;
        let mut next = agent();
        if !agents.is_empty() && !state.is_multiple_of(5) {
            next.parent = Some(agents[(state >> 8) as usize % agents.len()].id);
        }
        agents.push(next);
    }
    agents
}

#[test]
fn subtree_helpers_match_in_memory_walk_on_random_forest() {
    let store = SqliteStore::open_in_memory().expect("open");
    let agents = forest(60);
    let mut commit = Commit::new();
    for a in &agents {
        commit = commit.mutate(Mutation::UpsertAgent(a.clone()));
    }
    store.commit(commit).expect("seed");
    assert!(agents.iter().any(|a| a.parent.is_some()));
    assert!(agents.iter().filter(|a| a.parent.is_none()).count() > 1);

    for root in &agents {
        let expected: BTreeSet<_> = subtree_of(&agents, root.id).into_iter().collect();
        assert_eq!(store.subtree_ids(root.id).expect("subtree"), expected);
        for other in &agents {
            assert_eq!(
                store.is_descendant(root.id, other.id).expect("walk"),
                expected.contains(&other.id),
                "root {} other {}",
                root.id,
                other.id
            );
        }
    }
}

#[test]
fn subtree_helpers_treat_unknown_agents_as_outside() {
    let store = SqliteStore::open_in_memory().expect("open");
    let root = agent();
    store
        .commit(Commit::new().mutate(Mutation::UpsertAgent(root.clone())))
        .expect("seed");
    let stranger = AgentId::new();

    assert!(store.subtree_ids(stranger).expect("subtree").is_empty());
    assert!(!store.is_descendant(stranger, root.id).expect("walk"));
    assert!(!store.is_descendant(root.id, stranger).expect("walk"));
    assert!(store.is_descendant(root.id, root.id).expect("self"));
}

fn seed(store: &SqliteStore, agents: &[Agent]) {
    let mut commit = Commit::new();
    for a in agents {
        commit = commit.mutate(Mutation::UpsertAgent(a.clone()));
    }
    store.commit(commit).expect("seed");
}

fn ids(agents: &[Agent]) -> Vec<AgentId> {
    agents.iter().map(|a| a.id).collect()
}

#[test]
fn subtree_agents_keep_the_fleet_listing_order_including_created_at_ties() {
    let store = SqliteStore::open_in_memory().expect("open");
    let mut agents = forest(60);
    let base = now();
    for (i, a) in agents.iter_mut().enumerate() {
        a.created_at = base + chrono::Duration::seconds(i as i64 / 4);
    }
    // Inserted newest id first, so a tie broken by id instead of insertion differs.
    let mut insertion = agents.clone();
    insertion.reverse();
    seed(&store, &insertion);
    let listing = ids(&store.agents(&AgentFilter::default()).expect("listing"));

    let mut checked = 0;
    for root in &agents {
        let members = store.subtree_ids(root.id).expect("ids");
        let expected: Vec<_> = listing
            .iter()
            .copied()
            .filter(|id| members.contains(id))
            .collect();
        let got = store.subtree_agents(root.id).expect("subtree agents");
        assert_eq!(ids(&got), expected, "root {}", root.id);
        checked += usize::from(expected.len() > 1);
    }
    assert!(checked > 5, "the forest has real subtrees ({checked})");
}

#[test]
fn subtree_agents_decode_every_member_whatever_its_status() {
    let store = SqliteStore::open_in_memory().expect("open");
    let root = agent();
    let mut done = agent();
    done.parent = Some(root.id);
    done.status = AgentStatus::Created;
    done.archived = true;
    done.paused = true;
    seed(&store, &[root.clone(), done.clone()]);

    let got = store.subtree_agents(root.id).expect("subtree agents");
    assert_eq!(got.len(), 2);
    assert!(got.contains(&done), "the stored row comes back unchanged");
}

#[test]
fn subtree_agents_end_on_a_parent_cycle_and_skip_an_unknown_root() {
    let store = SqliteStore::open_in_memory().expect("open");
    let mut a = agent();
    let mut b = agent();
    a.parent = Some(b.id);
    b.parent = Some(a.id);
    let outside = agent();
    seed(&store, &[a.clone(), b.clone(), outside]);

    let got = store.subtree_agents(a.id).expect("cycle ends");
    let mut members = ids(&got);
    members.sort();
    let mut expected = vec![a.id, b.id];
    expected.sort();
    assert_eq!(members, expected);
    assert!(
        store
            .subtree_agents(AgentId::new())
            .expect("unknown")
            .is_empty()
    );
}

#[test]
fn ancestry_returns_the_requested_agents_and_every_stored_ancestor() {
    let store = SqliteStore::open_in_memory().expect("open");
    let agents = forest(60);
    seed(&store, &agents);

    for pick in agents.iter().step_by(7) {
        let mut expected: BTreeSet<_> = BTreeSet::from([pick.id]);
        let mut cursor = pick.parent;
        while let Some(parent) = cursor {
            expected.insert(parent);
            cursor = agents
                .iter()
                .find(|a| a.id == parent)
                .and_then(|a| a.parent);
        }
        let got = store.ancestry(&[pick.id]).expect("ancestry");
        assert_eq!(got.iter().map(|a| a.id).collect::<BTreeSet<_>>(), expected);
        assert_eq!(got.len(), expected.len(), "no agent comes back twice");
    }

    let two = [agents[10].id, agents[11].id];
    let got = store.ancestry(&two).expect("two chains");
    assert!(two.iter().all(|id| got.iter().any(|a| a.id == *id)));
}

#[test]
fn ancestry_skips_unknown_ids_and_missing_parents_and_ends_on_cycles() {
    let store = SqliteStore::open_in_memory().expect("open");
    let mut orphan = agent();
    orphan.parent = Some(AgentId::new());
    let mut a = agent();
    let mut b = agent();
    a.parent = Some(b.id);
    b.parent = Some(a.id);
    let bystander = agent();
    seed(
        &store,
        &[orphan.clone(), a.clone(), b.clone(), bystander.clone()],
    );

    assert!(store.ancestry(&[]).expect("empty").is_empty());
    assert!(
        store
            .ancestry(&[AgentId::new()])
            .expect("unknown")
            .is_empty()
    );
    assert_eq!(
        ids(&store.ancestry(&[orphan.id]).expect("orphan")),
        vec![orphan.id]
    );
    let cycle = store.ancestry(&[a.id]).expect("cycle ends");
    assert_eq!(cycle.len(), 2);
    assert!(cycle.iter().all(|x| x.id != bystander.id));
}
