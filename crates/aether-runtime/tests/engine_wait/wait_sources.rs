use super::*;

#[tokio::test]
async fn full_native_input_does_not_block_cancellation_or_parking() {
    for cancel in [true, false] {
        let run_gate = Arc::new(tokio::sync::Semaphore::new(0));
        let mut h = harness_full(
            Duration::from_millis(100),
            OnKill::Report(vec![cancelled_with(Usage::default())]),
            Script {
                input_gate: Some(Arc::new(tokio::sync::Semaphore::new(0))),
                run_gate: Some(run_gate.clone()),
                prefill_input: true,
                // Keep the receipt pending so the next turn cannot consume
                // the deferred inbox before this turn's assertions.
                linger_after_stop: Some(Duration::from_secs(30)),
                ..Default::default()
            },
        )
        .await;
        let agent = start_coordinator(&h);
        let turn = turn_of(&mut h, agent).await;
        h.runtime
            .send_message(agent, vec![ContentBlock::text("deferred")], Actor::System)
            .expect("deferred message");
        run_gate.add_permits(1);
        let (reply, error) = raw_call(&h, &turn, "list_agents", json!({})).await;
        assert!(!error, "{reply}");
        let cursor = h.store.agent(agent).unwrap().unwrap().inbox_cursor;
        if cancel {
            h.runtime
                .cancel_agent(agent, Some("operator stop".into()), Actor::user())
                .expect("cancel agent");
        } else {
            let (reply, error) =
                raw_call(&h, &turn, "wait_for", json!({"timeout_seconds": 3600})).await;
            assert!(!error, "{reply}");
        }
        wait_until_stopped(&turn.observed, Duration::from_secs(2)).await;
        assert!(turn.observed.steered.lock().expect("steered").is_empty());
        assert_eq!(h.store.agent(agent).unwrap().unwrap().inbox_cursor, cursor);
        assert!(h.store.inbox_last_seq(agent).unwrap() > cursor);
        let _ = turn.end.send(());
        h.runtime.shutdown().await;
    }
}

#[tokio::test]
async fn full_native_input_keeps_bridge_responsive_and_retries_without_new_inbox() {
    let input_gate = Arc::new(tokio::sync::Semaphore::new(0));
    let run_gate = Arc::new(tokio::sync::Semaphore::new(0));
    let mut h = harness_full(
        Duration::from_secs(30),
        OnKill::Silent,
        Script {
            input_gate: Some(input_gate.clone()),
            run_gate: Some(run_gate.clone()),
            prefill_input: true,
            ..Default::default()
        },
    )
    .await;
    let agent = start_coordinator(&h);
    let turn = turn_of(&mut h, agent).await;
    // Arrives after context assembly but before the drive loop's first steer.
    h.runtime
        .send_message(agent, vec![ContentBlock::text("deferred")], Actor::System)
        .expect("deferred message");
    run_gate.add_permits(1);
    let (reply, error) = raw_call(&h, &turn, "list_agents", json!({})).await;
    assert!(!error, "{reply}");

    input_gate.add_permits(2);
    wait_until(h.store.as_ref(), agent, |row| {
        row.inbox_cursor == h.store.inbox_last_seq(agent).expect("inbox seq")
    })
    .await;
    let steered = turn.observed.steered.lock().expect("steered").clone();
    assert_eq!(steered.len(), 1);
    assert!(steered[0].contains("deferred"));
    let _ = turn.end.send(());
    h.runtime.shutdown().await;
}

#[tokio::test]
async fn bare_wait_rejects_an_assigned_task_without_a_wake_source() {
    for status in [TaskStatus::Assigned, TaskStatus::Active] {
        let mut h = harness(Duration::from_secs(30)).await;
        let agent = start_agent(&h, assigned_worker_spec());
        let turn = turn_of(&mut h, agent).await;
        let row = h.store.agent(agent).unwrap().unwrap();
        let mut task = h.store.task(row.current_task.unwrap()).unwrap().unwrap();
        task.status = status;
        h.store
            .commit(Commit::new().mutate(Mutation::UpsertTask(task)))
            .unwrap();
        assert_eq!(h.store.inbox_last_seq(agent).unwrap(), row.inbox_cursor);

        let (reply, is_error) = call(&h, &turn, "wait_for", json!({})).await;
        assert!(is_error, "{reply}");
        for guidance in [
            "Nothing can wake you",
            "complete_task",
            "fail_task",
            "delegate",
            "message_from",
            "tasks",
            "timeout_seconds",
            "wake_at",
        ] {
            assert!(reply.contains(guidance), "missing {guidance}: {reply}");
        }
        assert!(!turn.observed.stopped());
        assert!(!turn.observed.stdin_closed());
        assert!(!turn.observed.cancelled());
        assert_eq!(
            h.store.agent(agent).unwrap().unwrap().status,
            AgentStatus::Running
        );
        assert!(
            seqs(h.store.as_ref(), |e| e.agent_id == Some(agent)
                && matches!(e.kind, EventKind::AgentWaiting { .. }))
            .is_empty()
        );

        let (reply, is_error) = call(
            &h,
            &turn,
            "complete_task",
            json!({"summary": "finished instead"}),
        )
        .await;
        assert!(
            !is_error,
            "the refused wait must leave task completion available: {reply}"
        );
        turn.end.send(()).expect("native completion report");
        wait_until(h.store.as_ref(), agent, |a| {
            a.status.kind() == AgentStatusKind::Completed
        })
        .await;
        h.runtime.shutdown().await;
    }
}

#[tokio::test]
async fn bare_wait_counts_only_live_children_and_deeper_descendants() {
    for (child_live, grandchild_live) in [(true, false), (false, true), (false, false)] {
        let mut h = harness(Duration::from_secs(30)).await;
        let agent = start_agent(&h, assigned_worker_spec());
        let child = idle_child(&h, agent);
        if grandchild_live {
            idle_child(&h, child);
        }
        if !child_live {
            let mut row = h.store.agent(child).unwrap().unwrap();
            row.status = AgentStatus::Completed { summary: None };
            h.store
                .commit(Commit::new().mutate(Mutation::UpsertAgent(row)))
                .unwrap();
        }
        let turn = turn_of(&mut h, agent).await;
        let (reply, is_error) = call(&h, &turn, "wait_for", json!({})).await;
        let allowed = child_live || grandchild_live;
        assert_eq!(is_error, !allowed, "{reply}");
        assert_eq!(turn.observed.stopped(), allowed);
        if allowed {
            let _ = turn.end.send(());
            let parked = wait_for_park(&h, agent).await;
            assert_eq!(condition_of(&parked.status), &WaitCondition::Inbox);
        }
        h.runtime.shutdown().await;
    }
}

#[tokio::test]
async fn bare_wait_allows_unread_non_user_inbox_and_delivers_it_on_resume() {
    let mut h = harness(Duration::from_secs(30)).await;
    let agent = start_agent(&h, assigned_worker_spec());
    let turn = turn_of(&mut h, agent).await;
    // A direct user message requires a visible answer before another wait.
    // A system update is still a valid wake source for an assigned worker.
    let update = "A dependent task finished.";
    h.runtime
        .send_message(agent, vec![ContentBlock::text(update)], Actor::System)
        .unwrap();
    let row = h.store.agent(agent).unwrap().unwrap();
    assert!(h.store.inbox_last_seq(agent).unwrap() > row.inbox_cursor);
    let (reply, is_error) = call(&h, &turn, "wait_for", json!({})).await;
    assert!(!is_error, "{reply}");
    assert!(turn.observed.stopped());
    let _ = turn.end.send(());
    let resumed = turn_of(&mut h, agent).await;
    assert!(resumed.prompt.contains(update));
    h.runtime.shutdown().await;
}

#[tokio::test]
async fn user_message_waking_a_wait_requires_visible_reply_before_another_wait() {
    let mut h = harness(Duration::from_secs(30)).await;
    let agent = start_coordinator(&h);
    let first = turn_of(&mut h, agent).await;
    let (reply, error) = call(&h, &first, "wait_for", json!({})).await;
    assert!(!error, "{reply}");
    let _ = first.end.send(());
    wait_for_park(&h, agent).await;

    h.runtime
        .send_message(
            agent,
            vec![ContentBlock::text("Is the child stuck?")],
            Actor::user(),
        )
        .unwrap();
    let resumed = turn_of(&mut h, agent).await;
    let (reply, error) = call(&h, &resumed, "wait_for", json!({})).await;
    assert!(
        error && reply.contains("Reply in visible assistant text"),
        "{reply}"
    );
    assert!(!resumed.observed.stopped());

    resumed
        .events
        .upgrade()
        .unwrap()
        .send(EngineEvent::Assistant {
            content: vec![ContentBlock::text("The child is still working.")],
            usage: None,
            model: None,
        })
        .await
        .unwrap();
    wait_for_event(h.store.as_ref(), |event| matches!(&event.kind,
        EventKind::AgentMessage { content, .. }
            if content.iter().any(|block| matches!(block, ContentBlock::Text { text } if text == "The child is still working."))
    )).await;
    let (reply, error) = call(&h, &resumed, "wait_for", json!({})).await;
    assert!(!error, "{reply}");
    let _ = resumed.end.send(());
    wait_for_park(&h, agent).await;
    h.runtime.shutdown().await;
}

#[tokio::test]
async fn explicit_waits_allow_an_assigned_task_without_other_pending_work() {
    for source in ["message_from", "timeout_seconds", "wake_at"] {
        let mut h = harness(Duration::from_secs(30)).await;
        let input = match source {
            "message_from" => json!({"message_from": idle_agent(&h).to_string()}),
            "timeout_seconds" => json!({"timeout_seconds": 3600}),
            _ => json!({"wake_at": (now() + chrono::Duration::hours(1)).to_rfc3339()}),
        };
        let agent = start_agent(&h, assigned_worker_spec());
        let turn = turn_of(&mut h, agent).await;
        let (reply, is_error) = call(&h, &turn, "wait_for", input).await;
        assert!(!is_error, "{reply}");
        assert!(turn.observed.stopped());
        let _ = turn.end.send(());
        wait_for_park(&h, agent).await;
        h.runtime.shutdown().await;
    }
}

#[tokio::test]
async fn explicit_task_wait_still_uses_caller_scope_validation() {
    let mut h = harness(Duration::from_secs(30)).await;
    let agent = start_agent(&h, assigned_worker_spec());
    let turn = turn_of(&mut h, agent).await;
    let task = Task::new("external work", Actor::user(), Default::default(), now());
    h.store
        .commit(Commit::new().mutate(Mutation::UpsertTask(task.clone())))
        .unwrap();
    let (reply, is_error) = call(&h, &turn, "wait_for", json!({})).await;
    assert!(
        is_error,
        "an unrelated task must not justify a bare wait: {reply}"
    );
    let (reply, is_error) = call(&h, &turn, "wait_for", json!({"tasks": [TaskId::new()]})).await;
    assert!(is_error && reply.contains("is unknown"), "{reply}");
    assert!(!turn.observed.stopped());
    let (reply, is_error) = call(&h, &turn, "wait_for", json!({"tasks": [task.id]})).await;
    assert!(!is_error, "{reply}");
    let _ = turn.end.send(());
    let parked = wait_for_park(&h, agent).await;
    assert_eq!(waited_on(&parked.status), vec![task.id]);
    h.runtime.shutdown().await;
}

#[tokio::test]
async fn bare_wait_without_a_task_is_unchanged_for_long_lived_and_short_lived_agents() {
    for long_lived in [false, true] {
        let mut h = harness(Duration::from_secs(30)).await;
        let agent = start_agent(
            &h,
            CreateAgentSpec {
                objective: None,
                config: Some(AgentConfig {
                    long_lived,
                    ..Default::default()
                }),
                ..coordinator_spec()
            },
        );
        h.runtime
            .send_message(agent, vec![ContentBlock::text("hello")], Actor::user())
            .unwrap();
        let turn = turn_of(&mut h, agent).await;
        assert!(
            h.store
                .agent(agent)
                .unwrap()
                .unwrap()
                .current_task
                .is_none()
        );
        turn.events
            .upgrade()
            .unwrap()
            .send(EngineEvent::Assistant {
                content: vec![ContentBlock::text("Hello. I am available.")],
                usage: None,
                model: None,
            })
            .await
            .unwrap();
        wait_for_event(h.store.as_ref(), |event| matches!(&event.kind,
            EventKind::AgentMessage { content, .. }
                if content.iter().any(|block| matches!(block, ContentBlock::Text { text } if text == "Hello. I am available."))
        )).await;
        let (reply, is_error) = call(&h, &turn, "wait_for", json!({})).await;
        assert!(!is_error, "{reply}");
        let _ = turn.end.send(());
        let parked = wait_for_park(&h, agent).await;
        assert_eq!(condition_of(&parked.status), &WaitCondition::Inbox);
        h.runtime.shutdown().await;
    }
}

#[tokio::test]
async fn bare_wait_counts_unsettled_tasks_created_by_the_caller() {
    for status in [
        TaskStatus::Queued,
        TaskStatus::Assigned,
        TaskStatus::Active,
        TaskStatus::Blocked {
            reason: "dependency".into(),
            dependency: None,
        },
        TaskStatus::Completed {
            result: TaskResult::summary("done"),
        },
        TaskStatus::Failed {
            error: "failed".into(),
            structured: None,
        },
        TaskStatus::Cancelled { reason: None },
    ] {
        let mut h = harness(Duration::from_secs(30)).await;
        let agent = start_agent(&h, assigned_worker_spec());
        let turn = turn_of(&mut h, agent).await;
        // Ownerless tasks can have a different run; creation still makes them pending work.
        let mut task = Task::new(
            "pending work",
            Actor::agent(agent),
            Default::default(),
            now(),
        );
        let settled = status.is_terminal();
        task.status = status;
        h.store
            .commit(Commit::new().mutate(Mutation::UpsertTask(task)))
            .unwrap();
        let (reply, is_error) = call(&h, &turn, "wait_for", json!({})).await;
        assert_eq!(is_error, settled, "{reply}");
        assert_eq!(turn.observed.stopped(), !settled);
        if !settled {
            let _ = turn.end.send(());
            wait_for_park(&h, agent).await;
        }
        h.runtime.shutdown().await;
    }
}

#[tokio::test]
async fn bare_wait_counts_only_active_schedules_targeting_the_caller() {
    for (active, targets_caller) in [(true, true), (false, true), (true, false)] {
        let mut h = harness(Duration::from_secs(30)).await;
        let agent = start_agent(&h, assigned_worker_spec());
        let target = if targets_caller {
            agent
        } else {
            start_agent(
                &h,
                CreateAgentSpec {
                    start: false,
                    ..coordinator_spec()
                },
            )
        };
        let turn = turn_of(&mut h, agent).await;
        let spec: CreateScheduleSpec = serde_json::from_value(json!({
            "name": "later", "target": target,
            "recurrence": {"kind": "interval", "seconds": 3600}
        }))
        .unwrap();
        let mut schedule = h.runtime.create_schedule(spec, Actor::user()).unwrap();
        if !active {
            schedule.status = ScheduleStatus::Paused {
                next_due: schedule.status.next_due().unwrap(),
            };
            h.store
                .commit(Commit::new().mutate(Mutation::UpsertSchedule(schedule)))
                .unwrap();
        }
        let (reply, is_error) = call(&h, &turn, "wait_for", json!({})).await;
        let allowed = active && targets_caller;
        assert_eq!(is_error, !allowed, "{reply}");
        assert_eq!(turn.observed.stopped(), allowed);
        if allowed {
            let _ = turn.end.send(());
            wait_for_park(&h, agent).await;
        }
        h.runtime.shutdown().await;
    }
}

/// A stage agent under a pipeline run node (a root never started) asks its
/// parent and waits: the bridge refuses both waits instead of parking.
#[tokio::test]
async fn waits_on_a_run_node_parent_are_refused_and_on_a_live_parent_park() {
    let mut h = harness(Duration::from_secs(30)).await;
    let (node, _) = h
        .runtime
        .create_agent(
            CreateAgentSpec {
                role: "pipeline:run".into(),
                start: false,
                ..Default::default()
            },
            Actor::user(),
        )
        .expect("run node");
    let agent = start_agent(&h, stage_spec(node.id));
    let turn = turn_of(&mut h, agent).await;
    for input in [json!({"message_from": node.id.to_string()}), json!({})] {
        let (reply, is_error) = call(&h, &turn, "wait_for", input).await;
        assert!(is_error, "{reply}");
        assert!(reply.contains(&node.id.to_string()), "{reply}");
        assert!(reply.contains("never started"), "{reply}");
        assert!(!turn.observed.stopped(), "a refused wait never parks");
    }
    let _ = turn.end.send(());
    h.runtime.shutdown().await;

    let mut h = harness(Duration::from_secs(30)).await;
    let parent = idle_agent(&h);
    let agent = start_agent(&h, stage_spec(parent));
    let turn = turn_of(&mut h, agent).await;
    let (reply, is_error) = call(
        &h,
        &turn,
        "wait_for",
        json!({"message_from": parent.to_string()}),
    )
    .await;
    assert!(!is_error, "{reply}");
    assert!(turn.observed.stopped());
    let _ = turn.end.send(());
    wait_for_park(&h, agent).await;
    h.runtime.shutdown().await;
}

/// An assigned worker delegated under `parent`, as a pipeline stage is.
fn stage_spec(parent: AgentId) -> CreateAgentSpec {
    let mut spec = assigned_worker_spec();
    let config = spec.config.as_mut().expect("worker config");
    config.effort = Some(aether_core::model::Effort::Low);
    config.long_lived = false;
    spec.parent = Some(parent);
    spec
}
