//! The deterministic merge (parity X5): a pure function of the graph and one
//! audited extraction. A link appends to its target, anything else creates
//! (or finds, by content id) a node; aliases and provenance are unioned;
//! chains are deduplicated by their content id; a chain concluding one of
//! its premises is dropped; then the chain rules of [`simplify`] apply.

use std::collections::{BTreeMap, BTreeSet};

use super::simplify::simplify;
use super::{ChainNode, Graph, Node, Origin, ids};

/// One extracted statement after the auditor: `link` names the existing
/// proposition it restates, or is `None` for a new one.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Finding {
    /// The extractor's own key for it, unique within the extraction.
    pub key: String,
    pub name: String,
    pub statement: String,
    pub aliases: Vec<String>,
    pub origin: Origin,
    pub link: Option<String>,
}

/// A chain endpoint: a finding of this extraction, or an existing proposition.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Endpoint {
    Local(String),
    Existing(String),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FoundChain {
    pub premises: Vec<Endpoint>,
    pub conclusion: Endpoint,
    pub origin: Origin,
}

/// One unit's validated and audited extraction.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Extraction {
    pub findings: Vec<Finding>,
    pub chains: Vec<FoundChain>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MergeOutcome {
    pub graph: Graph,
    /// Propositions the extraction created or added to.
    pub nodes: u32,
    /// Chains of the extraction the merged graph keeps.
    pub chains: u32,
    /// Chains dropped as circular or by the simplification rules.
    pub dropped_chains: u32,
}

/// Merge `extraction` into `graph`. The result depends on the contents of
/// both only: never on time, randomness or the order of the extraction's items.
pub fn merge(graph: &Graph, extraction: &Extraction) -> MergeOutcome {
    let mut merged = graph.clone();
    let targets = resolve_targets(graph, &extraction.findings);
    let touched = merge_findings(&mut merged, &extraction.findings, &targets);
    let (asserted, circular) = merge_chains(&mut merged, &extraction.chains, &targets);
    let simplified = simplify(&mut merged.chains);
    let kept = asserted
        .iter()
        .filter(|id| merged.chains.contains_key(*id))
        .count();
    debug_assert!(
        merged.check_structure().is_ok(),
        "{:?}",
        merged.check_structure()
    );
    assert!(
        merged.nodes.len() >= graph.nodes.len(),
        "a merge never removes a proposition"
    );
    MergeOutcome {
        graph: merged,
        nodes: count(touched.len()),
        chains: count(kept),
        dropped_chains: count(circular + simplified.len()),
    }
}

fn count(n: usize) -> u32 {
    u32::try_from(n).unwrap_or(u32::MAX)
}

/// Each finding's proposition id: its link when that names a node of the
/// graph, otherwise the id its statement derives (an unknown link creates).
fn resolve_targets(graph: &Graph, findings: &[Finding]) -> BTreeMap<String, String> {
    let mut targets = BTreeMap::new();
    for finding in findings {
        let linked = finding
            .link
            .as_ref()
            .filter(|id| graph.nodes.contains_key(*id));
        let target = match linked {
            Some(id) => id.clone(),
            None => ids::proposition_id(&finding.statement),
        };
        let previous = targets.insert(finding.key.clone(), target);
        assert!(
            previous.is_none(),
            "finding key {} is not unique",
            finding.key
        );
    }
    targets
}

/// Add every finding to its target; returns the ids touched.
fn merge_findings(
    graph: &mut Graph,
    findings: &[Finding],
    targets: &BTreeMap<String, String>,
) -> BTreeSet<String> {
    let mut groups: BTreeMap<&str, Vec<&Finding>> = BTreeMap::new();
    for finding in findings {
        groups
            .entry(&targets[&finding.key])
            .or_default()
            .push(finding);
    }
    for (id, group) in &groups {
        let node = graph
            .nodes
            .entry((*id).to_string())
            .or_insert_with(|| new_node(group));
        for finding in group {
            node.aliases.insert(finding.name.clone());
            node.aliases.extend(finding.aliases.iter().cloned());
            node.provenance.insert(finding.origin.clone());
        }
        let name = node.name.clone();
        node.aliases.remove(&name);
        node.aliases.retain(|alias| !alias.trim().is_empty());
    }
    groups.keys().map(|id| (*id).to_string()).collect()
}

/// A node created by `group`, all restating one statement: the least name
/// and the least wording, so the choice does not depend on their order.
fn new_node(group: &[&Finding]) -> Node {
    let name = group.iter().map(|f| f.name.as_str()).min();
    let statement = group.iter().map(|f| f.statement.as_str()).min();
    let (Some(name), Some(statement)) = (name, statement) else {
        unreachable!("a group holds at least one finding");
    };
    Node {
        name: name.to_string(),
        statement: statement.to_string(),
        aliases: BTreeSet::new(),
        provenance: BTreeSet::new(),
    }
}

/// Add the extraction's chains; returns the ids asserted and how many were
/// dropped for naming a missing proposition or concluding a premise.
fn merge_chains(
    graph: &mut Graph,
    chains: &[FoundChain],
    targets: &BTreeMap<String, String>,
) -> (BTreeSet<String>, usize) {
    let mut asserted = BTreeSet::new();
    let mut dropped = 0;
    for found in chains {
        let Some((premises, conclusion)) = resolve_chain(graph, found, targets) else {
            dropped += 1;
            continue;
        };
        let id = ids::chain_id(&premises, &conclusion);
        let chain = graph.chains.entry(id.clone()).or_insert_with(|| ChainNode {
            premises,
            conclusion,
            provenance: BTreeSet::new(),
        });
        chain.provenance.insert(found.origin.clone());
        asserted.insert(id);
    }
    (asserted, dropped)
}

fn resolve_chain(
    graph: &Graph,
    found: &FoundChain,
    targets: &BTreeMap<String, String>,
) -> Option<(BTreeSet<String>, String)> {
    let resolve = |end: &Endpoint| match end {
        Endpoint::Local(key) => targets.get(key).cloned(),
        Endpoint::Existing(id) => graph.nodes.contains_key(id).then(|| id.clone()),
    };
    let conclusion = resolve(&found.conclusion)?;
    let premises: Option<BTreeSet<String>> = found.premises.iter().map(resolve).collect();
    let premises = premises?;
    if premises.is_empty() || premises.contains(&conclusion) {
        return None;
    }
    Some((premises, conclusion))
}
