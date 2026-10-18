//! Swarm peers: siblings message each other, and nothing past the parent's subtree.
use super::*;

/// An idle agent: no task, no turn, until something is assigned to it.
fn idle(parent: Option<AgentId>, leaf: bool) -> CreateAgentSpec {
    let mut s = spec("unused");
    s.objective = None;
    s.parent = parent;
    s.config.as_mut().expect("config").leaf = leaf;
    s
}

fn assign(h: &Harness, owner: AgentId, objective: &str) {
    h.runtime
        .create_task(
            aether_runtime::CreateTaskSpec {
                objective: objective.into(),
                owner: Some(owner),
                ..Default::default()
            },
            Actor::user(),
        )
        .expect("task");
}

fn messages(h: &Harness, id: AgentId) -> Vec<(Actor, String)> {
    h.store
        .inbox(id, 0)
        .expect("inbox")
        .into_iter()
        .filter_map(|i| match i.kind {
            aether_core::inbox::InboxKind::Message { from, content, .. } => {
                Some((from, aether_core::model::text_of(&content)))
            }
            _ => None,
        })
        .collect()
}

/// A leaf worker keeps `send_message`, and its message wakes a sibling
/// waiting on it.
#[tokio::test]
async fn a_leaf_workers_message_wakes_a_waiting_sibling() {
    let h = harness(vec![]).await;
    let mut lead_spec = idle(None, false);
    lead_spec.config.as_mut().expect("config").long_lived = true;
    let (lead, _) = h
        .runtime
        .create_agent(lead_spec, Actor::user())
        .expect("lead");
    let (provider, _) = h
        .runtime
        .create_agent(idle(Some(lead.id), true), Actor::user())
        .expect("provider");
    let (consumer, _) = h
        .runtime
        .create_agent(idle(Some(lead.id), true), Actor::user())
        .expect("consumer");

    h.client.push(ScriptedClient::tool_call(
        "wait_for",
        json!({"message_from": provider.id.to_string()}),
    ));
    assign(&h, consumer.id, "use the parser once it exists");
    h.wait_until(consumer.id, |s| s.kind() == AgentStatusKind::Waiting)
        .await;

    // One response, so the woken consumer cannot take the provider's next call.
    h.client.push(ScriptedClient::tool_calls(
        &[
            (
                "send_message",
                json!({"to": consumer.id.to_string(), "content": "parse() is ready"}),
            ),
            ("complete_task", json!({"summary": "parser built"})),
        ],
        None,
    ));
    h.client.push(ScriptedClient::tool_call(
        "complete_task",
        json!({"summary": "used the parser"}),
    ));
    assign(&h, provider.id, "build the parser");

    h.wait_until(provider.id, |s| s.kind() == AgentStatusKind::Completed)
        .await;
    h.wait_until(consumer.id, |s| s.kind() == AgentStatusKind::Completed)
        .await;
    assert_eq!(
        messages(&h, consumer.id),
        vec![(Actor::agent(provider.id), "parse() is ready".to_owned())]
    );
    assert!(messages(&h, lead.id).is_empty(), "nothing went to the lead");
}

/// The `send_message` tool refuses a cousin and a foreign tree, and
/// delivers nothing to either.
#[tokio::test]
async fn send_message_refuses_agents_outside_the_parents_subtree() {
    let h = harness(vec![]).await;
    let long_lived = |parent| {
        let mut s = idle(parent, false);
        s.config.as_mut().expect("config").long_lived = true;
        s
    };
    let (root, _) = h
        .runtime
        .create_agent(long_lived(None), Actor::user())
        .expect("root");
    let (lead, _) = h
        .runtime
        .create_agent(long_lived(Some(root.id)), Actor::user())
        .expect("lead");
    let (uncle, _) = h
        .runtime
        .create_agent(long_lived(Some(root.id)), Actor::user())
        .expect("uncle");
    let (cousin, _) = h
        .runtime
        .create_agent(long_lived(Some(uncle.id)), Actor::user())
        .expect("cousin");
    let (foreign, _) = h
        .runtime
        .create_agent(long_lived(None), Actor::user())
        .expect("foreign");
    let (worker, _) = h
        .runtime
        .create_agent(idle(Some(lead.id), true), Actor::user())
        .expect("worker");

    h.client.push(ScriptedClient::tool_calls(
        &[
            (
                "send_message",
                json!({"to": cousin.id.to_string(), "content": "psst"}),
            ),
            (
                "send_message",
                json!({"to": foreign.id.to_string(), "content": "psst"}),
            ),
            (
                "send_message",
                json!({"to": root.id.to_string(), "content": "psst", "subtree": true}),
            ),
            ("complete_task", json!({"summary": "tried"})),
        ],
        None,
    ));
    assign(&h, worker.id, "reach out");
    h.wait_until(worker.id, |s| s.kind() == AgentStatusKind::Completed)
        .await;

    for id in [root.id, lead.id, uncle.id, cousin.id, foreign.id] {
        assert!(messages(&h, id).is_empty(), "{id} was reached");
    }
    let refusals = h
        .store
        .events(0, 10_000, &EventFilter::for_agent(worker.id))
        .expect("events")
        .into_iter()
        .filter(|e| {
            matches!(&e.kind, EventKind::ToolSucceeded { outcome, .. }
                if outcome.is_error && outcome.content.contains("is outside your reach"))
        })
        .count();
    assert_eq!(
        refusals, 3,
        "each refusal is an error outcome the model sees"
    );
}

/// A delegated leaf is offered `list_peers` and finds its sibling with it,
/// while the lead's `swarm_playbook` is neither offered nor run.
#[tokio::test]
async fn a_delegated_leaf_lists_its_peers_but_not_the_playbook() {
    let h = harness(vec![]).await;
    let mut lead_spec = idle(None, false);
    lead_spec.config.as_mut().expect("config").long_lived = true;
    let (lead, _) = h
        .runtime
        .create_agent(lead_spec, Actor::user())
        .expect("lead");
    let (sibling, _) = h
        .runtime
        .create_agent(idle(Some(lead.id), true), Actor::user())
        .expect("sibling");
    let (worker, _) = h
        .runtime
        .create_agent(idle(Some(lead.id), true), Actor::user())
        .expect("worker");

    h.client.push(ScriptedClient::tool_calls(
        &[
            ("list_peers", json!({})),
            ("swarm_playbook", json!({})),
            ("complete_task", json!({"summary": "looked around"})),
        ],
        None,
    ));
    assign(&h, worker.id, "find your peers");
    h.wait_until(worker.id, |s| s.kind() == AgentStatusKind::Completed)
        .await;

    let requests = h.client.requests();
    assert_eq!(requests.len(), 1);
    let offered: Vec<_> = requests[0].tools.iter().map(|t| t.name.as_str()).collect();
    assert!(offered.contains(&"list_peers"), "{offered:?}");
    assert!(!offered.contains(&"swarm_playbook"), "{offered:?}");

    let events = h
        .store
        .events(0, 10_000, &EventFilter::for_agent(worker.id))
        .expect("events");
    let listed = events.iter().any(|e| {
        matches!(&e.kind, EventKind::ToolSucceeded { outcome, .. }
            if !outcome.is_error && outcome.content.contains(&sibling.id.to_string()))
    });
    assert!(listed, "list_peers names the sibling");
    let refused = events.iter().any(|e| {
        matches!(&e.kind, EventKind::ToolFailed { error, .. }
            if error.contains("`swarm_playbook` is withheld from leaf agents"))
    });
    assert!(refused, "the playbook is refused to a leaf");
}
