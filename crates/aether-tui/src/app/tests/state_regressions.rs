use super::*;

#[test]
fn restart_reply_preserves_a_newer_selection() {
    let (mut app, from) = app_with_agent();
    let other = AgentId::new();
    let replacement = AgentId::new();
    let then = app
        .run_command(crate::commands::SlashCommand::Restart)
        .into_iter()
        .find_map(|effect| match effect {
            Effect::Request {
                command: Command::Restart { .. },
                then,
            } => Some(then),
            _ => None,
        })
        .expect("restart request");
    app.select(other);
    app.reduce(AppEvent::Reply {
        then,
        result: Ok(Reply::Agent(agent(replacement, "restarted"))),
    });
    assert_eq!(app.selected, Some(other));
    assert!(app.live_flash().unwrap().contains("restarted as"));

    app.select(from);
    app.reduce(AppEvent::Reply {
        then: Then::Restarted { from },
        result: Ok(Reply::Agent(agent(replacement, "restarted"))),
    });
    assert_eq!(app.selected, Some(replacement));
}

#[test]
fn restart_reply_does_not_abandon_an_awaited_root_start() {
    let (mut app, from) = app_with_agent();
    let effects = app.await_root_in(None, None);
    let start = app.awaited_start().expect("root start");
    assert!(effects.is_some());
    app.reduce(AppEvent::Reply {
        then: Then::Restarted { from },
        result: Ok(Reply::Agent(agent(AgentId::new(), "restarted"))),
    });
    assert_eq!(app.awaited_start(), Some(start));
}

#[test]
fn stale_approval_list_cannot_restore_a_resolved_approval() {
    let (mut app, id) = app_with_agent();
    let approval = ApprovalId::new();
    let effects = app.on_event(event(
        1,
        id,
        EventKind::ApprovalRequested {
            approval,
            call: ToolCallId::new(),
            tool: "bash".into(),
            description: "echo hi".into(),
            argument: None,
            side_effect: SideEffectClass::Mutation,
        },
    ));
    let then = effects
        .into_iter()
        .find_map(|effect| match effect {
            Effect::Request {
                command: Command::ListApprovals,
                then,
            } => Some(then),
            _ => None,
        })
        .expect("list request");
    let snapshot = app.approvals.clone();
    app.on_event(event(
        2,
        id,
        EventKind::ApprovalResolved {
            approval,
            decision: ApprovalDecision::Denied,
            by: Actor::user(),
            note: None,
        },
    ));
    let effects = app.on_reply(then, Ok(Reply::Approvals(snapshot)));
    assert!(app.approvals.is_empty());
    assert!(effects.contains(&app.approvals_request()));
    app.on_reply(
        Then::Approvals {
            revision: app.approvals_revision,
        },
        Ok(Reply::Approvals(vec![])),
    );
    assert!(app.approvals.is_empty());
}

#[test]
fn stale_approval_list_cannot_drop_a_new_request_or_undo_a_local_resolution() {
    let (mut app, id) = app_with_agent();
    let before = app.approvals_revision;
    let approval = ApprovalId::new();
    app.on_event(event(
        1,
        id,
        EventKind::ApprovalRequested {
            approval,
            call: ToolCallId::new(),
            tool: "bash".into(),
            description: "echo hi".into(),
            argument: None,
            side_effect: SideEffectClass::Mutation,
        },
    ));
    app.on_reply(
        Then::Approvals { revision: before },
        Ok(Reply::Approvals(vec![])),
    );
    assert_eq!(app.approvals[0].id, approval);
    let snapshot = app.approvals.clone();
    let before = app.approvals_revision;
    app.on_reply(Then::ApprovalResolved(approval), Ok(Reply::Ok));
    app.on_reply(
        Then::Approvals { revision: before },
        Ok(Reply::Approvals(snapshot)),
    );
    assert!(app.approvals.is_empty());
}

#[test]
fn agent_name_collision_uses_creation_time_before_id_order() {
    let (mut app, root) = app_with_agent();
    let mut ids = [AgentId::new(), AgentId::new()];
    ids.sort();
    let mut newer = agent(ids[0], "worker");
    newer.parent = Some(root);
    let mut older = agent(ids[1], "worker");
    older.parent = Some(AgentId::new());
    older.created_at = newer.created_at - chrono::Duration::seconds(1);
    app.tree.nodes.extend(tree(newer).nodes);
    app.tree.nodes.extend(tree(older).nodes);
    app.view_agent_named("WORKER");
    assert_eq!(app.selected, Some(ids[1]));
}

#[test]
fn context_token_event_saturates_instead_of_wrapping() {
    let (mut app, id) = app_with_agent();
    app.on_event(event(
        1,
        id,
        EventKind::InvocationCompleted {
            invocation: InvocationId::new(),
            snapshot: aether_core::id::SnapshotId::new(),
            model: ModelRef::new("mock", "m"),
            usage: Usage {
                input_tokens: u64::MAX,
                cache_read_tokens: 1,
                cache_write_tokens: 1,
                ..Default::default()
            },
            stop_reason: None,
            duration_ms: 1,
        },
    ));
    assert_eq!(app.context_tokens[&id], u64::MAX);
}

#[test]
fn invocation_context_tokens_saturate_in_the_fallback_reply() {
    let (mut app, id) = app_with_agent();
    let invocation = aether_core::snapshot::Invocation {
        id: InvocationId::new(),
        snapshot_id: aether_core::id::SnapshotId::new(),
        agent_id: id,
        turn: 1,
        model: ModelRef::new("mock", "m"),
        started_at: app.now,
        finished_at: None,
        content: vec![],
        usage: Usage {
            input_tokens: u64::MAX,
            cache_read_tokens: 1,
            cache_write_tokens: 1,
            ..Default::default()
        },
        stop_reason: None,
        error: None,
        provider_message_id: None,
    };
    app.on_reply(
        Then::Invocations(id),
        Ok(Reply::Invocations(vec![invocation])),
    );
    assert_eq!(app.context_tokens[&id], u64::MAX);
}

#[test]
#[cfg(unix)]
fn ctrl_z_suspends_from_composer_and_overlays_without_changing_the_draft() {
    let (mut app, _) = app_with_agent();
    app.input.set_text("unsent draft");
    for overlay in [false, true] {
        app.bottom.shortcuts = overlay;
        let effects = app.dispatch_key(KeyEvent::new(KeyCode::Char('z'), KeyModifiers::CONTROL));
        assert_eq!(effects, vec![Effect::Suspend]);
        assert_eq!(app.input.text, "unsent draft");
    }
}

fn tree_requests(effects: &[Effect]) -> usize {
    effects
        .iter()
        .filter(|e| {
            matches!(
                e,
                Effect::Request {
                    command: Command::GetTree { .. },
                    ..
                }
            )
        })
        .count()
}

#[test]
fn lease_renewal_does_not_refetch_the_tree_but_a_task_event_does() {
    let (mut app, id) = app_with_agent();
    app.tree_in_flight = false;
    let renewed = event(
        10,
        id,
        EventKind::LeaseRenewed {
            expires_at: now() + chrono::Duration::seconds(30),
        },
    );
    let effects = app.reduce(AppEvent::Frame(SubscriptionFrame::Event(Box::new(renewed))));
    assert_eq!(tree_requests(&effects), 0);
    assert!(!app.tree_stale);

    let started = event(11, id, EventKind::TaskStarted);
    let effects = app.reduce(AppEvent::Frame(SubscriptionFrame::Event(Box::new(started))));
    assert_eq!(tree_requests(&effects), 1);
}
