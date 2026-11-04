//! The knowledge graph: propositions (true, self-contained statements) and
//! chains (premises that jointly entail a conclusion). It holds claims about
//! the world only, never anything about the learner, and never reads or
//! writes the learner tables (NOESIS.md §4, §5).

use std::collections::{BTreeMap, BTreeSet};

pub mod ids;
mod merge;
mod simplify;
pub mod store;

pub use merge::{Endpoint, Extraction, Finding, FoundChain, MergeOutcome, merge};
pub use simplify::simplify;

/// Where a proposition or chain was asserted: a session or source id and the
/// message (or chunk) in it; `message_ref` is empty when there is none.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Origin {
    pub source: String,
    pub message_ref: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Node {
    pub name: String,
    pub statement: String,
    pub aliases: BTreeSet<String>,
    pub provenance: BTreeSet<Origin>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ChainNode {
    pub premises: BTreeSet<String>,
    pub conclusion: String,
    pub provenance: BTreeSet<Origin>,
}

/// The whole graph, keyed by content-derived id. Ordered maps and sets make
/// every traversal, and so every merge, independent of insertion order.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Graph {
    pub nodes: BTreeMap<String, Node>,
    pub chains: BTreeMap<String, ChainNode>,
}

impl Graph {
    /// Every chain names propositions of this graph, has a premise, and
    /// never concludes one of its own premises.
    pub fn check_structure(&self) -> Result<(), String> {
        for (id, chain) in &self.chains {
            if chain.premises.is_empty() {
                return Err(format!("chain {id} has no premise"));
            }
            if chain.premises.contains(&chain.conclusion) {
                return Err(format!("chain {id} concludes one of its premises"));
            }
            let mut ends = chain.premises.iter().chain([&chain.conclusion]);
            if let Some(missing) = ends.find(|end| !self.nodes.contains_key(*end)) {
                return Err(format!("chain {id} names a missing proposition {missing}"));
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests;
