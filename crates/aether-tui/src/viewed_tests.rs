use super::*;

fn scratch() -> (tempfile::TempDir, PathBuf) {
    let dir = tempfile::tempdir().expect("scratch dir");
    let viewed = viewed_dir(&dir.path().join("aether.sock"));
    (dir, viewed)
}

fn files(dir: &Path) -> Vec<String> {
    let mut names: Vec<String> = std::fs::read_dir(dir)
        .expect("list")
        .map(|entry| {
            entry
                .expect("entry")
                .file_name()
                .into_string()
                .expect("utf-8")
        })
        .collect();
    names.sort();
    names
}

#[test]
fn the_viewed_agent_is_published_followed_and_withdrawn() {
    let (_scratch, dir) = scratch();
    assert!(dir.ends_with("aether.sock.viewed"));
    assert_eq!(read(&dir).expect("nothing yet"), None);
    let mut tui = Publisher::new(dir.clone());
    let (first, second) = (AgentId::new(), AgentId::new());
    tui.sync(Some(first)).expect("publish");
    assert_eq!(read(&dir).expect("read"), Some(first));
    tui.sync(Some(second)).expect("follow");
    assert_eq!(read(&dir).expect("read"), Some(second));
    tui.sync(None).expect("withdraw");
    assert_eq!(read(&dir).expect("read"), None);
    tui.finish().expect("finish");
    assert!(files(&dir).is_empty(), "an exiting TUI removes its files");
}

#[test]
fn when_the_newest_tui_closes_the_other_live_view_names_the_agent() {
    let (_scratch, dir) = scratch();
    let (older_view, newer_view) = (AgentId::new(), AgentId::new());
    let mut older = Publisher::new(dir.clone());
    older.sync(Some(older_view)).expect("older");
    let mut newer = Publisher::new(dir.clone());
    newer.sync(Some(newer_view)).expect("newer");
    assert_eq!(read(&dir).expect("read"), Some(newer_view));
    newer.finish().expect("clean exit");
    assert_eq!(read(&dir).expect("read"), Some(older_view));
    assert_eq!(files(&dir).len(), 2, "the older TUI's lock and record stay");
}

#[test]
fn a_crashed_tuis_record_is_stale_whatever_its_pid_became() {
    let (_scratch, dir) = scratch();
    let (live_view, crashed_view) = (AgentId::new(), AgentId::new());
    let mut live = Publisher::new(dir.clone());
    live.sync(Some(live_view)).expect("live");
    let mut crashed = Publisher::new(dir.clone());
    crashed.sync(Some(crashed_view)).expect("crashed");
    // Dropping without `finish` releases the lock as a dying process does.
    drop(crashed);
    assert_eq!(files(&dir).len(), 4, "the crashed TUI left its files");
    assert_eq!(read(&dir).expect("read"), Some(live_view));
    assert_eq!(files(&dir).len(), 2, "the reader removed the stale record");
}

#[test]
fn a_reader_cleaning_a_stale_record_does_not_make_it_look_live() {
    let (_scratch, dir) = scratch();
    let (live_view, crashed_view) = (AgentId::new(), AgentId::new());
    let mut live = Publisher::new(dir.clone());
    live.sync(Some(live_view)).expect("live");
    let mut crashed = Publisher::new(dir.clone());
    crashed.sync(Some(crashed_view)).expect("crashed");
    let crashed_lock = lock_path(&dir, &crashed.slot.as_ref().expect("slot").id);
    drop(crashed);
    // Another reader paused mid-cleanup, holding the dead TUI's lock.
    let cleaner = File::open(&crashed_lock).expect("stale lock");
    cleaner.lock_shared().expect("a reader's lock");
    assert_eq!(read(&dir).expect("read"), Some(live_view));
    assert!(
        !crashed_lock.exists(),
        "either reader may finish the cleanup"
    );
}

#[test]
fn a_corrupt_live_record_or_a_shared_directory_is_reported() {
    let (_scratch, dir) = scratch();
    let mut tui = Publisher::new(dir.clone());
    tui.sync(Some(AgentId::new())).expect("publish");
    let record = files(&dir)
        .into_iter()
        .find(|name| name.ends_with(".json"))
        .expect("a record");
    std::fs::write(dir.join(record), b"not json").expect("corrupt");
    assert!(read(&dir).is_err(), "a corrupt record is reported");

    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt as _;
        let (_scratch, shared) = scratch();
        std::fs::create_dir(&shared).expect("dir");
        std::fs::set_permissions(&shared, std::fs::Permissions::from_mode(0o755)).expect("mode");
        assert!(
            read(&shared).is_err(),
            "a group-readable directory is refused"
        );
        let mut tui = Publisher::new(shared);
        assert!(tui.sync(Some(AgentId::new())).is_err());
    }
}
