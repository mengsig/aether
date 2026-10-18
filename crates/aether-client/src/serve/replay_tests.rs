#![allow(clippy::expect_used, clippy::unwrap_used)]

use super::*;

const NOW: i64 = 1_700_000_000;

fn stamp(nonce: &str, timestamp: i64) -> Stamp {
    Stamp {
        timestamp,
        nonce: nonce.into(),
    }
}

#[test]
fn both_key_types_and_nonces_survive_reopening() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("replay.db");
    let original = stamp("nonce", NOW);
    for id in [None, Some("delivery-1")] {
        let route = if id.is_some() { "/id" } else { "/signature" };
        {
            let guard = ReplayGuard::open(&path).unwrap();
            assert_eq!(guard.admit(route, &original, "sha256=abc", id, NOW), Ok(()));
        }
        let guard = ReplayGuard::open(&path).unwrap();
        assert_eq!(
            guard.admit(route, &original, "sha256=abc", id, NOW + 1),
            Err(Refused::Replayed)
        );
        assert_eq!(
            guard.admit(route, &original, "sha256=abc", Some("altered"), NOW + 1),
            Err(Refused::Replayed)
        );
        if id.is_some() {
            assert_eq!(
                guard.admit(
                    route,
                    &stamp("fresh-nonce", NOW),
                    "fresh-signature",
                    id,
                    NOW + 1
                ),
                Err(Refused::Replayed)
            );
        }
    }
    let guard = ReplayGuard::open(&path).unwrap();
    assert_eq!(
        guard.admit("/other", &original, "sha256=abc", None, NOW),
        Ok(())
    );
}

#[test]
fn time_bounds_and_extremes_fail_closed() {
    let directory = tempfile::tempdir().unwrap();
    let guard = ReplayGuard::open(&directory.path().join("replay.db")).unwrap();
    for timestamp in [NOW - 301, i64::MIN] {
        assert!(matches!(
            guard.admit("/hook", &stamp("old", timestamp), "sig", None, NOW),
            Err(Refused::Stale { .. })
        ));
    }
    for timestamp in [NOW + 301, i64::MAX] {
        assert!(matches!(
            guard.admit("/hook", &stamp("new", timestamp), "sig", None, NOW),
            Err(Refused::Ahead { .. })
        ));
    }
    for timestamp in [NOW - 300, NOW + 300] {
        assert_eq!(
            guard.admit(
                "/hook",
                &stamp(&timestamp.to_string(), timestamp),
                "sig",
                None,
                NOW
            ),
            Ok(())
        );
    }
}

#[test]
fn future_dated_delivery_is_remembered_until_its_last_valid_second() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("replay.db");
    let future = stamp("future", NOW + 300);
    let guard = ReplayGuard::open(&path).unwrap();
    assert_eq!(guard.admit("/hook", &future, "sig", None, NOW), Ok(()));
    drop(guard);
    let guard = ReplayGuard::open(&path).unwrap();
    assert_eq!(
        guard.admit("/hook", &future, "sig", None, NOW + 600),
        Err(Refused::Replayed)
    );
    assert!(matches!(
        guard.admit("/hook", &future, "sig", None, NOW + 601),
        Err(Refused::Stale { .. })
    ));
    assert_eq!(
        guard.admit("/hook", &stamp("fresh", NOW + 631), "sig2", None, NOW + 631),
        Ok(())
    );
    let connection = guard.connection.lock().unwrap();
    let count: i64 = connection
        .query_row("SELECT COUNT(*) FROM deliveries", [], |row| row.get(0))
        .unwrap();
    assert_eq!(count, 1, "expired entries are pruned on writes");
}

#[test]
fn capacity_is_per_route_and_does_not_evict_live_entries() {
    let directory = tempfile::tempdir().unwrap();
    let guard = ReplayGuard::open(&directory.path().join("replay.db")).unwrap();
    let mut connection = guard.connection.lock().unwrap();
    let original = stamp("n1", NOW);
    assert_eq!(
        remember(&mut connection, "/busy", &original, "key1", NOW, 1).unwrap(),
        Ok(())
    );
    assert_eq!(
        remember(&mut connection, "/busy", &stamp("n2", NOW), "key2", NOW, 1).unwrap(),
        Err(Refused::Saturated)
    );
    assert_eq!(
        remember(&mut connection, "/busy", &original, "key1", NOW, 1).unwrap(),
        Err(Refused::Replayed)
    );
    assert_eq!(
        remember(&mut connection, "/quiet", &original, "key1", NOW, 1).unwrap(),
        Ok(())
    );
}

#[test]
fn independent_connections_cannot_admit_the_same_delivery_concurrently() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("replay.db");
    let first = ReplayGuard::open(&path).unwrap();
    let second = ReplayGuard::open(&path).unwrap();
    let barrier = std::sync::Arc::new(std::sync::Barrier::new(2));
    let (completed, outcomes) = std::sync::mpsc::channel();
    let handles: Vec<_> = [first, second]
        .into_iter()
        .map(|guard| {
            let barrier = barrier.clone();
            let completed = completed.clone();
            std::thread::spawn(move || {
                barrier.wait();
                let outcome = guard.admit("/hook", &stamp("nonce", NOW), "sig", None, NOW);
                completed.send(outcome).unwrap();
            })
        })
        .collect();
    let outcomes: Vec<_> = (0..2)
        .map(|_| {
            outcomes
                .recv_timeout(Duration::from_secs(10))
                .expect("admission must not hang")
        })
        .collect();
    for handle in handles {
        handle.join().unwrap();
    }
    assert_eq!(outcomes.iter().filter(|outcome| outcome.is_ok()).count(), 1);
    assert_eq!(
        outcomes
            .iter()
            .filter(|outcome| **outcome == Err(Refused::Replayed))
            .count(),
        1
    );
}

#[test]
fn corrupt_or_unwritable_state_never_admits_delivery() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("replay.db");
    std::fs::write(&path, b"not sqlite").unwrap();
    assert!(ReplayGuard::open(&path).is_err());
    let guard = ReplayGuard::open(&directory.path().join("valid.db")).unwrap();
    guard
        .connection
        .lock()
        .unwrap()
        .execute_batch("PRAGMA query_only=ON")
        .unwrap();
    assert_eq!(
        guard.admit("/hook", &stamp("nonce", NOW), "sig", None, NOW),
        Err(Refused::Unavailable)
    );
}

#[cfg(unix)]
#[test]
fn state_is_private_and_symlinks_are_refused() {
    use std::os::unix::fs::PermissionsExt as _;
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("replay.db");
    let _guard = ReplayGuard::open(&path).unwrap();
    assert_eq!(
        std::fs::metadata(&path).unwrap().permissions().mode() & 0o777,
        0o600
    );
    let link = directory.path().join("link.db");
    std::os::unix::fs::symlink(&path, &link).unwrap();
    assert!(ReplayGuard::open(&link).is_err());
}
