//! Chain simplification (parity X6). A chain is dropped when its conclusion
//! already follows from its premises through the other chains: that covers
//! a superset of another chain's premises for the same conclusion, and a
//! shortcut whose premises reach the conclusion by ancestry.
//!
//! Tie-break order: chains with more premises are considered first, then by
//! id, and each drop is decided against the chains still kept. So of two
//! chains that make each other redundant the larger, then the lesser id, goes.

use std::collections::{BTreeMap, BTreeSet, HashMap, VecDeque};

use super::ChainNode;

/// Drop the redundant chains of `chains`; returns the ids dropped, in order.
pub fn simplify(chains: &mut BTreeMap<String, ChainNode>) -> Vec<String> {
    let mut order: Vec<(usize, String)> = chains
        .iter()
        .map(|(id, chain)| (chain.premises.len(), id.clone()))
        .collect();
    order.sort_by(|a, b| b.0.cmp(&a.0).then_with(|| a.1.cmp(&b.1)));
    let mut dropped = Vec::new();
    for (_, id) in order {
        let chain = &chains[&id];
        if derivable_without(chains, &id, &chain.premises, &chain.conclusion) {
            let removed = chains.remove(&id);
            debug_assert!(removed.is_some(), "a dropped chain was kept until now");
            dropped.push(id);
        }
    }
    debug_assert!(dropped.iter().all(|id| !chains.contains_key(id)));
    dropped
}

/// Whether `goal` follows from `known` through every chain but `skip`: a
/// forward closure in which a chain fires once all its premises are known.
fn derivable_without(
    chains: &BTreeMap<String, ChainNode>,
    skip: &str,
    known: &BTreeSet<String>,
    goal: &str,
) -> bool {
    assert!(
        !known.contains(goal),
        "a kept chain never concludes a premise"
    );
    let mut waiting: HashMap<&str, usize> = HashMap::new();
    let mut by_premise: HashMap<&str, Vec<&str>> = HashMap::new();
    for (id, chain) in chains.iter().filter(|(id, _)| id.as_str() != skip) {
        waiting.insert(id, chain.premises.len());
        for premise in &chain.premises {
            by_premise.entry(premise).or_default().push(id);
        }
    }
    let mut reached: BTreeSet<&str> = known.iter().map(String::as_str).collect();
    let mut queue: VecDeque<&str> = reached.iter().copied().collect();
    while let Some(fact) = queue.pop_front() {
        for chain in by_premise.get(fact).into_iter().flatten() {
            let Some(left) = waiting.get_mut(chain) else {
                continue;
            };
            *left -= 1;
            if *left > 0 {
                continue;
            }
            let conclusion = chains[*chain].conclusion.as_str();
            if conclusion == goal {
                return true;
            }
            if reached.insert(conclusion) {
                queue.push_back(conclusion);
            }
        }
    }
    false
}
