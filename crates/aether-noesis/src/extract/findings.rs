//! Checking what the extractor and the auditor return (parity X3, X4). A
//! reply that is not the JSON asked for fails the unit; an item that breaks
//! a rule (an anchor outside the batch, an endpoint that is neither a local
//! key nor an unambiguous existing or local proposition) is dropped and counted.

use std::collections::{BTreeMap, BTreeSet};

use serde::Deserialize;
use serde_json::Value;

use crate::graph::{Finding, FoundChain, Graph, Origin, ids};

use super::endpoints::Endpoints;

pub const MAX_NAME_CHARS: usize = 200;
pub const MAX_STATEMENT_CHARS: usize = 2_000;
pub const MAX_ALIASES: usize = 16;
pub const MAX_PREMISES: usize = 16;
/// The most findings or chains one extractor reply may carry.
pub const MAX_ITEMS: usize = 200;

#[derive(Debug, Deserialize)]
struct RawProposition {
    key: String,
    name: String,
    statement: String,
    #[serde(default)]
    aliases: Vec<String>,
    anchor: String,
}

#[derive(Debug, Deserialize)]
struct RawChain {
    #[serde(alias = "premise_ids")]
    premises: Vec<Value>,
    #[serde(alias = "conclusion_id")]
    conclusion: Value,
    anchor: String,
}

#[derive(Debug, Deserialize)]
struct RawSubmission {
    #[serde(default)]
    propositions: Vec<Value>,
    #[serde(default)]
    #[serde(alias = "links")]
    chains: Vec<Value>,
}

/// The extractor's valid findings (not yet linked) and chains.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Submission {
    pub findings: Vec<Finding>,
    pub chains: Vec<FoundChain>,
    pub rejected: u32,
}

/// The one JSON object in a model reply, allowing a fence or prose around it.
pub fn json_object(text: &str) -> Result<Value, String> {
    let start = text.find('{').ok_or("the reply holds no JSON object")?;
    let end = text.rfind('}').ok_or("the reply holds no JSON object")?;
    if end < start {
        return Err("the reply holds no JSON object".into());
    }
    let value: Value = serde_json::from_str(&text[start..=end])
        .map_err(|e| format!("the reply is not valid JSON: {e}"))?;
    if !value.is_object() {
        return Err("the reply is not a JSON object".into());
    }
    Ok(value)
}

/// Check an extractor reply against the batch it read (`refs`) and the graph.
pub fn submission(
    text: &str,
    source: &str,
    refs: &BTreeSet<String>,
    graph: &Graph,
) -> Result<Submission, String> {
    assert!(!refs.is_empty(), "a batch holds at least one message");
    let raw: RawSubmission = serde_json::from_value(json_object(text)?)
        .map_err(|e| format!("the extractor reply has the wrong shape: {e}"))?;
    if raw.propositions.len() > MAX_ITEMS || raw.chains.len() > MAX_ITEMS {
        return Err(format!(
            "the extractor reply has more than {MAX_ITEMS} items"
        ));
    }
    let mut chains = raw.chains;
    let mut out = Submission::default();
    let mut keys = BTreeSet::new();
    for value in raw.propositions {
        let nested = value.get("chains").cloned();
        match proposition(value, source, refs, &keys) {
            Some(finding) => {
                nested_chains(nested, &finding, &mut chains)?;
                keys.insert(finding.key.clone());
                out.findings.push(finding);
            }
            None => out.rejected += 1,
        }
    }
    let endpoints = Endpoints::new(&out.findings, graph);
    for value in chains {
        match chain(value, source, refs, &endpoints) {
            Some(found) => out.chains.push(found),
            None => out.rejected += 1,
        }
    }
    debug_assert_eq!(keys.len(), out.findings.len(), "finding keys are unique");
    Ok(out)
}

fn text_ok(text: &str, max: usize) -> bool {
    !text.trim().is_empty() && text.chars().count() <= max
}

fn proposition(
    value: Value,
    source: &str,
    refs: &BTreeSet<String>,
    keys: &BTreeSet<String>,
) -> Option<Finding> {
    let raw: RawProposition = serde_json::from_value(value).ok()?;
    let valid = text_ok(&raw.key, MAX_NAME_CHARS)
        && !raw.key.starts_with("p_")
        && !keys.contains(&raw.key)
        && text_ok(&raw.name, MAX_NAME_CHARS)
        && text_ok(&raw.statement, MAX_STATEMENT_CHARS)
        && !ids::normalize(&raw.statement).is_empty()
        && raw.aliases.len() <= MAX_ALIASES
        && raw
            .aliases
            .iter()
            .all(|alias| text_ok(alias, MAX_NAME_CHARS))
        && refs.contains(&raw.anchor);
    valid.then(|| Finding {
        key: raw.key,
        name: raw.name.trim().to_string(),
        statement: raw.statement.trim().to_string(),
        aliases: raw.aliases.iter().map(|a| a.trim().to_string()).collect(),
        origin: Origin {
            source: source.to_string(),
            message_ref: raw.anchor,
        },
        link: None,
    })
}

fn nested_chains(
    value: Option<Value>,
    finding: &Finding,
    chains: &mut Vec<Value>,
) -> Result<(), String> {
    debug_assert!(!finding.key.is_empty());
    debug_assert!(chains.len() <= MAX_ITEMS);
    let Some(value) = value else {
        return Ok(());
    };
    let Value::Array(nested) = value else {
        return Err("a proposition's chains must be an array".into());
    };
    if chains.len() + nested.len() > MAX_ITEMS {
        return Err(format!(
            "the extractor reply has more than {MAX_ITEMS} chains"
        ));
    }
    for mut value in nested {
        if let Some(fields) = value.as_object_mut() {
            if !fields.contains_key("conclusion_id") {
                fields
                    .entry("conclusion")
                    .or_insert_with(|| Value::String(finding.key.clone()));
            }
            fields
                .entry("anchor")
                .or_insert_with(|| Value::String(finding.origin.message_ref.clone()));
        }
        chains.push(value);
    }
    Ok(())
}

fn chain(
    value: Value,
    source: &str,
    refs: &BTreeSet<String>,
    endpoints: &Endpoints,
) -> Option<FoundChain> {
    debug_assert!(!refs.is_empty());
    let raw: RawChain = serde_json::from_value(value).ok()?;
    if raw.premises.is_empty() || raw.premises.len() > MAX_PREMISES || !refs.contains(&raw.anchor) {
        return None;
    }
    let premises: Option<Vec<_>> = raw
        .premises
        .iter()
        .map(|value| endpoints.resolve(value))
        .collect();
    let premises = premises?;
    debug_assert_eq!(
        premises.len(),
        raw.premises.len(),
        "never weaken a chain by dropping a premise"
    );
    Some(FoundChain {
        premises,
        conclusion: endpoints.resolve(&raw.conclusion)?,
        origin: Origin {
            source: source.to_string(),
            message_ref: raw.anchor,
        },
    })
}

#[derive(Debug, Deserialize)]
struct RawLinks {
    links: Vec<RawLink>,
}

#[derive(Debug, Deserialize)]
struct RawLink {
    key: String,
    #[serde(default)]
    link: Option<String>,
}

/// The auditor's links, kept only where they name one of the finding's
/// candidates; any other link, or none, leaves the finding new.
pub fn links(
    text: &str,
    candidates: &BTreeMap<String, Vec<String>>,
) -> Result<BTreeMap<String, String>, String> {
    let raw: RawLinks = serde_json::from_value(json_object(text)?)
        .map_err(|e| format!("the auditor reply has the wrong shape: {e}"))?;
    let mut out = BTreeMap::new();
    for link in raw.links {
        let (Some(target), Some(allowed)) = (link.link, candidates.get(&link.key)) else {
            continue;
        };
        if allowed.contains(&target) && !out.contains_key(&link.key) {
            out.insert(link.key, target);
        }
    }
    debug_assert!(out.keys().all(|key| candidates.contains_key(key)));
    Ok(out)
}
