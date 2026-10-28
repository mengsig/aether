//! Input safety regressions: an approval answers only from a card on screen,
//! and keys reach only what the operator can see.

use aether_core::agent::AgentStatus;
use aether_core::id::ApprovalId;
use aether_core::tool::ApprovalDecision;
use aether_protocol::Command;
use chrono::Duration;
use crossterm::event::KeyCode;

use crate::app::approval_flow::fixtures::{
    Fleet, at, ctrl, fleet, key, node_mut, read_card, shell_approval,
};
use crate::app::approval_flow::{MIN_VISIBLE_MS, NeedsYouKind, TYPING_GUARD_MS};
use crate::app::board::BoardFocus;
use crate::app::{AppEvent, Effect, Then};
use crate::commands::SlashCommand;
use crate::ui::bottom::find::FindBar;

fn resolved(effects: &[Effect]) -> Option<ApprovalDecision> {
    effects.iter().find_map(|e| match e {
        Effect::Request {
            command: Command::ResolveApproval { decision, .. },
            ..
        } => Some(*decision),
        _ => None,
    })
}

fn cancels(effects: &[Effect]) -> bool {
    effects.iter().any(|e| {
        matches!(
            e,
            Effect::Request {
                command: Command::Cancel { .. },
                ..
            }
        )
    })
}

/// A wide fleet with `/settings` open and tester's `rm -rf build` due behind it.
fn settings_over_an_approval() -> (Fleet, ApprovalId) {
    let mut f = fleet();
    f.app.size = (120, 40);
    f.app.run_slash(SlashCommand::Settings);
    assert!(!f.app.bottom.view_stack.is_empty(), "settings open");
    let request = shell_approval(f.tester, "rm -rf build", at(-5_000));
    let id = request.id;
    f.app.approvals.push(request);
    f.app.track_approvals();
    (f, id)
}

#[test]
fn an_approval_behind_a_view_takes_no_keys_and_waits_in_the_strip() {
    let (mut f, id) = settings_over_an_approval();
    assert!(f.app.due_approval().is_some());
    assert!(f.app.shown_approval().is_none());
    let screen = crate::testing::render_to(&f.app, 120, 40);
    assert!(!screen.contains("Allow tester"), "card hidden:\n{screen}");
    assert!(
        screen.contains("tester wants approval: rm -rf build") && screen.contains("esc to answer"),
        "the strip names it:\n{screen}"
    );
    assert!(matches!(
        f.app.needs_you()[0].what,
        NeedsYouKind::Approval { id: shown, .. } if shown == id
    ));

    for code in [KeyCode::Enter, KeyCode::Char('p'), KeyCode::Char('y')] {
        f.app.now += Duration::seconds(2);
        let effects = f.app.dispatch_key(key(code));
        assert_eq!(resolved(&effects), None, "{code:?} answered an unseen card");
    }
    assert!(f.app.resolving.is_empty());
}

#[test]
fn esc_meant_for_the_view_closes_it_and_the_card_follows_after_a_pause() {
    let (mut f, _) = settings_over_an_approval();
    f.app.now = at(10_000);
    let effects = f.app.dispatch_key(key(KeyCode::Esc));
    assert_eq!(resolved(&effects), None, "Esc denied an unseen card");
    assert!(f.app.bottom.view_stack.is_empty(), "Esc closed the view");

    // Closing was a keystroke: the card waits out the typing guard, then is drawn.
    f.app.track_approvals();
    assert!(f.app.shown_approval().is_none());
    let wake = f.app.next_tick_in().expect("a wake for the guard's end");
    assert!(wake <= crate::app::view_state::ticks_in(TYPING_GUARD_MS as u64));
    f.app.now = at(10_000 + TYPING_GUARD_MS);
    f.app.dirty = false;
    f.app.track_approvals();
    assert!(f.app.shown_approval().is_some());
    assert!(f.app.dirty, "the card is drawn when it becomes due");
    assert_eq!(resolved(&f.app.dispatch_key(key(KeyCode::Enter))), None);
    read_card(&mut f.app);
    let effects = f.app.dispatch_key(key(KeyCode::Enter));
    assert_eq!(resolved(&effects), Some(ApprovalDecision::Approved));
}

#[test]
fn a_fullscreen_view_or_popup_holds_the_card_back_too() {
    let mut f = fleet();
    f.app.size = (120, 40);
    f.app.input.set_text("/");
    f.app.open_popup(crate::ui::bottom::popup::PopupKind::Slash);
    f.app
        .approvals
        .push(shell_approval(f.tester, "rm -rf build", at(-5_000)));
    read_card(&mut f.app);
    assert!(f.app.shown_approval().is_none(), "a popup covers the slot");
    let effects = f.app.dispatch_key(key(KeyCode::Char('y')));
    assert_eq!(resolved(&effects), None);
    assert_eq!(
        f.app.input.text, "/y",
        "typing still reaches the popup's draft"
    );

    f.app.bottom.popup = None;
    f.app.input.set_text("");
    f.app
        .apply_view_actions(vec![crate::ui::bottom::ViewAction::Push(Box::new(
            crate::ui::views::pager::PagerView::new(crate::ui::views::pager::PagerSource::Report),
        ))]);
    assert!(f.app.bottom.top_view().is_some_and(|v| v.fullscreen()));
    read_card(&mut f.app);
    assert!(
        f.app.shown_approval().is_none(),
        "a fullscreen view covers it"
    );
    assert_eq!(resolved(&f.app.dispatch_key(key(KeyCode::Enter))), None);
}

#[test]
fn answer_keys_wait_until_the_card_has_been_on_screen() {
    let mut f = fleet();
    f.app.now = at(0);
    f.app
        .approvals
        .push(shell_approval(f.tester, "git push --force", at(-5_000)));
    f.app.track_approvals();
    for (i, code) in [
        KeyCode::Char('y'),
        KeyCode::Char('a'),
        KeyCode::Char('p'),
        KeyCode::Char('n'),
        KeyCode::Char('1'),
        KeyCode::Enter,
        KeyCode::Esc,
    ]
    .into_iter()
    .enumerate()
    {
        f.app.now = at(10 * (i as i64 + 1));
        let effects = f.app.dispatch_key(key(code));
        assert_eq!(resolved(&effects), None, "{code:?} counted on a fresh card");
    }
    assert!(
        f.app.input.text.is_empty(),
        "the card still swallows typing"
    );
    f.app.now = at(MIN_VISIBLE_MS - 1);
    assert_eq!(resolved(&f.app.dispatch_key(key(KeyCode::Char('y')))), None);
    f.app.now = at(MIN_VISIBLE_MS);
    assert_eq!(
        resolved(&f.app.dispatch_key(key(KeyCode::Char('y')))),
        Some(ApprovalDecision::Approved)
    );
}

#[test]
fn the_typing_guard_books_a_wake_and_the_card_is_drawn_before_it_answers() {
    let mut f = fleet();
    for id in [f.root, f.implementer, f.researcher, f.tester] {
        node_mut(&mut f.app, id).status = AgentStatus::Ready;
    }
    f.app.connected = false;
    f.app.now = at(0);
    f.app.bottom.approval.note_key(at(0));
    f.app
        .approvals
        .push(shell_approval(f.researcher, "git push --force", at(500)));
    f.app.now = at(600);
    f.app.track_approvals();
    assert!(f.app.due_approval().is_none());
    assert_eq!(
        f.app.next_tick_in(),
        Some(crate::app::view_state::ticks_in(400)),
        "a wake at the guard's end"
    );
    // The loop slept through the wake: the first key after it still cannot answer.
    f.app.now = at(5_000);
    let effects = f.app.dispatch_key(key(KeyCode::Enter));
    assert_eq!(resolved(&effects), None);
    assert!(f.app.shown_approval().is_some());
}

#[test]
fn an_old_approval_learned_mid_typing_waits_for_the_typing_guard() {
    let mut f = fleet();
    f.app.bottom.approval.note_key(at(0));
    f.app.bottom.approval.note_key(at(300));
    f.app.now = at(400);
    // As on attach: `ListApprovals` brings one the daemon asked an hour ago.
    let old = shell_approval(f.tester, "rm -rf /data", at(-3_600_000));
    f.app.reduce(AppEvent::Reply {
        then: Then::Approvals {
            revision: f.app.approvals_revision,
        },
        result: Ok(aether_protocol::Reply::Approvals(vec![old])),
    });
    let effects = f.app.dispatch_key(key(KeyCode::Char('p')));
    assert_eq!(resolved(&effects), None, "typed p wrote a rule");
    assert_eq!(f.app.input.text, "p", "the key stayed typing");
}

#[test]
fn cancel_confirm_never_filters_and_n_means_no() {
    let mut f = fleet();
    f.app.size = (120, 40);
    f.app.run_slash(SlashCommand::Cancel);
    let effects = f.app.dispatch_key(key(KeyCode::Char('n')));
    assert!(!cancels(&effects));
    assert!(f.app.bottom.view_stack.is_empty(), "n closes it");
    assert!(!cancels(&f.app.dispatch_key(key(KeyCode::Enter))));

    f.app.run_slash(SlashCommand::Cancel);
    for code in [KeyCode::Char('x'), KeyCode::Backspace, KeyCode::Char('e')] {
        f.app.dispatch_key(key(code));
    }
    let effects = f.app.dispatch_key(key(KeyCode::Enter));
    assert!(!cancels(&effects), "the cursor stayed on No");
    assert!(f.app.bottom.view_stack.is_empty());

    f.app.run_slash(SlashCommand::Cancel);
    f.app.dispatch_key(key(KeyCode::Up));
    assert!(cancels(&f.app.dispatch_key(key(KeyCode::Enter))));
    f.app.run_slash(SlashCommand::Cancel);
    assert!(cancels(&f.app.dispatch_key(key(KeyCode::Char('y')))));
}

#[test]
fn ctrl_g_waits_for_an_open_view_and_a_focused_board_comes_first() {
    for size in [(80, 30), (140, 40)] {
        let mut f = fleet();
        f.app.size = size;
        f.app.run_slash(SlashCommand::Cancel);
        f.app.dispatch_key(ctrl('g'));
        assert!(!f.app.board.open, "{size:?}: the view keeps the pane");
        assert_eq!(f.app.bottom.view_stack.len(), 1);
        let flash = f.app.live_flash().unwrap_or_default().to_owned();
        assert!(flash.contains("esc closes this view first"), "{flash}");
        let screen = crate::testing::render_to(&f.app, size.0, size.1);
        assert!(screen.contains("end it"), "confirm visible:\n{screen}");
    }

    // However it got there, a focused board takes keys before a view.
    let mut f = fleet();
    f.app.size = (140, 40);
    f.app.run_slash(SlashCommand::Cancel);
    f.app.board.open = true;
    f.app.board.focus = BoardFocus::Board;
    f.app.dispatch_key(key(KeyCode::Up));
    let effects = f.app.dispatch_key(key(KeyCode::Enter));
    assert!(!cancels(&effects), "{effects:?}");
    assert_eq!(
        f.app.bottom.view_stack.len(),
        1,
        "the confirm saw neither key"
    );
}

#[test]
fn a_paste_counts_as_typing_and_never_lands_in_a_hidden_composer() {
    let mut f = fleet();
    f.app.now = at(0);
    f.app.dispatch_paste("hello");
    assert_eq!(f.app.input.text, "hello");
    // Arriving right after the paste, the approval waits for idle.
    f.app
        .approvals
        .push(shell_approval(f.tester, "make", at(100)));
    f.app.now = at(100);
    f.app.track_approvals();
    assert!(f.app.due_approval().is_none(), "the paste started a burst");

    f.app.now = at(5_000);
    read_card(&mut f.app);
    assert!(f.app.shown_approval().is_some());
    f.app.dispatch_paste(" world");
    assert_eq!(f.app.input.text, "hello", "the card hides the composer");

    let mut f = fleet();
    f.app.size = (140, 40);
    f.app.board.open = true;
    f.app.board.focus = BoardFocus::Board;
    f.app.dispatch_paste("stray");
    assert!(f.app.input.text.is_empty(), "the board has focus");
}

#[test]
fn a_paste_with_the_find_bar_open_searches() {
    let mut f = fleet();
    f.app.bottom.find = Some(FindBar::default());
    f.app.dispatch_paste("needle\n");
    assert_eq!(
        f.app.bottom.find.as_ref().map(|bar| bar.query.as_str()),
        Some("needle ")
    );
    assert!(f.app.input.text.is_empty());
}

#[test]
fn voice_keys_wait_for_open_views_and_popups() {
    let mut f = fleet();
    f.app.voice.available = true;
    f.app.size = (120, 40);
    f.app.run_slash(SlashCommand::Settings);
    for code in [KeyCode::F(4), KeyCode::F(5)] {
        let effects = f.app.dispatch_key(key(code));
        assert!(
            !effects.iter().any(|e| matches!(e, Effect::Voice(_))),
            "{code:?}: {effects:?}"
        );
    }
    f.app.bottom.view_stack.clear();
    f.app.open_find();
    let effects = f.app.dispatch_key(key(KeyCode::F(4)));
    assert!(!effects.iter().any(|e| matches!(e, Effect::Voice(_))));
}

#[test]
fn resolved_approvals_leave_the_deferred_set() {
    let mut f = fleet();
    let request = shell_approval(f.tester, "make", at(-1_000));
    let id = request.id;
    f.app.approvals.push(request);
    f.app.dispatch_key(ctrl('c'));
    assert!(f.app.bottom.approval.deferred.contains(&id));
    f.app.reduce(AppEvent::Reply {
        then: Then::ApprovalResolved(id),
        result: Ok(aether_protocol::Reply::Ok),
    });
    assert!(f.app.bottom.approval.deferred.is_empty());
    assert!(f.app.bottom.approval.first_seen.is_empty());
}

#[test]
fn ctrl_c_on_a_draft_keeps_the_pasted_text_in_history() {
    let mut f = fleet();
    let long = "x".repeat(2_000);
    f.app.dispatch_paste(&long);
    assert!(
        f.app.input.text.starts_with("[Pasted"),
        "{}",
        f.app.input.text
    );
    f.app.dispatch_key(ctrl('c'));
    assert!(f.app.input.text.is_empty());
    assert_eq!(f.app.input.history().last(), Some(&long));
    assert!(f.app.input.pastes.is_empty());
}

#[test]
fn late_palette_reply_keys_never_answer_a_visible_approval_or_change_the_draft() {
    use crossterm::event::{KeyEvent, KeyModifiers};
    for bel in [false, true] {
        let mut f = fleet();
        f.app.size = (120, 40);
        f.app.board.open = false;
        f.app
            .approvals
            .push(shell_approval(f.tester, "rm -rf build", at(-5000)));
        read_card(&mut f.app);
        f.app.now += Duration::seconds(5);
        assert!(f.app.shown_approval().is_some());
        let mut keys = Vec::new();
        for query in ["10;rgb:d0d0/d0d0/d0d0", "11;rgb:1c1c/1c1c/1c1c"] {
            keys.push(KeyEvent::new(KeyCode::Char(']'), KeyModifiers::ALT));
            keys.extend(query.chars().map(|c| key(KeyCode::Char(c))));
            keys.push(if bel {
                ctrl('g')
            } else {
                KeyEvent::new(KeyCode::Char('\\'), KeyModifiers::ALT)
            });
        }
        for event in keys {
            assert!(
                f.app.dispatch_key(event).is_empty(),
                "reply key {event:?} had an effect"
            );
        }
        assert!(f.app.input.text.is_empty());
        assert!(f.app.resolving.is_empty());
        assert_eq!(
            resolved(&f.app.dispatch_key(key(KeyCode::Char('y')))),
            Some(ApprovalDecision::Approved)
        );
    }
}

fn dcs_reply_keys() -> Vec<crossterm::event::KeyEvent> {
    use crossterm::event::{KeyEvent, KeyModifiers};
    let mut keys = Vec::new();
    // Crossterm turns ESC P into Alt+Shift+P and ESC \ into Alt+\.
    for payload in ["1$r0;48;2;1;2;3m", "1+r524742=31", "0+r524742"] {
        keys.push(KeyEvent::new(
            KeyCode::Char('P'),
            KeyModifiers::ALT | KeyModifiers::SHIFT,
        ));
        keys.extend(payload.chars().map(|c| key(KeyCode::Char(c))));
        keys.push(KeyEvent::new(KeyCode::Char('\\'), KeyModifiers::ALT));
    }
    keys
}

#[test]
fn late_truecolor_dcs_replies_never_reach_the_draft() {
    let mut f = fleet();
    f.app.board.open = false;
    for c in "draft".chars() {
        f.app.dispatch_key(key(KeyCode::Char(c)));
    }
    for event in dcs_reply_keys() {
        assert!(
            f.app.dispatch_key(event).is_empty(),
            "reply key {event:?} had an effect"
        );
    }
    assert_eq!(f.app.input.text, "draft");
    f.app.dispatch_key(key(KeyCode::Char('!')));
    assert_eq!(f.app.input.text, "draft!");
}

#[test]
fn late_truecolor_dcs_replies_never_answer_a_visible_approval() {
    let mut f = fleet();
    f.app.size = (120, 40);
    f.app.board.open = false;
    f.app
        .approvals
        .push(shell_approval(f.tester, "rm -rf build", at(-5000)));
    read_card(&mut f.app);
    f.app.now += Duration::seconds(5);
    assert!(f.app.shown_approval().is_some());
    for event in dcs_reply_keys() {
        assert!(
            f.app.dispatch_key(event).is_empty(),
            "reply key {event:?} had an effect"
        );
    }
    assert!(f.app.input.text.is_empty());
    assert!(f.app.resolving.is_empty());
    assert_eq!(
        resolved(&f.app.dispatch_key(key(KeyCode::Char('y')))),
        Some(ApprovalDecision::Approved)
    );
}

#[test]
fn an_approval_must_fit_and_resizing_restarts_the_visible_timer() {
    for height in 3..=12 {
        for answer in ['y', 'a', 'p', '1'] {
            let mut f = fleet();
            f.app.size = (80, height);
            f.app.board.open = false;
            f.app
                .approvals
                .push(shell_approval(f.tester, "rm -rf important", at(-5000)));
            read_card(&mut f.app);
            assert!(
                f.app.shown_approval().is_none(),
                "80x{height} cannot show the full card"
            );
            assert_eq!(
                resolved(&f.app.dispatch_key(key(KeyCode::Char(answer)))),
                None
            );
            assert_eq!(resolved(&f.app.dispatch_key(key(KeyCode::Enter))), None);
            assert!(!f.app.needs_you().is_empty());
            assert!(f.app.bottom.approval.shown.is_none());
        }
    }
    let mut f = fleet();
    f.app.size = (80, 40);
    f.app.board.open = false;
    f.app
        .approvals
        .push(shell_approval(f.tester, "rm -rf important", at(-5000)));
    read_card(&mut f.app);
    assert!(f.app.shown_approval().is_some());
    f.app.reduce(AppEvent::Resize(80, 8));
    assert!(f.app.bottom.approval.shown.is_none());
    let text = crate::testing::render_to(&f.app, 80, 8);
    assert!(text.contains("Terminal too short"), "{text}");
    f.app.reduce(AppEvent::Resize(80, 40));
    assert!(f.app.shown_approval().is_some());
    assert_eq!(resolved(&f.app.dispatch_key(key(KeyCode::Char('p')))), None);
    f.app.now += Duration::milliseconds(MIN_VISIBLE_MS + 1);
    assert_eq!(
        resolved(&f.app.dispatch_key(key(KeyCode::Char('p')))),
        Some(ApprovalDecision::Approved)
    );
}
