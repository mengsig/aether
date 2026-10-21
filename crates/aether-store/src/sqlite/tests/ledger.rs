use super::*;
use aether_core::idempotency::{SubmissionLedger, UnprovenAbsence};

fn record(store: &SqliteStore, key: &str, at: Timestamp) {
    store
        .commit(Commit::new().mutate(Mutation::RecordSubmission(submission(key, at))))
        .expect("record");
}

fn ledger(store: &SqliteStore) -> SubmissionLedger {
    let ledger = store.submission_ledger().expect("ledger");
    assert!(ledger.forgotten <= ledger.recorded, "{ledger:?}");
    ledger
}

#[test]
fn a_new_store_numbers_receipts_and_has_forgotten_none() {
    let store = SqliteStore::open_in_memory().expect("open");
    let fresh = ledger(&store);
    assert_eq!((fresh.recorded, fresh.forgotten), (0, 0));
    assert_eq!(fresh.store, store.instance_id().expect("instance"));
    for key in ["a", "b", "c"] {
        record(&store, key, now());
    }
    let after = ledger(&store);
    assert_eq!((after.recorded, after.forgotten), (3, 0));
    assert_eq!(
        after.epoch, fresh.epoch,
        "recording never restarts the ledger"
    );
    assert_eq!(after.proves_absence(Some(&fresh.horizon())), Ok(()));
}

/// A refused duplicate must not leave a hole in the numbering that a later
/// receipt would be numbered past.
#[test]
fn a_refused_record_takes_its_ordinal_back() {
    let store = SqliteStore::open_in_memory().expect("open");
    record(&store, "k", now());
    let clash =
        store.commit(Commit::new().mutate(Mutation::RecordSubmission(submission("k", now()))));
    assert!(clash.is_err());
    assert_eq!(ledger(&store).recorded, 1);
}

#[test]
fn expiry_moves_the_watermark_in_the_commit_that_swept() {
    let store = SqliteStore::open_in_memory().expect("open");
    let at = now();
    let stale = at - chrono::Duration::days(8);
    let sent_before = ledger(&store).horizon();
    record(&store, "kept", at);
    // The age pass runs in every keyed commit, so a receipt already outside
    // the window goes in the commit that recorded it.
    record(&store, "old", stale);
    let swept = ledger(&store);
    assert_eq!((swept.recorded, swept.forgotten), (2, 2));
    assert_eq!(swept.forgotten_at, Some(stale));
    assert!(
        store
            .submission(&IdempotencyKey("old".into()))
            .expect("read")
            .is_none()
    );
    assert_eq!(
        swept.proves_absence(Some(&sent_before)),
        Err(UnprovenAbsence::Forgotten),
        "a receipt numbered after that horizon went"
    );
    assert_eq!(swept.proves_absence(Some(&swept.horizon())), Ok(()));
}

#[test]
fn eviction_by_the_cap_moves_the_watermark_to_the_newest_evicted() {
    let store = SqliteStore::open_in_memory()
        .expect("open")
        .with_submission_limits(2, 4);
    let at = now();
    for i in 0..4 {
        record(
            &store,
            &format!("k-{i}"),
            at + chrono::Duration::milliseconds(i),
        );
    }
    let evicted = ledger(&store);
    assert_eq!((evicted.recorded, evicted.forgotten), (4, 2));
    assert_eq!(
        evicted.forgotten_at,
        Some(at + chrono::Duration::milliseconds(1))
    );
    for (key, kept) in [("k-0", false), ("k-1", false), ("k-2", true), ("k-3", true)] {
        let found = store.submission(&IdempotencyKey(key.into())).expect("read");
        assert_eq!(found.is_some(), kept, "{key}");
    }
}

/// The ledger is durable: a daemon that restarts still knows what it dropped,
/// and a horizon handed out before the restart still measures against it.
#[test]
fn the_ledger_survives_reopening_the_database() {
    let dir = tempfile::tempdir().expect("tmp");
    let path = dir.path().join("state.db");
    let before = {
        let store = SqliteStore::open(&path).expect("open");
        record(&store, "old", now() - chrono::Duration::days(8));
        record(&store, "new", now());
        ledger(&store)
    };
    assert_eq!((before.recorded, before.forgotten), (2, 1));
    let reopened = SqliteStore::open(&path).expect("reopen");
    assert_eq!(ledger(&reopened), before);
    record(&reopened, "later", now());
    assert_eq!(ledger(&reopened).recorded, 3);
}

/// A ledger carried into another store's database - a copied or spliced file
/// whose identity was changed - vouches for nothing recorded so far.
#[test]
fn a_ledger_from_another_store_is_replaced_and_its_horizons_are_foreign() {
    let dir = tempfile::tempdir().expect("tmp");
    let path = dir.path().join("state.db");
    let before = {
        let store = SqliteStore::open(&path).expect("open");
        record(&store, "a", now());
        record(&store, "b", now());
        store
            .conn
            .lock()
            .execute(
                "UPDATE meta SET value = 'another-store' WHERE key = 'instance_id'",
                [],
            )
            .expect("change identity");
        ledger(&store)
    };
    let replaced = SqliteStore::open(&path).expect("reopen");
    let after = ledger(&replaced);
    assert_eq!(after.store, "another-store");
    assert_ne!(after.epoch, before.epoch);
    assert_eq!((after.recorded, after.forgotten), (2, 2));
    assert_eq!(
        after.proves_absence(Some(&before.horizon())),
        Err(UnprovenAbsence::ForeignHorizon)
    );
}

/// A store from before the ledger has receipts nobody numbered. They keep
/// answering retries, and the ledger started over them begins after them.
#[test]
fn a_store_migrated_from_before_the_ledger_starts_one_conservatively() {
    let dir = tempfile::tempdir().expect("tmp");
    let path = dir.path().join("state.db");
    {
        let store = SqliteStore::open(&path).expect("open");
        record(&store, "legacy", now());
        let conn = store.conn.lock();
        conn.execute_batch(
            "DELETE FROM meta WHERE key = 'submissions.ledger';
             UPDATE submissions SET ordinal = 0;",
        )
        .expect("as an older store left it");
    }
    let store = SqliteStore::open(&path).expect("reopen");
    let started = ledger(&store);
    assert_eq!((started.recorded, started.forgotten), (0, 0));
    assert!(
        store
            .submission(&IdempotencyKey("legacy".into()))
            .expect("read")
            .is_some()
    );
    record(&store, "first-numbered", now());
    assert_eq!(ledger(&store).recorded, 1);
}

#[test]
fn a_ledger_that_does_not_parse_is_corruption_not_a_fresh_start() {
    let store = SqliteStore::open_in_memory().expect("open");
    store
        .conn
        .lock()
        .execute(
            "UPDATE meta SET value = 'not json' WHERE key = 'submissions.ledger'",
            [],
        )
        .expect("damage");
    assert!(matches!(
        store.submission_ledger(),
        Err(StoreError::Corrupt(_))
    ));
    let refused =
        store.commit(Commit::new().mutate(Mutation::RecordSubmission(submission("k", now()))));
    assert!(refused.is_err(), "a receipt is never recorded unnumbered");
}

#[test]
fn invalid_stored_ledgers_fail_before_core_invariants_or_writes() {
    let store = SqliteStore::open_in_memory().unwrap();
    let original = store.submission_ledger().unwrap();
    for (recorded, forgotten, empty_epoch) in [
        (0, 1, false),
        (u64::MAX, 0, false),
        (0, 0, true),
        (i64::MAX as u64, 0, false),
    ] {
        let mut bad = original.clone();
        bad.recorded = recorded;
        bad.forgotten = forgotten;
        if empty_epoch {
            bad.epoch.clear();
        }
        let body = serde_json::to_string(&bad).unwrap();
        store
            .conn
            .lock()
            .execute(
                "UPDATE meta SET value=?1 WHERE key='submissions.ledger'",
                [&body],
            )
            .unwrap();
        let error = store
            .commit(Commit::new().mutate(Mutation::RecordSubmission(submission("refused", now()))))
            .unwrap_err();
        assert!(matches!(error, StoreError::Corrupt(_)), "{error}");
        assert!(error.to_string().contains("aether doctor"));
        assert!(
            store
                .submission(&IdempotencyKey("refused".into()))
                .unwrap()
                .is_none()
        );
        assert_eq!(store.meta("submissions.ledger").unwrap(), Some(body));
    }
}

#[test]
fn reopening_with_an_empty_store_identity_returns_a_diagnostic_error() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("store.db");
    let store = SqliteStore::open(&path).unwrap();
    store
        .conn
        .lock()
        .execute("UPDATE meta SET value='' WHERE key='instance_id'", [])
        .unwrap();
    drop(store);
    let error = SqliteStore::open(&path).unwrap_err();
    assert!(matches!(error, StoreError::Corrupt(_)));
    assert!(error.to_string().contains("instance_id is empty"));
    assert!(error.to_string().contains("aether diagnose"));
}
