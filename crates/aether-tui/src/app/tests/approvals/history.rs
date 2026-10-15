use super::*;
use aether_core::model::{ContentBlock, MessageRole};

use crate::app::transcript::ScrollIntent;

fn answer(seq: u64, agent: AgentId, text: &str) -> Event {
    event(
        seq,
        agent,
        EventKind::AgentMessage {
            role: MessageRole::Assistant,
            content: vec![ContentBlock::text(text)],
            invocation: None,
        },
    )
}

fn history_reply(app: &mut App, agent: AgentId, events: Vec<Event>) -> Vec<Effect> {
    app.reduce(AppEvent::Reply {
        then: crate::testing::backfill_then(app, agent),
        result: Ok(Reply::Events(events)),
    })
}

fn requester_reading_history() -> (App, AgentId, ApprovalId) {
    let (mut app, requester) = app_with_agent();
    app.reduce(AppEvent::Frame(SubscriptionFrame::Event(Box::new(answer(
        10,
        requester,
        "current answer",
    )))));
    app.transcripts.get_mut(&requester).unwrap().older_available = true;
    assert!(app.older_history_request(requester).is_some());
    assert!(history_reply(&mut app, requester, vec![answer(5, requester, "old")]).is_empty());
    app.transcripts.get_mut(&requester).unwrap().older_available = true;
    assert!(matches!(
        app.older_history_request(requester),
        Some(Effect::Request {
            command: Command::GetEvents {
                query: aether_protocol::EventQuery { before: Some(5), .. },
                ..
            },
            then: Then::Backfill { agent: id, .. },
        }) if id == requester
    ));
    app.reduce(AppEvent::Frame(SubscriptionFrame::Event(Box::new(answer(
        11,
        requester,
        "arrived while reading",
    )))));
    let approval = request_approval(&mut app, 12, requester, "cargo test");
    app.reduce(AppEvent::Frame(SubscriptionFrame::Delta {
        agent: requester,
        invocation: InvocationId::new(),
        delta: aether_protocol::Delta::Text {
            text: "live prefix".into(),
        },
    }));
    read_card(&mut app);
    assert!(app.reading_history());
    assert_eq!(app.transcripts[&requester].history.back().unwrap().seq, 5);
    assert_eq!(app.shown_approval().map(|a| a.id), Some(approval));
    (app, requester, approval)
}

fn view_other_agent(app: &mut App, requester: AgentId) {
    let other = AgentId::new();
    app.tree.roots.push(other);
    app.tree.nodes.extend(tree(agent(other, "other")).nodes);
    app.select(other);
    assert!(history_reply(app, other, vec![]).is_empty());
    assert_eq!(app.selected, Some(other));
    assert!(app.transcripts[&requester].live.is_some());
}

fn assert_live_and_accepts_reply(app: &mut App, requester: AgentId, current: Vec<Cell>) {
    assert_eq!(app.selected, Some(requester));
    assert!(!app.reading_history());
    assert_eq!(app.transcripts[&requester].cells, current);
    assert_eq!(app.transcripts[&requester].stream.text, "live prefix");
    let last = app.scroll_log.since(0).last().unwrap();
    assert_eq!((last.agent, last.intent), (requester, ScrollIntent::Latest));
    app.reduce(AppEvent::Frame(SubscriptionFrame::Event(Box::new(answer(
        13,
        requester,
        "new reply after navigation",
    )))));
    assert!(app.transcripts[&requester].live.is_none());
    assert!(models(app).contains(&CellModel::Assistant {
        text: "new reply after navigation".into(),
    }));
}

#[test]
fn opening_requester_restores_live_cells_and_new_replies() {
    let (mut app, requester, approval) = requester_reading_history();
    let current = app.transcripts[&requester].live_state().cells.clone();
    view_other_agent(&mut app, requester);

    let effects = app.reduce(key(KeyCode::Char('o')));

    assert!(resolved(&effects).is_empty());
    assert!(effects.iter().any(|effect| matches!(
        effect,
        Effect::Request {
            command: Command::ListInvocations { agent },
            then: Then::Invocations(id),
        } if *agent == requester && *id == requester
    )));
    assert!(app.bottom.approval.deferred.contains(&approval));
    assert!(!app.resolving.contains(&approval));
    assert!(app.needs_you().iter().any(|row| row.agent == requester));
    assert_live_and_accepts_reply(&mut app, requester, current);
}

#[test]
fn denying_approval_restores_live_cells_and_preserves_denial() {
    for already_selected in [false, true] {
        let (mut app, requester, approval) = requester_reading_history();
        let current = app.transcripts[&requester].live_state().cells.clone();
        if !already_selected {
            view_other_agent(&mut app, requester);
        }

        let effects = app.reduce(key(KeyCode::Char('n')));

        assert_eq!(
            resolved(&effects),
            vec![(approval, ApprovalDecision::Denied)]
        );
        assert!(matches!(
            effects.first(),
            Some(Effect::Request {
                command: Command::ResolveApproval {
                    id, decision: ApprovalDecision::Denied, note: None,
                    remember: None, idempotency_key: None,
                },
                then: Then::ApprovalResolved(resolved),
            }) if *id == approval && *resolved == approval
        ));
        assert_eq!(app.bottom.approval.deny_target, Some(requester));
        assert!(app.resolving.contains(&approval));
        assert!(!app.bottom.approval.deferred.contains(&approval));
        assert_live_and_accepts_reply(&mut app, requester, current);
    }
}

#[test]
fn approval_navigation_ignores_a_late_older_page() {
    for action in ['o', 'n'] {
        let (mut app, requester, _) = requester_reading_history();
        view_other_agent(&mut app, requester);
        app.reduce(key(KeyCode::Char(action)));
        let before = app.transcripts[&requester].clone();

        let effects = history_reply(&mut app, requester, vec![answer(1, requester, "stale")]);

        assert!(effects.is_empty());
        assert_eq!(app.transcripts[&requester], before, "action {action}");
        assert!(!app.reading_history());
        assert_eq!(app.transcripts[&requester].history.back().unwrap().seq, 12);
    }
}

#[test]
fn opening_already_selected_requester_keeps_history_and_approval() {
    let (mut app, requester, approval) = requester_reading_history();
    let before = app.transcripts[&requester].clone();
    let scrolls = app.scroll_log.since(0).count();

    assert!(app.reduce(key(KeyCode::Char('o'))).is_empty());

    assert_eq!(app.selected, Some(requester));
    assert_eq!(app.transcripts[&requester], before);
    assert_eq!(app.scroll_log.since(0).count(), scrolls);
    assert!(!app.bottom.approval.deferred.contains(&approval));
    assert!(!app.resolving.contains(&approval));
    assert_eq!(app.shown_approval().map(|a| a.id), Some(approval));
    assert!(history_reply(&mut app, requester, vec![answer(1, requester, "older")]).is_empty());
    assert!(app.reading_history());
    assert_eq!(app.transcripts[&requester].history.back().unwrap().seq, 1);
}
