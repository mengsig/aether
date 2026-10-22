//! crash recovery scenarios and fixtures.
use super::*;

/// Mid-steer, for real: a person speaks to an agent whose turn is already in
/// flight, and the daemon dies before either finishes. §17 line 7: the
/// message is not lost - the next daemon picks the agent back up and answers
/// it, and the person's words are in the transcript.
#[tokio::test]
async fn a_kill_mid_steer_loses_neither_the_turn_nor_the_message() {
    let h = harness(vec![ScriptedClient::text("never arrives")]).await;
    h.client.set_delay(Duration::from_secs(3600));
    let mut s = idle_spec();
    s.objective = Some("watch the build".into());
    let (agent, _) = h.runtime.create_agent(s, Actor::user()).expect("create");
    wait_for_event(h.store.as_ref(), agent.id, "snapshot.created").await;
    assert_eq!(
        h.store.agent(agent.id).expect("a").expect("e").status,
        AgentStatus::Running,
        "the turn is in flight when the person speaks"
    );
    h.runtime
        .send_message(
            agent.id,
            vec![ContentBlock::text("ship the release")],
            Actor::user(),
        )
        .expect("a person steers mid-turn");
    let pending = h.store.agent(agent.id).expect("a").expect("e");
    assert!(
        h.store.inbox_last_seq(agent.id).expect("seq") > pending.inbox_cursor,
        "committed against a turn that never read it"
    );

    let h2 = second_daemon_over(&h, vec![ScriptedClient::text("On it.")]).await;
    let deadline = tokio::time::Instant::now() + Duration::from_secs(15);
    let transcript = loop {
        let t = h2.store.transcript(agent.id, 0).expect("transcript");
        if t.iter().any(|e| e.message.text() == "On it.") {
            break t;
        }
        assert!(
            tokio::time::Instant::now() < deadline,
            "the message a person sent before the restart was never answered: {:?}",
            types(h2.store.as_ref(), agent.id)
        );
        tokio::time::sleep(Duration::from_millis(20)).await;
    };
    assert!(
        transcript
            .iter()
            .any(|e| e.message.text().contains("ship the release")),
        "the person's words survived the restart"
    );
    let after = h2.store.agent(agent.id).expect("a").expect("e");
    assert_eq!(
        after.inbox_cursor,
        h2.store.inbox_last_seq(agent.id).expect("seq"),
        "nothing is left unread"
    );
}

/// Mid-approval: the daemon dies with a person's answer outstanding, and then
/// again once they have given it. The question survives the first restart
/// without being asked twice, and the answer survives the second.
#[tokio::test]
async fn a_drill_kills_the_daemon_mid_approval_and_loses_neither_question_nor_answer() {
    let h = harness(vec![ScriptedClient::tool_call(
        "write_file",
        json!({"path": "note.txt", "content": "hello"}),
    )])
    .await;
    let mut s = spec("write a note");
    s.config = Some(AgentConfig {
        approval: ApprovalPolicy {
            require_for: [SideEffectClass::IdempotentMutation].into(),
        },
        ..Default::default()
    });
    let (agent, _) = h.runtime.create_agent(s, Actor::user()).expect("create");
    h.wait_until(agent.id, |s| s.kind() == AgentStatusKind::Waiting)
        .await;
    assert_eq!(h.store.pending_approvals().expect("pending").len(), 1);

    // Killed with the question outstanding: it is still outstanding, and the
    // person is not asked again.
    let h2 = restart_over(
        &h,
        vec![ScriptedClient::tool_call(
            "complete_task",
            json!({"summary": "written after the restart"}),
        )],
    )
    .await;
    let pending = h2.store.pending_approvals().expect("pending");
    assert_eq!(pending.len(), 1, "the question a person owes an answer to");
    assert_eq!(pending[0].tool, "write_file");
    assert!(
        !h.workspace_root.join("note.txt").exists(),
        "nothing ran while the answer was outstanding"
    );

    h2.runtime
        .resolve_approval(
            pending[0].id,
            ApprovalDecision::Approved,
            Actor::user(),
            None,
            None,
        )
        .expect("approve");
    let status = h2.wait_until(agent.id, |s| s.is_terminal()).await;
    assert_eq!(
        status.kind(),
        AgentStatusKind::Completed,
        "{:?}",
        types(h2.store.as_ref(), agent.id)
    );
    assert_eq!(
        std::fs::read_to_string(h.workspace_root.join("note.txt")).expect("file"),
        "hello",
        "the call the person allowed ran, once"
    );

    // Killed again with the answer given: it is not asked for a second time.
    let h3 = restart_over(&h2, vec![]).await;
    assert!(
        h3.store.pending_approvals().expect("pending").is_empty(),
        "a resolved approval must not come back pending"
    );
    let t = types(h3.store.as_ref(), agent.id);
    assert_eq!(
        t.iter().filter(|k| *k == "approval.requested").count(),
        1,
        "the person was asked exactly once across two restarts: {t:?}"
    );
    assert_eq!(t.iter().filter(|k| *k == "approval.resolved").count(), 1);
}

/// The stop is a fence: a daemon that has recorded `daemon.stopped` starts no
/// turn. Its scheduler can be mid-tick when the stop lands, and a turn begun
/// then takes a lease and calls a provider nobody is left to wait for - the
/// next daemon inherits an interrupted turn that should never have started.
/// The drill above is where it showed: the second daemon over the store had
/// the answer, and the first one - stopped, its script spent - took the turn.
#[tokio::test]
async fn a_stopped_daemon_starts_no_turn() {
    let h = harness(vec![ScriptedClient::text("never asked")]).await;
    let (agent, _) = h
        .runtime
        .create_agent(idle_spec(), Actor::user())
        .expect("create");
    h.wait_until(agent.id, |s| s.kind() == AgentStatusKind::Waiting)
        .await;

    h.runtime.shutdown().await;
    h.runtime
        .send_message(
            agent.id,
            vec![ContentBlock::text("anyone there?")],
            Actor::user(),
        )
        .expect("a person speaks after the daemon stopped");
    // By hand, because the loop this would have woken is gone: the pass still
    // runs, and only the dispatch at its end must decline.
    aether_runtime::scheduler::tick(&h.runtime).expect("tick");

    assert!(
        h.runtime
            .provider_health()
            .expect("provider health")
            .iter()
            .all(|p| p.running == 0),
        "a stopped daemon dispatched a turn"
    );
}

/// Mid-tool, the read-only case: an interrupted call that changed nothing is
/// retryable, the agent is schedulable again, and the outcome another call had
/// already committed is still in the log.
#[tokio::test]
async fn a_drill_kills_the_daemon_mid_tool_and_keeps_every_committed_outcome() {
    let id = AgentId::new();
    let settled = aether_core::id::ToolCallId::new();
    let h = harness_with(vec![], |store| {
        seed_crashed_turn(store, id, Some("read_file"), Vec::new(), true);
        store
            .commit(
                Commit::new().event(
                    EventDraft::new(
                        Actor::System,
                        EventKind::ToolSucceeded {
                            call: settled,
                            outcome: aether_core::tool::ToolOutcome::ok("indexed 412 files"),
                        },
                    )
                    .agent(id),
                ),
            )
            .expect("committed outcome");
    })
    .await;

    assert_eq!(
        recovery_class(h.store.as_ref(), id),
        Some(aether_core::event::RecoveryClass::Retry)
    );
    let agent = h.store.agent(id).expect("a").expect("exists");
    assert!(
        matches!(
            agent.status.kind(),
            AgentStatusKind::Ready | AgentStatusKind::Waiting
        ),
        "a read-only interruption is safe to retry: {:?}",
        agent.status
    );
    let t = types(h.store.as_ref(), id);
    assert!(t.contains(&"tool.cancelled".to_owned()), "{t:?}");
    assert!(
        t.contains(&"tool.succeeded".to_owned()),
        "the outcome committed before the crash is still there: {t:?}"
    );
    assert!(h.store.lease(id).expect("lease").is_none());
}

/// Mid-tool, the mutating case, and the gap ARCHITECTURE §15 named: the
/// process that took these keys may have outlived the daemon, so the agent
/// parked for inspection keeps them. Nobody else mutates beside an orphan, and
/// only a person letting the inspection go hands them back.
#[tokio::test]
async fn an_inspected_agent_holds_its_keys_until_a_person_clears_it() {
    let id = AgentId::new();
    let key = aether_core::tool::ResourceKey::repo("/repo/aether");
    let h = harness_with(vec![], |store| {
        assert!(
            store
                .meta("runtime.daemon_boot_id")
                .expect("legacy metadata")
                .is_none()
        );
        seed_crashed_turn(store, id, Some("bash"), vec![key.clone()], true);
    })
    .await;

    assert_eq!(
        recovery_class(h.store.as_ref(), id),
        Some(aether_core::event::RecoveryClass::Inspect)
    );
    let agent = h.store.agent(id).expect("a").expect("exists");
    assert!(
        matches!(
            agent.status,
            AgentStatus::Blocked {
                reason: BlockReason::Inspection { .. }
            }
        ),
        "{:?}",
        agent.status
    );
    let held = h.runtime.resource_holdings();
    assert!(
        held.iter().any(|h| h.agent == id && h.keys.contains(&key)),
        "the interrupted call's keys are still fenced off: {held:?}"
    );
    // A conflicting key, not just the same one: the whole repository is held.
    assert!(
        held.iter().any(|h| h.keys.iter().any(|k| k.conflicts_with(
            &aether_core::tool::ResourceKey::file("/repo/aether/src/main.rs")
        ))),
        "a file inside the held repository is covered too: {held:?}"
    );
    // And it says so in the status a person is pointed at: a fenced-off key
    // read like a live lease held by a running turn.
    let served = h.runtime.status().expect("status").resource_leases;
    assert_eq!(served.len(), 1, "{served:?}");
    assert_eq!(served[0].agent, id);
    assert!(served[0].quarantined, "{served:?}");

    h.runtime
        .resume_agent(id, false, Actor::user())
        .expect("a person clears the inspection");
    assert!(
        h.runtime.resource_holdings().is_empty(),
        "clearing the inspection hands the keys back"
    );
    let t = types(h.store.as_ref(), id);
    assert!(t.contains(&"lease.resource_acquired".to_owned()), "{t:?}");
    assert!(t.contains(&"lease.resource_released".to_owned()), "{t:?}");
}

/// The other way out of an inspection: the agent is cancelled instead of
/// cleared. It has ended, so it fences nothing off any more.
#[tokio::test]
async fn cancelling_an_inspected_agent_frees_the_keys_it_held() {
    let id = AgentId::new();
    let key = aether_core::tool::ResourceKey::repo("/repo/aether");
    let h = harness_with(vec![], |store| {
        seed_crashed_turn(store, id, Some("bash"), vec![key.clone()], true);
    })
    .await;
    assert!(!h.runtime.resource_holdings().is_empty());

    h.runtime
        .cancel_agent(id, Some("orphan cleaned up by hand".into()), Actor::user())
        .expect("cancel");
    assert!(
        h.runtime.resource_holdings().is_empty(),
        "an agent that has ended holds nothing"
    );
}

/// Mid-turn, for real: the model call is in flight and nothing has come back
/// when the daemon dies. The turn is retryable, the unanswered call is
/// recorded rather than left open, and the agent is schedulable again.
#[tokio::test]
async fn a_kill_mid_turn_records_the_unanswered_call_and_stays_retryable() {
    let h = harness(vec![ScriptedClient::text("never arrives")]).await;
    // The reply stalls far past the test: the turn is genuinely in flight.
    h.client.set_delay(Duration::from_secs(3600));
    let (agent, _) = h
        .runtime
        .create_agent(spec("index the repository"), Actor::user())
        .expect("create");
    wait_for_event(h.store.as_ref(), agent.id, "snapshot.created").await;
    assert_eq!(
        h.store.agent(agent.id).expect("a").expect("e").status,
        AgentStatus::Running
    );
    assert!(h.store.lease(agent.id).expect("lease").is_some());

    let h2 = second_daemon_over(&h, vec![]).await;
    assert_eq!(
        recovery_class(h2.store.as_ref(), agent.id),
        Some(aether_core::event::RecoveryClass::Retry)
    );
    let failed = h2
        .store
        .events(0, 10_000, &EventFilter::for_agent(agent.id))
        .expect("events")
        .into_iter()
        .find_map(|e| match e.kind {
            EventKind::InvocationFailed {
                retryable, error, ..
            } => Some((retryable, error)),
            _ => None,
        })
        .expect("the interrupted invocation is recorded, not left open");
    assert!(failed.0, "an unanswered model call is retryable");
    assert!(failed.1.contains("restarted"), "{}", failed.1);
    assert!(h2.store.lease(agent.id).expect("lease").is_none());
    assert!(
        h2.runtime.resource_holdings().is_empty(),
        "a turn that held no keys quarantines nothing"
    );
    let status = h2.store.agent(agent.id).expect("a").expect("e").status;
    assert!(
        matches!(
            status.kind(),
            AgentStatusKind::Ready | AgentStatusKind::Running | AgentStatusKind::Waiting
        ),
        "{status:?}"
    );
}

/// The engine case of a mid-turn kill: Claude Code and Codex run the turn in
/// their own process and commit nothing between the start and the reply, so
/// the log alone reads as idle. The agent's Running status is the evidence
/// that a turn was lost, and recovery must say so instead of reporting that
/// nothing was in flight.
#[tokio::test]
async fn a_drill_kills_the_daemon_mid_engine_turn_and_records_the_loss() {
    let id = AgentId::new();
    let h = harness_with(vec![], |store| {
        seed_crashed_turn(store, id, None, Vec::new(), false);
    })
    .await;

    let classified = h
        .store
        .events(0, 10_000, &EventFilter::for_agent(id))
        .expect("events")
        .into_iter()
        .find_map(|e| match e.kind {
            EventKind::RecoveryClassified { class, detail } => Some((class, detail)),
            _ => None,
        })
        .expect("recovery.classified");
    assert_eq!(
        classified.0,
        aether_core::event::RecoveryClass::Retry,
        "a turn was in flight: {}",
        classified.1
    );
    assert_eq!(
        classified.1,
        "the turn in flight was lost when the daemon stopped; nothing it produced was committed",
        "the loss is named, not papered over, and reads as one sentence"
    );
    let agent = h.store.agent(id).expect("a").expect("exists");
    assert!(
        matches!(
            agent.status.kind(),
            AgentStatusKind::Ready | AgentStatusKind::Waiting
        ),
        "{:?}",
        agent.status
    );
    // Ready alone is not a retry: the wake is what makes the agent run again.
    let woken = h.store.inbox(id, 0).expect("inbox").into_iter().any(|i| {
        matches!(&i.kind, aether_core::inbox::InboxKind::Wake { reason }
            if reason.contains("lost"))
    });
    assert!(woken, "the lost turn is retried, not left parked");
}

/// Startup compares the persisted boot before advancing it; recovery of an
/// earlier inspection still works although its calls were already cancelled.
#[cfg(target_os = "linux")]
#[tokio::test]
async fn a_host_reboot_retries_interrupted_and_previously_inspected_mutations() {
    let interrupted = AgentId::new();
    let inspected = AgentId::new();
    let key = aether_core::tool::ResourceKey::repo("/repo/recovery");
    let h = harness_with(vec![], |store| {
        for id in [interrupted, inspected] {
            seed_crashed_turn(store, id, Some("Bash"), vec![key.clone()], true);
            let mut agent = store.agent(id).expect("agent").expect("exists");
            // Keep this drill in recovery, without dispatching a new turn.
            agent.paused = true;
            let mut commit = Commit::new();
            if id == inspected {
                let call = store.events(0, 100, &EventFilter::own_events(id))
                    .expect("events").into_iter().find_map(|event| match event.kind {
                        EventKind::ToolRequested { call, .. } => Some(call.id),
                        _ => None,
                    }).expect("call");
                agent.status = AgentStatus::Blocked { reason: BlockReason::Inspection {
                    detail: format!("mutating tool calls were in flight when the daemon stopped: Bash ({call})"),
                }};
                commit.push_event(EventDraft::new(Actor::System, EventKind::ToolCancelled { call }).agent(id));
                commit.push_mutation(Mutation::ClearLease(id));
            }
            commit.push_mutation(Mutation::UpsertAgent(agent));
            store.commit(commit).expect("seed pause and prior inspection");
        }
        store.commit(Commit::new().mutate(Mutation::SetMeta {
            key: "runtime.daemon_boot_id".into(),
            value: "a-previous-host-boot".into(),
        })).expect("previous boot");
    }).await;

    for id in [interrupted, inspected] {
        assert_eq!(
            recovery_class(h.store.as_ref(), id),
            Some(aether_core::event::RecoveryClass::Retry)
        );
        let agent = h.store.agent(id).expect("agent").expect("exists");
        assert_eq!(agent.status, AgentStatus::Ready);
        assert!(agent.paused);
        assert!(h.store.lease(id).expect("lease").is_none());
        let inbox = h.store.inbox(id, 0).expect("inbox");
        assert!(
            inbox.iter().any(|item| matches!(&item.kind,
            aether_core::inbox::InboxKind::Wake { reason }
                if reason.contains("host restarted") && reason.contains("Bash")
                    && reason.contains("call_") && reason.contains("re-check state"))),
            "{inbox:?}"
        );
        assert!(types(h.store.as_ref(), id).contains(&"lease.resource_released".into()));
    }
    assert!(h.runtime.resource_holdings().is_empty());
    let current =
        std::fs::read_to_string("/proc/sys/kernel/random/boot_id").expect("Linux boot id");
    assert_eq!(
        h.store
            .meta("runtime.daemon_boot_id")
            .expect("boot id")
            .as_deref(),
        Some(current.trim())
    );

    let h2 = restart_over(&h, vec![]).await;
    assert!(
        h2.runtime.resource_holdings().is_empty(),
        "released keys stay released"
    );
    for id in [interrupted, inspected] {
        assert_eq!(
            h2.store.inbox(id, 0).expect("inbox").len(),
            h.store.inbox(id, 0).expect("inbox").len()
        );
        assert_eq!(
            types(h2.store.as_ref(), id)
                .iter()
                .filter(|kind| *kind == "recovery.classified")
                .count(),
            1
        );
    }
}

fn reply_obligation_events(store: &dyn Store, id: AgentId) -> Vec<String> {
    store
        .events(0, 10_000, &EventFilter::own_events(id))
        .expect("events")
        .into_iter()
        .filter_map(|event| match event.kind {
            EventKind::Custom { name, .. } if name.starts_with("reply.") => Some(name),
            _ => None,
        })
        .collect()
}

/// A person's question is consumed and the daemon dies before the answer: the
/// restarted turn still owes the reply, refuses to wait, and clears the debt
/// once it answers in text.
#[tokio::test]
async fn a_kill_after_a_user_message_is_consumed_keeps_the_reply_owed() {
    let h = harness(vec![ScriptedClient::text("never arrives")]).await;
    h.client.set_delay(Duration::from_secs(3600));
    let mut s = idle_spec();
    s.objective = Some("watch the build".into());
    let (agent, _) = h.runtime.create_agent(s, Actor::user()).expect("create");
    wait_for_event(h.store.as_ref(), agent.id, "snapshot.created").await;
    h.runtime
        .send_message(agent.id, vec![ContentBlock::text("status?")], Actor::user())
        .expect("a person asks");

    let h2 = second_daemon_over(&h, vec![ScriptedClient::text("never arrives")]).await;
    h2.client.set_delay(Duration::from_secs(3600));
    wait_until_events(h2.store.as_ref(), agent.id, |_| {
        reply_obligation_events(h2.store.as_ref(), agent.id) == ["reply.owed"]
    })
    .await;

    let h3 = second_daemon_over(
        &h2,
        vec![
            ScriptedClient::tool_call("wait_for", json!({"timeout_seconds": 3600})),
            ScriptedClient::text("All green."),
        ],
    )
    .await;
    wait_until_events(h3.store.as_ref(), agent.id, |_| {
        reply_obligation_events(h3.store.as_ref(), agent.id) == ["reply.owed", "reply.given"]
    })
    .await;
    let log = format!(
        "{:?}",
        h3.store
            .events(0, 10_000, &EventFilter::for_agent(agent.id))
            .expect("events")
    );
    assert!(
        log.contains("Reply in visible assistant text"),
        "the restarted turn must refuse to wait while the reply is owed"
    );
}
