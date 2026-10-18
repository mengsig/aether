//! leases scenarios and fixtures.
use super::*;

/// Limit 5: two agents mutating the same repository never run their tool
/// calls at once; the later one waits as Blocked with the holder named.
#[tokio::test]
async fn a_resource_lease_blocks_the_second_agent_and_names_the_holder() {
    let sleeping = || {
        ScriptedClient::tool_call(
            "bash",
            json!({"command": "for ((i=0;i<1000;i++)); do if test -f release; then exit 0; fi; sleep 0.01; done; exit 1"}),
        )
    };
    let done = || ScriptedClient::tool_call("complete_task", json!({"summary": "done"}));
    let h = harness_configured(
        vec![sleeping(), sleeping(), done(), done()],
        |_| {},
        |c| c.max_concurrent_turns = 2,
    )
    .await;
    let (a, _) = h
        .runtime
        .create_agent(spec("touch the repo"), Actor::user())
        .expect("a");
    let (b, _) = h
        .runtime
        .create_agent(spec("touch it too"), Actor::user())
        .expect("b");
    wait_for_with_budget(
        h.wait_ceiling,
        WAIT_UNTIL_STALL_WINDOW,
        || {
            let blocked = h
                .store
                .agents(&AgentFilter {
                    statuses: vec![AgentStatusKind::Blocked],
                    ..Default::default()
                })
                .expect("blocked agents");
            (!blocked.is_empty()).then_some(())
        },
        || {
            (
                activity_signature(&h.store.agent(a.id).expect("a").expect("exists")),
                activity_signature(&h.store.agent(b.id).expect("b").expect("exists")),
            )
        },
        || "no resource waiter appeared".to_owned(),
    )
    .await;
    let status = h.runtime.status().expect("status while parked");
    assert_eq!(
        status.agents_running, 1,
        "the resource waiter must not count as running"
    );
    assert_eq!(status.agents_blocked, 1);
    std::fs::write(h.workspace_root.join("release"), "release").expect("release holder");
    h.wait_until(a.id, |s| s.is_terminal()).await;
    h.wait_until(b.id, |s| s.is_terminal()).await;

    let waits: Vec<(AgentId, AgentId)> = h
        .store
        .events(0, 10_000, &EventFilter::all())
        .expect("events")
        .into_iter()
        .filter_map(|e| match e.kind {
            EventKind::ResourceWaiting { holder, .. } => Some((e.agent_id?, holder)),
            _ => None,
        })
        .collect();
    assert_eq!(waits.len(), 1, "expected exactly one waiter: {waits:?}");
    let (waiter, holder) = waits[0];
    assert_ne!(waiter, holder);
    assert!([a.id, b.id].contains(&waiter) && [a.id, b.id].contains(&holder));
    let waiter_events = types(h.store.as_ref(), waiter);
    assert!(
        waiter_events.contains(&"agent.blocked".to_owned()),
        "{waiter_events:?}"
    );
    assert!(
        waiter_events.contains(&"agent.resumed".to_owned()),
        "{waiter_events:?}"
    );
    assert!(
        waiter_events.contains(&"lease.resource_released".to_owned()),
        "{waiter_events:?}"
    );

    // The shell calls themselves never overlapped.
    let (sa, ea) = shell_window(h.store.as_ref(), a.id);
    let (sb, eb) = shell_window(h.store.as_ref(), b.id);
    assert!(
        ea < sb || eb < sa,
        "mutating tool calls overlapped: {a:?} {sa}..{ea} vs {sb}..{eb}",
        a = a.id
    );
    assert!(
        h.runtime.resource_holdings().is_empty(),
        "keys outlived their turns: {:?}",
        h.runtime.resource_holdings()
    );
    assert_eq!(h.client.remaining(), 0);
}

/// A turn that outlives its heartbeat interval commits lease renewals; they
/// must carry the run, or retention treats the run as referenced from outside.
#[tokio::test]
async fn a_run_whose_turn_outlived_a_heartbeat_can_expire() {
    let h = harness_full(
        vec![
            ScriptedClient::tool_call("bash", json!({"command": "sleep 0.6"})),
            ScriptedClient::tool_call("complete_task", json!({"summary": "done"})),
        ],
        |_| {},
        |c| c.lease_ttl = Duration::from_millis(100),
        None,
        None,
        None,
        |_| {},
    )
    .await;
    let (agent, _) = h
        .runtime
        .create_agent(spec("run past a heartbeat"), Actor::user())
        .expect("agent");
    let status = h.wait_until(agent.id, |s| s.is_terminal()).await;
    assert_eq!(status.kind(), AgentStatusKind::Completed, "{status:?}");

    let events = h
        .store
        .events(0, 10_000, &EventFilter::for_agent(agent.id))
        .expect("events");
    let renewals = events
        .iter()
        .filter(|e| matches!(e.kind, EventKind::LeaseRenewed { .. }))
        .count();
    assert!(renewals >= 1, "the turn never renewed its lease");
    assert!(
        events.iter().all(|e| e.run == Some(agent.run)),
        "an agent event carries no run"
    );

    let expiry = h
        .runtime
        .expire_run_history(agent.run, now() + chrono::Duration::days(1))
        .expect("expiry");
    assert!(
        matches!(expiry, aether_store::RunExpiry::Expired { .. }),
        "{expiry:?}"
    );
}

#[cfg(debug_assertions)]
#[tokio::test]
#[should_panic(expected = "without a run")]
async fn committing_an_agent_event_without_a_run_is_refused_in_debug_builds() {
    let h = harness_configured(vec![], |_| {}, |_| {}).await;
    let draft = EventDraft::new(Actor::Scheduler, EventKind::AgentReady).agent(AgentId::new());
    let _ = h.runtime.apply(Commit::new().event(draft));
}
