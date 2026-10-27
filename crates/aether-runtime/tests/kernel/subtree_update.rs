//! `UpdateAgent` with `subtree`: a raised deadline or cost budget reaches the
//! live descendants, never lowers one, and nothing else propagates (#323).
use super::*;
use aether_core::agent::DeadlineUpdate;

/// An idle, long-lived agent on a priced model, so a cost budget is allowed.
fn idle(
    parent: Option<AgentId>,
    deadline: Option<Timestamp>,
    budget: Option<f64>,
) -> CreateAgentSpec {
    let mut s = spec_on("unused", "anthropic", "claude-sonnet-5-5");
    s.objective = None;
    s.parent = parent;
    let config = s.config.as_mut().expect("config");
    config.long_lived = true;
    config.deadline = deadline;
    config.cost_budget = budget;
    config.instructions = Some("as created".into());
    s
}

fn create(h: &Harness, spec: CreateAgentSpec) -> AgentId {
    h.runtime
        .create_agent(spec, Actor::user())
        .expect("create")
        .0
        .id
}

fn stored(h: &Harness, id: AgentId) -> Agent {
    h.store.agent(id).expect("agent").expect("exists")
}

fn config_updates(h: &Harness, id: AgentId) -> usize {
    types(h.store.as_ref(), id)
        .iter()
        .filter(|t| *t == "agent.config_updated")
        .count()
}

fn update_subtree(h: &Harness, id: AgentId, update: AgentUpdate) -> Agent {
    h.runtime
        .update_agent_keyed(id, update, true, Actor::user(), None)
        .expect("subtree update")
}

/// The acceptance case of #323: extending a parent with `subtree` leaves it
/// and both children on the new deadline, and the scheduler stops none of
/// them at the old one. The same move without `subtree` still loses the child.
#[tokio::test]
async fn a_subtree_extension_keeps_every_child_past_the_old_deadline() {
    let h = harness(vec![]).await;
    let old = now() + chrono::Duration::seconds(2);
    let new = now() + chrono::Duration::hours(1);
    let parent = create(&h, idle(None, Some(old), None));
    let children = [
        create(&h, idle(Some(parent), Some(old), None)),
        create(&h, idle(Some(parent), Some(old), None)),
    ];
    let plain = create(&h, idle(None, Some(old), None));
    let plain_child = create(&h, idle(Some(plain), Some(old), None));
    let extend = || AgentUpdate {
        deadline: Some(DeadlineUpdate::Set(new)),
        ..Default::default()
    };

    let answered = update_subtree(&h, parent, extend());
    assert_eq!(answered.id, parent, "the reply is the named agent");
    h.runtime
        .update_agent(plain, extend(), Actor::user())
        .expect("plain update");
    for id in std::iter::once(parent).chain(children) {
        assert_eq!(stored(&h, id).config.deadline, Some(new), "{id}");
        assert_eq!(config_updates(&h, id), 1, "one event per changed agent");
    }
    assert_eq!(stored(&h, plain_child).config.deadline, Some(old));

    let past_old = (old - now()).to_std().unwrap_or_default() + Duration::from_millis(200);
    tokio::time::sleep(past_old).await;
    aether_runtime::scheduler::tick(&h.runtime).expect("tick");
    h.wait_until(plain_child, |s| s.is_terminal()).await;
    for id in std::iter::once(parent).chain(children) {
        let agent = stored(&h, id);
        assert!(!agent.status.is_terminal(), "{id}: {:?}", agent.status);
        assert!(
            !types(h.store.as_ref(), id).contains(&"deadline.passed".to_owned()),
            "{id} was stopped at the old deadline"
        );
    }
    h.runtime.shutdown().await;
}

/// Raise only: a later deadline, a larger budget or no limit at all is kept,
/// a descendant left unchanged gets no event, and a grandchild is reached.
#[tokio::test]
async fn a_subtree_update_never_lowers_a_descendants_limit() {
    let h = harness(vec![]).await;
    let soon = now() + chrono::Duration::hours(1);
    let target = now() + chrono::Duration::hours(2);
    let later = now() + chrono::Duration::hours(3);
    let parent = create(&h, idle(None, Some(soon), Some(1.0)));
    let generous = create(&h, idle(Some(parent), Some(later), Some(10.0)));
    let unlimited = create(&h, idle(Some(parent), None, None));
    let mixed = create(&h, idle(Some(parent), Some(later), Some(0.5)));
    let grandchild = create(&h, idle(Some(mixed), Some(soon), Some(0.5)));

    update_subtree(
        &h,
        parent,
        AgentUpdate {
            deadline: Some(DeadlineUpdate::Set(target)),
            cost_budget: Some(2.0),
            ..Default::default()
        },
    );

    let limits = |id| {
        let config = stored(&h, id).config;
        (config.deadline, config.cost_budget)
    };
    assert_eq!(limits(parent), (Some(target), Some(2.0)));
    assert_eq!(limits(generous), (Some(later), Some(10.0)));
    assert_eq!(limits(unlimited), (None, None));
    assert_eq!(limits(mixed), (Some(later), Some(2.0)));
    assert_eq!(limits(grandchild), (Some(target), Some(2.0)));
    assert_eq!(config_updates(&h, generous), 0, "unchanged: no event");
    assert_eq!(config_updates(&h, unlimited), 0, "unchanged: no event");
    assert_eq!(config_updates(&h, mixed), 1);
    assert_eq!(config_updates(&h, grandchild), 1);

    // Clearing removes the limit: the highest raise there is.
    update_subtree(
        &h,
        parent,
        AgentUpdate {
            deadline: Some(DeadlineUpdate::Clear),
            ..Default::default()
        },
    );
    assert_eq!(limits(generous), (None, Some(10.0)));
    assert_eq!(limits(grandchild), (None, Some(2.0)));
    h.runtime.shutdown().await;
}

/// A finished descendant is a record: its limits and events stay as they
/// were, while its live sibling is raised.
#[tokio::test]
async fn a_subtree_update_leaves_terminal_descendants_untouched() {
    let h = harness(vec![]).await;
    let old = now() + chrono::Duration::hours(1);
    let new = now() + chrono::Duration::hours(2);
    let parent = create(&h, idle(None, Some(old), Some(1.0)));
    let live = create(&h, idle(Some(parent), Some(old), Some(1.0)));
    let ended = create(&h, idle(Some(parent), Some(old), Some(1.0)));
    h.runtime
        .cancel_agent(ended, None, Actor::user())
        .expect("cancel");
    h.wait_until(ended, |s| s.is_terminal()).await;
    let before = stored(&h, ended);
    let events_before = types(h.store.as_ref(), ended);

    update_subtree(
        &h,
        parent,
        AgentUpdate {
            deadline: Some(DeadlineUpdate::Set(new)),
            cost_budget: Some(3.0),
            ..Default::default()
        },
    );

    let after = stored(&h, ended);
    assert_eq!(after.config, before.config);
    assert_eq!(after.updated_at, before.updated_at);
    assert_eq!(types(h.store.as_ref(), ended), events_before);
    assert_eq!(stored(&h, live).config.deadline, Some(new));
    assert_eq!(stored(&h, live).config.cost_budget, Some(3.0));
    h.runtime.shutdown().await;
}

/// Only the two limits travel: a name, instructions, effort, turn cap or
/// token budget in the same patch changes the named agent alone, and a patch
/// with neither limit writes no descendant at all.
#[tokio::test]
async fn a_subtree_update_propagates_nothing_but_deadline_and_budget() {
    let h = harness(vec![]).await;
    let old = now() + chrono::Duration::hours(1);
    let new = now() + chrono::Duration::hours(2);
    let parent = create(&h, idle(None, Some(old), None));
    let child = create(&h, idle(Some(parent), Some(old), None));
    let child_before = stored(&h, child);

    let knobs = AgentUpdate {
        name: Some("renamed".into()),
        instructions: Some("changed".into()),
        effort: Some(aether_core::model::Effort::High),
        max_turns: Some(7),
        token_budget: Some(1_000),
        ..Default::default()
    };
    update_subtree(&h, parent, knobs.clone());
    assert_eq!(stored(&h, child).config, child_before.config);
    assert_eq!(config_updates(&h, child), 0, "no limit patched: untouched");

    update_subtree(
        &h,
        parent,
        AgentUpdate {
            deadline: Some(DeadlineUpdate::Set(new)),
            ..knobs
        },
    );
    let parent_after = stored(&h, parent);
    assert_eq!(parent_after.name, "renamed");
    assert_eq!(parent_after.config.instructions.as_deref(), Some("changed"));
    let child_after = stored(&h, child);
    assert_eq!(child_after.name, child_before.name);
    assert_eq!(
        child_after.config,
        AgentConfig {
            deadline: Some(new),
            ..child_before.config
        },
        "the deadline alone moved"
    );
    h.runtime.shutdown().await;
}
