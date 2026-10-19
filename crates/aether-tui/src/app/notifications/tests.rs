//! The notification center's reducer: what connecting asks for, how live
//! events and a list in flight combine, and what an old daemon shows.

use aether_core::event::{Actor, Event};
use aether_core::id::EventId;
use aether_core::notification::{
    Notification, NotificationAction, NotificationChange, NotificationKind, NotificationPost,
    NotificationState,
};
use aether_core::time::{Timestamp, now};
use aether_protocol::{Command, Reply};

use super::{MAX_KEPT, NotificationCenter};
use crate::app::{App, AppEvent, ClientError, Effect, Then, TuiOptions};

fn post(kind: NotificationKind, title: &str) -> NotificationPost {
    NotificationPost {
        kind,
        title: title.into(),
        body: String::new(),
        key: None,
        link: None,
        actions: vec![NotificationAction {
            id: "1".into(),
            label: "Ship it".into(),
        }],
        free_text: false,
    }
}

/// A notification created `age` seconds ago.
fn notification(kind: NotificationKind, title: &str, age: i64) -> Notification {
    let at: Timestamp = now() - chrono::Duration::seconds(age);
    Notification::new("archon".into(), post(kind, title), at)
}

fn event(change: NotificationChange, record: &Notification, seq: u64) -> Event {
    let draft = record.draft(change, Actor::user(), None).expect("draft");
    Event::from_draft(draft, EventId::new(), seq, now())
}

fn served_app() -> App {
    let mut app = App::new(TuiOptions::default());
    app.size = (120, 40);
    app.notifications.set_served(true);
    app
}

fn lists(effects: &[Effect]) -> usize {
    effects
        .iter()
        .filter(|effect| {
            matches!(
                effect,
                Effect::Request {
                    command: Command::ListNotifications { .. },
                    then: Then::Notifications,
                }
            )
        })
        .count()
}

#[test]
fn connecting_lists_the_center_only_when_the_daemon_serves_it() {
    let mut old = App::new(TuiOptions::default());
    let effects = old.reduce(AppEvent::Connected);
    assert_eq!(lists(&effects), 0, "an old daemon is never asked");
    let mut app = served_app();
    let effects = app.reduce(AppEvent::Connected);
    assert_eq!(lists(&effects), 1);
}

#[test]
fn an_old_daemon_shows_no_indicator_and_ignores_stray_events() {
    let mut app = served_app();
    app.on_event(event(
        NotificationChange::Posted,
        &notification(NotificationKind::Question, "Ship?", 1),
        1,
    ));
    assert_eq!(app.notifications.unread(), 1);
    app.notifications.set_served(false);
    assert_eq!(
        app.notifications.unread(),
        0,
        "nothing lingers from the last daemon"
    );
    app.on_event(event(
        NotificationChange::Posted,
        &notification(NotificationKind::Blocker, "Stuck", 1),
        2,
    ));
    assert!(app.notifications.items().is_empty());
    let screen = crate::testing::render_to(&app, 120, 40);
    assert!(!screen.contains("unread"), "{screen}");
}

#[test]
fn a_list_in_flight_neither_drops_nor_rolls_back_live_changes() {
    let mut app = served_app();
    app.reduce(AppEvent::Connected);
    let older = notification(NotificationKind::Info, "Older", 60);
    let mut read = older.clone();
    assert!(read.mark_read(now()));
    let fresh = notification(NotificationKind::Question, "Fresh", 1);
    // Both changes land after the list was asked for, so it predates them.
    app.on_event(event(NotificationChange::Read, &read, 1));
    app.on_event(event(NotificationChange::Posted, &fresh, 2));
    app.on_reply(
        Then::Notifications,
        Ok(Reply::Notifications(vec![older.clone()])),
    );
    let held: Vec<_> = app
        .notifications
        .items()
        .iter()
        .map(|n| n.title.as_str())
        .collect();
    assert_eq!(held, ["Fresh", "Older"], "newest first, the live post kept");
    let older_now = app.notifications.get(older.id).expect("listed");
    assert_eq!(
        older_now.state,
        NotificationState::Read,
        "the list rolled back a read"
    );
}

#[test]
fn a_list_drops_what_the_daemon_no_longer_holds() {
    let mut app = served_app();
    let gone = notification(NotificationKind::Outcome, "Pruned", 30);
    app.on_event(event(NotificationChange::Posted, &gone, 1));
    app.reduce(AppEvent::Connected);
    app.on_reply(Then::Notifications, Ok(Reply::Notifications(Vec::new())));
    assert!(app.notifications.get(gone.id).is_none());
    assert_eq!(app.notifications.unread(), 0);
}

#[test]
fn resolved_history_never_crowds_out_an_older_open_question() {
    let mut app = served_app();
    let effects = app.reduce(AppEvent::Connected);
    let asked = effects.iter().find_map(|effect| match effect {
        Effect::Request {
            command: Command::ListNotifications { states, .. },
            then: Then::Notifications,
        } => Some(states.clone()),
        _ => None,
    });
    let open = [NotificationState::Unread, NotificationState::Read];
    assert_eq!(asked.as_deref(), Some(open.as_slice()), "open ones first");
    let waiting = notification(NotificationKind::Question, "Waiting", 50_000);
    let next = app.on_reply(
        Then::Notifications,
        Ok(Reply::Notifications(vec![waiting.clone()])),
    );
    assert!(matches!(
        next.as_slice(),
        [Effect::Request { command: Command::ListNotifications { states, .. }, then: Then::ResolvedNotifications }]
            if states == &[NotificationState::Resolved]
    ));
    let history = (0..MAX_KEPT as i64)
        .map(|age| {
            let mut n = notification(NotificationKind::Outcome, "done", age);
            n.resolve(now());
            n
        })
        .collect();
    app.on_reply(
        Then::ResolvedNotifications,
        Ok(Reply::Notifications(history)),
    );
    assert_eq!(app.notifications.items().len(), MAX_KEPT);
    assert!(
        app.notifications.get(waiting.id).is_some(),
        "the question survives the cap"
    );
    assert_eq!(app.notifications.urgent_unread(), 1);
}

#[test]
fn an_urgent_post_alerts_an_unfocused_terminal_and_nothing_else_does() {
    let mut app = served_app();
    app.terminal_focused = false;
    let question = notification(NotificationKind::Question, "Merge #231?", 1);
    let effects = app.on_event(event(NotificationChange::Posted, &question, 1));
    assert!(effects.iter().any(|e| matches!(e, Effect::Bell)));
    assert!(
        effects
            .iter()
            .any(|e| matches!(e, Effect::Notify { body, .. } if body == "Merge #231?"))
    );
    let info = notification(NotificationKind::Info, "FYI", 1);
    let effects = app.on_event(event(NotificationChange::Posted, &info, 2));
    assert!(!effects.iter().any(|e| matches!(e, Effect::Bell)));
    let effects = app.on_event(event(NotificationChange::Updated, &question, 3));
    assert!(
        !effects.iter().any(|e| matches!(e, Effect::Bell)),
        "only a new post rings"
    );
    app.terminal_focused = true;
    let blocker = notification(NotificationKind::Blocker, "Stuck", 1);
    let effects = app.on_event(event(NotificationChange::Posted, &blocker, 4));
    assert!(
        !effects.iter().any(|e| matches!(e, Effect::Bell)),
        "focused: the indicator is enough"
    );
    assert_eq!(app.notifications.urgent_unread(), 2);
}

#[test]
fn an_older_record_never_replaces_a_newer_one() {
    let mut center = NotificationCenter::default();
    let first = notification(NotificationKind::Question, "Q", 5);
    let mut resolved = first.clone();
    assert!(resolved.resolve(now()));
    assert!(center.apply(resolved.clone()));
    assert!(!center.apply(first), "an earlier version arrived late");
    assert!(
        !center.apply(resolved),
        "the same version twice changes nothing"
    );
    assert_eq!(center.items().len(), 1);
    assert_eq!(center.items()[0].state, NotificationState::Resolved);
}

#[test]
fn grouping_puts_unread_then_read_then_resolved_newest_first() {
    let mut center = NotificationCenter::default();
    let mut read = notification(NotificationKind::Info, "read", 1);
    read.mark_read(now());
    let mut resolved = notification(NotificationKind::Info, "resolved", 0);
    resolved.resolve(now());
    for n in [
        read,
        resolved,
        notification(NotificationKind::Info, "old unread", 50),
        notification(NotificationKind::Info, "new unread", 2),
    ] {
        center.apply(n);
    }
    let order: Vec<_> = center.grouped().iter().map(|n| n.title.as_str()).collect();
    assert_eq!(order, ["new unread", "old unread", "read", "resolved"]);
}

#[test]
fn past_the_cap_the_oldest_resolved_go_first() {
    let mut center = NotificationCenter::default();
    let mut oldest_resolved = notification(NotificationKind::Outcome, "settled", 10_000);
    oldest_resolved.resolve(now());
    center.apply(oldest_resolved.clone());
    let oldest_unread = notification(NotificationKind::Question, "waiting", 20_000);
    center.apply(oldest_unread.clone());
    for age in 0..MAX_KEPT as i64 {
        center.apply(notification(NotificationKind::Info, "news", age));
    }
    assert_eq!(center.items().len(), MAX_KEPT);
    assert!(
        center.get(oldest_resolved.id).is_none(),
        "a settled one goes first"
    );
    assert!(
        center.get(oldest_unread.id).is_none(),
        "then the oldest unread"
    );
}

#[test]
fn answering_flashes_who_received_it_and_a_failure_is_shown() {
    let mut app = crate::testing::populated_app();
    let agent = *app.tree.nodes.keys().next().expect("one agent");
    app.notifications.set_served(true);
    let mut answered = notification(NotificationKind::Question, "Ship?", 1);
    answered
        .answer(Some("1".into()), None, Actor::user(), now())
        .expect("answerable");
    app.on_reply(
        Then::NotificationAnswered,
        Ok(Reply::NotificationAnswered {
            notification: answered.clone(),
            delivered_to: Some(agent),
        }),
    );
    let flash = app.live_flash().expect("a flash").to_owned();
    assert!(flash.contains("assistant has the answer"), "{flash}");
    assert_eq!(
        app.notifications.get(answered.id).map(|n| n.state),
        Some(NotificationState::Resolved)
    );
    let refusal = aether_protocol::ErrorReply::new(
        aether_protocol::ErrorCode::InvalidArgument,
        "already answered",
    );
    app.on_reply(
        Then::NotificationChanged,
        Err(ClientError::Protocol(refusal)),
    );
    let flash = app.live_flash().expect("a flash");
    assert!(flash.contains("already answered"), "{flash}");
}
