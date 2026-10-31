#![allow(clippy::unwrap_used)]

use super::*;

#[test]
fn chords_round_trip_and_terminal_aliases_normalise() {
    for raw in [
        "ctrl+k",
        "alt+enter",
        "f2",
        "shift+up",
        "ctrl+alt+left",
        "?",
        "space",
        "shift+tab",
        "alt+backspace",
    ] {
        let chord: KeyChord = raw.parse().unwrap();
        assert_eq!(chord.to_string(), raw);
        assert_eq!(chord.to_string().parse::<KeyChord>().unwrap(), chord);
    }
    for (raw, canonical) in [("Ctrl+I", "tab"), ("ctrl+m", "enter"), ("ctrl+[", "esc")] {
        assert_eq!(raw.parse::<KeyChord>().unwrap().to_string(), canonical);
    }
    for raw in [
        "",
        "ctrl+",
        "cmd+k",
        "ctrl+ctrl+k",
        "f0",
        "f25",
        "ctrl+h",
        "unknown",
    ] {
        assert!(raw.parse::<KeyChord>().is_err(), "{raw}");
    }
    assert_eq!(
        KeyChord::from_event(KeyEvent::new(KeyCode::Char('?'), KeyModifiers::SHIFT)).unwrap(),
        "?".parse().unwrap()
    );
    assert_eq!(
        KeyChord::from_event(KeyEvent::new(KeyCode::BackTab, KeyModifiers::NONE)).unwrap(),
        "shift+tab".parse().unwrap()
    );
}

#[test]
fn conflicts_reserved_and_resets_are_refused_without_mutation() {
    let mut keys = KeyBindings::default();
    let initial = keys.clone();
    for raw in [
        "ctrl+c", "ctrl+d", "esc", "ctrl+[", "ctrl+z", "alt+]", "a", "up",
    ] {
        assert!(
            keys.assign(KeyAction::Details, raw.parse().unwrap())
                .unwrap_err()
                .contains("reserved"),
            "{raw}"
        );
        assert_eq!(keys, initial);
    }
    assert!(
        keys.assign(KeyAction::Details, "ctrl+g".parse().unwrap())
            .unwrap_err()
            .contains("go to the agents")
    );
    keys.assign(KeyAction::Agents, "f2".parse().unwrap())
        .unwrap();
    keys.assign(KeyAction::Details, "ctrl+g".parse().unwrap())
        .unwrap();
    assert!(
        keys.reset(KeyAction::Agents)
            .unwrap_err()
            .contains("details")
    );
    assert!(
        keys.assign(KeyAction::Queue, "ctrl+m".parse().unwrap())
            .unwrap_err()
            .contains("send")
    );
    assert!(
        keys.assign(KeyAction::Send, "ctrl+i".parse().unwrap())
            .unwrap_err()
            .contains("queue")
    );
}

#[test]
fn bad_entries_fall_back_and_collisions_after_fallback_are_rejected() {
    let raw = [
        ("details".into(), "f2".into()),
        ("unknown_action".into(), "f3".into()),
        ("history".into(), "not a chord".into()),
        ("find".into(), toml::Value::Integer(3)),
        ("agents".into(), "ctrl+c".into()),
        ("workspace".into(), "ctrl+g".into()),
    ]
    .into();
    let (keys, notices) = KeyBindings::load(&raw);
    assert!(keys.customised(KeyAction::Details));
    assert!(!keys.customised(KeyAction::Agents));
    assert!(!keys.customised(KeyAction::Workspace));
    for name in ["unknown_action", "history", "find", "agents", "workspace"] {
        assert!(
            notices.iter().any(|n| n.contains(&format!("keys.{name}:"))),
            "{notices:?}"
        );
    }
    let raw = [
        ("agents".into(), "f3".into()),
        ("details".into(), "f3".into()),
    ]
    .into();
    let (keys, notices) = KeyBindings::load(&raw);
    assert_eq!(keys, KeyBindings::default());
    assert_eq!(notices.len(), 2);
}

#[test]
fn preferences_round_trip_only_overrides_and_preserve_bad_entries() {
    let dir = tempfile::tempdir().unwrap();
    let mut keys = KeyBindings::default();
    keys.assign(KeyAction::Details, "f2".parse().unwrap())
        .unwrap();
    let mut prefs = crate::prefs::UiPrefs {
        keys: keys.overrides(),
        ..Default::default()
    };
    prefs.save(dir.path()).unwrap();
    let text = std::fs::read_to_string(dir.path().join("ui.toml")).unwrap();
    assert!(text.contains("[keys]\ndetails = \"f2\""), "{text}");
    assert!(!text.contains("ctrl+g"));
    assert_eq!(crate::prefs::UiPrefs::load(dir.path()).unwrap(), prefs);
    prefs.keys.insert("bad".into(), toml::Value::Integer(3));
    prefs.save(dir.path()).unwrap();
    let loaded = crate::prefs::UiPrefs::load(dir.path()).unwrap();
    assert_eq!(loaded.keys["bad"], toml::Value::Integer(3));
    assert!(KeyBindings::load(&loaded.keys).1[0].contains("keys.bad"));
}

#[test]
fn captured_ascii_chords_round_trip_without_losing_modifiers() {
    for code in (b' '..=b'~').map(|c| KeyCode::Char(char::from(c))).chain([
        KeyCode::Enter,
        KeyCode::Tab,
        KeyCode::BackTab,
        KeyCode::Up,
        KeyCode::PageDown,
        KeyCode::F(2),
    ]) {
        for modifiers in [
            KeyModifiers::NONE,
            KeyModifiers::SHIFT,
            KeyModifiers::CONTROL,
            KeyModifiers::ALT,
            KeyModifiers::CONTROL | KeyModifiers::ALT,
            KeyModifiers::CONTROL | KeyModifiers::SHIFT,
        ] {
            let Ok(chord) = KeyChord::from_event(KeyEvent::new(code, modifiers)) else {
                continue;
            };
            assert_eq!(
                chord.to_string().parse::<KeyChord>().unwrap(),
                chord,
                "{code:?} {modifiers:?}"
            );
        }
    }
    assert_ne!(
        "ctrl+enter".parse::<KeyChord>().unwrap(),
        "enter".parse::<KeyChord>().unwrap()
    );
}

#[test]
fn every_default_chord_resolves_to_exactly_one_action() {
    let keys = KeyBindings::default();
    for (action, chords) in &keys.bindings {
        for chord in chords {
            let event = KeyEvent::new(chord.code, chord.modifiers);
            let matching: Vec<_> = KeyAction::ALL
                .iter()
                .filter(|a| keys.matches(**a, event))
                .copied()
                .collect();
            assert_eq!(matching, [*action]);
        }
    }
}

#[test]
fn clipboard_paste_defaults_and_help_include_ctrl_v_and_keep_alt_v() {
    let keys = KeyBindings::default();
    for modifiers in [KeyModifiers::CONTROL, KeyModifiers::ALT] {
        let event = KeyEvent::new(KeyCode::Char('v'), modifiers);
        assert!(keys.matches(KeyAction::PasteImage, event));
        assert_eq!(
            KeyAction::ALL
                .iter()
                .filter(|a| keys.matches(**a, event))
                .count(),
            1
        );
    }
    let row = crate::commands::KEYS
        .iter()
        .find(|spec| spec.bindings == [KeyAction::PasteImage])
        .unwrap();
    assert_eq!(row.keys, "Ctrl+V / Alt+V");
    assert_eq!(row.live_chord(&keys, false), "Ctrl+V");
    assert_eq!(row.scope, crate::commands::KeyScope::Input);
}

#[test]
fn existing_ctrl_v_overrides_take_precedence_over_the_new_default() {
    for action in [KeyAction::Details, KeyAction::Yank] {
        let raw = [(action.name().into(), "ctrl+v".into())].into();
        let (keys, notices) = KeyBindings::load(&raw);
        assert!(notices.is_empty(), "{notices:?}");
        assert!(keys.matches(
            action,
            KeyEvent::new(KeyCode::Char('v'), KeyModifiers::CONTROL)
        ));
        assert!(!keys.matches(
            KeyAction::PasteImage,
            KeyEvent::new(KeyCode::Char('v'), KeyModifiers::CONTROL)
        ));
        assert!(keys.matches(
            KeyAction::PasteImage,
            KeyEvent::new(KeyCode::Char('v'), KeyModifiers::ALT)
        ));
        assert_eq!(KeyBindings::load(&keys.overrides()).0, keys);
        assert!(!keys.customised(KeyAction::PasteImage));
        assert!(!keys.overrides().contains_key("paste_image"));
        assert_eq!(keys.help_chord(&[KeyAction::PasteImage], true), "Alt+V");
    }
    let raw = [("paste_image".into(), "f6".into())].into();
    let (keys, notices) = KeyBindings::load(&raw);
    assert!(notices.is_empty());
    assert!(keys.matches(
        KeyAction::PasteImage,
        KeyEvent::new(KeyCode::F(6), KeyModifiers::NONE)
    ));
    assert!(!keys.matches(
        KeyAction::PasteImage,
        KeyEvent::new(KeyCode::Char('v'), KeyModifiers::CONTROL)
    ));
    assert!(!keys.matches(
        KeyAction::PasteImage,
        KeyEvent::new(KeyCode::Char('v'), KeyModifiers::ALT)
    ));
}

#[test]
fn an_explicit_paste_image_override_is_still_customised() {
    let raw = [
        ("details".into(), "ctrl+v".into()),
        ("paste_image".into(), "alt+v".into()),
    ]
    .into();
    let (keys, notices) = KeyBindings::load(&raw);
    assert!(notices.is_empty(), "{notices:?}");
    assert!(keys.customised(KeyAction::PasteImage));
    assert_eq!(keys.overrides(), raw);
}

#[test]
fn notices_escape_control_characters_from_bad_file_entries() {
    let raw = [
        ("bad\n\u{1b}[2J".into(), "f2".into()),
        ("details".into(), "bad\n\u{1b}[2J".into()),
    ]
    .into();
    let (keys, notices) = KeyBindings::load(&raw);
    assert_eq!(keys, KeyBindings::default());
    assert_eq!(notices.len(), 2);
    for notice in notices {
        assert!(!notice.chars().any(char::is_control), "{notice:?}");
        assert!(
            notice.contains("\\n") && notice.contains("\\u{1b}"),
            "{notice}"
        );
    }
}

/// #351: what legacy, kitty and kitty-with-Num-Lock terminals decode an Alt
/// chord to must match its binding, defaults and remapped Alt+Shift alike.
#[test]
fn alt_chords_match_in_every_decoded_form() {
    use crossterm::event::KeyEventState;
    let keys = KeyBindings::default();
    let mut alt_defaults = 0;
    for action in KeyAction::ALL {
        for chord in defaults(*action) {
            if !chord.modifiers.contains(KeyModifiers::ALT) {
                continue;
            }
            alt_defaults += 1;
            let event = KeyEvent::new(chord.code, chord.modifiers);
            let mut locked = event;
            locked.state = KeyEventState::NUM_LOCK | KeyEventState::CAPS_LOCK;
            assert!(keys.matches(*action, event), "{chord}");
            assert!(keys.matches(*action, locked), "{chord} with lock keys");
            let mut shifted = event;
            shifted.modifiers.insert(KeyModifiers::SHIFT);
            if matches!(chord.code, KeyCode::Char(c) if c.is_ascii_alphabetic()) {
                assert!(
                    !keys.matches(*action, shifted),
                    "{chord} is not its Shift chord"
                );
            }
        }
    }
    assert_eq!(alt_defaults, 16, "every documented Alt default is checked");

    let raw = [(
        "notifications".to_owned(),
        toml::Value::String("alt+shift+n".into()),
    )];
    let (keys, notices) = KeyBindings::load(&raw.into_iter().collect());
    assert!(notices.is_empty(), "{notices:?}");
    let alt_shift = KeyModifiers::ALT | KeyModifiers::SHIFT;
    // Legacy ESC N decodes as a capital, kitty CSI 110;4u as the base key.
    for event in [
        KeyEvent::new(KeyCode::Char('N'), alt_shift),
        KeyEvent::new(KeyCode::Char('n'), alt_shift),
    ] {
        assert!(keys.matches(KeyAction::Notifications, event), "{event:?}");
    }
    let plain_alt = KeyEvent::new(KeyCode::Char('n'), KeyModifiers::ALT);
    assert!(!keys.matches(KeyAction::Notifications, plain_alt));
}

/// The Esc join asks this before turning Esc then a Ctrl key into Ctrl+Alt.
#[test]
fn binds_answers_for_any_action_only() {
    let ctrl_alt = KeyModifiers::CONTROL | KeyModifiers::ALT;
    let ctrl_alt_u = KeyEvent::new(KeyCode::Char('u'), ctrl_alt);
    let mut keys = KeyBindings::default();
    assert!(keys.binds(KeyEvent::new(KeyCode::Char('n'), KeyModifiers::ALT)));
    assert!(!keys.binds(KeyEvent::new(KeyCode::Char('c'), ctrl_alt)));
    assert!(!keys.binds(ctrl_alt_u));
    keys.assign(KeyAction::Details, "ctrl+alt+u".parse().unwrap())
        .unwrap();
    assert!(keys.binds(ctrl_alt_u));
}
