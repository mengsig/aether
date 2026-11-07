//! The graph as `noesis_graph` returns it (parity X7): filtered nodes, the
//! chains among them (a chain with an endpoint outside the result is
//! dropped), and clusters of at least 3 nodes born in the same session.

use std::collections::{BTreeMap, BTreeSet};

use aether_core::noesis::{Chain, Cluster, Proposition};
use rusqlite::Connection;

use super::{GraphError, load, wire_chain, wire_proposition};
use crate::graph::Node;

/// The most nodes one snapshot returns when the caller names no limit.
pub const DEFAULT_LIMIT: u32 = 1000;
/// The fewest members a cluster has.
pub const MIN_CLUSTER: usize = 3;

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct SnapshotFilter {
    pub query: Option<String>,
    pub session: Option<String>,
    pub source: Option<String>,
    pub limit: Option<u32>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Snapshot {
    pub nodes: Vec<Proposition>,
    pub chains: Vec<Chain>,
    pub clusters: Vec<Cluster>,
    pub truncated: bool,
}

fn created_at(conn: &Connection, table: &str) -> Result<BTreeMap<String, String>, GraphError> {
    let mut statement = conn.prepare(&format!("SELECT id, created_at FROM {table}"))?;
    let rows = statement.query_map([], |row| Ok((row.get(0)?, row.get(1)?)))?;
    Ok(rows.collect::<Result<_, _>>()?)
}

fn matches(node: &Node, filter: &SnapshotFilter) -> bool {
    let from = |wanted: &Option<String>| {
        wanted
            .as_ref()
            .is_none_or(|source| node.provenance.iter().any(|o| o.source == *source))
    };
    let query = filter
        .query
        .as_deref()
        .map(str::trim)
        .filter(|q| !q.is_empty());
    let text = query.is_none_or(|query| {
        let query = query.to_lowercase();
        let mut texts = [&node.name, &node.statement]
            .into_iter()
            .chain(&node.aliases);
        texts.any(|text| text.to_lowercase().contains(&query))
    });
    text && from(&filter.session) && from(&filter.source)
}

/// The filtered graph, oldest nodes first, cut at the filter's limit.
pub fn snapshot(conn: &Connection, filter: &SnapshotFilter) -> Result<Snapshot, GraphError> {
    let limit = filter.limit.unwrap_or(DEFAULT_LIMIT) as usize;
    assert!(limit > 0, "the protocol refuses a zero limit");
    // One read transaction, so a merge committing meanwhile is seen whole or not at all.
    let read = conn.unchecked_transaction()?;
    let graph = load(&read)?;
    let node_times = created_at(&read, "propositions")?;
    let chain_times = created_at(&read, "chains")?;
    read.finish()?;
    let mut selected: Vec<(&String, &String, &Node)> = graph
        .nodes
        .iter()
        .filter(|(_, node)| matches(node, filter))
        .map(|(id, node)| (&node_times[id], id, node))
        .collect();
    selected.sort_unstable_by(|a, b| (a.0, a.1).cmp(&(b.0, b.1)));
    let truncated = selected.len() > limit;
    selected.truncate(limit);
    let kept: BTreeSet<&str> = selected.iter().map(|(_, id, _)| id.as_str()).collect();
    let mut nodes = Vec::with_capacity(selected.len());
    for (at, id, node) in &selected {
        nodes.push(wire_proposition(id, node, at)?);
    }
    let mut chains = Vec::new();
    for (id, chain) in &graph.chains {
        let mut ends = chain.premises.iter().chain([&chain.conclusion]);
        if ends.all(|end| kept.contains(end.as_str())) {
            chains.push(wire_chain(id, chain, &chain_times[id])?);
        }
    }
    let clusters = clusters(&selected);
    debug_assert!(nodes.len() <= limit);
    Ok(Snapshot {
        nodes,
        chains,
        clusters,
        truncated,
    })
}

/// Group nodes by their birth session: the earliest session (session ids
/// sort by creation) in their provenance. Sources form no cluster.
fn clusters(selected: &[(&String, &String, &Node)]) -> Vec<Cluster> {
    let mut by_session: BTreeMap<&str, Vec<String>> = BTreeMap::new();
    for (_, id, node) in selected {
        let birth = node
            .provenance
            .iter()
            .map(|o| o.source.as_str())
            .filter(|s| s.starts_with("ns_"))
            .min();
        if let Some(session) = birth {
            by_session.entry(session).or_default().push((*id).clone());
        }
    }
    by_session
        .into_iter()
        .filter(|(_, members)| members.len() >= MIN_CLUSTER)
        .map(|(session, mut members)| {
            members.sort_unstable();
            Cluster {
                id: format!("cl_{session}"),
                session: session.to_string(),
                members,
            }
        })
        .collect()
}
