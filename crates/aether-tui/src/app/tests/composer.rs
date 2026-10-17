//! composer reducer regressions.
use super::*;

#[test]
fn horizontal_edits_preserve_whole_graphemes() {
    for cluster in ["e\u{301}", "👍🏽", "🇩🇰", "👩‍👩‍👧‍👦"] {
        let mut input = InputState::default();
        input.set_text(format!("{cluster}x"));
        input.left();
        assert_eq!(input.cursor, cluster.len());
        input.left();
        assert_eq!(input.cursor, 0);
        input.right();
        assert_eq!(input.cursor, cluster.len());
        input.backspace();
        assert_eq!(input.text, "x");
        assert_eq!(input.cursor, 0);
        input.set_text(format!("{cluster}x"));
        input.cursor = 0;
        input.delete();
        assert_eq!(input.text, "x");
    }
}

#[test]
fn insertion_and_vertical_moves_keep_the_cursor_on_a_grapheme_boundary() {
    let mut input = InputState::default();
    input.set_text("\u{301}");
    input.cursor = 0;
    input.insert_char('e');
    assert_eq!(input.cursor, input.text.len());
    input.set_text("中x\ne\u{301}yz");
    input.cursor = "中".len();
    assert!(input.down());
    assert_eq!(input.cursor, "中x\ne\u{301}".len());
    assert!(input.up());
    assert_eq!(input.cursor, "中".len());
}

#[test]
fn capital_f_is_text_in_the_composer() {
    let (mut app, _) = app_with_agent();
    app.reduce(shift_key(KeyCode::Char('F')));
    assert_eq!(app.input.text, "F");
    app.input.set_text("one ");
    app.reduce(shift_key(KeyCode::Char('F')));
    assert_eq!(app.input.text, "one F");
}

#[test]
fn modified_enter_inserts_a_newline_without_submitting() {
    for modifiers in [KeyModifiers::SHIFT, KeyModifiers::ALT] {
        let (mut app, _) = app_with_agent();
        app.input.set_text("first line");
        let effects = app.reduce(AppEvent::Key(KeyEvent::new(KeyCode::Enter, modifiers)));
        assert!(effects.is_empty());
        assert_eq!(app.input.text, "first line\n");
        let effects = app.reduce(key(KeyCode::Enter));
        assert!(effects.iter().any(|effect| matches!(
            effect,
            Effect::Request {
                command: Command::SendMessage { .. },
                ..
            }
        )));
    }
}

#[test]
fn input_editor_handles_lines_and_history() {
    let mut input = InputState::default();
    input.insert_str("ab");
    input.insert_char('\n');
    input.insert_str("cd");
    assert_eq!(input.line_count(), 2);
    assert_eq!(input.cursor_line_col(), (1, 2));
    assert!(input.up());
    assert_eq!(input.cursor_line_col(), (0, 2));
    assert!(!input.up());
    input.home();
    assert_eq!(input.cursor, 0);
    input.end();
    assert_eq!(input.cursor, 2);
    let text = input.take();
    assert_eq!(text, "ab\ncd");
    assert!(input.text.is_empty());
    assert!(input.history_prev());
    assert_eq!(input.text, "ab\ncd");
    assert!(input.history_next());
    assert!(input.text.is_empty());
}

/// Every printable is text, never a shortcut; only `?` on an empty draft
/// opens the shortcuts panel instead (SPEC §4.1).
#[test]
fn composer_printable_characters_never_dispatch_actions() {
    let characters: Vec<char> = (' '..='~').chain("é中🦀\u{301}".chars()).collect();
    for draft in ["", "draft "] {
        for modifiers in [KeyModifiers::NONE, KeyModifiers::SHIFT] {
            let mut app = composer_fixture(false);
            for &c in &characters {
                app.input.set_text(draft);
                app.bottom.popup = None;
                app.bottom.shortcuts = false;
                let details = app.details;
                let notice = app.live_flash().map(str::to_owned);
                let effects = app.reduce(AppEvent::Key(KeyEvent::new(KeyCode::Char(c), modifiers)));
                assert!(
                    effects.is_empty(),
                    "{c:?} {draft:?} {modifiers:?}: {effects:?}"
                );
                if c == '?' && draft.is_empty() {
                    assert!(app.bottom.shortcuts, "{modifiers:?}");
                    assert!(app.input.text.is_empty());
                } else {
                    assert_eq!(app.input.text, format!("{draft}{c}"));
                }
                assert!(app.bottom.view_stack.is_empty());
                assert!(!app.board.focused());
                assert_eq!(app.details, details);
                assert_eq!(app.live_flash(), notice.as_deref());
                assert!(app.resolving.is_empty());
                assert!(!app.quit_requested);
            }
        }
    }
}

#[test]
fn composer_printables_stay_text_after_send_and_clear() {
    for clear in [false, true] {
        let mut app = composer_fixture(false);
        app.input.set_text("message");
        let effects = if clear {
            app.reduce(ctrl_key(KeyCode::Char('c')))
        } else {
            app.reduce(key(KeyCode::Enter))
        };
        assert_eq!(effects.is_empty(), clear);
        assert!(app.input.text.is_empty());
        for c in "ieomyna".chars() {
            assert!(app.reduce(key(KeyCode::Char(c))).is_empty());
            assert_eq!(app.input.text, c.to_string());
            assert!(app.bottom.view_stack.is_empty());
            assert!(app.resolving.is_empty());
            app.input.set_text("");
        }
    }
}

#[test]
fn input_editor_word_motions_and_kills() {
    let mut input = InputState::default();
    input.set_text("one two  three");
    input.word_left();
    assert_eq!(&input.text[input.cursor..], "three");
    input.delete_word_back();
    assert_eq!(input.text, "one three");
    assert_eq!(input.cursor, 4);
    input.home();
    input.word_right();
    assert_eq!(input.cursor, 3);
    input.word_right();
    assert_eq!(input.cursor, 9);
    input.word_left();
    input.kill_to_line_end();
    assert_eq!(input.text, "one ");
    input.set_text("ab\ncd");
    input.kill_to_line_start();
    assert_eq!(input.text, "ab\n");
    assert_eq!(input.cursor, 3);
}

#[test]
fn operator_provenance_is_metadata_not_the_typed_message() {
    for from in [
        "user",
        "user:operator",
        "user:tcp:[::1]:54321#connection",
        "user:local_socket#connection",
    ] {
        let text =
            format!("[inbox 3] Message from {from}: [authenticated operator instruction]\nhello");
        assert_eq!(
            split_inbox_text(&text),
            vec![InboxEntry::User("hello".into())]
        );
    }
    let body = "[authenticated operator instruction]";
    let text = format!("[inbox 3] Message from user:\n{body}");
    assert_eq!(split_inbox_text(&text), vec![InboxEntry::User(body.into())]);
}

#[test]
fn inbox_text_splits_into_user_bubbles_and_notes() {
    let text = "[inbox 3] Message from user:\nhello there\n[inbox 4] Task tsk_x completed: done\n[inbox 5] Message from agt_y:\nhi";
    let parts = split_inbox_text(text);
    assert_eq!(
        parts,
        vec![
            InboxEntry::User("hello there".into()),
            InboxEntry::Note("Task tsk_x completed: done".into()),
            InboxEntry::Note("message from agt_y: hi".into()),
        ]
    );
    assert_eq!(
        split_inbox_text("plain"),
        vec![InboxEntry::User("plain".into())]
    );
    assert!(split_inbox_text("   ").is_empty());
}

pub(super) fn sent_messages(effects: &[Effect]) -> Vec<(AgentId, String)> {
    effects
        .iter()
        .filter_map(|effect| match effect {
            Effect::Request {
                command: Command::SendMessage { to, content, .. },
                ..
            } => Some((*to, content.clone())),
            _ => None,
        })
        .collect()
}

#[test]
fn unknown_slash_commands_reach_the_viewed_agent_verbatim() {
    let (mut app, id) = app_with_agent();
    app.input.set_text("/answer 3 yes");
    let effects = app.reduce(key(KeyCode::Enter));
    assert_eq!(sent_messages(&effects), [(id, "/answer 3 yes".to_owned())]);
    assert_eq!(
        app.live_flash(),
        Some("/answer is not an Aether command; sent to root")
    );
    app.flash = None;
    for line in [
        "/a 3 no",
        "/approve 7",
        "/reject 7 too broad",
        "/merge 12",
        "/archon status",
        "/Answer 1  Yes",
    ] {
        app.input.set_text(line);
        let effects = app.reduce(key(KeyCode::Enter));
        assert_eq!(sent_messages(&effects), [(id, line.to_owned())], "{line}");
        assert_eq!(app.live_flash(), None, "{line} repeats the notice");
        assert!(app.input.text.is_empty(), "{line} stays in the draft");
    }
}

#[test]
fn unknown_slash_commands_without_a_live_agent_stay_unknown() {
    let mut app = app_with_status(TuiOptions::default());
    app.input.set_text("/answer 3 yes");
    let effects = app.reduce(key(KeyCode::Enter));
    assert!(sent_messages(&effects).is_empty(), "{effects:?}");
    assert!(
        app.live_flash()
            .unwrap()
            .contains("unknown command /answer")
    );

    let (mut app, id) = app_with_agent();
    app.tree.nodes.get_mut(&id).unwrap().agent.status = AgentStatus::Completed { summary: None };
    app.input.set_text("/answer 3 yes");
    let effects = app.reduce(key(KeyCode::Enter));
    assert!(sent_messages(&effects).is_empty(), "{effects:?}");
    assert!(
        app.live_flash()
            .unwrap()
            .contains("unknown command /answer")
    );
}

#[test]
fn registry_commands_and_view_modes_still_run_with_an_agent_in_view() {
    let (app, _) = app_with_agent();
    assert_eq!(
        app.parse_input("/help"),
        InputAction::Command(SlashCommand::Help)
    );
    assert!(matches!(
        app.parse_input("/focus"),
        InputAction::Command(SlashCommand::ViewMode(Some(_)))
    ));
    assert_eq!(
        app.parse_input("  /answer 3 yes "),
        InputAction::Message("/answer 3 yes".into())
    );
}

#[test]
fn typed_unknown_slash_commands_pass_the_popup_to_the_agent() {
    for line in ["/answer 3 yes", "/archon", "/merge"] {
        let (mut app, id) = app_with_agent();
        for c in line.chars() {
            app.reduce(key(KeyCode::Char(c)));
        }
        assert_eq!(app.input.text, line);
        let effects = app.reduce(key(KeyCode::Enter));
        assert_eq!(sent_messages(&effects), [(id, line.to_owned())], "{line}");
    }
}
