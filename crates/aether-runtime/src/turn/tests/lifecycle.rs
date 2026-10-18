use super::*;
use crate::turn::context::lineage;

#[test]
fn only_direct_user_inbox_messages_require_a_reply() {
    use aether_core::id::{InboxItemId, MessageId};
    use aether_core::inbox::{InboxItem, InboxKind};

    let recipient = AgentId::new();
    let item = |from| InboxItem {
        id: InboxItemId::new(),
        agent_id: recipient,
        seq: 1,
        kind: InboxKind::Message {
            message_id: MessageId::new(),
            from,
            content: vec![ContentBlock::Text {
                text: "status?".into(),
            }],
        },
        delivered_at: now(),
    };
    assert!(has_direct_user_message(&[item(Actor::user())]));
    assert!(has_direct_user_message(&[item(Actor::Client {
        id: "api".into()
    })]));
    assert!(!has_direct_user_message(&[item(Actor::agent(
        AgentId::new()
    ))]));
}

#[tokio::test]
async fn direct_user_question_requires_visible_reply_before_waiting() {
    let (mut harness, _dir) = ComposeHarness::new().await;
    let condition = WaitCondition::MessageFrom {
        agent: AgentId::new(),
    };
    assert!(
        validate_wait(&harness.runtime, &harness.ctx, &condition, false)
            .expect("validate ordinary wait")
            .is_none()
    );

    harness.ctx.user_reply_pending = true;
    let refusal = validate_wait(&harness.runtime, &harness.ctx, &condition, false)
        .expect("validate wait after user message")
        .expect("must require an answer");
    assert!(refusal.is_error);
    assert!(refusal.content.contains("Reply in visible assistant text"));

    harness.ctx.user_reply_pending = false;
    assert!(
        validate_wait(&harness.runtime, &harness.ctx, &condition, false)
            .expect("validate wait after reply")
            .is_none()
    );
}

/// Stored parent links are data, not a guarantee: a chain deeper than the
/// bound, a cycle or a dangling link must all end the walk.
#[test]
fn the_memory_lineage_is_bounded_and_survives_cycles_and_dangling_links() {
    let ids: Vec<AgentId> = (0..12).map(|_| AgentId::new()).collect();
    let parent_in = |links: Vec<(AgentId, AgentId)>| {
        let links: BTreeMap<AgentId, AgentId> = links.into_iter().collect();
        move |id: AgentId| Ok::<_, std::convert::Infallible>(links.get(&id).copied())
    };
    // A chain ids[0] <- ids[1] <- ... : nearest first, cut at the bound.
    let chain = parent_in(ids.windows(2).map(|w| (w[1], w[0])).collect());
    let walked = lineage(ids[11], Some(ids[10]), chain).expect("walk");
    assert_eq!(walked.len(), aether_memory::MAX_LINEAGE);
    assert_eq!(walked[0], ids[10]);
    assert_eq!(walked[aether_memory::MAX_LINEAGE - 1], ids[3]);

    // a -> b -> c -> a: each ancestor once, never the agent itself.
    let (a, b, c) = (ids[0], ids[1], ids[2]);
    let walked = lineage(a, Some(b), parent_in(vec![(b, c), (c, a)])).expect("walk");
    assert_eq!(walked, vec![b, c]);
    let walked = lineage(a, Some(b), parent_in(vec![(b, c), (c, b)])).expect("walk");
    assert_eq!(walked, vec![b, c]);

    // A parent that is not stored ends the walk at itself.
    let walked = lineage(a, Some(b), parent_in(vec![])).expect("walk");
    assert_eq!(walked, vec![b]);
    assert!(
        lineage(a, None, parent_in(vec![]))
            .expect("walk")
            .is_empty()
    );

    // A failed lookup is the caller's to handle, not a shorter lineage.
    let failed = lineage(a, Some(b), |_| Err("store is down"));
    assert_eq!(failed, Err("store is down"));
}

/// A bare `Inbox` wait is exempt from `with_deadline`'s wait_timeout; a
/// naive merge into `All { [Inbox, Inbox] }` would lose that exemption
/// and park a long_lived agent for 30 minutes instead of indefinitely.
#[test]
fn merging_the_same_wait_condition_twice_stays_singular() {
    let mut effects = ToolEffects::default();
    merge_wait(&mut effects, WaitCondition::Inbox);
    merge_wait(&mut effects, WaitCondition::Inbox);
    let Some(Next::Waiting(condition)) = &effects.next else {
        panic!("expected a wait, got {:?}", effects.next);
    };
    assert_eq!(*condition, WaitCondition::Inbox);
}

/// Two different conditions still combine: only an exact repeat collapses.
#[test]
fn merging_distinct_wait_conditions_combines_them() {
    let mut effects = ToolEffects::default();
    let agent = AgentId::new();
    merge_wait(&mut effects, WaitCondition::Inbox);
    merge_wait(&mut effects, WaitCondition::MessageFrom { agent });
    let Some(Next::Waiting(condition)) = &effects.next else {
        panic!("expected a wait, got {:?}", effects.next);
    };
    assert_eq!(
        *condition,
        WaitCondition::All {
            of: vec![WaitCondition::Inbox, WaitCondition::MessageFrom { agent }],
        }
    );
}

/// A settle after a park is admitted only for a `long_lived`
/// agent, whose complete/fail parks it on `Inbox` rather than ending it.
/// `next` is set to the `Waiting` a real park always leaves behind -
/// `admits_after_park` is only ever consulted once `parked()` is true.
#[test]
fn admits_after_park_settles_only_for_long_lived_agents() {
    let effects = ToolEffects {
        next: Some(Next::Waiting(WaitCondition::Inbox)),
        ..Default::default()
    };
    let complete = HarnessAction::CompleteTask {
        summary: "done".into(),
        artifacts: Vec::new(),
        structured: None,
    };
    let fail = HarnessAction::FailTask {
        error: "nope".into(),
        structured: None,
    };
    assert!(effects.admits_after_park(&complete, true));
    assert!(effects.admits_after_park(&fail, true));
    assert!(!effects.admits_after_park(&complete, false));
    assert!(!effects.admits_after_park(&fail, false));
}

/// The turn's *first* question merges into whatever wait already
/// parked it - an ordinary `WaitFor`/delegate-wait, or a long_lived
/// settle's `Inbox` - on both the batch path (`apply_action`, exercised
/// by the engine-bridge integration tests) and this classification
/// helper the engine bridge consults before ever reaching `apply_action`.
#[test]
fn admits_after_park_admits_the_first_ask_user_as_a_merging_wait() {
    let ask = HarnessAction::AskUser {
        question: "which one?".into(),
    };
    let after_wait_for = ToolEffects {
        next: Some(Next::Waiting(WaitCondition::Inbox)),
        ..Default::default()
    };
    assert!(after_wait_for.admits_after_park(&ask, false));
    let after_long_lived_settle = ToolEffects {
        next: Some(Next::Waiting(WaitCondition::Inbox)),
        ..Default::default()
    };
    assert!(after_long_lived_settle.admits_after_park(&ask, true));
}

/// A second question is refused regardless of `long_lived`: only the
/// first one is a merging wait.
#[test]
fn admits_after_park_never_admits_a_second_ask_user() {
    let effects = ToolEffects {
        next: Some(Next::Waiting(WaitCondition::Inbox)),
        asked: Some("already asked".into()),
        ..Default::default()
    };
    let ask = HarnessAction::AskUser {
        question: "again?".into(),
    };
    assert!(!effects.admits_after_park(&ask, true));
    assert!(!effects.admits_after_park(&ask, false));
}

/// A question that would be swallowed by a pending approval's own wait is
/// refused, exactly like `apply_action`'s own `AskUser` guard.
#[test]
fn admits_after_park_refuses_ask_user_under_a_pending_approval() {
    let effects = ToolEffects {
        next: Some(Next::Waiting(WaitCondition::Inbox)),
        pending_approvals: vec![ApprovalId::new()],
        ..Default::default()
    };
    let ask = HarnessAction::AskUser {
        question: "ok?".into(),
    };
    assert!(!effects.admits_after_park(&ask, false));
    assert!(!effects.admits_after_park(&ask, true));
}

/// A question after a genuinely terminal settle (a non-long_lived
/// complete/fail) is refused: there is nothing left to wait on an answer.
#[test]
fn admits_after_park_refuses_ask_user_after_a_terminal_settle() {
    let completed = ToolEffects {
        next: Some(Next::Completed(None)),
        ..Default::default()
    };
    let failed = ToolEffects {
        next: Some(Next::Failed("nope".into())),
        ..Default::default()
    };
    let ask = HarnessAction::AskUser {
        question: "still relevant?".into(),
    };
    assert!(!completed.admits_after_park(&ask, false));
    assert!(!failed.admits_after_park(&ask, false));
}

/// The entry in `sched.running` *is* the concurrency slot, so it has to
/// leave with the frame that owns it - not from a tail statement an
/// unwind skips. `run`'s `catch_unwind` is the second line of defence;
/// this covers the guard on its own.
#[tokio::test]
async fn an_unwinding_turn_takes_its_engine_slot_with_it() {
    let (runtime, _config_dir) =
        crate::test_runtime(|c| c.max_concurrent_engine_turns = Some(1)).await;
    let agent = AgentId::new();
    runtime.sched.lock().running.insert(
        agent,
        crate::RunningTurn {
            task: None,
            cancel: CancellationToken::new(),
            cancel_request: None,
            stop_reason: None,
            interrupt: CancellationToken::new(),
            addressed: false,
            provider: "claude-code".into(),
            engine: true,
            on_provider: true,
        },
    );
    assert_eq!(runtime.sched.lock().on_engine(), 1, "the slot is held");
    let unwound = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        let _slot = RunningSlot {
            runtime: runtime.clone(),
            agent,
        };
        panic!("the turn panicked");
    }));
    assert!(unwound.is_err(), "the panic must stay visible");
    assert!(
        !runtime.sched.lock().running.contains_key(&agent),
        "the slot outlived the frame that owned it"
    );
    assert_eq!(runtime.sched.lock().on_engine(), 0);
}

/// After a wait parks the turn, a batched sibling that starts work or
/// delivers an effect is refused; read-only calls still answer.
#[test]
fn admits_after_park_refuses_effects_and_admits_reads() {
    use aether_core::id::ScheduleId;
    use aether_core::model::{Effort, ModelRef};
    use aether_core::schedule::ScheduleAction;
    use aether_tools::WorkspaceMode;

    let effects = ToolEffects {
        next: Some(Next::Waiting(WaitCondition::Inbox)),
        ..Default::default()
    };
    let delegate = |wait| HarnessAction::Delegate {
        role: "worker".into(),
        name: None,
        objective: "do it".into(),
        instructions: None,
        model: ModelRef::new("p", "m"),
        effort: Effort::Low,
        scope: None,
        workspace_mode: WorkspaceMode::Shared,
        wait,
        leaf: false,
        host_context: None,
    };
    let refused = [
        delegate(false),
        HarnessAction::SendMessage {
            to: AgentId::new(),
            content: "hi".into(),
            subtree: false,
        },
        HarnessAction::UpdateSchedule {
            id: ScheduleId::new(),
            action: ScheduleAction::Pause,
        },
        HarnessAction::IntegrateWorkspace {
            child: AgentId::new(),
        },
    ];
    for action in &refused {
        assert!(!effects.admits_after_park(action, false), "{action:?}");
        assert!(!effects.admits_after_park(action, true), "{action:?}");
    }
    let admitted = [
        delegate(true),
        HarnessAction::InspectState,
        HarnessAction::ListAgents {
            after: None,
            limit: 5,
        },
        HarnessAction::ListSchedules { id: None },
    ];
    for action in &admitted {
        assert!(effects.admits_after_park(action, false), "{action:?}");
    }
}
