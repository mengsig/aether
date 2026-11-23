use super::*;
use aether_core::delegation::{DelegationPolicy, WorkerChoice};

const PRIMARY: &str = "anthropic/claude-sonnet-5-5";
const FALLBACK: &str = "groq/openai/gpt-oss-120b";
const SECOND: &str = "openai/gpt-6.1-sol";

fn policy(models: &[&str]) -> DelegationPolicy {
    assert!(!models.is_empty());
    let workers = models
        .iter()
        .map(|model| WorkerChoice {
            model: Some(ModelRef::parse(model).expect("valid model")),
            tier: None,
            effort: None,
        })
        .collect::<Vec<_>>();
    DelegationPolicy {
        workers,
        ..Default::default()
    }
}

fn delegate(fallbacks: Option<serde_json::Value>) -> aether_model::testing::ScriptedResponse {
    let mut input = json!({"role": "worker", "objective": "do the work", "model": PRIMARY, "effort": "low", "wait": true, "leaf": true});
    if let Some(fallbacks) = fallbacks {
        input["fallbacks"] = fallbacks;
    }
    ScriptedClient::tool_call("delegate", input)
}

async fn start(fallbacks: Option<serde_json::Value>, allowed: &[&str]) -> (Harness, AgentId) {
    let h = harness_configured(
        vec![],
        |_| {},
        |c| {
            c.exhaustion_probe_floor = NO_PROBE_IN_THIS_TEST;
        },
    )
    .await;
    h.client.push_for("openai", delegate(fallbacks));
    h.client.push_for(
        "anthropic",
        ScriptedClient::error(ModelErrorClass::QuotaExhausted, "credit balance too low"),
    );
    h.client.push_for(
        "groq",
        ScriptedClient::tool_call("inspect_state", json!({})),
    );
    h.client.push_for(
        "groq",
        ScriptedClient::tool_call("complete_task", json!({"summary": "finished on fallback"})),
    );
    let mut parent = spec("delegate the work");
    parent.model = Some(ModelRef::parse(SECOND).expect("parent model"));
    parent.config.as_mut().expect("config").delegation_policy = Some(policy(allowed));
    let (parent, _) = h
        .runtime
        .create_agent(parent, Actor::user())
        .expect("parent");
    (h, parent.id)
}

fn events(h: &Harness, id: AgentId) -> Vec<Event> {
    h.store
        .events(0, 10_000, &EventFilter::for_agent(id))
        .expect("events")
}

#[tokio::test]
async fn delegated_chain_runs_on_fallback_and_reports_its_order() {
    let (h, parent) = start(
        Some(json!([FALLBACK, SECOND])),
        &[PRIMARY, FALLBACK, SECOND],
    )
    .await;
    let child = tokio::time::timeout(Duration::from_secs(10), child_of(&h, parent))
        .await
        .expect("child created");
    let status = tokio::time::timeout(
        Duration::from_secs(10),
        h.wait_until(child, |s| s.is_terminal()),
    )
    .await
    .expect("child finished");
    assert!(
        matches!(status, AgentStatus::Completed { .. }),
        "{status:?}"
    );
    let child_events = events(&h, child);
    assert!(child_events.iter().any(|event| matches!(&event.kind,
        EventKind::ProviderFallback { from, to, .. }
            if *from == ModelRef::parse(PRIMARY).unwrap() && *to == ModelRef::parse(FALLBACK).unwrap()
    )), "{child_events:?}");
    let outcome = settled_outcome(&child_events, "inspect_state");
    let state: serde_json::Value =
        serde_json::from_str(&outcome.content).expect("inspect_state JSON");
    assert_eq!(state["model"], json!(ModelRef::parse(PRIMARY).unwrap()));
    assert_eq!(
        state["fallbacks"],
        json!([
            ModelRef::parse(FALLBACK).unwrap(),
            ModelRef::parse(SECOND).unwrap()
        ])
    );
    h.runtime.shutdown().await;
}

#[tokio::test]
async fn forbidden_fallback_is_refused_without_creating_work() {
    let (h, parent) = start(Some(json!([FALLBACK])), &[PRIMARY]).await;
    tokio::time::timeout(
        Duration::from_secs(10),
        h.wait_for("delegate answer", || {
            events(&h, parent)
                .iter()
                .any(|event| matches!(&event.kind, EventKind::ToolSucceeded { .. }))
        }),
    )
    .await
    .expect("delegate settled");
    let parent_events = events(&h, parent);
    let outcome = settled_outcome(&parent_events, "delegate");
    assert!(
        outcome.is_error && outcome.content.contains(FALLBACK),
        "{outcome:?}"
    );
    assert!(
        h.store
            .agents(&AgentFilter {
                parent: Some(parent),
                ..Default::default()
            })
            .unwrap()
            .is_empty()
    );
    assert_eq!(
        h.store.tasks(&TaskFilter::default()).unwrap().len(),
        1,
        "only the parent's task exists"
    );
    assert!(
        !h.store
            .events(0, 10_000, &EventFilter::all())
            .unwrap()
            .iter()
            .any(|event| matches!(event.kind, EventKind::ProviderFallback { .. }))
    );
    h.runtime.shutdown().await;
}

#[tokio::test]
async fn legacy_delegation_stays_blocked_on_its_exhausted_primary() {
    let (h, parent) = start(None, &[PRIMARY]).await;
    let child = tokio::time::timeout(Duration::from_secs(10), child_of(&h, parent))
        .await
        .expect("child created");
    let status = tokio::time::timeout(
        Duration::from_secs(10),
        h.wait_until(child, blocked_on_exhaustion),
    )
    .await
    .expect("child blocked");
    let BlockReason::Exhausted { model, .. } = exhausted_reason(&status) else {
        unreachable!()
    };
    assert_eq!(model, ModelRef::parse(PRIMARY).unwrap());
    assert!(h.store.agent(child).unwrap().unwrap().fallbacks.is_empty());
    assert!(
        !events(&h, child)
            .iter()
            .any(|event| matches!(event.kind, EventKind::ProviderFallback { .. }))
    );
    tokio::time::timeout(
        Duration::from_secs(10),
        h.wait_for("parent suspension notice", || {
            !child_suspensions(h.store.as_ref(), parent).is_empty()
        }),
    )
    .await
    .expect("parent receives the block");
    h.runtime.shutdown().await;
}
