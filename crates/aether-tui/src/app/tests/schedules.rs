//! schedules reducer regressions.
use super::*;
use crate::ui::views::ViewKind;

#[test]
fn schedules_view_requests_history_and_confirms_cancellation() {
    use aether_core::schedule::ScheduleAction;
    let (mut app, target) = app_with_agent();
    for c in "/schedules".chars() {
        app.reduce(key(KeyCode::Char(c)));
    }
    let effects = app.reduce(key(KeyCode::Enter));
    assert!(app.bottom.showing(ViewKind::Schedules));
    assert!(effects.iter().any(|effect| matches!(
        effect,
        Effect::Request {
            command: Command::ListSchedules { .. },
            ..
        }
    )));
    let job = crate::testing::schedule(target);
    let effects = app.on_reply(Then::Schedules, Ok(Reply::Schedules(vec![job.clone()])));
    assert!(effects.iter().any(|effect| matches!(effect, Effect::Request { command: Command::ScheduleHistory { id, .. }, .. } if *id == job.id)));
    let effects = app.reduce(key(KeyCode::Char('p')));
    assert!(effects.iter().any(|effect| matches!(effect, Effect::Request { command: Command::UpdateSchedule { id, action: ScheduleAction::Pause , idempotency_key: None, }, .. } if *id == job.id)));

    let armed = "c again to cancel";
    assert!(app.reduce(key(KeyCode::Char('c'))).is_empty());
    let text = crate::testing::render_to(&app, 120, 40);
    assert!(text.contains(armed), "the first c must ask:\n{text}");
    app.reduce(key(KeyCode::PageDown));
    let text = crate::testing::render_to(&app, 120, 40);
    assert!(!text.contains(armed), "any other key disarms:\n{text}");
    assert!(app.reduce(key(KeyCode::Char('c'))).is_empty());
    let effects = app.reduce(key(KeyCode::Char('c')));
    assert!(effects.iter().any(|effect| matches!(effect, Effect::Request { command: Command::UpdateSchedule { id, action: ScheduleAction::Cancel , idempotency_key: None, }, .. } if *id == job.id)));
}

#[test]
fn late_schedule_replies_feed_the_view_without_moving_another() {
    let (mut app, target) = app_with_agent();
    app.open_view(ViewKind::Usage);
    let job = crate::testing::schedule(target);
    let effects = app.on_reply(Then::Schedules, Ok(Reply::Schedules(vec![job.clone()])));
    assert_eq!(app.bottom.view_stack.len(), 1, "the open view stays alone");
    assert!(!app.bottom.showing(ViewKind::Schedules));
    // The `/schedules` view shows the first job's history until its cursor moves.
    assert!(matches!(
        &effects[..],
        [Effect::Request { command: Command::ScheduleHistory { id, .. }, .. }] if *id == job.id
    ));
    app.on_reply(
        Then::ScheduleHistory(job.id),
        Ok(Reply::ScheduleHistory(vec![])),
    );
    assert_eq!(app.schedules.history, Some((job.id, vec![])));
    // History for a job no longer listed is dropped.
    let gone = crate::testing::schedule(target);
    app.on_reply(
        Then::ScheduleHistory(gone.id),
        Ok(Reply::ScheduleHistory(vec![])),
    );
    assert_eq!(
        app.schedules.history.as_ref().map(|(id, _)| *id),
        Some(job.id)
    );
}

#[test]
fn stale_schedule_history_errors_leave_the_selected_job_unchanged() {
    let (mut app, target) = app_with_agent();
    app.open_view(ViewKind::Schedules);
    app.schedules.jobs = vec![crate::testing::schedule(target)];
    app.schedules.error = Some("current job error".into());
    let stale = crate::testing::schedule(target);
    let effects = app.on_reply(
        Then::ScheduleHistory(stale.id),
        Err(ClientError::Malformed("stale history error".into())),
    );
    assert!(effects.is_empty());
    assert_eq!(app.schedules.error.as_deref(), Some("current job error"));
}

/// `/providers` opens the usage view: it asks on open, and `r` asks again
/// once the first answer is in.
#[test]
fn the_providers_view_asks_on_open_and_on_r() {
    let (mut app, _) = app_with_agent();
    for c in "/providers".chars() {
        app.reduce(key(KeyCode::Char(c)));
    }
    app.dirty = false;
    let effects = app.reduce(key(KeyCode::Enter));
    assert_eq!(app.bottom.view_stack.len(), 1);
    assert!(
        effects.iter().any(|e| matches!(
            e,
            Effect::Request {
                command: Command::Providers,
                then: Then::Usage
            }
        )),
        "opening the view asks the daemon: {effects:?}"
    );
    assert!(app.dirty, "the render gate must see the view open");

    app.reduce(AppEvent::Reply {
        then: Then::Usage,
        result: Ok(Reply::Providers(vec![])),
    });
    assert!(app.providers.is_empty());
    app.dirty = false;
    let effects = app.reduce(key(KeyCode::Char('r')));
    assert!(
        effects.iter().any(|e| matches!(
            e,
            Effect::Request {
                command: Command::Providers,
                ..
            }
        )),
        "r refreshes: {effects:?}"
    );
    assert!(app.dirty);
    app.reduce(key(KeyCode::Esc));
    assert!(app.bottom.view_stack.is_empty());
}

/// R6b: a delivery names its schedule once the TUI lists it; Ctrl+T keeps the id.
#[test]
fn a_schedule_delivery_names_its_schedule_and_details_keep_the_id() {
    use aether_core::inbox::{InboxItem, InboxKind};
    let (mut app, target) = app_with_agent();
    let mut job = crate::testing::schedule(target);
    job.name = "bf4-tick".into();
    let delivered = |seq: u64| {
        let item = InboxItem {
            id: aether_core::id::InboxItemId::new(),
            agent_id: target,
            seq,
            kind: InboxKind::ExternalTrigger {
                source: format!("schedule:{}", job.id),
                payload: serde_json::json!("hello tick"),
                operator: false,
            },
            delivered_at: now(),
        };
        let event = event(seq, target, EventKind::InboxDelivered { item });
        AppEvent::Frame(SubscriptionFrame::Event(Box::new(event)))
    };
    let lists = |effects: &[Effect]| {
        effects.iter().any(|effect| {
            matches!(
                effect,
                Effect::Request {
                    command: Command::ListSchedules { .. },
                    ..
                }
            )
        })
    };
    let mut cache = crate::ui::transcript::ViewCache::default();
    let mut screen = |app: &App| crate::testing::render_with(app, &mut cache, 120, 30);

    assert!(
        lists(&app.reduce(delivered(10))),
        "an unlisted schedule is looked up"
    );
    let unknown = format!("Trigger from schedule:{} · hello tick", job.id);
    assert!(screen(&app).contains(&unknown), "{}", screen(&app));

    app.on_reply(Then::Schedules, Ok(Reply::Schedules(vec![job.clone()])));
    let named = screen(&app);
    assert!(
        named.contains("Trigger from schedule bf4-tick · hello tick"),
        "{named}"
    );
    assert!(!named.contains(&job.id.to_string()), "{named}");

    app.reduce(ctrl_key(KeyCode::Char('t')));
    let details = screen(&app);
    let with_id = format!("Trigger from schedule bf4-tick ({}) · hello tick", job.id);
    assert!(details.contains(&with_id), "{details}");

    assert!(
        !lists(&app.reduce(delivered(11))),
        "a listed schedule asks nothing"
    );

    app.reduce(ctrl_key(KeyCode::Char('t')));
    let mut renamed = job.clone();
    renamed.name = "renamed-tick".into();
    app.on_reply(Then::Schedules, Ok(Reply::Schedules(vec![renamed])));
    let text = screen(&app);
    assert!(
        text.contains("Trigger from schedule renamed-tick · hello tick"),
        "{text}"
    );
    assert!(!text.contains("bf4-tick"), "{text}");
    app.on_reply(Then::Schedules, Ok(Reply::Schedules(vec![])));
    assert!(screen(&app).contains(&unknown));
}

fn schedule_delivery(target: AgentId, schedule: aether_core::id::ScheduleId, seq: u64) -> Event {
    use aether_core::inbox::{InboxItem, InboxKind};
    event(
        seq,
        target,
        EventKind::InboxDelivered {
            item: InboxItem {
                id: aether_core::id::InboxItemId::new(),
                agent_id: target,
                seq,
                kind: InboxKind::ExternalTrigger {
                    source: format!("schedule:{schedule}"),
                    payload: serde_json::Value::Null,
                    operator: false,
                },
                delivered_at: now(),
            },
        },
    )
}

fn schedule_lists(effects: &[Effect]) -> usize {
    effects
        .iter()
        .filter(|effect| {
            matches!(
                effect,
                Effect::Request {
                    command: Command::ListSchedules { .. },
                    ..
                }
            )
        })
        .count()
}

#[test]
fn missing_schedule_lookups_are_not_repeated_after_replies_or_errors() {
    for failed in [false, true] {
        let (mut app, target) = app_with_agent();
        let schedule = crate::testing::schedule(target).id;
        assert_eq!(
            schedule_lists(&app.on_event(schedule_delivery(target, schedule, 10))),
            1
        );
        for seq in 11..30 {
            assert_eq!(
                schedule_lists(&app.on_event(schedule_delivery(target, schedule, seq))),
                0
            );
        }
        let reply = if failed {
            Err(ClientError::Disconnected)
        } else {
            Ok(Reply::Schedules(vec![]))
        };
        app.on_reply(Then::Schedules, reply);
        if failed {
            assert!(app.schedules.error.is_some());
        }
        for seq in 30..60 {
            assert_eq!(
                schedule_lists(&app.on_event(schedule_delivery(target, schedule, seq))),
                0,
                "a missing name must not poll on every delivery (failed={failed})"
            );
        }
        let next = crate::testing::schedule(target).id;
        assert_eq!(
            schedule_lists(&app.on_event(schedule_delivery(target, next, 60))),
            1
        );
    }
}

#[test]
fn backfill_remembers_every_missing_schedule_lookup() {
    let (mut app, target) = app_with_agent();
    let first = crate::testing::schedule(target).id;
    let second = crate::testing::schedule(target).id;
    let history: Vec<_> = (1..100)
        .map(|seq| schedule_delivery(target, if seq % 2 == 0 { first } else { second }, seq))
        .collect();
    for pass in 0..2 {
        app.transcripts.get_mut(&target).unwrap().loaded = false;
        app.select(target);
        let effects = app.on_reply(Then::Backfill(target), Ok(Reply::Events(history.clone())));
        assert_eq!(schedule_lists(&effects), usize::from(pass == 0));
        app.on_reply(Then::Schedules, Ok(Reply::Schedules(vec![])));
    }
    for (seq, schedule) in [(100, first), (101, second)] {
        assert_eq!(
            schedule_lists(&app.on_event(schedule_delivery(target, schedule, seq))),
            0
        );
    }
}
