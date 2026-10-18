//! Peers: the agents sharing a delegated agent's parent, and how far that
//! agent's messages may reach.
use crate::inspection::{brief, outcome};
use crate::{Result, Runtime, RuntimeError};
use aether_core::agent::Agent;
use aether_core::id::AgentId;
use aether_core::tool::ToolOutcome;
use aether_store::AgentFilter;
use aether_tools::builtin::inspection::MAX_LIST_ITEMS;
use serde_json::json;

/// The caller's siblings: every other agent its parent delegated. A root
/// agent has none.
pub(crate) fn list(
    runtime: &Runtime,
    caller: AgentId,
    after: Option<AgentId>,
    limit: usize,
) -> Result<ToolOutcome> {
    if !(1..=MAX_LIST_ITEMS).contains(&limit) {
        return Ok(ToolOutcome::error(format!(
            "limit must be between 1 and {MAX_LIST_ITEMS}"
        )));
    }
    let me = runtime
        .store
        .agent(caller)?
        .ok_or_else(|| RuntimeError::NotFound(caller.to_string()))?;
    let Some(parent) = me.parent else {
        return Ok(outcome(json!({"parent":null,"peers":[],"next_after":null})));
    };
    let mut peers: Vec<_> = runtime
        .store
        .agents(&AgentFilter {
            parent: Some(parent),
            ..Default::default()
        })?
        .into_iter()
        .filter(|a| a.id != caller && after.is_none_or(|after| a.id > after))
        .collect();
    debug_assert!(peers.iter().all(|a| a.parent == Some(parent)));
    peers.sort_by_key(|a| a.id);
    let more = peers.len() > limit;
    peers.truncate(limit);
    let next = more.then(|| peers.last().map(|a| a.id)).flatten();
    let mut rows = Vec::with_capacity(peers.len());
    for peer in peers {
        let objective = match peer.current_task {
            Some(task) => runtime.store.task(task)?.map(|t| brief(&t.objective, 160)),
            None => None,
        };
        rows.push(json!({
            "id":peer.id, "name":brief(&peer.name,64), "role":peer.role,
            "status":peer.status.kind(), "task":peer.current_task, "objective":objective,
        }));
    }
    Ok(outcome(
        json!({"parent":parent,"peers":rows,"next_after":next}),
    ))
}

/// Why `sender` may not message `to`, if it may not. A delegated agent
/// reaches its parent's subtree and its own ancestors; a fan-out only the
/// parent's subtree, which an ancestor's subtree would leave. Roots answer
/// to the operator and keep their reach.
pub(crate) fn message_reach_refusal(
    runtime: &Runtime,
    sender: &Agent,
    to: AgentId,
    subtree: bool,
) -> Result<Option<String>> {
    let Some(parent) = sender.parent else {
        return Ok(None);
    };
    let store = &runtime.store;
    debug_assert!(store.is_descendant(parent, sender.id)?);
    if store.is_descendant(parent, to)? {
        return Ok(None);
    }
    // `to` is an ancestor of the sender exactly when the sender sits below it.
    if !subtree && to != sender.id && store.is_descendant(to, sender.id)? {
        return Ok(None);
    }
    let reach = if subtree {
        "A subtree message reaches only your parent's subtree"
    } else {
        "You may message your parent's subtree (your parent, your peers and their descendants) and your own ancestors"
    };
    Ok(Some(format!(
        "{to} is outside your reach. {reach}; list_peers shows your peers."
    )))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{RuntimeConfig, commands::CreateAgentSpec};
    use aether_core::agent::{AgentConfig, ApprovalPolicy};
    use aether_core::event::Actor;
    use aether_core::model::ModelRef;
    use aether_model::{Models, testing::ScriptedClient};
    use aether_store::SqliteStore;
    use aether_tools::ToolRegistry;
    use serde_json::Value;
    use std::collections::BTreeSet;
    use std::sync::Arc;

    async fn runtime() -> (Arc<Runtime>, tempfile::TempDir) {
        let config = tempfile::tempdir().unwrap();
        let runtime = Runtime::start_with_engines(
            RuntimeConfig {
                default_model: Some(ModelRef::new("anthropic", "claude-sonnet-5-5")),
                ..Default::default()
            },
            Arc::new(SqliteStore::open_in_memory().unwrap()),
            Arc::new(Models::load(Some(config.path().into())).unwrap()),
            Arc::new(ToolRegistry::with_builtins()),
            Some(Arc::new(ScriptedClient::new())),
            Default::default(),
        )
        .await
        .unwrap();
        (runtime, config)
    }

    fn create(runtime: &Runtime, parent: Option<AgentId>, objective: &str) -> Agent {
        runtime
            .create_agent(
                CreateAgentSpec {
                    role: "worker".into(),
                    objective: Some(objective.into()),
                    parent,
                    config: Some(AgentConfig {
                        effort: Some(aether_core::model::Effort::Low),
                        approval: ApprovalPolicy::none(),
                        ..Default::default()
                    }),
                    start: false,
                    ..Default::default()
                },
                parent.map(Actor::agent).unwrap_or_else(Actor::user),
            )
            .unwrap()
            .0
    }

    /// grandparent -> {lead -> {me -> mine, sibling -> nephew}, uncle -> cousin}, and a foreign root.
    struct Fleet {
        grandparent: Agent,
        lead: Agent,
        me: Agent,
        mine: Agent,
        sibling: Agent,
        nephew: Agent,
        uncle: Agent,
        cousin: Agent,
        foreign: Agent,
    }

    fn fleet(runtime: &Runtime) -> Fleet {
        let grandparent = create(runtime, None, "grandparent");
        let lead = create(runtime, Some(grandparent.id), "lead");
        let me = create(runtime, Some(lead.id), "build the parser");
        let sibling = create(runtime, Some(lead.id), "build the lexer");
        let uncle = create(runtime, Some(grandparent.id), "uncle");
        Fleet {
            mine: create(runtime, Some(me.id), "mine"),
            nephew: create(runtime, Some(sibling.id), "nephew"),
            cousin: create(runtime, Some(uncle.id), "cousin"),
            foreign: create(runtime, None, "foreign"),
            grandparent,
            lead,
            me,
            sibling,
            uncle,
        }
    }

    fn listed(page: &Value) -> BTreeSet<AgentId> {
        page["peers"]
            .as_array()
            .unwrap()
            .iter()
            .map(|row| serde_json::from_value(row["id"].clone()).unwrap())
            .collect()
    }

    #[tokio::test]
    async fn peers_are_exactly_the_other_children_of_the_parent() {
        let (runtime, _config) = runtime().await;
        let f = fleet(&runtime);
        let third = create(&runtime, Some(f.lead.id), "build the printer");

        let first = list(&runtime, f.me.id, None, 1)
            .unwrap()
            .structured
            .unwrap();
        assert_eq!(first["parent"], json!(f.lead.id));
        let cursor = serde_json::from_value(first["next_after"].clone()).unwrap();
        let second = list(&runtime, f.me.id, Some(cursor), 1)
            .unwrap()
            .structured
            .unwrap();
        assert!(second["next_after"].is_null());
        let found: BTreeSet<_> = listed(&first).union(&listed(&second)).copied().collect();
        assert_eq!(found, BTreeSet::from([f.sibling.id, third.id]));

        let row = first["peers"][0].clone();
        let row = if row["id"] == json!(f.sibling.id) {
            row
        } else {
            second["peers"][0].clone()
        };
        assert_eq!(row["objective"]["text"], "build the lexer");
        assert_eq!(row["role"], "worker");

        let root = list(&runtime, f.foreign.id, None, 10)
            .unwrap()
            .structured
            .unwrap();
        assert!(root["parent"].is_null());
        assert!(listed(&root).is_empty(), "a root agent has no peers");
        assert!(list(&runtime, f.me.id, None, 0).unwrap().is_error);
        runtime.shutdown().await;
    }

    #[tokio::test]
    async fn a_delegated_agent_reaches_only_its_parents_subtree_and_ancestors() {
        let (runtime, _config) = runtime().await;
        let f = fleet(&runtime);
        let reaches = |to: AgentId, subtree: bool| {
            message_reach_refusal(&runtime, &f.me, to, subtree)
                .unwrap()
                .is_none()
        };
        for to in [f.lead.id, f.sibling.id, f.nephew.id, f.mine.id] {
            assert!(reaches(to, false), "{to} is in the parent's subtree");
            assert!(reaches(to, true), "{to} roots a subtree inside it");
        }
        assert!(
            reaches(f.grandparent.id, false),
            "an ancestor stays reachable"
        );
        assert!(
            !reaches(f.grandparent.id, true),
            "a fan-out from an ancestor would reach the cousins"
        );
        for to in [f.uncle.id, f.cousin.id, f.foreign.id] {
            assert!(!reaches(to, false), "{to} is outside the parent's subtree");
            assert!(!reaches(to, true), "{to} is outside the parent's subtree");
        }
        let refusal = message_reach_refusal(&runtime, &f.me, f.cousin.id, false)
            .unwrap()
            .unwrap();
        assert!(refusal.contains("list_peers"), "{refusal}");

        let root = message_reach_refusal(&runtime, &f.grandparent, f.foreign.id, true).unwrap();
        assert!(root.is_none(), "a root agent keeps its reach");
        runtime.shutdown().await;
    }
}
