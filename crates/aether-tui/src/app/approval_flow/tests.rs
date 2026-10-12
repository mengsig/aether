use aether_core::approval::RuleScope;
use aether_core::tool::ApprovalDecision;
use aether_protocol::{Command, RememberSpec};
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

use super::fixtures::{at, ctrl, fleet, key, push_cell, read_card, shell_approval};
use super::*;
use crate::app::transcript::{CellModel, ScrollIntent};

fn resolve(effects: &[Effect]) -> Vec<&Command> {
    effects
        .iter()
        .filter_map(|e| match e {
            Effect::Request {
                command: command @ Command::ResolveApproval { .. },
                ..
            } => Some(command),
            _ => None,
        })
        .collect()
}

#[test]
fn approvals_queue_fifo_across_three_agents() {
    let mut f = fleet();
    let late = shell_approval(f.researcher, "ls", at(-1_000));
    let first = shell_approval(f.tester, "cargo test", at(-3_000));
    let middle = shell_approval(f.implementer, "cargo build", at(-2_000));
    let ids = [first.id, middle.id, late.id];
    f.app.approvals = vec![late, first, middle];

    for expected in ids {
        read_card(&mut f.app);
        let shown = f.app.due_approval().expect("one is due").id;
        assert_eq!(shown, expected);
        let effects = f
            .app
            .approval_key(key(KeyCode::Char('y')))
            .expect("handled");
        assert_eq!(resolve(&effects).len(), 1);
    }
    assert!(f.app.due_approval().is_none(), "every answer is in flight");
}

#[test]
fn typing_guard_holds_an_approval_until_one_second_of_idle() {
    let mut f = fleet();
    f.app.bottom.approval.note_key(at(0));
    f.app.approvals = vec![shell_approval(f.tester, "cargo test", at(500))];

    f.app.now = at(500);
    assert!(f.app.due_approval().is_none(), "arrived mid-typing");
    f.app.now = at(999);
    assert!(f.app.due_approval().is_none());
    f.app.now = at(1_000);
    assert!(f.app.due_approval().is_some(), "shown after 1 s idle");
}

#[test]
fn typing_guard_restarts_on_each_keystroke_but_never_hides_a_shown_approval() {
    let mut f = fleet();
    f.app.bottom.approval.note_key(at(0));
    f.app.bottom.approval.note_key(at(800));
    f.app.approvals = vec![shell_approval(f.tester, "cargo test", at(900))];
    f.app.now = at(900);
    f.app.track_approvals();
    f.app.now = at(1_500);
    assert!(f.app.due_approval().is_none(), "the burst is still going");
    f.app.now = at(1_800);
    assert!(f.app.due_approval().is_some());

    // Keys pressed while it is shown start a new burst after it arrived.
    f.app.bottom.approval.note_key(at(1_900));
    f.app.now = at(1_950);
    assert!(f.app.due_approval().is_some());
}

#[test]
fn each_answer_sends_its_exact_command() {
    for key_char in ['y', 'a', 'p', 'n'] {
        let mut f = fleet();
        let request = shell_approval(f.tester, "cargo test", at(-1_000));
        let id = request.id;
        f.app.approvals = vec![request];
        let (decision, scope) = match key_char {
            'y' => (ApprovalDecision::Approved, None),
            'a' => (
                ApprovalDecision::Approved,
                Some(RuleScope::Subtree { root: f.tester }),
            ),
            'p' => (ApprovalDecision::Approved, Some(RuleScope::Everywhere)),
            _ => (ApprovalDecision::Denied, None),
        };
        read_card(&mut f.app);

        let effects = f
            .app
            .approval_key(key(KeyCode::Char(key_char)))
            .expect("handled");
        assert_eq!(
            effects[0],
            Effect::Request {
                command: Command::ResolveApproval {
                    id,
                    decision,
                    note: None,
                    remember: scope.map(|scope| RememberSpec {
                        scope: Some(scope),
                        tool_wide: false,
                        note: None,
                    }),
                    idempotency_key: None,
                },
                then: Then::ApprovalResolved(id),
            },
            "key {key_char}"
        );
        assert!(f.app.resolving.contains(&id));
    }
}

#[test]
fn digits_enter_and_arrows_pick_the_same_answers() {
    let mut f = fleet();
    let request = shell_approval(f.tester, "cargo test", at(-1_000));
    let id = request.id;
    f.app.approvals = vec![request];
    read_card(&mut f.app);

    assert_eq!(f.app.approval_key(key(KeyCode::Down)), Some(vec![]));
    assert_eq!(f.app.approval_key(key(KeyCode::Down)), Some(vec![]));
    assert_eq!(f.app.bottom.approval.cursor_for(id), 2);
    let effects = f.app.approval_key(key(KeyCode::Enter)).expect("handled");
    let [Command::ResolveApproval { remember, .. }] = resolve(&effects)[..] else {
        panic!("one resolve: {effects:?}");
    };
    assert_eq!(
        remember.as_ref().and_then(|r| r.scope.clone()),
        Some(RuleScope::Everywhere)
    );

    let mut f = fleet();
    f.app.approvals = vec![shell_approval(f.tester, "cargo test", at(-1_000))];
    read_card(&mut f.app);
    let effects = f
        .app
        .approval_key(key(KeyCode::Char('4')))
        .expect("handled");
    let [Command::ResolveApproval { decision, .. }] = resolve(&effects)[..] else {
        panic!("one resolve: {effects:?}");
    };
    assert_eq!(*decision, ApprovalDecision::Denied);
}

#[test]
fn deny_retargets_the_composer_at_the_requester() {
    let mut f = fleet();
    f.app.approvals = vec![shell_approval(f.tester, "cargo test", at(-1_000))];
    read_card(&mut f.app);
    let effects = f.app.approval_key(key(KeyCode::Esc)).expect("handled");

    assert_eq!(resolve(&effects).len(), 1);
    assert_eq!(f.app.selected, Some(f.tester));
    assert_eq!(
        f.app.live_flash(),
        Some("viewing tester · tell it what to do instead"),
        "the switch is announced"
    );
    assert_eq!(f.app.bottom.approval.deny_target, Some(f.tester));
    let scroll = f.app.scroll_log.since(0).last().expect("a scroll request");
    assert_eq!(
        (scroll.agent, scroll.intent),
        (f.tester, ScrollIntent::Latest)
    );
}

#[test]
fn ctrl_c_defers_into_needs_you_and_the_next_one_shows() {
    let mut f = fleet();
    let first = shell_approval(f.tester, "cargo test -p aether", at(-2_000));
    let second = shell_approval(f.implementer, "cargo build", at(-1_000));
    let (first_id, second_id) = (first.id, second.id);
    f.app.approvals = vec![first, second];

    assert_eq!(f.app.approval_key(ctrl('c')), Some(vec![]));
    assert_eq!(f.app.due_approval().map(|a| a.id), Some(second_id));
    assert_eq!(
        f.app.needs_you(),
        vec![NeedsYou {
            agent: f.tester,
            name: "tester".to_owned(),
            what: NeedsYouKind::Approval {
                id: first_id,
                summary: "cargo test -p aether".to_owned(),
            },
        }]
    );
    assert!(f.app.needs_you_agents().contains(&f.tester));

    f.app.reopen_approvals(f.tester);
    assert!(f.app.needs_you().is_empty());
    assert_eq!(f.app.due_approval().map(|a| a.id), Some(first_id));
}

#[test]
fn o_opens_the_requester_and_parks_the_approval() {
    let mut f = fleet();
    let request = shell_approval(f.tester, "cargo test", at(-1_000));
    let id = request.id;
    f.app.approvals = vec![request];

    let effects = f
        .app
        .approval_key(key(KeyCode::Char('o')))
        .expect("handled");
    assert!(resolve(&effects).is_empty(), "nothing is decided");
    assert_eq!(f.app.selected, Some(f.tester));
    assert!(f.app.bottom.approval.deferred.contains(&id));
    assert!(f.app.due_approval().is_none());
}

#[test]
fn global_chords_pass_through_and_stray_keys_are_swallowed() {
    let mut f = fleet();
    f.app.approvals = vec![shell_approval(f.tester, "cargo test", at(-1_000))];
    assert_eq!(f.app.approval_key(ctrl('b')), None);
    assert_eq!(
        f.app
            .approval_key(KeyEvent::new(KeyCode::Char('x'), KeyModifiers::ALT)),
        None
    );
    assert_eq!(f.app.approval_key(key(KeyCode::Char('x'))), Some(vec![]));
    assert!(f.app.resolving.is_empty());

    let mut idle = fleet();
    assert_eq!(idle.app.approval_key(key(KeyCode::Char('y'))), None);
}

#[test]
fn answering_a_vanished_approval_flashes_instead_of_sending() {
    let mut f = fleet();
    let gone = ApprovalId::new();
    assert!(f.app.answer_approval(gone, ApprovalAnswer::Once).is_empty());
    assert_eq!(
        f.app.live_flash(),
        Some("that approval is no longer pending")
    );
}

#[test]
fn needs_you_lists_open_questions_once_per_agent_oldest_first() {
    let mut f = fleet();
    let question = |asker, name: &str, text: &str| CellModel::Question {
        asker,
        asker_name: name.to_owned(),
        text: text.to_owned(),
        state: crate::app::transcript::QuestionState::Pending,
        seq: 0,
    };
    let tester = f.tester;
    let researcher = f.researcher;
    push_cell(
        &mut f.app,
        f.root,
        at(-500),
        question(researcher, "researcher", "Which crate?"),
    );
    push_cell(
        &mut f.app,
        f.root,
        at(-900),
        question(tester, "tester", "Include imported runs?\nor not"),
    );
    push_cell(
        &mut f.app,
        f.root,
        at(-100),
        question(tester, "tester", "Second question"),
    );

    let rows = f.app.needs_you();
    let names: Vec<&str> = rows.iter().map(|r| r.name.as_str()).collect();
    assert_eq!(names, ["tester", "researcher"]);
    assert_eq!(
        rows[0].what,
        NeedsYouKind::Question {
            first_line: "Include imported runs?".to_owned()
        }
    );
}

#[test]
fn mode_only_completion_preserves_the_turn_clock_fallback() {
    use aether_client::SubscriptionFrame;
    use aether_core::event::{Actor, Event, EventDraft, EventKind};
    use aether_core::id::EventId;
    use aether_core::task::TaskResult;

    let mut app = crate::testing::app_with_transcript(&[]);
    let agent = app.selected.expect("agent");
    app.tree
        .nodes
        .get_mut(&agent)
        .expect("agent")
        .agent
        .status_since = Some(app.now);
    let before = turn_started_at(&app, agent).expect("status timestamp");
    let event = Event::from_draft(
        EventDraft::new(
            Actor::System,
            EventKind::TaskCompleted {
                result: TaskResult::summary("done"),
            },
        )
        .agent(agent),
        EventId::new(),
        app.transcripts[&agent].max_seq + 1,
        before + chrono::Duration::minutes(1),
    );
    app.reduce(crate::app::AppEvent::Frame(SubscriptionFrame::Event(
        Box::new(event),
    )));
    assert_eq!(app.transcripts[&agent].named_only.len(), 1);
    assert_eq!(turn_started_at(&app, agent), Some(before));
}

#[test]
fn the_turn_clock_pauses_over_open_and_answered_approvals() {
    let mut f = fleet();
    let tester = f.tester;
    push_cell(
        &mut f.app,
        tester,
        at(-60_000),
        CellModel::User {
            from: crate::app::transcript::UserFrom::Operator,
            text: "go".to_owned(),
        },
    );
    let started = turn_started_at(&f.app, tester).expect("a turn");
    assert_eq!(started, at(-60_000));

    let request = shell_approval(tester, "cargo test", at(-20_000));
    let id = request.id;
    f.app.approvals = vec![request];
    assert_eq!(approval_pause_ms(&f.app, tester, started), 20_000);

    f.app.answer_approval(id, ApprovalAnswer::Once);
    f.app.now = at(5_000);
    assert_eq!(
        approval_pause_ms(&f.app, tester, started),
        20_000,
        "banked; the clock runs again after the answer"
    );
}
