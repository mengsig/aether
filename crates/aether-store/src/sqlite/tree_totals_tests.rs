//! `agent_tree` and the `tree:` totals against the parent links they are
//! kept from, and a roll-up read from them against the fold of the subtree.

use std::collections::{BTreeMap, BTreeSet};

use aether_core::agent::Agent;
use aether_core::id::{AgentId, RunId};
use aether_core::model::Usage;

use crate::sqlite::SqliteStore;
use crate::sqlite::usage_totals::read_totals;
use crate::sqlite::usage_totals::tests::{agent, shaped};
use crate::{Commit, Mutation, Store, tree_usages_of};

/// Agents over two runs, each under an earlier one or a root, written
/// children first so the closure must link a subtree when its parent arrives.
fn seeded(store: &SqliteStore, size: usize, runs: [RunId; 2]) -> Vec<Agent> {
    let mut agents: Vec<Agent> = Vec::new();
    let mut state: u64 = 0x9E37_79B9_7F4A_7C15;
    for n in 0..size {
        state ^= state << 13;
        state ^= state >> 7;
        state ^= state << 17;
        let mut next = agent(runs[n % 3 / 2], shaped(n));
        if !agents.is_empty() && !state.is_multiple_of(6) {
            next.parent = Some(agents[(state >> 8) as usize % agents.len()].id);
        }
        agents.push(next);
    }
    for half in agents.rchunks(size / 2 + 1) {
        let mut commit = Commit::new();
        for agent in half.iter().rev() {
            commit = commit.mutate(Mutation::UpsertAgent(agent.clone()));
        }
        store.commit(commit).unwrap();
    }
    agents
}

/// Each agent's subtree by parent links among `agents`: itself, then
/// every agent whose chain of present parents reaches it.
fn subtrees_of(agents: &[Agent]) -> BTreeMap<AgentId, BTreeSet<AgentId>> {
    let parent: BTreeMap<AgentId, Option<AgentId>> =
        agents.iter().map(|a| (a.id, a.parent)).collect();
    let mut subtrees: BTreeMap<AgentId, BTreeSet<AgentId>> = BTreeMap::new();
    for agent in agents {
        let mut at = Some(agent.id);
        while let Some(up) = at.filter(|up| parent.contains_key(up)) {
            if !subtrees.entry(up).or_default().insert(agent.id) {
                break;
            }
            at = parent[&up];
        }
    }
    assert_eq!(
        subtrees.len(),
        agents.len(),
        "every agent is in its own subtree"
    );
    subtrees
}

/// The kept closure and every `tree:` row, checked against `agents`.
fn assert_tree_kept(store: &SqliteStore, agents: &[Agent]) {
    let subtrees = subtrees_of(agents);
    let conn = store.conn.lock();
    let mut statement = conn
        .prepare("SELECT ancestor, descendant FROM agent_tree")
        .unwrap();
    let mut kept: BTreeMap<AgentId, BTreeSet<AgentId>> = BTreeMap::new();
    for row in statement
        .query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?)))
        .unwrap()
    {
        let (ancestor, descendant) = row.unwrap();
        let ancestor = ancestor.parse().unwrap();
        kept.entry(ancestor)
            .or_default()
            .insert(descendant.parse().unwrap());
    }
    assert_eq!(kept, subtrees, "the closure is the parent links'");
    let by_id: BTreeMap<AgentId, &Agent> = agents.iter().map(|a| (a.id, a)).collect();
    let mut scopes = 0;
    for (ancestor, members) in &subtrees {
        let mut by_run: BTreeMap<RunId, (u64, Usage)> = BTreeMap::new();
        for member in members {
            let entry = by_run.entry(by_id[member].run).or_default();
            entry.0 += 1;
            entry.1 += by_id[member].stats.usage;
        }
        for (run, expected) in by_run {
            let read = read_totals(&conn, &format!("tree:{ancestor}:{run}")).unwrap();
            assert_eq!(read, expected, "agent {ancestor}'s subtree in run {run}");
            scopes += 1;
        }
    }
    let rows: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM usage_totals WHERE scope LIKE 'tree:%'",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(rows, scopes, "no subtree row outlives its members");
}

#[test]
fn the_kept_closure_and_subtree_totals_follow_inserts_moves_and_deletes() {
    let store = SqliteStore::open_in_memory().unwrap();
    let runs = [RunId::new(), RunId::new()];
    let mut agents = seeded(&store, 48, runs);
    assert!(agents.iter().filter(|a| a.parent.is_some()).count() > 24);
    assert_tree_kept(&store, &agents);

    let mut moves = Commit::new();
    for n in (3..agents.len()).step_by(5) {
        let below = subtrees_of(&agents).remove(&agents[n].id).unwrap();
        let elsewhere = agents.iter().map(|a| a.id).find(|id| !below.contains(id));
        agents[n].parent = if n % 2 == 0 { None } else { elsewhere };
        agents[n].stats.usage = shaped(n * 7 + 1);
        agents[n].run = runs[n % 2];
        moves = moves.mutate(Mutation::UpsertAgent(agents[n].clone()));
    }
    store.commit(moves).unwrap();
    assert_tree_kept(&store, &agents);

    let gone: Vec<String> = agents.iter().step_by(4).map(|a| a.id.to_string()).collect();
    let conn = store.conn.lock();
    for id in &gone {
        conn.execute("DELETE FROM agents WHERE id = ?1", [id])
            .unwrap();
    }
    drop(conn);
    agents.retain(|a| !gone.contains(&a.id.to_string()));
    assert_tree_kept(&store, &agents);

    let returning = agent(runs[0], shaped(5));
    let mut child = agent(runs[1], shaped(6));
    child.parent = Some(returning.id);
    let mut grandchild = agent(runs[0], shaped(8));
    grandchild.parent = Some(child.id);
    store
        .commit(Commit::new().mutate(Mutation::UpsertAgent(grandchild.clone())))
        .unwrap();
    store
        .commit(Commit::new().mutate(Mutation::UpsertAgent(child.clone())))
        .unwrap();
    store
        .commit(Commit::new().mutate(Mutation::UpsertAgent(returning.clone())))
        .unwrap();
    agents.extend([grandchild, child, returning]);
    assert_tree_kept(&store, &agents);
}

#[test]
fn a_roll_up_read_from_the_kept_totals_is_the_fold_of_its_subtree() {
    let store = SqliteStore::open_in_memory().unwrap();
    let runs = [RunId::new(), RunId::new()];
    let agents = seeded(&store, 40, runs);
    let shown_sets: Vec<Vec<Agent>> = vec![
        agents.clone(),
        agents.iter().step_by(3).cloned().collect(),
        agents
            .iter()
            .filter(|a| a.parent.is_none())
            .cloned()
            .collect(),
        agents[..5].to_vec(),
    ];
    let mut some_hidden = false;
    for shown in &shown_sets {
        let ids: Vec<AgentId> = shown.iter().map(|a| a.id).collect();
        for run in [None, Some(runs[0]), Some(runs[1])] {
            let read = store.tree_usages(shown, run).unwrap();
            let folded = tree_usages_of(store.subtree_usages(&ids).unwrap(), shown, run);
            assert_eq!(read, folded, "{} shown, run {run:?}", shown.len());
            some_hidden |= read.values().any(|usage| usage.hidden.is_some());
        }
    }
    assert!(
        some_hidden,
        "some shown agent leaves part of its subtree out"
    );
}
