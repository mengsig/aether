use aether_core::agent::AgentStatusKind;
use aether_protocol::Command;
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

use crate::app::approval_flow::fixtures::{at, fleet, read_card, shell_approval};
use crate::app::{App, Effect};
use crate::commands::SlashCommand;
use crate::keys::KeyAction;
use crate::ui::bottom::popup::{MentionTab, Popup, PopupKind};
use crate::ui::views::ViewKind;

fn rebind(app: &mut App, action: KeyAction, chord: &str) {
    app.keys.assign(action, chord.parse().unwrap()).unwrap();
}

fn plain(code: KeyCode) -> KeyEvent {
    KeyEvent::new(code, KeyModifiers::NONE)
}

fn unmatched_mention() -> App {
    let mut app = crate::testing::app_with_transcript(&[]);
    app.input.set_text("hello @no_matching_agent");
    let mut popup = Popup::new(PopupKind::Mention(MentionTab::Agents));
    popup.query = "no_matching_agent".into();
    app.bottom.popup = Some(popup);
    app
}

fn assert_sent(app: &App, effects: &[Effect]) {
    assert_eq!(effects.len(), 1, "{effects:?}");
    assert!(matches!(
        &effects[0],
        Effect::Request {
            command: Command::SendMessage { to, content, .. }, ..
        } if Some(*to) == app.selected && content == "hello @no_matching_agent"
    ));
    assert!(app.input.text.is_empty());
}

#[test]
fn unmatched_mention_uses_rebound_send_after_old_enter_is_ignored() {
    let mut app = unmatched_mention();
    rebind(&mut app, KeyAction::Send, "f6");
    assert!(app.dispatch_key(plain(KeyCode::Enter)).is_empty());
    assert_eq!(app.input.text, "hello @no_matching_agent");
    let effects = app.dispatch_key(plain(KeyCode::F(6)));
    assert_sent(&app, &effects);
}

#[test]
fn unmatched_mention_uses_rebound_send_while_popup_is_open() {
    let mut app = unmatched_mention();
    rebind(&mut app, KeyAction::Send, "f6");
    let effects = app.dispatch_key(plain(KeyCode::F(6)));
    assert_sent(&app, &effects);
    assert!(app.bottom.popup.is_none());
}

#[test]
fn unmatched_mention_still_sends_on_default_enter() {
    let mut app = unmatched_mention();
    let effects = app.dispatch_key(plain(KeyCode::Enter));
    assert_sent(&app, &effects);
}

#[test]
fn enter_activates_a_slash_row_when_send_is_rebound() {
    let mut app = crate::testing::app_with_transcript(&[]);
    rebind(&mut app, KeyAction::Send, "f6");
    app.dispatch_key(plain(KeyCode::Char('/')));
    for c in "settings".chars() {
        app.dispatch_key(plain(KeyCode::Char(c)));
    }
    assert!(app.dispatch_key(plain(KeyCode::Enter)).is_empty());
    assert_eq!(
        app.bottom.top_view().and_then(|view| view.kind()),
        Some(ViewKind::Settings)
    );
    assert!(app.input.text.is_empty());
    assert!(app.bottom.popup.is_none());
}

#[test]
fn typing_theme_and_enter_opens_the_theme_list_not_settings() {
    let mut app = crate::testing::app_with_transcript(&[]);
    app.dispatch_key(plain(KeyCode::Char('/')));
    for c in "theme".chars() {
        app.dispatch_key(plain(KeyCode::Char(c)));
    }
    assert!(app.dispatch_key(plain(KeyCode::Enter)).is_empty());
    assert_eq!(
        app.bottom.top_view().and_then(|view| view.kind()),
        Some(ViewKind::Theme)
    );
}

#[test]
fn rebound_history_repeats_before_local_modified_enter_handling() {
    let mut app = crate::testing::app_with_transcript(&[]);
    app.input.history = vec!["older".into(), "newer".into()];
    app.input.set_text("draft");
    rebind(&mut app, KeyAction::Newline, "f2");
    rebind(&mut app, KeyAction::History, "alt+enter");
    let chord = KeyEvent::new(KeyCode::Enter, KeyModifiers::ALT);
    assert!(app.dispatch_key(chord).is_empty());
    assert_eq!(app.input.text, "newer");
    assert!(app.dispatch_key(chord).is_empty());
    assert_eq!(app.input.text, "older");
    assert_eq!(app.bottom.popup.as_ref().unwrap().cursor, 1);
    assert!(app.dispatch_key(plain(KeyCode::Enter)).is_empty());
    assert!(app.bottom.popup.is_none());
    assert_eq!(app.input.text, "older");
}

#[test]
fn modified_global_chords_pass_through_find_and_history() {
    for history in [false, true] {
        for (chord, event) in [
            (
                "alt+l",
                KeyEvent::new(KeyCode::Char('l'), KeyModifiers::ALT),
            ),
            ("alt+up", KeyEvent::new(KeyCode::Up, KeyModifiers::ALT)),
            (
                "ctrl+u",
                KeyEvent::new(KeyCode::Char('u'), KeyModifiers::CONTROL),
            ),
            ("shift+up", KeyEvent::new(KeyCode::Up, KeyModifiers::SHIFT)),
            (
                "alt+enter",
                KeyEvent::new(KeyCode::Enter, KeyModifiers::ALT),
            ),
            (
                "alt+backspace",
                KeyEvent::new(KeyCode::Backspace, KeyModifiers::ALT),
            ),
            (
                "ctrl+alt+u",
                KeyEvent::new(
                    KeyCode::Char('u'),
                    KeyModifiers::CONTROL | KeyModifiers::ALT,
                ),
            ),
        ] {
            let mut app = crate::testing::app_with_transcript(&[]);
            app.input.history = vec!["query older".into(), "query newer".into()];
            app.input.set_text("draft");
            rebind(&mut app, KeyAction::Parent, "f2");
            rebind(&mut app, KeyAction::Newline, "f3");
            rebind(&mut app, KeyAction::EraseWord, "f6");
            rebind(&mut app, KeyAction::EraseStart, "f7");
            rebind(&mut app, KeyAction::LineUp, "f8");
            rebind(&mut app, KeyAction::Details, chord);
            if history {
                app.open_popup(PopupKind::History);
            } else {
                app.open_find();
            }
            for c in "query".chars() {
                app.dispatch_key(plain(KeyCode::Char(c)));
            }
            let draft = app.input.text.clone();
            assert!(app.dispatch_key(event).is_empty());
            assert!(app.details, "history={history}, chord={chord}");
            assert_eq!(app.input.text, draft);
            if history {
                let popup = app.bottom.popup.as_ref().unwrap();
                assert_eq!(popup.query, "query");
                assert_eq!(popup.cursor, 0);
            } else {
                assert_eq!(app.bottom.find.as_ref().unwrap().query, "query");
            }
        }
    }
}

#[test]
fn finished_agent_queue_notice_uses_rebound_unqueue_in_both_glyph_modes() {
    for ascii in [false, true] {
        let mut app = crate::testing::app_with_transcript(&[]);
        app.theme = crate::ui::theme::tokens::Theme::resolve(crate::ui::theme::tokens::ThemeSpec {
            ascii,
            ..app.theme.spec()
        });
        rebind(&mut app, KeyAction::Unqueue, "alt+left");
        let agent = app.selected.unwrap();
        app.transcripts.entry(agent).or_default().queue = vec!["later".into()];
        assert!(
            app.composer_turn_ended(agent, AgentStatusKind::Completed, 1)
                .is_empty()
        );
        let flash = app.live_flash().unwrap();
        let chord = if ascii { "alt+left" } else { "alt+←" };
        assert!(
            flash.contains(&format!("{chord} pulls them back")),
            "{flash}"
        );
        assert!(!flash.contains("shift+left"));
        assert_eq!(app.transcripts[&agent].queue, ["later"]);
        app.dispatch_key(KeyEvent::new(KeyCode::Left, KeyModifiers::ALT));
        assert_eq!(app.input.text, "later");
        assert!(app.transcripts[&agent].queue.is_empty());
    }
}

fn modal_chords() -> [(&'static str, KeyEvent); 7] {
    [
        ("f6", plain(KeyCode::F(6))),
        (
            "shift+enter",
            KeyEvent::new(KeyCode::Enter, KeyModifiers::SHIFT),
        ),
        ("shift+up", KeyEvent::new(KeyCode::Up, KeyModifiers::SHIFT)),
        ("shift+tab", plain(KeyCode::BackTab)),
        (
            "shift+tab",
            KeyEvent::new(KeyCode::Tab, KeyModifiers::SHIFT),
        ),
        (
            "alt+enter",
            KeyEvent::new(KeyCode::Enter, KeyModifiers::ALT),
        ),
        (
            "ctrl+l",
            KeyEvent::new(KeyCode::Char('l'), KeyModifiers::CONTROL),
        ),
    ]
}

#[test]
fn rebound_global_chords_on_a_readable_card_never_answer_an_approval() {
    for (chord, event) in modal_chords() {
        let mut f = fleet();
        rebind(&mut f.app, KeyAction::Newline, "f7");
        rebind(&mut f.app, KeyAction::LineUp, "f8");
        rebind(&mut f.app, KeyAction::Details, chord);
        let request = shell_approval(f.tester, "rm -rf build", at(-5_000));
        let id = request.id;
        f.app.approvals.push(request);
        read_card(&mut f.app);
        assert!(f.app.bottom.approval.answerable(id, f.app.now));

        assert!(f.app.dispatch_key(event).is_empty(), "{chord}");
        assert!(f.app.details, "{chord}");
        assert!(f.app.resolving.is_empty(), "{chord} answered the request");
        assert_eq!(f.app.shown_approval().unwrap().id, id);
        assert_eq!(f.app.bottom.approval.cursor_for(id), 0);
        assert!(f.app.input.text.is_empty());
        f.app
            .dispatch_key(KeyEvent::new(KeyCode::Char('t'), KeyModifiers::CONTROL));
        assert!(f.app.details, "old binding still fired");

        let effects = f.app.dispatch_key(plain(KeyCode::Enter));
        assert!(
            matches!(effects.as_slice(), [Effect::Request {
            command: Command::ResolveApproval { id: answered, .. }, ..
        }] if *answered == id),
            "plain Enter must still answer: {effects:?}"
        );
    }
}

#[test]
fn rebound_global_chords_in_settings_never_activate_or_move_a_row() {
    for (chord, event) in modal_chords() {
        let mut app = crate::testing::app_with_transcript(&[]);
        rebind(&mut app, KeyAction::Newline, "f7");
        rebind(&mut app, KeyAction::LineUp, "f8");
        rebind(&mut app, KeyAction::Details, chord);
        app.run_slash(SlashCommand::Settings);
        app.dispatch_key(plain(KeyCode::Down));
        let reduced = app.motion.reduced();

        assert!(app.dispatch_key(event).is_empty());
        assert!(app.details, "{chord}");
        assert_eq!(app.motion.reduced(), reduced, "{chord} activated Motion");
        assert_eq!(app.bottom.view_stack.len(), 1, "{chord}");
        assert_eq!(
            app.bottom.top_view().unwrap().kind(),
            Some(ViewKind::Settings)
        );
        assert!(
            app.dispatch_key(KeyEvent::new(KeyCode::Char('t'), KeyModifiers::CONTROL))
                .is_empty()
        );
        assert!(app.details, "old binding still fired");

        assert!(app.dispatch_key(plain(KeyCode::Enter)).is_empty());
        assert_ne!(app.motion.reduced(), reduced, "{chord} moved off Motion");
        assert_eq!(app.bottom.view_stack.len(), 1);
    }
}

#[test]
fn configured_global_chord_wins_over_a_view_specific_chord() {
    let mut app = crate::testing::app_with_transcript(&[]);
    rebind(&mut app, KeyAction::Details, "ctrl+p");
    app.run_slash(SlashCommand::Workspace(None));
    assert!(
        app.dispatch_key(KeyEvent::new(KeyCode::Char('p'), KeyModifiers::CONTROL))
            .is_empty()
    );
    assert!(app.details);
    assert_eq!(
        app.bottom.top_view().unwrap().kind(),
        Some(ViewKind::Workspace)
    );
}

#[test]
fn shifted_punctuation_still_types_in_a_view_filter() {
    let mut app = crate::testing::app_with_transcript(&[]);
    app.run_slash(SlashCommand::Workspace(None));
    assert!(
        app.dispatch_key(KeyEvent::new(KeyCode::Char('?'), KeyModifiers::SHIFT))
            .is_empty()
    );
    assert!(!app.bottom.shortcuts);
    let screen = crate::testing::render_to(&app, 120, 40);
    assert!(screen.contains("filter ?"), "{screen}");
}

#[test]
fn key_capture_keeps_configured_global_chords_and_esc_cancels_it() {
    for (chord, event) in modal_chords() {
        let mut app = crate::testing::app_with_transcript(&[]);
        rebind(&mut app, KeyAction::Newline, "f7");
        rebind(&mut app, KeyAction::LineUp, "f8");
        rebind(&mut app, KeyAction::Details, chord);
        app.open_view(ViewKind::Keys);
        app.dispatch_key(plain(KeyCode::Enter));

        assert!(app.dispatch_key(event).is_empty());
        assert!(!app.details, "{chord} escaped capture");
        assert!(!app.keys.customised(KeyAction::Agents));
        assert_eq!(app.bottom.view_stack.len(), 1);
        let screen = crate::testing::render_to(&app, 120, 40);
        assert!(screen.contains("already used by details"), "{screen}");
        app.dispatch_key(plain(KeyCode::Esc));
        assert_eq!(app.bottom.top_view().unwrap().kind(), Some(ViewKind::Keys));
        assert!(app.dispatch_key(event).is_empty());
        assert!(app.details, "{chord} swallowed outside capture");
        app.dispatch_key(plain(KeyCode::Esc));
        assert!(app.bottom.view_stack.is_empty());
    }
}
