//! Exact endpoint resolution. Names may vary in case and whitespace, but
//! ambiguous labels never choose a proposition by order or similarity.

use std::collections::{BTreeMap, BTreeSet};

use serde_json::Value;

use crate::graph::{Endpoint, Finding, Graph, ids};

pub(super) struct Endpoints {
    keys: BTreeMap<String, String>,
    ids: BTreeSet<String>,
    labels: BTreeMap<String, Option<(String, Endpoint)>>,
}

impl Endpoints {
    pub(super) fn new(findings: &[Finding], graph: &Graph) -> Self {
        let mut out = Self {
            keys: findings
                .iter()
                .map(|finding| (finding.key.clone(), ids::proposition_id(&finding.statement)))
                .collect(),
            ids: graph.nodes.keys().cloned().collect(),
            labels: BTreeMap::new(),
        };
        debug_assert_eq!(out.keys.len(), findings.len());
        for finding in findings {
            let endpoint = Endpoint::Local(finding.key.clone());
            let identity = out.keys[&finding.key].clone();
            for label in [&finding.name, &finding.statement]
                .into_iter()
                .chain(&finding.aliases)
            {
                out.label(label, &identity, &endpoint);
            }
        }
        for (id, node) in &graph.nodes {
            let endpoint = Endpoint::Existing(id.clone());
            for label in [&node.name, &node.statement]
                .into_iter()
                .chain(&node.aliases)
            {
                out.label(label, id, &endpoint);
            }
        }
        debug_assert!(out.labels.keys().all(|label| !label.is_empty()));
        out
    }

    fn label(&mut self, label: &str, identity: &str, endpoint: &Endpoint) {
        debug_assert!(identity.starts_with("p_"));
        debug_assert_eq!(self.identity(endpoint), identity);
        let label = ids::normalize(label);
        if label.is_empty() {
            return;
        }
        self.labels
            .entry(label)
            .and_modify(|entry| {
                if entry
                    .as_ref()
                    .is_some_and(|(previous, _)| previous != identity)
                {
                    *entry = None;
                }
            })
            .or_insert_with(|| Some((identity.into(), endpoint.clone())));
    }

    pub(super) fn resolve(&self, value: &Value) -> Option<Endpoint> {
        match value {
            Value::String(label) => self.identifier(label).or_else(|| self.labeled(label)),
            Value::Object(fields) => {
                let mut resolved = None;
                for (key, label) in fields {
                    let label = label.as_str()?;
                    let endpoint = match key.as_str() {
                        "key" | "id" => self.identifier(label),
                        "name" | "statement" | "alias" => self.labeled(label),
                        _ => None,
                    }?;
                    if resolved
                        .as_ref()
                        .is_some_and(|previous| self.identity(previous) != self.identity(&endpoint))
                    {
                        return None;
                    }
                    resolved = Some(endpoint);
                }
                resolved
            }
            _ => None,
        }
    }

    fn identity<'a>(&'a self, endpoint: &'a Endpoint) -> &'a str {
        match endpoint {
            Endpoint::Local(key) => &self.keys[key],
            Endpoint::Existing(id) => id,
        }
    }

    fn identifier(&self, name: &str) -> Option<Endpoint> {
        if self.keys.contains_key(name) {
            return Some(Endpoint::Local(name.into()));
        }
        if self.ids.contains(name) {
            return Some(Endpoint::Existing(name.into()));
        }
        None
    }

    fn labeled(&self, name: &str) -> Option<Endpoint> {
        self.labels
            .get(&ids::normalize(name))?
            .as_ref()
            .map(|(_, endpoint)| endpoint.clone())
    }
}
