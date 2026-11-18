//! The delegation policy and the no-policy tier rules on `delegate`.
use super::*;
use aether_core::delegation::DelegationPolicy;
use aether_model::testing::ScriptedResponse;

const FABLE: &str = "anthropic/claude-fable-5-2";
const OPUS: &str = "anthropic/claude-opus-5-5";
const SONNET: &str = "anthropic/claude-sonnet-5-5";

fn delegate_call(model: &str, effort: &str, extra: serde_json::Value) -> ScriptedResponse {
    let mut input = json!({"role": "worker", "objective": "part", "model": model, "effort": effort, "wait": false, "leaf": true});
    if let (Some(fields), Some(more)) = (input.as_object_mut(), extra.as_object()) {
        fields.extend(more.clone());
    }
    ScriptedClient::tool_call("delegate", input)
}

fn lead(model: &str, policy: Option<DelegationPolicy>) -> CreateAgentSpec {
    let mut lead = spec("lead the change");
    lead.role = "lead".into();
    lead.model = ModelRef::parse(model);
    let config = lead.config.as_mut().expect("spec has a config");
    config.delegation_policy = policy;
    lead
}

/// Every tool answer the agent received, oldest first, once `count` landed.
async fn tool_answers(h: &Harness, id: AgentId, count: usize) -> Vec<String> {
    let answers = || {
        h.store
            .events(0, 10_000, &EventFilter::for_agent(id))
            .expect("events")
            .into_iter()
            .filter_map(|e| match e.kind {
                EventKind::ToolSucceeded { outcome, .. } => Some(outcome.content),
                EventKind::ToolFailed { error, .. } => Some(error),
                _ => None,
            })
            .collect::<Vec<_>>()
    };
    h.wait_for("tool answers", || answers().len() >= count)
        .await;
    let answers = answers();
    assert!(answers.len() >= count, "{answers:?}");
    answers
}

fn children(h: &Harness, parent: AgentId) -> Vec<Agent> {
    h.store
        .agents(&AgentFilter {
            parent: Some(parent),
            ..Default::default()
        })
        .expect("children")
}

#[tokio::test]
async fn without_a_policy_a_child_above_the_parents_tier_is_refused() {
    let h = harness(vec![delegate_call(OPUS, "low", json!({}))]).await;
    let (parent, _) = h
        .runtime
        .create_agent(lead(SONNET, None), Actor::user())
        .expect("lead");
    let told = tool_answers(&h, parent.id, 1).await.remove(0);
    assert!(
        told.contains("strong-tier model, above your own balanced tier"),
        "{told}"
    );
    assert!(
        children(&h, parent.id).is_empty(),
        "a refusal created a child"
    );
    h.runtime.shutdown().await;
}

#[tokio::test]
async fn a_clone_of_the_frontier_lead_needs_a_reason_that_is_recorded() {
    let h = harness(vec![
        delegate_call(FABLE, "high", json!({})),
        delegate_call(
            FABLE,
            "high",
            json!({"reason": "  the proof needs frontier depth  "}),
        ),
    ])
    .await;
    let (parent, _) = h
        .runtime
        .create_agent(lead(FABLE, None), Actor::user())
        .expect("lead");
    let answers = tool_answers(&h, parent.id, 2).await;
    assert!(
        answers[0].contains("is your own frontier-tier model"),
        "{answers:?}"
    );
    assert!(answers[0].contains("pass `reason`"), "{answers:?}");
    assert!(answers[1].starts_with("Delegated to agent"), "{answers:?}");
    let created = children(&h, parent.id);
    assert_eq!(
        created.len(),
        1,
        "only the call with a reason created a child"
    );
    let task = created[0].current_task.expect("the child has a task");
    let task = h.store.task(task).expect("read").expect("task");
    assert_eq!(
        task.delegation_reason.as_deref(),
        Some("the proof needs frontier depth")
    );
    assert!(created[0].config.delegation_policy.is_none());
    h.runtime.shutdown().await;
}

#[tokio::test]
async fn superseded_pins_keep_the_parent_ceiling_and_frontier_clone_rule() {
    let fable = "anthropic/claude-fable-5-1";
    for (parent, child, policy, expected) in [
        (
            SONNET,
            "openai/gpt-6-astra",
            None,
            "above your own balanced tier",
        ),
        (
            SONNET,
            fable,
            Some(DelegationPolicy::default()),
            "above your own balanced tier",
        ),
        (fable, fable, None, "is your own frontier-tier model"),
        (
            "openai/gpt-6.1-sol",
            OPUS,
            None,
            "above your own balanced tier",
        ),
    ] {
        let h = harness(vec![delegate_call(child, "low", json!({}))]).await;
        let (agent, _) = h
            .runtime
            .create_agent(lead(parent, policy), Actor::user())
            .unwrap();
        let answers = tool_answers(&h, agent.id, 1).await;
        assert!(answers[0].contains(expected), "{answers:?}");
        assert!(children(&h, agent.id).is_empty());
        h.runtime.shutdown().await;
    }
}

#[tokio::test]
async fn a_malformed_policy_is_refused_at_creation() {
    let h = harness(Vec::new()).await;
    let policy = DelegationPolicy {
        max_parallel: Some(0),
        ..DelegationPolicy::default()
    };
    let refused = h
        .runtime
        .create_agent(lead(SONNET, Some(policy)), Actor::user())
        .expect_err("max_parallel 0");
    assert!(refused.to_string().contains("max_parallel"), "{refused}");
    h.runtime.shutdown().await;
}
