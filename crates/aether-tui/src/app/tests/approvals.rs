//! approvals reducer regressions.
use super::*;
use aether_core::approval::RuleScope;
use chrono::Duration;

use crate::app::approval_flow::fixtures::read_card;
use crate::app::approval_flow::{ApprovalAnswer, TYPING_GUARD_MS};

mod history;

fn approval_event(approval: ApprovalId, argument: &str) -> EventKind {
    EventKind::ApprovalRequested {
        approval,
        call: ToolCallId::new(),
        tool: "bash".into(),
        description: argument.into(),
        argument: Some(argument.into()),
        side_effect: SideEffectClass::Mutation,
    }
}

/// `agent` asks to run `argument`; the clock moves a typing guard past the
/// request, so it shows whatever was typed before, as a later tick would.
fn request_approval(app: &mut App, seq: u64, agent: AgentId, argument: &str) -> ApprovalId {
    let approval = ApprovalId::new();
    app.reduce(AppEvent::Frame(SubscriptionFrame::Event(Box::new(event(
        seq,
        agent,
        approval_event(approval, argument),
    )))));
    app.now = idle_after_now();
    approval
}

fn idle_after_now() -> aether_core::time::Timestamp {
    now() + Duration::milliseconds(TYPING_GUARD_MS)
}

fn resolved(effects: &[Effect]) -> Vec<(ApprovalId, ApprovalDecision)> {
    effects
        .iter()
        .filter_map(|e| match e {
            Effect::Request {
                command: Command::ResolveApproval { id, decision, .. },
                ..
            } => Some((*id, *decision)),
            _ => None,
        })
        .collect()
}

fn refusal(message: &str) -> ClientError {
    ClientError::Protocol(aether_protocol::ErrorReply::new(
        aether_protocol::ErrorCode::Internal,
        message,
    ))
}

#[test]
fn approval_view_and_y_key_resolve() {
    let (mut app, id) = app_with_agent();
    let approval = request_approval(&mut app, 7, id, "rm -rf build");
    assert_eq!(app.due_approval().map(|a| a.id), Some(approval));
    let effects = app.reduce(key(KeyCode::Char('y')));
    assert!(
        matches!(&effects[0], Effect::Request { command: Command::ResolveApproval { id: a, decision: ApprovalDecision::Approved, .. }, then: Then::ApprovalResolved(t) } if *a == approval && *t == approval)
    );
    // The request stays until the daemon confirms: dropping it early would
    // leave the agent waiting with nothing left to answer it.
    assert_eq!(app.selected_approvals().len(), 1);
    // Answering again while one is in flight does nothing.
    assert!(app.due_approval().is_none());
    assert!(
        app.answer_approval(approval, ApprovalAnswer::Once)
            .is_empty()
    );

    // A failed answer puts it back, answerable again.
    app.reduce(AppEvent::Reply {
        then: Then::ApprovalResolved(approval),
        result: Err(refusal("store is busy")),
    });
    assert_eq!(app.selected_approvals().len(), 1);
    assert_eq!(app.due_approval().map(|a| a.id), Some(approval));
    // Back on screen just now: a `y` at once is a bounce, not an answer.
    assert!(resolved(&app.reduce(key(KeyCode::Char('y')))).is_empty());
    read_card(&mut app);
    let effects = app.reduce(key(KeyCode::Char('y')));
    assert_eq!(
        resolved(&effects),
        vec![(approval, ApprovalDecision::Approved)]
    );

    // Acknowledged: now it goes.
    app.reduce(AppEvent::Reply {
        then: Then::ApprovalResolved(approval),
        result: Ok(Reply::Ok),
    });
    assert!(app.selected_approvals().is_empty());
}

#[test]
fn remembering_from_the_view_is_narrow_and_waits_to_be_acknowledged() {
    let (mut app, id) = app_with_agent();
    let approval = request_approval(&mut app, 7, id, "cargo test");
    // `a` is one keystroke from `y`, so it must not mean "allow this
    // tool everywhere".
    let effects = app.reduce(key(KeyCode::Char('a')));
    let Effect::Request {
        command:
            Command::ResolveApproval {
                remember: Some(spec),
                decision,
                ..
            },
        then,
    } = &effects[0]
    else {
        panic!("expected a remembered answer, got {effects:?}");
    };
    assert_eq!(*decision, ApprovalDecision::Approved);
    assert!(!spec.tool_wide, "the single key must not cover the tool");
    assert_eq!(spec.scope, Some(RuleScope::Subtree { root: id }));
    assert_eq!(*then, Then::ApprovalResolved(approval));
    assert_eq!(app.selected_approvals().len(), 1, "not dropped yet");
    app.reduce(AppEvent::Reply {
        then: Then::ApprovalResolved(approval),
        result: Ok(Reply::Ok),
    });
    assert!(app.selected_approvals().is_empty());
}

#[test]
fn the_rules_view_lists_and_revokes_what_it_sits_on() {
    let (mut app, _) = app_with_agent();
    app.input.set_text("/rules");
    let effects = app.reduce(key(KeyCode::Enter));
    let kind = app.bottom.view_stack.last().and_then(|view| view.kind());
    assert_eq!(
        kind,
        Some(crate::ui::views::ViewKind::Approvals),
        "the rules view is open"
    );
    assert!(
        effects.iter().any(|e| matches!(
            e,
            Effect::Request {
                command: Command::ListApprovalRules {
                    include_revoked: false
                },
                then: Then::ApprovalRules,
            }
        )),
        "{effects:?}"
    );
    let rules = vec![rule("bash"), rule("write_file")];
    let (first, second) = (rules[0].id, rules[1].id);
    app.reduce(AppEvent::Reply {
        then: Then::ApprovalRules,
        result: Ok(Reply::ApprovalRules(rules)),
    });
    assert_eq!(app.approval_rules.len(), 2);
    // `d` revokes the highlighted rule.
    app.reduce(key(KeyCode::Down));
    let effects = app.reduce(key(KeyCode::Char('d')));
    assert!(
        matches!(&effects[0], Effect::Request { command: Command::RevokeApprovalRule { id, .. }, .. } if *id == second),
        "the cursor decides which rule goes: {effects:?}"
    );
    assert!(
        !effects.iter().any(|e| matches!(
            e,
            Effect::Request {
                command: Command::ListApprovalRules { .. },
                ..
            }
        )),
        "nothing is reported gone until the daemon answers: {effects:?}"
    );
    assert_ne!(app.live_flash(), Some(format!("revoked {second}").as_str()));
    // A refusal surfaces, and the rule is not reported gone.
    app.reduce(AppEvent::Reply {
        then: Then::RuleRevoked,
        result: Err(refusal("rule is already revoked")),
    });
    assert!(
        app.live_flash()
            .is_some_and(|n| n.contains("already revoked")),
        "a refused revoke must surface: {:?}",
        app.live_flash()
    );
    // The daemon's answer is what reports it gone, and refreshes the list.
    let effects = app.reduce(AppEvent::Reply {
        then: Then::RuleRevoked,
        result: Ok(Reply::ApprovalRule(revoked_rule(second))),
    });
    assert_eq!(app.live_flash(), Some(format!("revoked {second}").as_str()));
    assert!(effects.iter().any(|e| matches!(
        e,
        Effect::Request {
            command: Command::ListApprovalRules { .. },
            ..
        }
    )));
    assert_ne!(first, second);
    app.reduce(key(KeyCode::Esc));
    assert!(app.bottom.view_stack.is_empty());
}

/// Keys typed as the approval lands stay text until the operator pauses;
/// only then does the view take `n`. It rings while the window is unfocused.
#[test]
fn typing_is_never_answered_and_an_unfocused_approval_rings() {
    let (mut app, id) = app_with_agent();
    app.reduce(AppEvent::Focus(false));
    assert!(app.reduce(key(KeyCode::Char('n'))).is_empty());
    let approval = ApprovalId::new();
    let effects = app.reduce(AppEvent::Frame(SubscriptionFrame::Event(Box::new(event(
        7,
        id,
        approval_event(approval, "touch x"),
    )))));
    assert!(effects.contains(&Effect::Bell), "{effects:?}");
    assert!(app.reduce(key(KeyCode::Char('o'))).is_empty());
    assert_eq!(app.input.text, "no");
    assert!(app.resolving.is_empty());

    // A second of idle later the view is up and, once read, `n` denies.
    app.now = idle_after_now();
    read_card(&mut app);
    let effects = app.reduce(key(KeyCode::Char('n')));
    assert_eq!(
        resolved(&effects),
        vec![(approval, ApprovalDecision::Denied)]
    );
    assert_eq!(app.input.text, "no");
    app.reduce(AppEvent::Reply {
        then: Then::ApprovalResolved(approval),
        result: Ok(Reply::Ok),
    });
    // Without a pending approval the same key just types.
    app.input.set_text("");
    app.reduce(key(KeyCode::Char('n')));
    assert_eq!(app.input.text, "n");

    // A finished turn rings only while the window is unfocused.
    app.reduce(AppEvent::Frame(SubscriptionFrame::Event(Box::new(event(
        8,
        id,
        EventKind::AgentWaiting {
            condition: aether_core::agent::WaitCondition::Inbox,
        },
    )))));
    let turn_done = |seq| {
        AppEvent::Frame(SubscriptionFrame::Event(Box::new(event(
            seq,
            id,
            EventKind::AgentTurnCompleted {
                turn: 1,
                invocation: None,
                next: aether_core::agent::AgentStatusKind::Waiting,
            },
        ))))
    };
    assert!(app.reduce(turn_done(9)).contains(&Effect::Bell));
    app.reduce(AppEvent::Focus(true));
    assert!(!app.reduce(turn_done(10)).contains(&Effect::Bell));
}

/// A shown approval takes its keys whatever else has the keyboard.
#[test]
fn approval_keys_work_over_the_board_and_a_draft() {
    for surface in ["board", "draft"] {
        let (mut app, id) = app_with_agent();
        app.size = (120, 40);
        match surface {
            "board" => {
                app.reduce(ctrl_key(KeyCode::Char('g')));
                assert!(app.board.focused());
            }
            _ => app.input.set_text("half a thought"),
        }
        let approval = request_approval(&mut app, 7, id, "touch x");
        read_card(&mut app);
        let effects = app.reduce(key(KeyCode::Char('n')));
        assert_eq!(
            resolved(&effects),
            vec![(approval, ApprovalDecision::Denied)],
            "{surface}"
        );
    }
}

#[test]
fn approval_from_a_background_agent_notifies_by_its_own_name_without_scrolling() {
    let (mut app, selected) = app_with_agent();
    let other = AgentId::new();
    app.tree.roots.push(other);
    app.tree.nodes.insert(
        other,
        TreeNode {
            agent: agent(other, "background"),
            children: vec![],
            task: None,
            waiting_on: vec![],
            lease: None,
            subtree_usage: Usage::default(),
            hidden_usage: None,
        },
    );
    app.reduce(AppEvent::Focus(false));
    let effects = app.reduce(AppEvent::Frame(SubscriptionFrame::Event(Box::new(event(
        9,
        other,
        approval_event(ApprovalId::new(), "touch x"),
    )))));
    assert_eq!(
        app.scroll_log.since(0).count(),
        0,
        "a background agent's approval must not scroll the transcript the operator is reading"
    );
    assert!(effects.contains(&Effect::Notify {
        title: "aether · background".into(),
        body: "an approval is waiting".into(),
    }));
    assert_eq!(app.selected, Some(selected), "selection must not move");
}

#[test]
fn approval_from_the_selected_agent_notifies_by_its_name() {
    let (mut app, id) = app_with_agent();
    app.reduce(AppEvent::Focus(false));
    let effects = app.reduce(AppEvent::Frame(SubscriptionFrame::Event(Box::new(event(
        9,
        id,
        approval_event(ApprovalId::new(), "touch x"),
    )))));
    assert!(effects.contains(&Effect::Notify {
        title: "aether · root".into(),
        body: "an approval is waiting".into(),
    }));
}

/// A5: the wire event carries no `resources`, so a live approval must be
/// backfilled from the daemon's record - the view must not go on
/// showing the placeholder forever.
#[test]
fn a_live_approval_is_backfilled_with_the_resources_the_event_could_not_carry() {
    use aether_core::tool::ResourceKey;
    let (mut app, id) = app_with_agent();
    let approval_id = ApprovalId::new();
    let effects = app.reduce(AppEvent::Frame(SubscriptionFrame::Event(Box::new(event(
        9,
        id,
        EventKind::ApprovalRequested {
            approval: approval_id,
            call: ToolCallId::new(),
            tool: "write_file".into(),
            description: "write a scratch file".into(),
            argument: None,
            side_effect: SideEffectClass::Mutation,
        },
    )))));
    app.now = now();
    assert!(app.approvals[0].resources.is_empty(), "nothing to show yet");
    assert!(
        effects.iter().any(|e| matches!(
            e,
            Effect::Request {
                command: Command::ListApprovals,
                then: Then::Approvals { .. }
            }
        )),
        "a live approval must be reconciled against the daemon's full record: {effects:?}"
    );
    let text = crate::testing::render_to(&app, 100, 30);
    assert!(text.contains("write a scratch file"), "{text}");
    assert!(!text.contains("/tmp/x"), "{text}");

    let full = ApprovalRequest {
        id: approval_id,
        agent_id: id,
        task_id: None,
        call_id: ToolCallId::new(),
        tool: "write_file".into(),
        description: "write a scratch file".into(),
        argument: None,
        resources: vec![ResourceKey::file("/tmp/x")],
        side_effect: SideEffectClass::Mutation,
        requested_at: app.approvals[0].requested_at,
        status: ApprovalStatus::Pending,
    };
    app.reduce(AppEvent::Reply {
        then: Then::Approvals {
            revision: app.approvals_revision,
        },
        result: Ok(Reply::Approvals(vec![full.clone()])),
    });
    assert_eq!(app.approvals[0].resources, full.resources);

    let text = crate::testing::render_to(&app, 100, 30);
    assert!(
        text.contains("/tmp/x"),
        "the file the daemon named is shown: {text}"
    );
}
