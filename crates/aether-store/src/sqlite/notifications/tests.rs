use super::*;
use crate::{Commit, Mutation, Store};
use aether_core::notification::{NotificationKind, NotificationPost, NotificationState};
use aether_core::time::now;

fn notification(source: &str, key: Option<&str>, title: &str) -> Notification {
    let post = NotificationPost {
        kind: NotificationKind::Question,
        title: title.to_owned(),
        body: String::new(),
        key: key.map(str::to_owned),
        link: None,
        actions: Vec::new(),
        free_text: true,
    };
    Notification::new(source.to_owned(), post, now())
}

fn put(store: &SqliteStore, n: &Notification) -> Result<()> {
    store
        .commit(Commit::new().mutate(Mutation::PutNotification(n.clone())))
        .map(drop)
}

#[test]
fn a_notification_reads_back_by_id_key_and_list() {
    let store = SqliteStore::open_in_memory().unwrap();
    let first = notification("archon", Some("q-1"), "first");
    let second = notification("archon", None, "second");
    put(&store, &first).unwrap();
    put(&store, &second).unwrap();
    assert_eq!(store.notification(first.id).unwrap(), Some(first.clone()));
    assert_eq!(
        store.notification_by_key("archon", "q-1").unwrap(),
        Some(first.clone())
    );
    assert_eq!(store.notification_by_key("other", "q-1").unwrap(), None);
    let listed = store.notifications(&NotificationFilter::default()).unwrap();
    assert_eq!(listed, vec![second, first], "newest first");
}

#[test]
fn one_source_cannot_hold_two_notifications_under_a_key() {
    let store = SqliteStore::open_in_memory().unwrap();
    put(&store, &notification("archon", Some("q-1"), "a")).unwrap();
    let twin = notification("archon", Some("q-1"), "b");
    assert!(put(&store, &twin).is_err(), "the unique key holds");
    assert_eq!(store.notification(twin.id).unwrap(), None, "nothing landed");
    put(&store, &notification("ci", Some("q-1"), "c")).unwrap();
    let mut rekeyed = store.notification_by_key("ci", "q-1").unwrap().unwrap();
    rekeyed.key = Some("q-2".into());
    assert!(put(&store, &rekeyed).is_err(), "a key never moves");
}

#[test]
fn filters_by_state_and_source_and_keeps_the_state_column_in_step() {
    let store = SqliteStore::open_in_memory().unwrap();
    let mut read = notification("archon", None, "read");
    read.mark_read(now());
    let unread = notification("ci", None, "unread");
    put(&store, &read).unwrap();
    put(&store, &unread).unwrap();
    let filter = |states: Vec<NotificationState>, source: Option<&str>| NotificationFilter {
        states,
        source: source.map(str::to_owned),
        limit: 10,
    };
    let only_read = store
        .notifications(&filter(vec![NotificationState::Read], None))
        .unwrap();
    assert_eq!(only_read, vec![read.clone()]);
    let from_ci = store
        .notifications(&filter(Vec::new(), Some("ci")))
        .unwrap();
    assert_eq!(from_ci, vec![unread.clone()]);
    let conn = store.read_conn();
    assert_eq!(state_column(&conn, read.id), Some(NotificationState::Read));
    assert_eq!(
        state_column(&conn, unread.id),
        Some(NotificationState::Unread)
    );
}

#[test]
fn notifications_survive_reopening_the_store() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("aether.db");
    let mut answered = notification("archon", Some("q-9"), "Ship it?");
    answered
        .answer(
            None,
            Some("yes".into()),
            aether_core::event::Actor::user(),
            now(),
        )
        .unwrap();
    {
        let store = SqliteStore::open(&path).unwrap();
        put(&store, &answered).unwrap();
    }
    let reopened = SqliteStore::open(&path).unwrap();
    assert_eq!(
        reopened.notification_by_key("archon", "q-9").unwrap(),
        Some(answered)
    );
}

#[test]
fn pruning_drops_only_the_oldest_resolved_notifications() {
    let store = SqliteStore::open_in_memory().unwrap();
    let open = notification("archon", None, "open");
    put(&store, &open).unwrap();
    let mut resolved = Vec::new();
    for i in 0..3 {
        let mut n = notification("archon", None, &format!("done {i}"));
        n.resolve(now() + chrono::Duration::seconds(i));
        put(&store, &n).unwrap();
        resolved.push(n);
    }
    store
        .commit(Commit::new().mutate(Mutation::PruneResolvedNotifications { keep: 2 }))
        .unwrap();
    assert_eq!(store.notification(resolved[0].id).unwrap(), None);
    assert!(store.notification(resolved[2].id).unwrap().is_some());
    assert!(
        store.notification(open.id).unwrap().is_some(),
        "unresolved stays"
    );
    assert_eq!(
        store
            .notifications(&NotificationFilter::default())
            .unwrap()
            .len(),
        3
    );
}
