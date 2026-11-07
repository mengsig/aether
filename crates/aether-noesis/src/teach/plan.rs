//! Plan DAGs (T8) and the derived status of their nodes (T9). An edge runs
//! from a prerequisite to the step that needs it.

use std::collections::{BTreeMap, BTreeSet, HashSet};

use serde::{Deserialize, Serialize};

use super::records::{NodeStatus, PlanEdge, PlanNode, PlanTree};
use crate::tools::args::{PlanArgs, PlanNodeArg};

pub const MAX_NODES: usize = 64;
const MAX_SLUG: usize = 64;
const MAX_TITLE: usize = 200;

/// The plan as stored in `plans.tree`: no status, which is derived.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PlanDag {
    pub nodes: Vec<PlanNodeArg>,
    pub edges: Vec<PlanEdge>,
}

impl PlanDag {
    pub fn has_node(&self, id: &str) -> bool {
        self.nodes.iter().any(|node| node.id == id)
    }
}

/// Lowercase ASCII words joined by single hyphens.
pub fn is_slug(id: &str) -> bool {
    let words_ok = id.split('-').all(|word| {
        !word.is_empty()
            && word
                .bytes()
                .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit())
    });
    !id.is_empty() && id.len() <= MAX_SLUG && words_ok
}

/// Validate a whole plan, reporting every problem rather than the first.
pub fn validate(args: &PlanArgs) -> Result<PlanDag, Vec<String>> {
    let mut problems = node_problems(&args.nodes);
    let ids: HashSet<&str> = args.nodes.iter().map(|node| node.id.as_str()).collect();
    let mut seen_edges = HashSet::new();
    for edge in &args.edges {
        let label = format!("edge {} -> {}", edge.from, edge.to);
        for end in [&edge.from, &edge.to] {
            if !ids.contains(end.as_str()) {
                problems.push(format!("{label}: no node `{end}`"));
            }
        }
        if edge.from == edge.to {
            problems.push(format!("{label}: a node cannot be its own prerequisite"));
        }
        if !seen_edges.insert((edge.from.as_str(), edge.to.as_str())) {
            problems.push(format!("{label}: duplicate edge"));
        }
    }
    let cycle = nodes_on_cycles(&args.edges);
    if !cycle.is_empty() {
        let names: Vec<&str> = cycle.iter().map(String::as_str).collect();
        problems.push(format!(
            "the plan has a cycle; these nodes are on it or depend on it: {}",
            names.join(", ")
        ));
    }
    if !problems.is_empty() {
        return Err(problems);
    }
    Ok(PlanDag {
        nodes: args.nodes.clone(),
        edges: args.edges.clone(),
    })
}

fn node_problems(nodes: &[PlanNodeArg]) -> Vec<String> {
    let mut problems = Vec::new();
    if nodes.is_empty() {
        problems.push("a plan needs at least one node".to_string());
    }
    if nodes.len() > MAX_NODES {
        problems.push(format!("a plan has at most {MAX_NODES} nodes"));
    }
    let mut seen = HashSet::new();
    for node in nodes {
        if !is_slug(&node.id) {
            problems.push(format!(
                "node `{}`: an id is a slug of lowercase letters, digits and single hyphens",
                node.id
            ));
        }
        if !seen.insert(node.id.as_str()) {
            problems.push(format!("node `{}`: duplicate id", node.id));
        }
        if node.title.chars().count() > MAX_TITLE {
            problems.push(format!(
                "node `{}`: a title has at most {MAX_TITLE} characters",
                node.id
            ));
        }
    }
    problems
}

/// Every node that lies on a cycle or depends on one, found by repeatedly
/// removing nodes with no remaining prerequisite (Kahn). Self-loops are
/// reported on their own, so they are left out here.
fn nodes_on_cycles(edges: &[PlanEdge]) -> BTreeSet<String> {
    let mut incoming: BTreeMap<&str, usize> = BTreeMap::new();
    let mut outgoing: BTreeMap<&str, Vec<&str>> = BTreeMap::new();
    for edge in edges.iter().filter(|edge| edge.from != edge.to) {
        incoming.entry(edge.from.as_str()).or_insert(0);
        *incoming.entry(edge.to.as_str()).or_insert(0) += 1;
        outgoing
            .entry(edge.from.as_str())
            .or_default()
            .push(edge.to.as_str());
    }
    let mut ready: Vec<&str> = incoming
        .iter()
        .filter(|(_, n)| **n == 0)
        .map(|(id, _)| *id)
        .collect();
    while let Some(id) = ready.pop() {
        incoming.remove(id);
        for next in outgoing.get(id).into_iter().flatten() {
            let Some(count) = incoming.get_mut(next) else {
                debug_assert!(false, "every endpoint is counted");
                continue;
            };
            *count -= 1;
            if *count == 0 {
                ready.push(next);
            }
        }
    }
    incoming.keys().map(|id| id.to_string()).collect()
}

/// The plan with each node's status derived from the plan nodes of the
/// session's questions, oldest first: the latest is current, earlier ones
/// visited, the rest upcoming.
pub fn tree(revision: u32, dag: &PlanDag, tagged_in_order: &[String]) -> PlanTree {
    let current = tagged_in_order.last();
    let visited: HashSet<&String> = tagged_in_order.iter().collect();
    let nodes: Vec<PlanNode> = dag
        .nodes
        .iter()
        .map(|node| {
            let status = if Some(&node.id) == current {
                NodeStatus::Current
            } else if visited.contains(&node.id) {
                NodeStatus::Visited
            } else {
                NodeStatus::Upcoming
            };
            PlanNode {
                id: node.id.clone(),
                title: node.title.clone(),
                status,
            }
        })
        .collect();
    debug_assert!(
        nodes
            .iter()
            .filter(|n| n.status == NodeStatus::Current)
            .count()
            <= 1
    );
    PlanTree {
        revision,
        nodes,
        edges: dag.edges.clone(),
    }
}
