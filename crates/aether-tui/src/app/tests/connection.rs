//! connection reducer regressions.
use super::*;

/// The flash is painted in the hint row, not just stored.
fn assert_flash_painted(app: &App, text: &str) {
    let frame = crate::testing::render_to(app, 160, 30);
    assert!(frame.contains(text), "{frame}");
}

#[test]
fn status_since_comes_from_snapshots_on_late_attach_events_and_reconnect() {
    let (mut app, id) = app_with_agent();
    let at = app.now - chrono::Duration::minutes(4);
    let mut row = agent(id, "worker");
    row.status = AgentStatus::Waiting {
        condition: aether_core::agent::WaitCondition::Inbox,
    };
    row.status_since = Some(at);
    row.updated_at = app.now;
    let apply = |app: &mut App, row: Agent| {
        app.reduce(AppEvent::Reply {
            then: Then::Tree,
            result: Ok(Reply::Tree(tree(row))),
        });
    };
    apply(&mut app, row.clone());
    assert_eq!(app.selected_agent().unwrap().status_since, Some(at));
    app.reduce(AppEvent::Frame(SubscriptionFrame::Event(Box::new(event(
        99,
        id,
        EventKind::AgentStarted { turn: 2 },
    )))));
    assert_eq!(
        app.selected_agent().unwrap().status_since,
        Some(at),
        "events must not invent an observation timestamp"
    );
    assert!(matches!(
        app.selected_agent().unwrap().status,
        AgentStatus::Waiting { .. }
    ));
    row.status = AgentStatus::Running;
    row.status_since = Some(app.now);
    apply(&mut app, row.clone());
    assert_eq!(app.selected_agent().unwrap().status_since, row.status_since);
    row.status_since = Some(app.now + chrono::Duration::seconds(1));
    apply(&mut app, row.clone());
    assert_eq!(
        app.selected_agent().unwrap().status_since,
        row.status_since,
        "same-kind snapshot after unseen ABA replaces the timestamp"
    );
    app.reduce(AppEvent::Disconnected { because: None });
    app.reduce(AppEvent::Connected);
    apply(&mut app, row.clone());
    assert_eq!(app.selected_agent().unwrap().status_since, row.status_since);
    row.status_since = None;
    apply(&mut app, row);
    assert_eq!(
        app.selected_agent().unwrap().status_since,
        None,
        "legacy snapshot stays unknown"
    );
}

#[test]
fn tree_refresh_is_coalesced_while_in_flight() {
    let (mut app, id) = app_with_agent();
    let effects = app.reduce(AppEvent::Frame(SubscriptionFrame::Event(Box::new(event(
        2,
        id,
        EventKind::AgentStarted { turn: 1 },
    )))));
    assert_eq!(
        effects
            .iter()
            .filter(|e| matches!(
                e,
                Effect::Request {
                    then: Then::Tree,
                    ..
                }
            ))
            .count(),
        1
    );
    let effects = app.reduce(AppEvent::Frame(SubscriptionFrame::Event(Box::new(event(
        3,
        id,
        EventKind::AgentReady,
    )))));
    assert!(
        effects.iter().all(|e| !matches!(
            e,
            Effect::Request {
                then: Then::Tree,
                ..
            }
        )),
        "second refresh waits"
    );
    let effects = app.reduce(AppEvent::Reply {
        then: Then::Tree,
        result: Ok(Reply::Tree(tree(agent(id, "root")))),
    });
    assert!(
        effects.iter().any(|e| matches!(
            e,
            Effect::Request {
                then: Then::Tree,
                ..
            }
        )),
        "stale flag triggers one more"
    );
}

#[test]
fn disconnect_and_reconnect_resubscribe_from_last_seq() {
    let (mut app, id) = app_with_agent();
    app.reduce(AppEvent::Frame(SubscriptionFrame::Event(Box::new(event(
        42,
        id,
        EventKind::AgentReady,
    )))));
    app.reduce(AppEvent::Disconnected { because: None });
    assert!(!app.connected);
    let effects = app.reduce(AppEvent::Connected);
    assert!(matches!(effects[0], Effect::Subscribe { after: Some(42) }));
}

/// A disconnect notice with a reason used to drop the
/// word "reconnecting" even though the runner keeps reconnecting either
/// way - reading as if it had given up.
#[test]
fn a_disconnect_notice_with_a_reason_still_says_reconnecting() {
    let (mut app, _) = app_with_agent();
    app.reduce(AppEvent::Disconnected {
        because: Some("the daemon closed the connection".into()),
    });
    let notice = app.live_flash().expect("a notice was set");
    assert!(notice.contains("reconnecting"), "{notice}");
    assert!(
        notice.contains("the daemon closed the connection"),
        "{notice}"
    );
}

/// A disconnected reply must say so rather than vanish silently.
#[test]
fn a_disconnected_reply_produces_a_visible_notice() {
    let (mut app, _) = app_with_agent();
    app.reduce(AppEvent::Reply {
        then: Then::Restarted {
            from: AgentId::new(),
        },
        result: Err(ClientError::Disconnected),
    });
    let notice = app.live_flash().expect("a notice was set");
    assert!(notice.contains("did not complete"), "{notice}");
    assert!(notice.contains("daemon connection dropped"), "{notice}");
    assert_flash_painted(&app, "did not complete");
}

/// One disconnect can fail many in-flight replies; only the first gets a
/// notice so the line does not repeat once per request.
///
/// `spawn_request` (runner.rs) sends `Reply` then its own `Disconnected`
/// for *every* failed request, so a real outage looks like R,D,R,D,...
/// rather than R,R,D; replay that order here. Each `Disconnected` also
/// posts its own unconditional "connection lost" notice, deduped the
/// same way by `disconnect_notice_shown` - covered
/// separately below - so the count of interest here is the per-reply
/// line only.
#[test]
fn many_replies_failed_by_one_disconnect_notice_only_once() {
    let (mut app, _) = app_with_agent();
    // Whether this failed reply, on its own, flashed the per-reply line.
    let fail_reply = |app: &mut App, then: Then| {
        app.flash = None;
        app.reduce(AppEvent::Reply {
            then,
            result: Err(ClientError::Disconnected),
        });
        let noticed = app
            .live_flash()
            .is_some_and(|n| n.contains("did not complete"));
        app.reduce(AppEvent::Disconnected { because: None });
        noticed
    };
    assert!(fail_reply(
        &mut app,
        Then::Restarted {
            from: AgentId::new()
        }
    ));
    assert!(
        !fail_reply(&mut app, Then::Woken),
        "a second reply failed by the same outage must stay quiet, even \
         though the runner fires a Disconnected after each failed reply"
    );
    // Only reconnecting ends the outage; a fresh one then reports itself.
    app.reduce(AppEvent::Connected);
    assert!(fail_reply(&mut app, Then::Woken));
}

/// `Effect::Copy`/`Effect::SetMouse` failures used to be `tracing::warn!`
/// only; they must reach the person too.
#[test]
fn an_effect_failure_produces_a_visible_notice() {
    let (mut app, _) = app_with_agent();
    app.reduce(AppEvent::EffectFailed {
        action: "clipboard copy".into(),
        error: "no such device".into(),
    });
    assert_eq!(
        app.live_flash(),
        Some("clipboard copy failed: no such device")
    );
    assert_flash_painted(&app, "clipboard copy failed: no such device");
}

/// A failed external spawn used to collapse to `Err(_) => false`, losing
/// the real error.
#[test]
fn a_failed_external_spawn_shows_its_error_in_the_notice() {
    let (mut app, _) = app_with_agent();
    app.reduce(AppEvent::ExternalFinished {
        label: "aether login".into(),
        error: Some("exited with exit status: 1".into()),
    });
    let notice = app.live_flash().expect("a notice was set");
    assert!(notice.contains("aether login"), "{notice}");
    assert!(notice.contains("exited with exit status: 1"), "{notice}");
    assert_flash_painted(&app, "exited with exit status: 1");
}

/// SPEC §4.2: Ctrl+C clears a draft first, then quits at once with no
/// double-press arm; `q` never quits.
#[test]
fn quit_keys() {
    let (mut app, _) = app_with_agent();
    app.reduce(key(KeyCode::Char('d')));
    assert!(app.reduce(ctrl_key(KeyCode::Char('c'))).is_empty());
    assert!(app.input.text.is_empty(), "first Ctrl+C clears the draft");
    let effects = app.reduce(ctrl_key(KeyCode::Char('c')));
    assert_eq!(effects, vec![Effect::Quit]);
    assert!(
        app.exit_message
            .as_deref()
            .is_some_and(|m| m.contains("Agents keep running")),
        "{:?}",
        app.exit_message
    );
    let (mut app, _) = app_with_agent();
    let effects = app.reduce(key(KeyCode::Char('q')));
    assert!(!effects.contains(&Effect::Quit));
    assert_eq!(app.input.text, "q", "q types into the composer");
}

/// Ticks arrive in uneven steps (a frame, a spinner beat, an idle beat), so
/// the periodic status refresh must not depend on landing on a multiple.
#[test]
fn the_status_refresh_keeps_firing_after_an_uneven_tick() {
    let mut app = App::new(TuiOptions::default());
    app.reduce(AppEvent::Connected);
    let status_requests = |effects: Vec<Effect>| {
        effects
            .iter()
            .filter(|e| {
                matches!(
                    e,
                    Effect::Request {
                        command: Command::Status,
                        ..
                    }
                )
            })
            .count()
    };
    let mut requests = status_requests(app.reduce(AppEvent::Tick { frames: 4 }));
    for _ in 0..80 {
        let sent = status_requests(app.reduce(AppEvent::Tick { frames: 15 }));
        // Answered at once, as a healthy daemon does: the next poll waits on it.
        if sent > 0 {
            crate::testing::answer_status_poll(&mut app);
        }
        requests += sent;
    }
    // 1 204 frames is a little over 20 s: one refresh every 2 s.
    assert_eq!(app.tick, 1_204);
    assert_eq!(requests, 10, "the refresh stopped after an uneven step");
}

/// A daemon that stalls for a minute was sent a poll every 2 s, and each one
/// later failed with its own timeout notice. One poll waits; its end frees the next.
#[test]
fn a_slow_status_poll_is_not_stacked_and_its_failure_frees_the_next() {
    let mut app = App::new(TuiOptions::default());
    app.reduce(AppEvent::Connected);
    let polled = |effects: Vec<Effect>| {
        effects.iter().any(|e| {
            matches!(
                e,
                Effect::Request {
                    command: Command::Status,
                    then: Then::StatusPoll,
                }
            )
        })
    };
    assert!(polled(app.reduce(AppEvent::Tick { frames: 120 })));
    for _ in 0..30 {
        assert!(
            !polled(app.reduce(AppEvent::Tick { frames: 120 })),
            "polled again while the last poll was unanswered"
        );
    }
    app.reduce(AppEvent::Reply {
        then: Then::StatusPoll,
        result: Err(ClientError::TimedOut {
            command: "status".into(),
            timeout: std::time::Duration::from_secs(60),
            host: None,
        }),
    });
    assert!(polled(app.reduce(AppEvent::Tick { frames: 120 })));
}

/// Startup, login and agent creation ask for status too; their answers say
/// nothing about the periodic poll still out, so it must keep waiting.
#[test]
fn another_status_answer_does_not_free_a_pending_poll() {
    let mut app = App::new(TuiOptions::default());
    app.reduce(AppEvent::Connected);
    let polls = |effects: Vec<Effect>| {
        effects
            .iter()
            .filter(|e| {
                matches!(
                    e,
                    Effect::Request {
                        then: Then::StatusPoll,
                        ..
                    }
                )
            })
            .count()
    };
    assert_eq!(polls(app.reduce(AppEvent::Tick { frames: 120 })), 1);
    crate::testing::apply_status_with_suspended(&mut app, 0);
    assert_eq!(
        polls(app.reduce(AppEvent::Tick { frames: 120 })),
        0,
        "the startup answer freed the periodic poll"
    );
    crate::testing::answer_status_poll(&mut app);
    assert_eq!(polls(app.reduce(AppEvent::Tick { frames: 120 })), 1);
}

/// "an older build" is only a fair guess when the handshake went unanswered;
/// a daemon that greeted this client is this build, just slow.
#[test]
fn a_slow_reply_is_not_blamed_on_an_older_build() {
    let theme = crate::ui::theme::tokens::Theme::default();
    let timed_out = |command: &str| ClientError::TimedOut {
        command: command.into(),
        timeout: std::time::Duration::from_secs(60),
        host: None,
    };
    let slow = explain(&theme, &timed_out("status"), Then::Status, "aether");
    assert!(slow.contains("connected but slow"), "{slow}");
    assert!(!slow.contains("older build"), "{slow}");
    let silent = explain(&theme, &timed_out("hello"), Then::Status, "aether");
    assert!(silent.contains("older build"), "{silent}");
}

/// When the client could see that its host is overloaded, the hint says so
/// instead of guessing, and still keeps stopping the daemon as the last step.
#[test]
fn a_timeout_on_an_overloaded_host_names_the_host_not_a_guess() {
    let theme = crate::ui::theme::tokens::Theme::default();
    let error = ClientError::TimedOut {
        command: "status".into(),
        timeout: std::time::Duration::from_secs(60),
        host: Some("this host is overloaded: load 45.0 on 12 cores".into()),
    };
    let said = error.to_string();
    assert!(
        said.contains("load 45.0 on 12 cores, the likely cause"),
        "{said}"
    );
    let hint = explain(&theme, &error, Then::Status, "aether");
    assert!(hint.contains("this host is overloaded"), "{hint}");
    assert!(!hint.contains("older build"), "{hint}");
    let (ease, stop) = (hint.find("ease the load"), hint.find("aether stop"));
    assert!(
        ease.is_some() && ease < stop,
        "stopping is the last resort: {hint}"
    );
}
