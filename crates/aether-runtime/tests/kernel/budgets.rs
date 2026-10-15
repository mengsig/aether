//! budgets scenarios and fixtures.
use super::*;

/// VISION §3.10: a budget warns the person, then stops the agent cleanly as
/// Failed with the reason, and it covers the whole subtree.
#[tokio::test]
async fn a_cost_budget_warns_then_stops_the_agent_with_the_reason() {
    // The agent is parked with no objective: it takes no turn here, and the
    // suite would notice if it did.
    let h = harness(vec![]).await;
    let mut s = spec("spend carefully");
    s.objective = None;
    {
        let config = s.config.as_mut().expect("config");
        config.long_lived = true;
        config.cost_budget = Some(0.01);
    }
    let (agent, _) = h.runtime.create_agent(s, Actor::user()).expect("create");

    // Spend against the budget by hand: the scripted model reports no cost.
    let spend = |usd: f64| {
        let mut a = h.store.agent(agent.id).expect("a").expect("e");
        a.stats.usage += aether_core::model::Usage::cost_only(Some(usd));
        h.store
            .commit(Commit::new().mutate(Mutation::UpsertAgent(a)))
            .expect("commit");
    };
    spend(0.009);
    let deadline = tokio::time::Instant::now() + Duration::from_secs(15);
    loop {
        let warned = h
            .store
            .events(0, 10_000, &EventFilter::for_agent(agent.id))
            .expect("events")
            .into_iter()
            .any(|e| matches!(e.kind, EventKind::BudgetWarning { .. }));
        if warned {
            break;
        }
        assert!(
            tokio::time::Instant::now() < deadline,
            "no warning at 80% of the budget"
        );
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    assert!(
        !h.store
            .agent(agent.id)
            .expect("a")
            .expect("e")
            .status
            .is_terminal(),
        "a warning must not stop the agent"
    );
    // A soft limit repeated every tick is noise: several ticks, one warning.
    tokio::time::sleep(Duration::from_millis(300)).await;
    let warnings = h
        .store
        .events(0, 10_000, &EventFilter::for_agent(agent.id))
        .expect("events")
        .into_iter()
        .filter(|e| matches!(e.kind, EventKind::BudgetWarning { .. }))
        .count();
    assert_eq!(warnings, 1, "the warning repeated every tick");

    spend(0.005);
    let status = h.wait_until(agent.id, |s| s.is_terminal()).await;
    let AgentStatus::Failed { error } = &status else {
        panic!("expected Failed, got {status:?}");
    };
    assert!(error.contains("cost budget of $0.01"), "{error}");
    let t = types(h.store.as_ref(), agent.id);
    assert!(t.contains(&"budget.warning".to_owned()), "{t:?}");
    assert!(t.contains(&"budget.exceeded".to_owned()), "{t:?}");
    assert_eq!(h.client.remaining(), 0, "the parked agent took a turn");
}

/// A supervisor's budget covers what its workers spend, so delegating cannot
/// escape it.
#[tokio::test]
async fn a_budget_counts_what_the_subtree_spent() {
    let h = harness(vec![]).await;
    let mut s = spec("supervise");
    s.objective = None;
    {
        let config = s.config.as_mut().expect("config");
        config.long_lived = true;
        config.cost_budget = Some(1.0);
    }
    let (parent, _) = h.runtime.create_agent(s, Actor::user()).expect("parent");
    let (child, _) = h
        .runtime
        .create_agent(
            CreateAgentSpec {
                parent: Some(parent.id),
                objective: None,
                start: false,
                ..spec("worker")
            },
            Actor::user(),
        )
        .expect("child");
    // The parent spends nothing; the child spends past the parent's budget.
    let mut c = h.store.agent(child.id).expect("a").expect("e");
    c.stats.usage.cost_usd = 1.5;
    h.store
        .commit(Commit::new().mutate(Mutation::UpsertAgent(c)))
        .expect("commit");
    let status = h.wait_until(parent.id, |s| s.is_terminal()).await;
    assert_eq!(status.kind(), AgentStatusKind::Failed);
    let child = h.store.agent(child.id).expect("a").expect("e");
    assert!(
        child.status.is_terminal(),
        "the subtree kept running: {:?}",
        child.status
    );
}

/// VISION §3.10: a cost budget enforces on the priced floor. An agent in the
/// subtree that never reported a price cannot make the spend that *was*
/// reported disappear, so the budget still fires - and the person is told
/// once that the figure it fired on is a lower bound.
#[tokio::test]
async fn a_budget_fires_on_the_priced_part_of_a_partly_unpriced_subtree() {
    let h = harness(vec![]).await;
    let mut s = spec("supervise");
    s.objective = None;
    {
        let config = s.config.as_mut().expect("config");
        config.long_lived = true;
        config.cost_budget = Some(1.0);
    }
    let (parent, _) = h.runtime.create_agent(s, Actor::user()).expect("parent");
    let child = |name: &str| {
        h.runtime
            .create_agent(
                CreateAgentSpec {
                    parent: Some(parent.id),
                    objective: None,
                    start: false,
                    ..spec(name)
                },
                Actor::user(),
            )
            .expect("child")
            .0
    };
    let (priced, unpriced) = (child("priced"), child("unpriced"));

    // One worker's turns were priced past the parent's budget; the other's
    // engine never reported a price at all.
    let seed = |id, usage| {
        let mut a = h.store.agent(id).expect("a").expect("e");
        a.stats.usage = usage;
        h.store
            .commit(Commit::new().mutate(Mutation::UpsertAgent(a)))
            .expect("commit");
    };
    seed(
        unpriced.id,
        aether_core::model::Usage {
            input_tokens: 500,
            cost_complete: false,
            cost_gap: Some(aether_core::model::CostGap::Unpriced),
            ..Default::default()
        },
    );
    seed(
        priced.id,
        aether_core::model::Usage {
            input_tokens: 500,
            cost_usd: 1.5,
            ..Default::default()
        },
    );

    let status = h.wait_until(parent.id, |s| s.is_terminal()).await;
    let AgentStatus::Failed { error } = &status else {
        panic!("expected Failed, got {status:?}");
    };
    assert!(
        error.contains("cost budget of $1") && error.contains("some unpriced"),
        "the stop must fire and say what it fired on: {error}"
    );
    let t = types(h.store.as_ref(), parent.id);
    assert!(t.contains(&"budget.exceeded".to_owned()), "{t:?}");

    // And the lower-bound notice was said, once, alongside the stop.
    let notes: Vec<String> = h
        .store
        .latest_events(
            16,
            &aether_store::EventFilter {
                agent: Some(parent.id),
                types: vec!["budget.warning".into()],
                ..Default::default()
            },
        )
        .expect("events")
        .into_iter()
        .filter_map(|e| match e.kind {
            aether_core::event::EventKind::BudgetWarning { reason, .. } => Some(reason),
            _ => None,
        })
        .filter(|r| r.starts_with(aether_core::agent::COST_FLOOR_NOTE))
        .collect();
    assert_eq!(notes.len(), 1, "said once, not every tick: {notes:?}");
}

/// VISION §3.10 with §8: a budget that cannot be enforced is refused where it
/// is set, not left as a hard stop that silently never fires.
#[tokio::test]
async fn a_cost_budget_is_refused_on_a_provider_that_reports_no_cost() {
    let h = harness_full(
        vec![],
        |_| {},
        |_| {},
        Some(
            "[providers.frugal]\nprotocol = \"openai-chat-completions\"\n\
             base_url = \"http://127.0.0.1:1\"\n\
             [[providers.frugal.models]]\nid = \"nocost\"\n",
        ),
        None,
        None,
        |_| {},
    )
    .await;
    let budgeted = |provider: &str, model: &str| CreateAgentSpec {
        config: Some(AgentConfig {
            cost_budget: Some(1.0),
            approval: ApprovalPolicy::none(),
            ..Default::default()
        }),
        ..spec_on("spend a little", provider, model)
    };
    let err = h
        .runtime
        .create_agent(budgeted("frugal", "nocost"), Actor::user())
        .expect_err("a cost budget that cannot be enforced");
    assert!(err.to_string().contains("no dollar cost"), "{err}");
    assert!(
        err.to_string()
            .contains("token caps are supported: set token_budget"),
        "{err}"
    );

    // A priced model takes the same budget.
    let (agent, _) = h
        .runtime
        .create_agent(budgeted("anthropic", "claude-sonnet-5-5"), Actor::user())
        .expect("priced model");
    // And moving it onto the unpriced provider is refused too.
    let err = h
        .runtime
        .update_agent(
            agent.id,
            aether_runtime::AgentUpdate {
                model: Some(ModelRef::new("frugal", "nocost")),
                ..Default::default()
            },
            Actor::user(),
        )
        .expect_err("the pair is checked, not just the budget");
    assert!(err.to_string().contains("no dollar cost"), "{err}");
    assert!(
        err.to_string()
            .contains("token caps are supported: set token_budget"),
        "{err}"
    );
    assert_eq!(
        h.store
            .agent(agent.id)
            .expect("a")
            .expect("exists")
            .model
            .provider,
        "anthropic",
        "a refused update changes nothing"
    );
}

/// VISION §1.1: one terminal decision per agent. A supervisor and a worker
/// under it can both be over budget in the same pass; the worker must not be
/// failed from a snapshot taken before its parent cancelled it.
#[tokio::test]
async fn a_budget_stop_does_not_re_terminate_the_subtree_it_cancelled() {
    let h = harness(vec![]).await;
    let (parent, _) = h
        .runtime
        .create_agent(
            CreateAgentSpec {
                objective: None,
                config: Some(AgentConfig {
                    cost_budget: Some(10.0),
                    approval: ApprovalPolicy::none(),
                    ..Default::default()
                }),
                ..spec("supervise")
            },
            Actor::user(),
        )
        .expect("parent");
    let (child, _) = h
        .runtime
        .create_agent(
            CreateAgentSpec {
                objective: None,
                parent: Some(parent.id),
                config: Some(AgentConfig {
                    effort: Some(aether_core::model::Effort::Low),
                    cost_budget: Some(2.0),
                    approval: ApprovalPolicy::none(),
                    ..Default::default()
                }),
                ..spec("work")
            },
            Actor::user(),
        )
        .expect("child");
    // The child alone spends past both limits.
    let mut spent = h.store.agent(child.id).expect("a").expect("exists");
    spent.stats.usage.cost_usd = 12.0;
    h.store
        .commit(Commit::new().mutate(Mutation::UpsertAgent(spent)))
        .expect("seed spend");

    h.wait_until(parent.id, |s| s.is_terminal()).await;
    let ended = h.wait_until(child.id, |s| s.is_terminal()).await;
    assert_eq!(
        ended.kind(),
        AgentStatusKind::Cancelled,
        "the parent's stop cancelled it; a stale row must not overwrite that"
    );
    // Give the scheduler several more ticks to get it wrong.
    tokio::time::sleep(Duration::from_millis(300)).await;
    let terminal: Vec<String> = types(h.store.as_ref(), child.id)
        .into_iter()
        .filter(|t| ["agent.failed", "agent.cancelled", "agent.completed"].contains(&t.as_str()))
        .collect();
    assert_eq!(terminal, vec!["agent.cancelled".to_owned()], "{terminal:?}");
    assert_eq!(
        h.store
            .agent(child.id)
            .expect("a")
            .expect("exists")
            .status
            .kind(),
        AgentStatusKind::Cancelled
    );
}
