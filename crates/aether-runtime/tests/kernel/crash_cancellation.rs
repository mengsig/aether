//! crash cancellation scenarios and fixtures.
use super::*;

/// Mid-tool, for real: a turn runs two calls, one settles and one is still in
/// flight when the daemon dies. §17 line 7 says a committed tool outcome is
/// never lost - so recovery must cancel the call that was open and leave the
/// one that finished exactly as the turn recorded it.
#[tokio::test]
async fn a_kill_mid_tool_cancels_the_open_call_and_keeps_the_settled_one() {
    // Two batches in one turn: the read settles and its outcome is committed
    // before the shell call - which never comes back - is even requested.
    let h = harness(vec![
        ScriptedClient::tool_call("read_file", json!({"path": "note.txt"})),
        ScriptedClient::tool_call("bash", json!({"command": "sleep 30"})),
    ])
    .await;
    std::fs::write(h.workspace_root.join("note.txt"), "already indexed").expect("seed a file");
    let (agent, _) = h
        .runtime
        .create_agent(interactive(spec("index the repository")), Actor::user())
        .expect("create");

    // The turn is now parked on a call that will not come back: the shell is
    // sleeping, and the daemon dies around it.
    wait_for_event(h.store.as_ref(), agent.id, "tool.succeeded").await;
    let before = wait_until_events(h.store.as_ref(), agent.id, |events| {
        events
            .iter()
            .filter(|e| e.type_name() == "tool.started")
            .count()
            == 2
    })
    .await;
    let call_id = |name: &str| {
        before
            .iter()
            .find_map(|e| match &e.kind {
                EventKind::ToolRequested { call, .. } if call.name == name => Some(call.id),
                _ => None,
            })
            .unwrap_or_else(|| panic!("{name} was requested"))
    };
    let (settled, open) = (call_id("read_file"), call_id("bash"));
    assert!(
        before
            .iter()
            .any(|e| matches!(&e.kind, EventKind::ToolSucceeded { call, .. } if *call == settled)),
        "the read finished and its outcome is committed"
    );
    assert!(
        before
            .iter()
            .any(|e| matches!(&e.kind, EventKind::ToolStarted { call } if *call == open)),
        "the shell call started"
    );
    assert!(
        !before.iter().any(|e| matches!(
            &e.kind,
            EventKind::ToolSucceeded { call, .. } | EventKind::ToolFailed { call, .. }
            if *call == open
        )),
        "and has not settled"
    );
    assert_eq!(
        h.store.agent(agent.id).expect("a").expect("e").status,
        AgentStatus::Running
    );

    let h2 = second_daemon_over(&h, vec![]).await;
    let after: Vec<_> = h2
        .store
        .events(0, 10_000, &EventFilter::for_agent(agent.id))
        .expect("events")
        .into_iter()
        .filter(|e| e.agent_id == Some(agent.id))
        .collect();
    let cancelled: Vec<_> = after
        .iter()
        .filter_map(|e| match &e.kind {
            EventKind::ToolCancelled { call } => Some(*call),
            _ => None,
        })
        .collect();
    assert_eq!(
        cancelled,
        vec![open],
        "exactly the call that was open, and not the one that finished"
    );
    assert!(
        after
            .iter()
            .any(|e| matches!(&e.kind, EventKind::ToolSucceeded { call, .. } if *call == settled)),
        "the committed outcome is untouched"
    );
    // A mutating call was in flight, so a person looks before anyone else
    // touches what it held.
    assert_eq!(
        recovery_class(h2.store.as_ref(), agent.id),
        Some(aether_core::event::RecoveryClass::Inspect)
    );
    assert!(
        h2.runtime
            .resource_holdings()
            .iter()
            .any(|x| x.agent == agent.id),
        "the shell's keys are fenced off"
    );
    assert!(h2.store.lease(agent.id).expect("lease").is_none());
    // The first daemon was left standing on purpose, but only until the
    // drill is over: its `sleep 30` would otherwise outlive the test.
    h.runtime.shutdown().await;
}

/// A pause is durable, so a scheduler pass that decided from an older
/// snapshot must not write its copy of the row back over it. The pass re-reads
/// under the same hold on the write path that commits, so the flag survives.
#[tokio::test]
async fn a_pause_landing_while_the_scheduler_parks_an_agent_is_not_lost() {
    let inner: Arc<dyn Store> = Arc::new(SqliteStore::open_in_memory().expect("store"));
    let store = Arc::new(CommitsMidPass::new(
        inner,
        Hook::SchedulerAsksIfReadyAgentHasWork,
    ));
    let h = harness_full(
        vec![],
        |_| {},
        |_| {},
        None,
        None,
        Some(store.clone()),
        |_| {},
    )
    .await;
    let (agent, _) = h
        .runtime
        .create_agent(idle_spec(), Actor::user())
        .expect("agent");
    let id = agent.id;
    h.wait_until(id, |s| s.kind() == AgentStatusKind::Waiting)
        .await;

    // The operator pauses while the tick is deciding this agent has nothing
    // to do - between its snapshot and the park it is about to write.
    store.arm(Box::new(move |inner| {
        let mut held = inner.agent(id)?.expect("the agent");
        held.paused = true;
        inner.commit(Commit::new().mutate(Mutation::UpsertAgent(held)))?;
        Ok(())
    }));
    h.runtime
        .resume_agent(id, false, Actor::user())
        .expect("back to ready");
    for _ in 0..5 {
        aether_runtime::scheduler::tick(&h.runtime).expect("tick");
    }
    assert!(store.fired(), "the pause landed inside the pass");
    assert!(
        paused(h.store.as_ref(), id),
        "the pause survives the park the pass wrote"
    );
    assert_eq!(
        h.store.agent(id).expect("a").expect("e").status.kind(),
        AgentStatusKind::Waiting,
        "and the park still happened"
    );
}

/// VISION §16: a scope tightened while a child was terminal must not come
/// back when that child is restarted. `widest_live_descendant` only sees live
/// agents, so the parent tightens freely; the restart is the path that would
/// resurrect the old, wider scope under it.
#[tokio::test]
async fn a_restart_is_refused_when_the_parent_was_tightened_after_the_child_ended() {
    use aether_core::agent::NetworkPolicy;

    let h = harness(vec![]).await;
    let mut parent_spec = idle_spec();
    parent_spec.scope = Some(Scope {
        workspace_root: Some(h.workspace_root.clone()),
        network: NetworkPolicy::Allowed,
        ..Default::default()
    });
    let (parent, _) = h
        .runtime
        .create_agent(parent_spec, Actor::user())
        .expect("parent");
    let mut child_spec = idle_spec();
    child_spec.parent = Some(parent.id);
    let (child, _) = h
        .runtime
        .create_agent(child_spec, Actor::user())
        .expect("child");
    assert_eq!(child.scope.network, NetworkPolicy::Allowed);

    h.runtime
        .cancel_agent(child.id, Some("done with it".into()), Actor::user())
        .expect("cancel the child");
    h.wait_until(child.id, |s| s.is_terminal()).await;
    // Accepted precisely because the child is no longer live.
    h.runtime
        .update_agent(
            parent.id,
            aether_runtime::AgentUpdate {
                network: Some(NetworkPolicy::Denied),
                ..Default::default()
            },
            Actor::user(),
        )
        .expect("confine the parent");

    let err = h
        .runtime
        .restart_agent(child.id, Actor::user())
        .expect_err("the restart would widen the subtree again");
    assert!(err.to_string().contains("`network`"), "{err}");
    assert!(
        h.store
            .agent(child.id)
            .expect("child")
            .expect("exists")
            .restarted_as
            .is_none(),
        "nothing was created"
    );

    // Tighten the child's own scope to match and the restart goes through.
    let mut stored = h.store.agent(child.id).expect("child").expect("exists");
    stored.scope.network = NetworkPolicy::Denied;
    h.store
        .commit(Commit::new().mutate(Mutation::UpsertAgent(stored)))
        .expect("narrow the ended child");
    let fresh = h
        .runtime
        .restart_agent(child.id, Actor::user())
        .expect("a scope inside the parent's restarts");
    assert_eq!(fresh.scope.network, NetworkPolicy::Denied);
}

/// VISION §3.8: a pause holds "the agent, and optionally its subtree". Held
/// plainly, a supervisor's in-flight turn may still delegate, and the child it
/// creates is nobody's target: holding it would strand it, since the matching
/// resume lifts only the root.
#[tokio::test]
async fn a_child_created_under_a_plainly_paused_parent_is_not_held() {
    let h = harness(vec![ScriptedClient::text("done")]).await;
    let (root, _) = h
        .runtime
        .create_agent(idle_spec(), Actor::user())
        .expect("root");
    h.wait_until(root.id, |s| s.kind() == AgentStatusKind::Waiting)
        .await;
    h.runtime
        .pause_agent(root.id, false, Actor::user())
        .expect("pause the agent alone");
    assert_eq!(
        h.store
            .agent(root.id)
            .expect("root")
            .expect("exists")
            .paused_via,
        None,
        "a plain pause is not a subtree hold"
    );

    let mut child_spec = idle_spec();
    child_spec.parent = Some(root.id);
    let (child, _) = h
        .runtime
        .create_agent(child_spec, Actor::user())
        .expect("child");
    assert!(
        !paused(h.store.as_ref(), child.id),
        "only a subtree pause reaches a child created under it"
    );
    h.runtime
        .send_message(child.id, vec![ContentBlock::text("go")], Actor::user())
        .expect("message");
    h.wait_until(child.id, |s| s.kind() == AgentStatusKind::Waiting)
        .await;
    assert_eq!(h.client.remaining(), 0, "the child took its turn");

    // And the root's own hold still lifts the way it always did.
    let resumed = h
        .runtime
        .resume_agent(root.id, false, Actor::user())
        .expect("resume");
    assert_eq!(resumed.len(), 1);
    assert!(!paused(h.store.as_ref(), root.id));
}
