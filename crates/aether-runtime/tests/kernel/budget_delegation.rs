//! budget delegation scenarios and fixtures.
use super::*;

/// A run budget fails new delegations rather than cancelling running work,
/// and the refusal names the budget and the spend so the model can act on
/// it. Spend is a floor, so the refusal says so when it is one.
#[tokio::test]
async fn a_spent_run_budget_refuses_new_delegations() {
    let h = harness_configured(
        vec![ScriptedClient::tool_call(
            "delegate",
            json!({"model":"anthropic/claude-sonnet-5-5", "effort":"low", "role": "worker", "objective": "more work"}),
        )],
        |_| {},
        |c| c.run_budget_usd = Some(1.0),
    )
    .await;
    let (parent, _) = h
        .runtime
        .create_agent(
            CreateAgentSpec {
                objective: None,
                ..spec("supervise")
            },
            Actor::user(),
        )
        .expect("parent");
    // Parked with nothing to do, so the seeded spend is not written over by
    // a turn already in flight.
    h.wait_until(parent.id, |s| matches!(s, AgentStatus::Waiting { .. }))
        .await;
    // The run is already past its budget when the delegation is attempted.
    let mut spent = h.store.agent(parent.id).expect("read").expect("exists");
    spent.stats.usage.cost_usd = 2.5;
    spent
        .stats
        .usage
        .mark_cost_gap(aether_core::model::CostGap::Unpriced);
    h.store
        .commit(Commit::new().mutate(Mutation::UpsertAgent(spent)))
        .expect("seed spend");
    h.runtime
        .send_message(parent.id, vec![ContentBlock::text("go")], Actor::user())
        .expect("nudge");
    let told = wait_for_tool_answer(&h, parent.id).await;
    assert!(
        told.contains("delegation refused")
            && told.contains("of its $1 budget")
            && told.contains("$2.5")
            && told.contains("unpriced"),
        "the refusal did not name the budget and the spend as a floor: {told}"
    );
    let children = h
        .store
        .agents(&AgentFilter {
            parent: Some(parent.id),
            ..Default::default()
        })
        .expect("children");
    assert!(
        children.is_empty(),
        "a refused delegation left an agent behind: {children:?}"
    );
}
