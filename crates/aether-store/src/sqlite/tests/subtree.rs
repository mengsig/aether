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
