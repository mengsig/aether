use super::*;

#[test]
fn default_off_plain_mode_and_f4_exact_key() {
    let mut app = app();
    assert!(!app.voice.enabled);
    assert!(
        question(&mut app, 1)
            .iter()
            .all(|e| !matches!(e, Effect::Voice(_)))
    );
    for modifiers in [
        KeyModifiers::SHIFT,
        KeyModifiers::ALT,
        KeyModifiers::CONTROL,
    ] {
        let effects = app.reduce(AppEvent::Key(KeyEvent::new(KeyCode::F(4), modifiers)));
        assert!(effects.iter().all(|e| !matches!(e, Effect::Voice(_))));
        app.voice.recovered = Some((app.selected.unwrap(), "must not insert".into()));
        let before = app.input.text.clone();
        app.reduce(AppEvent::Key(KeyEvent::new(KeyCode::F(5), modifiers)));
        assert_eq!(app.input.text, before);
    }
    app.voice.available = false;
    assert!(app.voice_action(Action::Start).is_empty());
    assert!(app.live_flash().unwrap().contains("interactive"));
}

#[test]
fn f4_records_and_inserts_unicode_draft_without_sending_in_ascii_or_unicode_ui() {
    for ascii in [false, true] {
        let mut app = app();
        app.refresh_ui_theme(crate::ui::theme::ThemeSpec {
            ascii,
            ..app.theme.spec()
        });
        app.input.set_text("Existing draft: ");
        let id = record(&mut app);
        assert!(key(&mut app, KeyCode::F(4)).contains(&Effect::Voice(VoiceEffect::Stop(id))));
        let effects = app.reduce(AppEvent::Voice(Reply::Recorded {
            id,
            result: Ok("名前 café --help".into()),
        }));
        assert!(effects.is_empty(), "must not send any daemon message");
        assert_eq!(app.input.text, "Existing draft: 名前 café --help");
        assert!(app.live_flash().unwrap().contains("review before sending"));
    }
}

#[test]
fn recording_is_visible_while_composing() {
    let mut app = app();
    record(&mut app);
    let frame = render_to(&app, 120, 36);
    assert!(
        frame.contains("Listening") || frame.contains("voice recording F4 stop"),
        "{frame}"
    );
}

#[test]
fn fullscreen_viewer_retains_voice_until_chat_is_visible() {
    let mut app = app();
    app.input.set_text("kept ");
    let id = record(&mut app);
    app.viewer.open = true;
    app.viewer.fullscreen = true;
    app.reduce(AppEvent::Voice(Reply::Recorded {
        id,
        result: Ok("dictated".into()),
    }));
    assert_eq!(app.input.text, "kept ");
    assert!(app.voice.recovered.is_some());
    assert!(app.voice_action(Action::Insert).is_empty());
    assert_eq!(app.input.text, "kept ");
    assert!(app.voice.recovered.is_some());
    app.viewer.fullscreen = false;
    app.voice_action(Action::Insert);
    assert_eq!(app.input.text, "kept dictated");
    assert!(app.voice.recovered.is_none());
}

#[test]
fn typed_stop_retains_recoverable_draft_and_explicit_insert_never_overwrites() {
    let mut app = app();
    let id = record(&mut app);
    app.input.set_text("/voice stop");
    assert!(key(&mut app, KeyCode::Enter).contains(&Effect::Voice(VoiceEffect::Stop(id))));
    app.reduce(AppEvent::Voice(Reply::Recorded {
        id,
        result: Ok("dictated café".into()),
    }));
    assert!(app.input.text.is_empty());
    assert!(app.live_flash().unwrap().contains("/voice insert"));
    app.input.set_text("new draft ending");
    app.input.cursor = "new draft ".len();
    assert!(key(&mut app, KeyCode::F(5)).is_empty());
    assert_eq!(app.input.text, "new draft dictated caféending");
    assert!(app.voice.recovered.is_none());
}

#[test]
fn editing_aba_selection_and_late_errors_never_replace_a_new_draft() {
    let mut app = app();
    let original = app.selected.unwrap();
    app.input.set_text("kept");
    let id = record(&mut app);
    app.input.insert_char('x');
    app.input.backspace();
    app.reduce(AppEvent::Voice(Reply::Recorded {
        id,
        result: Ok("late".into()),
    }));
    assert_eq!(app.input.text, "kept");
    assert!(app.voice.recovered.is_some());
    app.voice_action(Action::Cancel);
    let id = record(&mut app);
    app.select(AgentId::new());
    app.select(original);
    app.reduce(AppEvent::Voice(Reply::Recorded {
        id,
        result: Ok("selection changed".into()),
    }));
    assert_eq!(app.input.text, "kept");
    app.select(AgentId::new());
    assert!(app.voice_action(Action::Insert).is_empty());
    assert_eq!(app.input.text, "kept");
    app.select(original);
    let id = record(&mut app);
    app.reduce(AppEvent::Voice(Reply::Recorded {
        id: id - 1,
        result: Err("stale failure".into()),
    }));
    assert!(app.voice.capture.is_some());
    app.reduce(AppEvent::Voice(Reply::Recorded {
        id,
        result: Err("bad model".into()),
    }));
    assert_eq!(app.input.text, "kept");
    assert!(app.live_flash().unwrap().contains("bad model"));
}

#[test]
fn cancel_disconnect_and_off_clear_sensitive_recovery() {
    for action in [Action::Cancel, Action::Off, Action::Mute] {
        let mut app = app();
        app.voice.recovered = Some((app.selected.unwrap(), "sensitive draft".into()));
        app.voice_action(action);
        assert!(app.voice.recovered.is_none());
    }
    let mut app = app();
    let id = record(&mut app);
    let effects = app.reduce(AppEvent::Disconnected { because: None });
    assert!(effects.contains(&Effect::Voice(VoiceEffect::Cancel(id))));
    app.reduce(AppEvent::Voice(Reply::Recorded {
        id,
        result: Ok("must not insert".into()),
    }));
    assert!(app.input.text.is_empty());
    assert!(app.voice.recovered.is_none());
}

#[test]
fn speech_is_new_only_deduplicated_bounded_and_mutable_independently_of_microphone() {
    let mut app = app();
    app.voice.enabled = true;
    app.reduce(AppEvent::Subscribed {
        id: 1,
        last_seq: 10,
    });
    assert!(
        question(&mut app, 9)
            .iter()
            .all(|e| !matches!(e, Effect::Voice(_)))
    );
    let effects = question(&mut app, 11);
    assert!(effects.iter().any(|e| matches!(e, Effect::Voice(VoiceEffect::Synthesize { text, .. }) if text.contains("asks: 名前 café --help; $(nope)"))));
    let speech_id = app.voice.speech.as_ref().unwrap().id;
    question(&mut app, 11);
    assert!(app.voice.queue.is_empty());
    for seq in 12..20 {
        question(&mut app, seq);
    }
    assert_eq!(app.voice.queue.len(), QUEUE_LIMIT);
    assert!(
        app.voice_action(Action::Mute)
            .contains(&Effect::Voice(VoiceEffect::Cancel(speech_id)))
    );
    assert!(app.voice.queue.is_empty());
    app.reduce(AppEvent::Voice(Reply::Played {
        id: speech_id,
        result: Err("cancelled".into()),
    }));
    let id = record(&mut app);
    app.voice_action(Action::Off);
    assert_eq!(
        app.voice.capture.as_ref().unwrap().id,
        id,
        "speech off does not secretly stop explicit recording"
    );
}

#[test]
fn answered_archived_expired_and_disconnected_questions_do_not_start_playback() {
    for mode in 0..4 {
        let mut app = app();
        app.voice.enabled = true;
        question(&mut app, 1);
        let id = app.voice.speech.as_ref().unwrap().id;
        let agent = app.selected.unwrap();
        match mode {
            0 => {
                answer_questions(&mut app, agent);
            }
            1 => {
                event(
                    &mut app,
                    agent,
                    2,
                    EventKind::AgentArchived { archived: true },
                );
            }
            2 => {
                app.tick += 31 * crate::app::TICKS_PER_SECOND;
            }
            _ => {
                app.reduce(AppEvent::Disconnected { because: None });
            }
        }
        let effects = app.reduce(AppEvent::Voice(Reply::Synthesized {
            id,
            result: Err("should not play".into()),
        }));
        assert!(
            effects
                .iter()
                .all(|e| !matches!(e, Effect::Voice(VoiceEffect::Play { .. })))
        );
        assert!(app.voice.speech.is_none());
    }
}

#[test]
fn reconnect_watermark_skips_outage_questions_and_off_wins_over_check_completion() {
    let mut app = app();
    app.voice.enabled = true;
    app.reduce(AppEvent::Subscribed {
        id: 2,
        last_seq: 50,
    });
    assert!(
        question(&mut app, 49)
            .iter()
            .all(|e| !matches!(e, Effect::Voice(_)))
    );
    app.voice_action(Action::On);
    let id = app.voice.checking.unwrap().0;
    app.voice_action(Action::Off);
    app.reduce(AppEvent::Voice(Reply::Checked {
        id,
        result: Ok("checked".into()),
    }));
    assert!(!app.voice.enabled);
    assert!(
        question(&mut app, 51)
            .iter()
            .all(|e| !matches!(e, Effect::Voice(_)))
    );
}

#[test]
fn playing_question_survives_freshness_deadline_but_stops_when_answered() {
    let mut app = app();
    app.voice.enabled = true;
    question(&mut app, 1);
    let id = app.voice.speech.as_ref().unwrap().id;
    app.tick += 20 * crate::app::TICKS_PER_SECOND;
    let config = toml::from_str("ffmpeg = '/unused'").unwrap();
    let effects = app.reduce(AppEvent::Voice(Reply::Synthesized {
        id,
        result: Ok((config, vec![0; 32], 16000)),
    }));
    assert!(
        effects
            .iter()
            .any(|e| matches!(e, Effect::Voice(VoiceEffect::Play { .. })))
    );
    app.tick += 20 * crate::app::TICKS_PER_SECOND;
    assert!(app.voice_reconcile().is_empty());
    assert!(!app.voice.speech.as_ref().unwrap().cancelled);
    let agent = app.selected.unwrap();
    answer_questions(&mut app, agent);
    assert!(
        app.voice_reconcile()
            .contains(&Effect::Voice(VoiceEffect::Cancel(id)))
    );
}
