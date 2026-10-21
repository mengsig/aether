#![allow(clippy::expect_used, clippy::unwrap_used)]

use super::*;

#[test]
fn distinct_store_extensions_keep_independent_evidence() {
    let dir = tempfile::tempdir().unwrap();
    let first = dir.path().join("aether.db");
    let second = dir.path().join("aether.sqlite");
    assert_ne!(path_for(&first), path_for(&second));
    record(&first, CrashKind::Panic, "first", None).unwrap();
    record(&second, CrashKind::FatalError, "second", None).unwrap();
    acknowledge_startup(&first).unwrap();
    assert_eq!(read(&first).unwrap().unwrap().message, "first");
    let second = read(&second).unwrap().unwrap();
    assert_eq!(second.message, "second");
    assert!(!second.acknowledged);
}

#[cfg(unix)]
#[test]
fn marker_paths_preserve_non_utf8_store_names() {
    use std::os::unix::ffi::{OsStrExt, OsStringExt};
    let store = PathBuf::from(std::ffi::OsString::from_vec(b"store-\xff.db".to_vec()));
    assert_eq!(
        path_for(&store).as_os_str().as_bytes(),
        b"store-\xff.db.crash.json"
    );
    assert_ne!(path_for(&store), path_for(Path::new("store-\u{fffd}.db")));
}

#[cfg(unix)]
#[test]
fn configured_store_alias_and_pinned_store_share_evidence() {
    let dir = tempfile::tempdir().unwrap();
    let store = dir.path().join("actual.db");
    std::fs::write(&store, []).unwrap();
    let alias = dir.path().join("alias.db");
    std::os::unix::fs::symlink(&store, &alias).unwrap();
    record(&alias, CrashKind::Panic, "same database", None).unwrap();
    assert_eq!(read(&alias).unwrap(), read(&store).unwrap());
    assert!(acknowledge_startup(&store).unwrap().is_some());
    assert!(acknowledge_startup(&alias).unwrap().is_none());
    assert!(path_for(&store).is_file());
    assert!(!path_for(&alias).exists());
}

#[test]
fn startup_acknowledges_once_but_keeps_evidence_for_diagnostics() {
    let dir = tempfile::tempdir().unwrap();
    let store = dir.path().join("state/aether.db");
    assert!(read(&store).unwrap().is_none());
    let before = now();
    record(
        &store,
        CrashKind::Panic,
        "worker failed",
        Some("worker.rs:12:4"),
    )
    .unwrap();
    let recorded = read(&store).unwrap().unwrap();
    assert!(recorded.at >= before && recorded.at <= now());
    assert_eq!(recorded.kind, CrashKind::Panic);
    assert_eq!(recorded.location.as_deref(), Some("worker.rs:12:4"));
    assert!(!recorded.acknowledged);
    let first = acknowledge_startup(&store).unwrap().unwrap();
    assert!(first.acknowledged);
    assert_eq!(first.at, recorded.at);
    assert!(acknowledge_startup(&store).unwrap().is_none());
    assert_eq!(read(&store).unwrap().unwrap(), first);
    let detail = problem(&store).unwrap();
    assert!(detail.contains("worker failed"));
    assert!(detail.contains("aether doctor") && detail.contains("aether diagnose"));
    record(&store, CrashKind::FatalError, "second failure", None).unwrap();
    assert!(acknowledge_startup(&store).unwrap().is_some());
    assert!(!problem(&store).unwrap().contains("worker failed"));
    assert_eq!(
        std::fs::read_dir(store.parent().unwrap()).unwrap().count(),
        1
    );
}

#[test]
fn unicode_and_json_escaping_stay_within_a_fixed_size_budget() {
    let dir = tempfile::tempdir().unwrap();
    let store = dir.path().join("aether.db");
    for message in ["🦀".repeat(100_000), "\0".repeat(100_000)] {
        record(&store, CrashKind::Panic, &message, Some(&message)).unwrap();
        let record = read(&store).unwrap().unwrap();
        assert!(record.message.len() <= MAX_MESSAGE_BYTES);
        assert!(record.location.unwrap().len() <= MAX_LOCATION_BYTES);
        assert!(std::fs::metadata(path_for(&store)).unwrap().len() <= MAX_RECORD_BYTES as u64);
    }
    assert_eq!(bounded("a🦀b", 4), "a");
    assert_eq!(bounded("a🦀b", 5), "a🦀");
}

#[test]
fn malformed_oversized_and_future_records_are_visible_and_never_acknowledged() {
    let dir = tempfile::tempdir().unwrap();
    let store = dir.path().join("aether.db");
    record(&store, CrashKind::Panic, "original", None).unwrap();
    let mut future = serde_json::to_value(read(&store).unwrap().unwrap()).unwrap();
    future["version"] = 2.into();
    for bad in [
        b"{".to_vec(),
        vec![b' '; MAX_RECORD_BYTES + 1],
        serde_json::to_vec(&future).unwrap(),
    ] {
        std::fs::write(path_for(&store), &bad).unwrap();
        assert!(read(&store).is_err());
        assert!(acknowledge_startup(&store).is_err());
        assert!(
            problem(&store)
                .unwrap()
                .contains("Cannot read daemon crash evidence")
        );
        assert_eq!(std::fs::read(path_for(&store)).unwrap(), bad);
    }
}

#[cfg(unix)]
#[test]
fn records_are_private_and_atomic_and_refuse_symlinks() {
    use std::os::unix::fs::{PermissionsExt, symlink};
    let dir = tempfile::tempdir().unwrap();
    let store = dir.path().join("aether.db");
    record(&store, CrashKind::Panic, "first", None).unwrap();
    let path = path_for(&store);
    let old = dir.path().join("old");
    std::fs::hard_link(&path, &old).unwrap();
    record(&store, CrashKind::Panic, "second", None).unwrap();
    assert_eq!(
        std::fs::metadata(&path).unwrap().permissions().mode() & 0o777,
        0o600
    );
    let previous: CrashRecord = serde_json::from_slice(&std::fs::read(&old).unwrap()).unwrap();
    assert_eq!(previous.message, "first");
    let linked_store = dir.path().join("linked.db");
    symlink(&path, path_for(&linked_store)).unwrap();
    assert!(read(&linked_store).is_err());
    assert!(record(&linked_store, CrashKind::Panic, "wrong", None).is_err());
    assert_eq!(read(&store).unwrap().unwrap().message, "second");
}

#[cfg(unix)]
#[test]
fn special_files_cannot_block_crash_reads() {
    let dir = tempfile::tempdir().unwrap();
    let store = dir.path().join("fifo.db");
    rustix::fs::mknodat(
        rustix::fs::CWD,
        path_for(&store),
        rustix::fs::FileType::Fifo,
        rustix::fs::Mode::from_bits_truncate(0o600),
        0,
    )
    .unwrap();
    assert!(read(&store).is_err());
    assert!(record(&store, CrashKind::Panic, "wrong", None).is_err());
    let store = dir.path().join("directory.db");
    std::fs::create_dir(path_for(&store)).unwrap();
    assert!(read(&store).is_err());
    assert!(acknowledge_startup(&store).is_err());
}
