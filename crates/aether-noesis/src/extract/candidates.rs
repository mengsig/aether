//! Lexical candidates (parity X4): for each finding, the existing
//! propositions sharing the most words with it, ranked by word overlap and
//! then by id, so the same graph and text always give the same list.

use std::collections::BTreeSet;

use crate::graph::Graph;

/// Candidates shown to the auditor per finding.
pub const PER_FINDING: usize = 5;

/// Lowercase words of three or more letters or digits.
pub fn words(text: &str) -> BTreeSet<String> {
    text.split(|c: char| !c.is_alphanumeric())
        .filter(|word| word.chars().count() >= 3)
        .map(str::to_lowercase)
        .collect()
}

/// The graph's propositions with their words, built once per unit.
pub struct Index {
    entries: Vec<(String, BTreeSet<String>)>,
}

impl Index {
    pub fn new(graph: &Graph) -> Self {
        let entries = graph
            .nodes
            .iter()
            .map(|(id, node)| {
                let mut all = words(&node.name);
                all.extend(words(&node.statement));
                for alias in &node.aliases {
                    all.extend(words(alias));
                }
                (id.clone(), all)
            })
            .collect();
        Self { entries }
    }

    /// The `limit` propositions most like `text`, best first; none share no word.
    pub fn nearest(&self, text: &str, limit: usize) -> Vec<String> {
        let wanted = words(text);
        if wanted.is_empty() || limit == 0 {
            return Vec::new();
        }
        let mut scored: Vec<(f64, &str)> = self
            .entries
            .iter()
            .filter_map(|(id, have)| {
                let shared = wanted.intersection(have).count();
                let union = wanted.union(have).count();
                (shared > 0).then(|| (shared as f64 / union as f64, id.as_str()))
            })
            .collect();
        scored.sort_by(|a, b| b.0.total_cmp(&a.0).then_with(|| a.1.cmp(b.1)));
        scored.truncate(limit);
        debug_assert!(
            scored
                .iter()
                .all(|(score, _)| *score > 0.0 && *score <= 1.0)
        );
        scored.into_iter().map(|(_, id)| id.to_string()).collect()
    }
}
