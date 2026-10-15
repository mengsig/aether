use super::*;

#[cfg(unix)]
#[test]
fn existing_public_log_directory_keeps_its_mode_and_makes_managed_files_private() {
    use std::os::unix::fs::PermissionsExt;
    let root = tempfile::tempdir().unwrap();
    std::fs::set_permissions(root.path(), std::fs::Permissions::from_mode(0o755)).unwrap();
    for name in ["writer.lock", "events.0.log", "events.1.log"] {
        let path = root.path().join(name);
        std::fs::write(&path, b"retained").unwrap();
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o644)).unwrap();
    }
    let mut log = BoundedLog::new(root.path().into(), 8, 3).unwrap();
    for name in ["writer.lock", "events.0.log", "events.1.log"] {
        let path = root.path().join(name);
        assert_eq!(
            std::fs::metadata(&path).unwrap().permissions().mode() & 0o777,
            0o600
        );
        assert_eq!(std::fs::read(path).unwrap(), b"retained");
    }
    log.write_all(b"nextnew").unwrap();
    assert_eq!(
        std::fs::metadata(root.path()).unwrap().permissions().mode() & 0o777,
        0o755
    );
    for entry in std::fs::read_dir(root.path()).unwrap() {
        assert_eq!(
            entry.unwrap().metadata().unwrap().permissions().mode() & 0o777,
            0o600
        );
    }
}

#[cfg(unix)]
#[test]
fn newly_created_log_directories_and_files_are_private() {
    use std::os::unix::fs::PermissionsExt;
    let root = tempfile::tempdir().unwrap();
    std::fs::set_permissions(root.path(), std::fs::Permissions::from_mode(0o755)).unwrap();
    let directory = root.path().join("stream").join("segments-v1");
    let mut log = BoundedLog::new(directory.clone(), 2, 3).unwrap();
    log.write_all(b"abcdef").unwrap();
    for path in [root.path().join("stream"), directory.clone()] {
        assert_eq!(
            std::fs::metadata(path).unwrap().permissions().mode() & 0o777,
            0o700
        );
    }
    for entry in std::fs::read_dir(directory).unwrap() {
        assert_eq!(
            entry.unwrap().metadata().unwrap().permissions().mode() & 0o777,
            0o600
        );
    }
    assert_eq!(
        std::fs::metadata(root.path()).unwrap().permissions().mode() & 0o777,
        0o755
    );
}

#[cfg(unix)]
#[test]
fn sticky_log_directory_is_preserved_but_unsafe_ancestor_is_rejected() {
    use std::os::unix::fs::PermissionsExt;
    let root = tempfile::tempdir().unwrap();
    std::fs::set_permissions(root.path(), std::fs::Permissions::from_mode(0o1777)).unwrap();
    BoundedLog::new(root.path().into(), 2, 1)
        .unwrap()
        .write_all(b"log")
        .unwrap();
    assert_eq!(
        std::fs::metadata(root.path()).unwrap().permissions().mode() & 0o7777,
        0o1777
    );
    let unsafe_parent = root.path().join("unsafe");
    std::fs::create_dir(&unsafe_parent).unwrap();
    std::fs::set_permissions(&unsafe_parent, std::fs::Permissions::from_mode(0o777)).unwrap();
    let directory = unsafe_parent.join("segments-v1");
    let error = BoundedLog::new(directory.clone(), 2, 1).err().unwrap();
    assert_eq!(error.kind(), io::ErrorKind::PermissionDenied);
    assert!(!directory.join("writer.lock").exists());
    assert_eq!(
        std::fs::metadata(unsafe_parent)
            .unwrap()
            .permissions()
            .mode()
            & 0o777,
        0o777
    );
}

#[cfg(unix)]
#[test]
fn lock_and_retained_segment_symlinks_do_not_change_their_target() {
    use std::os::unix::fs::PermissionsExt;
    let root = tempfile::tempdir().unwrap();
    let target = root.path().join("keep");
    std::fs::write(&target, b"keep").unwrap();
    std::fs::set_permissions(&target, std::fs::Permissions::from_mode(0o644)).unwrap();
    for name in ["writer.lock", "events.1.log"] {
        let directory = root.path().join(name);
        std::fs::create_dir(&directory).unwrap();
        std::os::unix::fs::symlink(&target, directory.join(name)).unwrap();
        assert!(BoundedLog::new(directory, 2, 2).is_err());
        assert_eq!(std::fs::read(&target).unwrap(), b"keep");
        assert_eq!(
            std::fs::metadata(&target).unwrap().permissions().mode() & 0o777,
            0o644
        );
    }
}

#[cfg(unix)]
#[test]
fn hardlinked_lock_current_and_retained_segments_leave_targets_unchanged() {
    use std::os::unix::fs::PermissionsExt;

    let root = tempfile::tempdir().unwrap();
    for name in ["writer.lock", "events.0.log", "events.1.log"] {
        let directory = root.path().join(name);
        std::fs::create_dir(&directory).unwrap();
        std::fs::set_permissions(&directory, std::fs::Permissions::from_mode(0o1777)).unwrap();
        let target = root.path().join(format!("keep-{name}"));
        std::fs::write(&target, b"keep bytes").unwrap();
        std::fs::set_permissions(&target, std::fs::Permissions::from_mode(0o644)).unwrap();
        std::fs::hard_link(&target, directory.join(name)).unwrap();
        let error = BoundedLog::new(directory, 32, 2)
            .err()
            .expect("hard link rejected");
        assert_eq!(error.kind(), io::ErrorKind::InvalidInput);
        assert_eq!(std::fs::read(&target).unwrap(), b"keep bytes");
        assert_eq!(
            std::fs::metadata(&target).unwrap().permissions().mode() & 0o777,
            0o644
        );
    }
}

#[cfg(unix)]
#[test]
fn late_hardlinks_are_rejected_before_writing_or_rotation() {
    use std::os::unix::fs::PermissionsExt;

    let root = tempfile::tempdir().unwrap();
    for name in ["writer.lock", "events.0.log", "events.1.log"] {
        let directory = root.path().join(name);
        let mut log = BoundedLog::new(directory.clone(), 2, 2).unwrap();
        log.write_all(b"ab").unwrap();
        let target = root.path().join(format!("keep-{name}"));
        std::fs::write(&target, b"keep bytes").unwrap();
        std::fs::set_permissions(&target, std::fs::Permissions::from_mode(0o644)).unwrap();
        let planted = directory.join(name);
        if planted.exists() {
            std::fs::remove_file(&planted).unwrap();
        }
        std::fs::hard_link(&target, &planted).unwrap();
        let error = log.write_all(b"c").unwrap_err();
        assert_eq!(error.kind(), io::ErrorKind::InvalidInput);
        assert_eq!(std::fs::read(&target).unwrap(), b"keep bytes");
        assert_eq!(std::fs::read(planted).unwrap(), b"keep bytes");
        assert_eq!(
            std::fs::metadata(&target).unwrap().permissions().mode() & 0o777,
            0o644
        );
        if name != "events.0.log" {
            assert_eq!(
                std::fs::read(directory.join("events.0.log")).unwrap(),
                b"ab"
            );
        }
    }
}

#[cfg(unix)]
#[test]
fn fifo_lock_and_retained_segment_fail_without_blocking() {
    let root = tempfile::tempdir().unwrap();
    for name in ["writer.lock", "events.1.log"] {
        let directory = root.path().join(name);
        std::fs::create_dir(&directory).unwrap();
        rustix::fs::mknodat(
            rustix::fs::CWD,
            directory.join(name),
            rustix::fs::FileType::Fifo,
            rustix::fs::Mode::RUSR | rustix::fs::Mode::WUSR,
            0,
        )
        .unwrap();
        let error = BoundedLog::new(directory, 2, 2).err().unwrap();
        assert_eq!(error.kind(), io::ErrorKind::InvalidInput);
    }
}

fn retained(directory: &Path, segments: usize) -> Vec<u8> {
    let mut bytes = Vec::new();
    for index in (0..segments).rev() {
        let path = directory.join(format!("events.{index}.log"));
        match std::fs::read(path) {
            Ok(part) => bytes.extend(part),
            Err(error) if error.kind() == io::ErrorKind::NotFound => {}
            Err(error) => panic!("read retained segment: {error}"),
        }
    }
    bytes
}

#[test]
fn oversized_writes_keep_only_the_newest_bounded_segments() {
    let root = tempfile::tempdir().unwrap();
    let bytes: Vec<u8> = (0..80).collect();
    let mut log = BoundedLog::new(root.path().into(), 16, 3).unwrap();
    log.write_all(&bytes).unwrap();
    assert_eq!(retained(root.path(), 3), bytes[32..]);
    let files: Vec<_> = std::fs::read_dir(root.path())
        .unwrap()
        .map(Result::unwrap)
        .collect();
    assert_eq!(files.len(), 4, "three segments plus the lock");
    assert!(
        files
            .iter()
            .all(|file| file.metadata().unwrap().len() <= 16)
    );
}

#[test]
fn reopening_and_other_writers_observe_the_current_file_size() {
    let root = tempfile::tempdir().unwrap();
    let mut first = BoundedLog::new(root.path().into(), 4, 2).unwrap();
    first.write_all(b"abcd").unwrap();
    let mut second = BoundedLog::new(root.path().into(), 4, 2).unwrap();
    second.write_all(b"efgh").unwrap();
    first.write_all(b"ijkl").unwrap();
    assert_eq!(retained(root.path(), 2), b"efghijkl");
}

#[test]
fn rotation_leaves_unmanaged_files_untouched() {
    let root = tempfile::tempdir().unwrap();
    let directory = root.path().join("segments-v1");
    std::fs::create_dir(&directory).unwrap();
    let legacy = root.path().join("events.2026-09-01.log");
    let unrelated = directory.join("notes.txt");
    std::fs::write(&legacy, "legacy").unwrap();
    std::fs::write(&unrelated, "notes").unwrap();
    BoundedLog::new(directory, 2, 1)
        .unwrap()
        .write_all(b"abcdefgh")
        .unwrap();
    assert_eq!(std::fs::read_to_string(legacy).unwrap(), "legacy");
    assert_eq!(std::fs::read_to_string(unrelated).unwrap(), "notes");
}

#[test]
fn concurrent_writers_preserve_records_through_rotation() {
    let root = tempfile::tempdir().unwrap();
    std::thread::scope(|scope| {
        for writer in 0..4 {
            let directory = root.path();
            scope.spawn(move || {
                let mut log = BoundedLog::new(directory.into(), 64, 32).unwrap();
                for record in 0..32 {
                    log.write_all(format!("{writer}:{record:04}\n").as_bytes())
                        .unwrap();
                }
            });
        }
    });
    let text = String::from_utf8(retained(root.path(), 32)).unwrap();
    let mut actual: Vec<_> = text.lines().map(str::to_owned).collect();
    actual.sort();
    let expected: Vec<_> = (0..4)
        .flat_map(|writer| (0..32).map(move |record| format!("{writer}:{record:04}")))
        .collect();
    assert_eq!(actual, expected);
}

#[test]
fn invalid_limits_and_non_file_segments_fail_visibly() {
    let root = tempfile::tempdir().unwrap();
    assert!(BoundedLog::new(root.path().into(), 0, 2).is_err());
    assert!(BoundedLog::new(root.path().into(), 1, 0).is_err());
    let mut log = BoundedLog::new(root.path().into(), 4, 2).unwrap();
    log.write_all(b"abcd").unwrap();
    std::fs::create_dir(root.path().join("events.1.log")).unwrap();
    assert_eq!(
        log.write_all(b"ef").unwrap_err().kind(),
        io::ErrorKind::InvalidInput
    );
    assert_eq!(
        std::fs::read(root.path().join("events.0.log")).unwrap(),
        b"abcd"
    );
}

#[cfg(unix)]
#[test]
fn symlink_segments_are_neither_written_nor_pruned() {
    let root = tempfile::tempdir().unwrap();
    let directory = root.path().join("segments-v1");
    std::fs::create_dir(&directory).unwrap();
    let target = root.path().join("keep.txt");
    std::fs::write(&target, "keep").unwrap();
    std::os::unix::fs::symlink(&target, directory.join("events.0.log")).unwrap();
    assert!(BoundedLog::new(directory, 1, 1).is_err());
    assert_eq!(std::fs::read_to_string(target).unwrap(), "keep");
}

/// Stands in for a disk that refuses writes: the current segment cannot be
/// opened as a file. Undone by `take_writes_again`.
fn refuse_writes(directory: &Path) {
    std::fs::rename(directory.join("events.0.log"), directory.join("aside")).unwrap();
    std::fs::create_dir(directory.join("events.0.log")).unwrap();
}

fn take_writes_again(directory: &Path) {
    std::fs::remove_dir(directory.join("events.0.log")).unwrap();
    std::fs::rename(directory.join("aside"), directory.join("events.0.log")).unwrap();
}

#[test]
fn records_the_disk_refused_are_written_in_order_once_it_takes_them() {
    let root = tempfile::tempdir().unwrap();
    let mut log = BoundedLog::new(root.path().into(), 1024, 2).unwrap();
    log.write_all(b"one\n").unwrap();
    refuse_writes(root.path());
    assert!(log.write_all(b"two\n").is_err());
    assert!(log.write_all(b"three\n").is_err());
    take_writes_again(root.path());
    log.flush().unwrap();
    log.write_all(b"four\n").unwrap();
    assert_eq!(
        std::fs::read_to_string(root.path().join("events.0.log")).unwrap(),
        "one\ntwo\nthree\nfour\n"
    );
}

#[test]
fn past_the_backlog_records_are_dropped_and_counted_in_their_place() {
    let root = tempfile::tempdir().unwrap();
    let mut log = BoundedLog::new(root.path().into(), 1024, 2).unwrap();
    log.backlog_bytes = 8;
    refuse_writes(root.path());
    assert!(log.write_all(b"kept\n").is_err());
    assert!(log.write_all(b"lost\n").is_err(), "past the backlog");
    assert!(log.write_all(b"x\n").is_err(), "would fit, but after a gap");
    // A plain `write` takes nothing it cannot write.
    assert!(log.write(b"y\n").is_err());
    take_writes_again(root.path());
    log.write_all(b"after\n").unwrap();
    let text = std::fs::read_to_string(root.path().join("events.0.log")).unwrap();
    let lines: Vec<_> = text.lines().collect();
    assert_eq!(lines.len(), 3, "{text}");
    assert_eq!(lines[0], "kept");
    assert!(
        lines[1].ends_with("WARN aether::logging: dropped 2 log records the disk would not take"),
        "{text}"
    );
    assert_eq!(lines[2], "after");
}

#[test]
fn oversized_failed_record_is_dropped_without_exceeding_backlog_cap() {
    let root = tempfile::tempdir().unwrap();
    let mut log = BoundedLog::new(root.path().into(), 1024, 2).unwrap();
    let record = vec![b'x'; BACKLOG_BYTES + 1];
    refuse_writes(root.path());
    assert!(log.write_all(&record).is_err());
    assert!(log.backlog.len() <= log.backlog_bytes);
    assert_eq!(log.dropped, 1);
    assert!(log.write_all(&record).is_err());
    assert!(log.backlog.len() <= log.backlog_bytes);
    take_writes_again(root.path());
    log.write_all(b"after\n").unwrap();
    let text = String::from_utf8(retained(root.path(), 2)).unwrap();
    assert_eq!(
        text.lines()
            .filter(|line| line
                .ends_with("WARN aether::logging: dropped 1 log records the disk would not take"))
            .count(),
        2,
        "{text}"
    );
    assert!(text.ends_with("after\n"));
    assert!(!text.contains('x'));
}

#[test]
fn failed_rotation_queues_only_a_bounded_unwritten_suffix() {
    for cap in [3, 4] {
        let root = tempfile::tempdir().unwrap();
        let mut log = BoundedLog::new(root.path().into(), 4, 2).unwrap();
        log.backlog_bytes = cap;
        std::fs::create_dir(root.path().join("events.1.log")).unwrap();
        assert!(log.write_all(b"abcdefgh").is_err());
        assert_eq!(
            std::fs::read(root.path().join("events.0.log")).unwrap(),
            b"abcd"
        );
        assert!(log.backlog.len() <= cap);
        if cap == 4 {
            assert_eq!(log.backlog, b"efgh");
            assert_eq!(log.dropped, 0);
        } else {
            assert!(log.backlog.is_empty());
            assert_eq!(log.dropped, 1);
        }
        std::fs::remove_dir(root.path().join("events.1.log")).unwrap();
        log.flush().unwrap();
        assert!(log.backlog.is_empty());
        assert_eq!(log.dropped, 0);
        if cap == 4 {
            assert_eq!(retained(root.path(), 2), b"abcdefgh");
        }
    }
}

#[test]
fn drop_marker_starts_a_new_line_after_a_dropped_tail() {
    for (record, first_line) in [(b"abcdefgh", "abcd"), (b"abc\ndefg", "abc")] {
        let root = tempfile::tempdir().unwrap();
        let mut log = BoundedLog::new(root.path().into(), 4, 64).unwrap();
        log.backlog_bytes = 3;
        let blocked = root.path().join("events.63.log");
        std::fs::create_dir(&blocked).unwrap();
        assert!(log.write_all(record).is_err());
        assert!(log.backlog.is_empty());
        assert_eq!(log.dropped, 1);
        std::fs::remove_dir(blocked).unwrap();
        log.flush().unwrap();
        let text = String::from_utf8(retained(root.path(), 64)).unwrap();
        let lines: Vec<_> = text.lines().collect();
        assert_eq!(lines.len(), 2, "{text}");
        assert_eq!(lines[0], first_line);
        assert!(lines[1].ends_with("dropped 1 log records the disk would not take"));
    }
}

const DAY: Duration = Duration::from_secs(24 * 60 * 60);

fn set_modified(path: &Path, modified: SystemTime) {
    OpenOptions::new()
        .write(true)
        .open(path)
        .unwrap()
        .set_times(std::fs::FileTimes::new().set_modified(modified))
        .unwrap();
}

fn maintain_at(log: &BoundedLog, now: SystemTime) -> io::Result<()> {
    let _lock = log.lock()?;
    log.maintain(now)
}

#[test]
fn archived_age_boundary_is_inclusive_and_future_files_are_preserved() {
    let root = tempfile::tempdir().unwrap();
    let log = BoundedLog::with_policy(root.path().into(), 64, 4, 1, None).unwrap();
    let now = SystemTime::now();
    for index in 1..4 {
        std::fs::write(log.path(index), b"old").unwrap();
    }
    set_modified(&log.path(1), now - DAY + Duration::from_secs(1));
    set_modified(&log.path(2), now - DAY);
    set_modified(&log.path(3), now + DAY);
    maintain_at(&log, now).unwrap();
    assert!(log.path(1).exists());
    assert!(!log.path(2).exists());
    assert!(log.path(3).exists());
    maintain_at(&log, now + Duration::from_secs(1)).unwrap();
    assert!(!log.path(1).exists());
    assert!(log.path(0).exists());
    assert!(log.path(3).exists());
}

#[test]
fn stale_active_segment_rotates_before_a_new_record_is_written() {
    let root = tempfile::tempdir().unwrap();
    let mut first = BoundedLog::with_policy(root.path().into(), 64, 3, 1, None).unwrap();
    let mut second = BoundedLog::with_policy(root.path().into(), 64, 3, 1, None).unwrap();
    first.write_all(b"expired\n").unwrap();
    set_modified(&first.path(0), SystemTime::now() - DAY);
    second.write_all(b"second\n").unwrap();
    first.write_all(b"first\n").unwrap();
    assert_eq!(std::fs::read(first.path(0)).unwrap(), b"second\nfirst\n");
    assert!(!first.path(1).exists());
}

#[test]
fn birth_time_rotates_a_busy_segment_even_when_its_modified_time_is_recent() {
    let root = tempfile::tempdir().unwrap();
    let mut log = BoundedLog::with_policy(root.path().into(), 64, 3, 1, None).unwrap();
    log.write_all(b"busy\n").unwrap();
    let metadata = std::fs::metadata(log.path(0)).unwrap();
    let created = match metadata.created() {
        Ok(created) => created,
        Err(error) if error.kind() == io::ErrorKind::Unsupported => return,
        Err(error) => panic!("read segment birth time: {error}"),
    };
    let now = created + DAY;
    set_modified(&log.path(0), now);
    maintain_at(&log, now - Duration::from_secs(1)).unwrap();
    assert!(!log.path(1).exists());
    maintain_at(&log, now).unwrap();
    assert_eq!(std::fs::read(log.path(1)).unwrap(), b"busy\n");
    assert!(std::fs::read(log.path(0)).unwrap().is_empty());
}

#[test]
fn zero_days_disables_age_pruning_but_preserves_size_rotation() {
    let root = tempfile::tempdir().unwrap();
    let mut log = BoundedLog::with_policy(root.path().into(), 4, 2, 0, None).unwrap();
    log.write_all(b"abcd").unwrap();
    set_modified(&log.path(0), SystemTime::UNIX_EPOCH);
    log.write_all(b"efgh").unwrap();
    assert_eq!(std::fs::read(log.path(1)).unwrap(), b"abcd");
    log.write_all(b"ijkl").unwrap();
    assert_eq!(retained(root.path(), 2), b"efghijkl");
}

#[test]
fn age_pruning_ignores_legacy_and_noncanonical_names() {
    let root = tempfile::tempdir().unwrap();
    let log = BoundedLog::with_policy(root.path().into(), 64, 3, 1, None).unwrap();
    for name in ["events.2020-01-01.log", "events.01.log", "notes.txt"] {
        let path = root.path().join(name);
        std::fs::write(&path, b"keep").unwrap();
        set_modified(&path, SystemTime::UNIX_EPOCH);
    }
    std::fs::write(log.path(1), b"expired").unwrap();
    set_modified(&log.path(1), SystemTime::UNIX_EPOCH);
    maintain_at(&log, SystemTime::now()).unwrap();
    assert!(!log.path(1).exists());
    for name in ["events.2020-01-01.log", "events.01.log", "notes.txt"] {
        assert_eq!(std::fs::read(root.path().join(name)).unwrap(), b"keep");
    }
}

#[cfg(unix)]
#[test]
fn age_pruning_validates_all_archives_before_deleting_any() {
    let root = tempfile::tempdir().unwrap();
    let log = BoundedLog::with_policy(root.path().into(), 64, 3, 1, None).unwrap();
    std::fs::write(log.path(1), b"expired").unwrap();
    set_modified(&log.path(1), SystemTime::UNIX_EPOCH);
    let target = root.path().join("legacy");
    std::fs::write(&target, b"keep").unwrap();
    std::os::unix::fs::symlink(&target, log.path(2)).unwrap();
    assert!(maintain_at(&log, SystemTime::now()).is_err());
    assert_eq!(std::fs::read(log.path(1)).unwrap(), b"expired");
    assert_eq!(std::fs::read(target).unwrap(), b"keep");
}

#[test]
fn concurrent_age_rotation_keeps_every_new_record_and_the_size_bound() {
    let root = tempfile::tempdir().unwrap();
    let mut writers: Vec<_> = (0..4)
        .map(|_| BoundedLog::with_policy(root.path().into(), 64, 32, 1, None).unwrap())
        .collect();
    writers[0].write_all(b"expired\n").unwrap();
    set_modified(&writers[0].path(0), SystemTime::UNIX_EPOCH);
    let barrier = std::sync::Barrier::new(4);
    std::thread::scope(|scope| {
        for (writer, mut log) in writers.into_iter().enumerate() {
            let barrier = &barrier;
            scope.spawn(move || {
                barrier.wait();
                for record in 0..32 {
                    log.write_all(format!("{writer}:{record:04}\n").as_bytes())
                        .unwrap();
                }
            });
        }
    });
    let text = String::from_utf8(retained(root.path(), 32)).unwrap();
    let mut actual: Vec<_> = text.lines().map(str::to_owned).collect();
    actual.sort();
    let expected: Vec<_> = (0..4)
        .flat_map(|writer| (0..32).map(move |record| format!("{writer}:{record:04}")))
        .collect();
    assert_eq!(actual, expected);
    for index in 0..32 {
        match std::fs::metadata(root.path().join(format!("events.{index}.log"))) {
            Ok(metadata) => assert!(metadata.len() <= 64),
            Err(error) if error.kind() == io::ErrorKind::NotFound => {}
            Err(error) => panic!("inspect managed segment: {error}"),
        }
    }
}

#[test]
fn configured_disk_floor_stops_writes_and_resumes_the_backlog_in_order() {
    let root = tempfile::tempdir().unwrap();
    let mut log = BoundedLog::with_policy(
        root.path().into(),
        1024,
        2,
        1,
        Some(DiskFloor::Bytes(u64::MAX)),
    )
    .unwrap();
    let error = log.write_all(b"queued\n").unwrap_err();
    assert!(error.to_string().contains("below floor"));
    assert!(std::fs::read(log.path(0)).unwrap().is_empty());
    assert!(log.flush().is_err());
    assert!(log.write(b"not accepted\n").is_err());
    log.disk_floor = None;
    log.write_all(b"after\n").unwrap();
    assert_eq!(std::fs::read(log.path(0)).unwrap(), b"queued\nafter\n");
}

#[test]
fn disk_floor_policy_defaults_and_explicit_zero_are_distinct() {
    let root = tempfile::tempdir().unwrap();
    let default =
        BoundedLog::with_policy(root.path().into(), 64, 2, 1, Some(DiskFloor::Default)).unwrap();
    assert_eq!(default.disk_floor, Some(DiskFloor::Default));
    let disabled = BoundedLog::with_policy(root.path().into(), 64, 2, 1, None).unwrap();
    assert_eq!(disabled.disk_floor, None);
    let explicit =
        BoundedLog::with_policy(root.path().into(), 64, 2, 1, Some(DiskFloor::Bytes(42))).unwrap();
    assert_eq!(explicit.disk_floor, Some(DiskFloor::Bytes(42)));
}
