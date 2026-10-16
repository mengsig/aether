use std::os::unix::fs::MetadataExt as _;
use std::sync::{Arc, Barrier};

use super::*;

fn image(dir: &Path, text: &str) -> PathBuf {
    let path = dir.join("aether");
    fs::write(&path, format!("#!/bin/sh\necho {text}\n")).unwrap();
    fs::set_permissions(&path, fs::Permissions::from_mode(0o755)).unwrap();
    path
}

fn names(dir: &Path) -> Vec<String> {
    let mut names: Vec<String> = fs::read_dir(dir)
        .unwrap()
        .map(|entry| entry.unwrap().file_name().into_string().unwrap())
        .collect();
    names.sort();
    names
}

fn actions(swept: &[SweptFile]) -> Vec<(String, &'static str)> {
    let mut actions: Vec<_> = swept
        .iter()
        .map(|file| {
            let name = file
                .path
                .file_name()
                .unwrap()
                .to_string_lossy()
                .into_owned();
            (name, file.action.as_str())
        })
        .collect();
    actions.sort();
    actions
}

#[test]
fn the_bin_dir_sits_beside_an_absolute_store() {
    assert_eq!(
        default_dir(Path::new("/data/aether.db")),
        Some(PathBuf::from("/data/bin"))
    );
    assert_eq!(default_dir(Path::new("aether.db")), None);
}

#[test]
fn a_published_copy_is_named_by_version_and_id_in_a_private_dir() {
    let temp = tempfile::tempdir().unwrap();
    let source = image(temp.path(), "one");
    let dir = temp.path().join("bin");
    let published = publish(&dir, &source, "1.2.3-rc.1").unwrap();

    let name = published.path().file_name().unwrap().to_str().unwrap();
    let id = name.strip_prefix("aether-1.2.3-rc.1-").unwrap();
    assert!(is_id(id), "{name}");
    assert_eq!(names(&dir), vec![format!("{id}.lock"), name.to_owned()]);
    assert_eq!(fs::metadata(&dir).unwrap().mode() & 0o777, 0o700);
    assert_eq!(
        fs::read(published.path()).unwrap(),
        fs::read(&source).unwrap()
    );
    assert_eq!(published.linked(), cfg!(target_os = "linux"));

    let error = publish(&dir, &source, "../1").unwrap_err();
    assert_eq!(error.kind(), io::ErrorKind::InvalidInput);
}

#[test]
fn a_copy_keeps_the_old_bytes_when_the_image_is_rewritten_in_place() {
    let temp = tempfile::tempdir().unwrap();
    let source = image(temp.path(), "old");
    let dir = temp.path().join("bin");
    make_private_dir(&dir).unwrap();
    copy(&dir, &source, "aether-1.0.0-copy").unwrap();
    fs::write(&source, "#!/bin/sh\necho new\n").unwrap();

    let copied = dir.join("aether-1.0.0-copy");
    assert_eq!(
        fs::read_to_string(&copied).unwrap(),
        "#!/bin/sh\necho old\n"
    );
    assert_eq!(fs::metadata(&copied).unwrap().mode() & 0o777, 0o500);
}

#[cfg(target_os = "linux")]
#[test]
fn the_running_image_is_linked_when_it_shares_the_filesystem() {
    let temp = tempfile::tempdir().unwrap();
    let dir = temp.path().join("bin");
    let published = publish(&dir, Path::new("/proc/self/exe"), "1.0.0").unwrap();
    let running = fs::metadata("/proc/self/exe").unwrap();
    let copy = fs::metadata(published.path()).unwrap();
    assert_eq!(copy.len(), running.len());
    if published.linked() {
        assert_eq!((copy.dev(), copy.ino()), (running.dev(), running.ino()));
    } else {
        assert_eq!(copy.mode() & 0o777, 0o500, "a copy is private");
    }
}

#[cfg(target_os = "linux")]
#[test]
fn an_image_others_may_rewrite_is_copied_not_linked() {
    let temp = tempfile::tempdir().unwrap();
    let source = image(temp.path(), "shared");
    fs::set_permissions(&source, fs::Permissions::from_mode(0o777)).unwrap();
    let published = publish(&temp.path().join("bin"), &source, "1.0.0").unwrap();
    assert!(!published.linked());
    let copy = fs::metadata(published.path()).unwrap();
    assert_ne!(copy.ino(), fs::metadata(&source).unwrap().ino());
    assert_eq!(copy.mode() & 0o777, 0o500);
}

#[test]
fn a_dir_that_is_a_link_or_open_to_others_is_refused() {
    let temp = tempfile::tempdir().unwrap();
    let source = image(temp.path(), "one");
    let real = temp.path().join("real");
    fs::create_dir(&real).unwrap();
    fs::set_permissions(&real, fs::Permissions::from_mode(0o700)).unwrap();
    let alias = temp.path().join("alias");
    std::os::unix::fs::symlink(&real, &alias).unwrap();
    let error = publish(&alias, &source, "1.0.0").unwrap_err();
    assert!(
        error.to_string().contains("not a real directory"),
        "{error}"
    );
    assert!(sweep(&alias, false).is_err());

    fs::set_permissions(&real, fs::Permissions::from_mode(0o750)).unwrap();
    let error = publish(&real, &source, "1.0.0").unwrap_err();
    assert!(error.to_string().contains("chmod 700"), "{error}");
    assert!(sweep(&real, false).is_err());
    assert!(names(&real).is_empty(), "nothing was published");
}

#[test]
fn a_clean_shutdown_removes_the_copy_and_its_lock() {
    let temp = tempfile::tempdir().unwrap();
    let source = image(temp.path(), "one");
    let dir = temp.path().join("bin");
    let published = publish(&dir, &source, "1.0.0").unwrap();
    let path = published.path().to_path_buf();
    published.remove().unwrap();
    assert!(!path.exists());
    assert!(names(&dir).is_empty());
    assert!(
        sweep(&temp.path().join("missing"), false)
            .unwrap()
            .is_empty()
    );
}

#[test]
fn a_sweep_removes_only_copies_no_running_daemon_holds() {
    let temp = tempfile::tempdir().unwrap();
    let source = image(temp.path(), "one");
    let dir = temp.path().join("bin");
    let live = publish(&dir, &source, "1.0.0").unwrap();
    // A daemon that died: the OS released its lock and left its files.
    let dead = publish(&dir, &source, "0.9.0").unwrap();
    let dead_path = dead.path().to_path_buf();
    drop(dead);
    let orphan = format!(".aether-0.8.0-{}", ulid::Ulid::generate());
    fs::write(dir.join(&orphan), "staged").unwrap();
    fs::write(dir.join("notes.txt"), "mine").unwrap();
    let dead_name = dead_path.file_name().unwrap().to_str().unwrap().to_owned();
    let dead_lock = format!("{}.lock", dead_name.rsplit_once('-').unwrap().1);
    let live_name = live
        .path()
        .file_name()
        .unwrap()
        .to_str()
        .unwrap()
        .to_owned();
    let live_lock = format!("{}.lock", live_name.rsplit_once('-').unwrap().1);

    let preview = sweep(&dir, true).unwrap();
    assert!(dead_path.exists(), "a preview removes nothing");
    let expected = |stale: SweepAction| {
        let mut expected = vec![
            (orphan.clone(), stale.as_str()),
            (dead_lock.clone(), stale.as_str()),
            (dead_name.clone(), stale.as_str()),
            (live_lock.clone(), "kept"),
            (live_name.clone(), "kept"),
            ("notes.txt".to_owned(), "kept"),
        ];
        expected.sort();
        expected
    };
    assert_eq!(actions(&preview), expected(SweepAction::WouldRemove));

    let swept = sweep(&dir, false).unwrap();
    assert_eq!(actions(&swept), expected(SweepAction::Removed));
    let mut left = vec![live_name, live_lock, "notes.txt".to_owned()];
    left.sort();
    assert_eq!(names(&dir), left);
    assert_eq!(fs::read(live.path()).unwrap(), fs::read(&source).unwrap());
}

#[test]
fn concurrent_startups_and_sweeps_never_remove_a_live_copy() {
    const DAEMONS: usize = 8;
    const SWEEPERS: usize = 4;
    let temp = tempfile::tempdir().unwrap();
    let source = image(temp.path(), "one");
    let dir = temp.path().join("bin");
    let start = Arc::new(Barrier::new(DAEMONS + SWEEPERS));
    let published = Arc::new(Barrier::new(DAEMONS + SWEEPERS));
    let mut daemons = Vec::new();
    for _ in 0..DAEMONS {
        let (start, published, dir, source) = (
            start.clone(),
            published.clone(),
            dir.clone(),
            source.clone(),
        );
        daemons.push(std::thread::spawn(move || {
            start.wait();
            let copy = publish(&dir, &source, "1.0.0").unwrap();
            published.wait();
            copy
        }));
    }
    let mut sweepers = Vec::new();
    for _ in 0..SWEEPERS {
        let (start, published, dir) = (start.clone(), published.clone(), dir.clone());
        sweepers.push(std::thread::spawn(move || {
            start.wait();
            // Sweep while the daemons publish, then again once they all have.
            for _ in 0..20 {
                sweep(&dir, false).unwrap();
            }
            published.wait();
            sweep(&dir, false).unwrap()
        }));
    }
    let copies: Vec<Published> = daemons.into_iter().map(|t| t.join().unwrap()).collect();
    for sweeper in sweepers {
        let last = sweeper.join().unwrap();
        assert!(
            last.iter().all(|file| file.action == SweepAction::Kept),
            "{last:?}"
        );
    }
    for copy in &copies {
        assert_eq!(fs::read(copy.path()).unwrap(), fs::read(&source).unwrap());
    }
    assert_eq!(names(&dir).len(), 2 * DAEMONS);

    drop(copies);
    let swept = sweep(&dir, false).unwrap();
    assert_eq!(swept.len(), 2 * DAEMONS);
    assert!(names(&dir).is_empty());
}

#[test]
fn a_publication_lock_is_new_and_excludes_others_until_dropped() {
    let temp = tempfile::tempdir().unwrap();
    let dir = temp.path().join("bin");
    let dir_fd = make_private_dir(&dir).unwrap();
    let name = format!("{}.lock", ulid::Ulid::generate());
    let held = take_lock(&dir_fd, &dir, &name).unwrap().unwrap();
    let error = take_lock(&dir_fd, &dir, &name).unwrap_err();
    assert_eq!(
        error.kind(),
        io::ErrorKind::AlreadyExists,
        "a lock is never adopted"
    );
    let contender = File::open(dir.join(&name)).unwrap();
    assert!(matches!(
        contender.try_lock(),
        Err(TryLockError::WouldBlock)
    ));
    drop(held);
    assert!(
        contender.try_lock().is_ok(),
        "the OS releases a dropped lock"
    );
}
