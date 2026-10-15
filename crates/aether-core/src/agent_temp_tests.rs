use std::os::unix::fs::{MetadataExt as _, PermissionsExt as _};
use std::path::Path;

use super::*;

fn mode(path: &Path) -> u32 {
    fs::symlink_metadata(path).expect("metadata").mode() & 0o777
}

fn our_uid() -> u32 {
    rustix::process::geteuid().as_raw()
}

#[test]
fn a_leaf_is_the_random_tail_of_the_agent_id() {
    let agent = AgentId::new();
    let name = leaf_name(agent);
    assert_eq!(name.len(), LEAF_LEN);
    assert!(agent.to_string().ends_with(&name));
    assert_ne!(leaf_name(AgentId::new()), name);
    assert!(is_leaf_name("0123456789AB"));
    for bad in [
        "0123456789ab",
        "0123456789AI",
        "0123456789A",
        "0123456789ABC",
        ".trash-0123",
    ] {
        assert!(!is_leaf_name(bad), "{bad}");
    }
    assert_eq!(
        default_root(Path::new("/data/aether/aether.db")),
        Some(PathBuf::from("/data/aether/t"))
    );
    assert_eq!(default_root(Path::new("aether.db")), None);
}

#[test]
fn a_root_is_created_private_and_claimed_once() {
    let dir = tempfile::tempdir().expect("dir");
    let root = dir.path().join("nested/t");
    prepare_root(&root, "instance-a").expect("prepare");
    assert_eq!(mode(&root), 0o700);
    assert_eq!(
        fs::read_to_string(root.join(MARKER)).expect("marker"),
        "instance-a"
    );
    assert_eq!(mode(&root.join(MARKER)), 0o600);
    prepare_root(&root, "instance-a").expect("the same instance reuses its root");
    let error = prepare_root(&root, "instance-b").expect_err("another instance");
    assert!(
        error
            .to_string()
            .contains("used by another Aether instance"),
        "{error}"
    );
    assert_eq!(
        fs::read_to_string(root.join(MARKER)).expect("marker"),
        "instance-a"
    );
    assert!(prepare_root(Path::new("relative/t"), "instance-a").is_err());
}

#[test]
fn an_unsafe_root_is_refused() {
    let dir = tempfile::tempdir().expect("dir");
    let real = dir.path().join("real");
    fs::create_dir(&real).expect("real");
    let dangling = dir.path().join("dangling");
    std::os::unix::fs::symlink(dir.path().join("absent"), &dangling).expect("symlink");
    let error = prepare_root(&dangling, "instance-a").expect_err("dangling root");
    assert!(error.to_string().contains("cannot resolve"), "{error}");

    let shared = dir.path().join("shared");
    fs::create_dir(&shared).expect("shared");
    fs::set_permissions(&shared, fs::Permissions::from_mode(0o777)).expect("chmod");
    let error = prepare_root(&shared, "instance-a").expect_err("world-writable root");
    assert!(
        error.to_string().contains("writable by group or others"),
        "{error}"
    );
    assert_eq!(mode(&shared), 0o777, "an existing root is never chmodded");

    let stat = rustix::fs::lstat(&real).expect("stat");
    let error = check_root(&real, &stat, our_uid() + 1).expect_err("foreign owner");
    assert!(error.to_string().contains("is owned by uid"), "{error}");
    assert!(inspect_root(&dangling, None).is_err() && inspect_root(&shared, None).is_err());
    assert!(inspect_root(&real, None).is_ok());
    assert!(inspect_root(&dir.path().join("absent"), None).is_ok());
    let marker_dir = dir.path().join("marked");
    fs::create_dir(&marker_dir).expect("marked");
    fs::create_dir(marker_dir.join(MARKER)).expect("marker dir");
    assert!(prepare_root(&marker_dir, "instance-a").is_err());
}

#[test]
fn a_leaf_is_created_lazily_and_recreated_after_deletion() {
    let dir = tempfile::tempdir().expect("dir");
    let root = prepare_root(&dir.path().join("t"), "instance-a").expect("prepare");
    let agent = AgentId::new();
    let leaf = leaf_path(&root, agent);
    assert!(!leaf.exists(), "computing the path creates nothing");
    ensure_leaf(&leaf, "instance-a").expect("create");
    assert_eq!(mode(&leaf), 0o700);
    fs::write(leaf.join("kept"), b"scratch").expect("write");
    ensure_leaf(&leaf, "instance-a").expect("reuse");
    assert!(
        leaf.join("kept").exists(),
        "an existing leaf is reused as it is"
    );
    fs::remove_dir_all(&leaf).expect("delete leaf");
    ensure_leaf(&leaf, "instance-a").expect("recreate leaf");
    assert_eq!(mode(&leaf), 0o700);
    fs::remove_dir_all(&root).expect("delete root");
    ensure_leaf(&leaf, "instance-a").expect("recreate root and leaf");
    assert_eq!(mode(&root), 0o700);
    assert_eq!(
        fs::read_to_string(root.join(MARKER)).expect("marker"),
        "instance-a"
    );
    let sibling = leaf_path(&root, AgentId::new());
    ensure_leaf(&sibling, "instance-a").expect("sibling");
    assert_ne!(sibling, leaf);
}

#[test]
fn a_leaf_that_is_not_our_private_directory_is_refused() {
    let dir = tempfile::tempdir().expect("dir");
    let root = dir.path().join("t");
    prepare_root(&root, "instance-a").expect("prepare");
    let elsewhere = dir.path().join("elsewhere");
    fs::create_dir(&elsewhere).expect("elsewhere");

    let linked = leaf_path(&root, AgentId::new());
    std::os::unix::fs::symlink(&elsewhere, &linked).expect("symlink");
    let error = ensure_leaf(&linked, "instance-a").expect_err("symlinked leaf");
    assert!(
        error.to_string().contains("not a real directory"),
        "{error}"
    );

    let open = leaf_path(&root, AgentId::new());
    fs::create_dir(&open).expect("open");
    fs::set_permissions(&open, fs::Permissions::from_mode(0o755)).expect("chmod");
    let error = ensure_leaf(&open, "instance-a").expect_err("wrong mode");
    assert!(error.to_string().contains("has mode 755"), "{error}");
    assert_eq!(mode(&open), 0o755, "a refused leaf is left as found");

    let file = leaf_path(&root, AgentId::new());
    fs::write(&file, b"").expect("file");
    assert!(ensure_leaf(&file, "instance-a").is_err());

    let ours = leaf_path(&root, AgentId::new());
    ensure_leaf(&ours, "instance-a").expect("create");
    let stat = rustix::fs::lstat(&ours).expect("stat");
    let error = check_leaf(&ours, &stat, our_uid() + 1).expect_err("foreign owner");
    assert!(error.to_string().contains("is owned by uid"), "{error}");
}

#[test]
fn a_long_root_warns_about_the_socket_budget() {
    let short = Path::new("/var/a/t");
    assert!(warnings(short).iter().all(|w| !w.contains("budget")));
    let edge = format!("/{}", "x".repeat(PATH_BUDGET - LEAF_LEN - 2));
    assert_eq!(edge.len() + 1 + LEAF_LEN, PATH_BUDGET);
    assert!(
        warnings(Path::new(&edge))
            .iter()
            .all(|w| !w.contains("budget"))
    );
    let long = format!("{edge}y");
    let warned = warnings(Path::new(&long));
    let budget = warned
        .iter()
        .find(|w| w.contains("budget"))
        .expect("over budget warns");
    assert!(
        budget.contains(&format!("{} bytes", PATH_BUDGET + 1)),
        "{budget}"
    );
    let uid = our_uid();
    let sandboxed = PATH_BUDGET + 1 + format!("/claude-{uid}").len();
    assert!(
        budget.contains(&format!(
            "({sandboxed} inside Claude Code's claude-{uid} sandbox dir)"
        )),
        "{budget}"
    );
    assert!(budget.contains("agents.temp_root"), "{budget}");
}

#[cfg(target_os = "linux")]
#[test]
fn a_root_held_in_memory_warns() {
    let shm = Path::new("/dev/shm");
    let kind = rustix::fs::statfs(shm).map(|s| s.f_type);
    if kind.is_err() {
        return;
    }
    #[allow(clippy::unnecessary_cast)]
    let tmpfs = kind.is_ok_and(|kind| kind as u64 == 0x0102_1994);
    let warned = warnings(&shm.join("aether-never-created/t"));
    assert_eq!(
        warned.iter().any(|w| w.contains("tmpfs")),
        tmpfs,
        "{warned:?}"
    );
}

#[test]
fn a_root_swapped_after_startup_is_refused() {
    let dir = tempfile::tempdir().expect("dir");
    let root = dir.path().join("t");
    prepare_root(&root, "instance-a").expect("prepare");
    ensure_leaf(&leaf_path(&root, AgentId::new()), "instance-a").expect("leaf");

    let elsewhere = dir.path().join("elsewhere");
    fs::rename(&root, &elsewhere).expect("move the root away");
    std::os::unix::fs::symlink(&elsewhere, &root).expect("symlink");
    let leaf = leaf_path(&root, AgentId::new());
    let error = ensure_leaf(&leaf, "instance-a").expect_err("symlinked root");
    assert!(
        error.to_string().contains("not a real directory"),
        "{error}"
    );
    assert!(!elsewhere.join(leaf_name_of(&leaf)).exists());

    fs::remove_file(&root).expect("unlink");
    let foreign = dir.path().join("foreign");
    prepare_root(&foreign, "instance-b").expect("another instance's root");
    fs::rename(&foreign, &root).expect("swap in");
    let error = ensure_leaf(&leaf, "instance-a").expect_err("foreign root");
    assert!(
        error
            .to_string()
            .contains("used by another Aether instance"),
        "{error}"
    );
    assert!(!leaf.exists(), "nothing is created in a foreign root");
}

fn leaf_name_of(leaf: &Path) -> &std::ffi::OsStr {
    leaf.file_name().expect("leaf name")
}

#[test]
fn a_symlinked_root_is_pinned_to_its_real_path() {
    let dir = tempfile::tempdir().expect("dir");
    let base = fs::canonicalize(dir.path()).expect("canonical");
    let real = base.join("real");
    fs::create_dir(&real).expect("real");
    fs::set_permissions(&real, fs::Permissions::from_mode(0o700)).expect("chmod");
    let link = base.join("link");
    std::os::unix::fs::symlink(&real, &link).expect("symlink");
    for spelled in ["link", "link/", "link/.", "link//"] {
        let root = base.join(spelled);
        assert_eq!(prepare_root(&root, "instance-a").expect(spelled), real);
        assert_eq!(
            inspect_root(&root, Some("instance-a")).expect(spelled),
            real
        );
    }
    assert_eq!(
        fs::read_to_string(real.join(MARKER)).expect("marker"),
        "instance-a"
    );
    assert_eq!(
        resolve_root(&link.join("missing/t")).expect("missing"),
        real.join("missing/t")
    );
}

#[test]
fn every_directory_a_linked_root_passes_through_is_checked() {
    let dir = tempfile::tempdir().expect("dir");
    let base = fs::canonicalize(dir.path()).expect("canonical");
    let (safe, shared, private) = (base.join("safe"), base.join("shared"), base.join("private"));
    for made in [&safe, &shared, &private] {
        fs::create_dir(made).expect("dir");
    }
    fs::set_permissions(&shared, fs::Permissions::from_mode(0o777)).expect("chmod");
    let open = shared.join("open");
    fs::create_dir(&open).expect("open");
    std::os::unix::fs::symlink(&private, shared.join("middle")).expect("symlink");
    std::os::unix::fs::symlink(shared.join("middle"), safe.join("alias")).expect("symlink");
    let root = prepare_root(&safe.join("alias/t"), "instance-a").expect("a private real path");
    assert_eq!(root, private.join("t"));

    // The same chain ending under the shared directory is refused.
    fs::remove_file(shared.join("middle")).expect("unlink");
    std::os::unix::fs::symlink(&open, shared.join("middle")).expect("symlink");
    let error = prepare_root(&safe.join("alias/t"), "instance-a").expect_err("shared parent");
    assert!(error.to_string().contains("sticky"), "{error}");
    assert!(inspect_root(&safe.join("alias/t"), None).is_err());
    assert!(!open.join("t").exists(), "nothing is created under it");
}

#[test]
fn a_parent_turned_into_a_link_stops_the_root_being_recreated() {
    let dir = tempfile::tempdir().expect("dir");
    let base = fs::canonicalize(dir.path()).expect("canonical");
    let root = prepare_root(&base.join("a/t"), "instance-a").expect("prepare");
    fs::remove_dir_all(&root).expect("delete root");
    fs::rename(base.join("a"), base.join("moved")).expect("move");
    std::os::unix::fs::symlink(base.join("moved"), base.join("a")).expect("symlink");
    let error = ensure_leaf(&leaf_path(&root, AgentId::new()), "instance-a").expect_err("link");
    assert!(
        error.to_string().contains("not a real directory"),
        "{error}"
    );
    assert!(
        !base.join("moved/t").exists(),
        "nothing is made through the link"
    );
}

#[test]
fn a_root_its_owner_cannot_write_is_refused() {
    let dir = tempfile::tempdir().expect("dir");
    let root = prepare_root(&dir.path().join("t"), "instance-a").expect("prepare");
    fs::set_permissions(&root, fs::Permissions::from_mode(0o500)).expect("chmod");
    let prepared = prepare_root(&root, "instance-a");
    let inspected = inspect_root(&root, Some("instance-a"));
    let leaf = ensure_leaf(&leaf_path(&root, AgentId::new()), "instance-a");
    fs::set_permissions(&root, fs::Permissions::from_mode(0o700)).expect("chmod");
    for result in [prepared.map(|_| ()), inspected.map(|_| ()), leaf] {
        let error = result.expect_err("an owner without write");
        assert!(error.to_string().contains("its owner"), "{error}");
    }
}

#[test]
fn only_leftover_staging_files_are_removed() {
    let dir = tempfile::tempdir().expect("dir");
    let root = prepare_root(&dir.path().join("t"), "instance-a").expect("prepare");
    let staged = format!("{MARKER}.{}", ulid::Ulid::generate());
    fs::write(root.join(&staged), b"instance-a").expect("staged");
    let kept = [
        format!("{MARKER}.{}", ulid::Ulid::generate()),
        format!("{MARKER}.notes"),
        format!("{MARKER}x{}", ulid::Ulid::generate()),
    ];
    fs::create_dir(root.join(&kept[0])).expect("dir");
    fs::write(root.join(&kept[1]), b"").expect("file");
    fs::write(root.join(&kept[2]), b"").expect("file");
    assert_eq!(remove_staged_markers(&root).expect("remove"), vec![staged]);
    assert!(kept.iter().all(|name| root.join(name).exists()));
    assert!(root.join(MARKER).is_file(), "the marker itself stays");
    assert!(remove_staged_markers(&root).expect("again").is_empty());
}

#[test]
fn a_root_under_a_shared_parent_needs_the_sticky_bit() {
    let dir = tempfile::tempdir().expect("dir");
    let open = dir.path().join("open");
    fs::create_dir(&open).expect("open");
    fs::set_permissions(&open, fs::Permissions::from_mode(0o777)).expect("chmod");
    let error = prepare_root(&open.join("t"), "instance-a").expect_err("non-sticky parent");
    assert!(error.to_string().contains("sticky"), "{error}");
    assert!(!open.join("t").exists(), "nothing is created under it");

    let nested = open.join("a/b");
    fs::create_dir_all(&nested).expect("nested");
    fs::set_permissions(open.join("a"), fs::Permissions::from_mode(0o700)).expect("chmod");
    assert!(prepare_root(&nested.join("t"), "instance-a").is_err());

    fs::set_permissions(&open, fs::Permissions::from_mode(0o1777)).expect("sticky");
    prepare_root(&open.join("t"), "instance-a").expect("a sticky parent is fine");
}

#[test]
fn racing_claims_on_a_fresh_root_all_agree() {
    const THREADS: usize = 16;
    let dir = tempfile::tempdir().expect("dir");
    // A leaf's root is real, as `prepare_root` returns it.
    let base = fs::canonicalize(dir.path()).expect("canonical");
    for round in 0..20 {
        let root = base.join(format!("t{round}"));
        let barrier = std::sync::Barrier::new(THREADS);
        std::thread::scope(|scope| {
            let racers: Vec<_> = (0..THREADS)
                .map(|_| {
                    scope.spawn(|| {
                        let leaf = leaf_path(&root, AgentId::new());
                        barrier.wait();
                        ensure_leaf(&leaf, "instance-a")
                    })
                })
                .collect();
            for racer in racers {
                racer.join().expect("join").expect("every racer claims");
            }
        });
        let names: Vec<_> = fs::read_dir(&root)
            .expect("root")
            .map(|entry| entry.expect("entry").file_name())
            .collect();
        assert_eq!(names.len(), THREADS + 1, "{names:?}");
        assert_eq!(
            fs::read_to_string(root.join(MARKER)).expect("marker"),
            "instance-a"
        );
    }
}

#[test]
fn a_fifo_marker_is_refused_without_blocking() {
    let dir = tempfile::tempdir().expect("dir");
    let root = dir.path().join("t");
    fs::create_dir(&root).expect("root");
    fs::set_permissions(&root, fs::Permissions::from_mode(0o700)).expect("chmod");
    let status = std::process::Command::new("mkfifo")
        .arg(root.join(MARKER))
        .status()
        .expect("mkfifo");
    assert!(status.success());
    let (sender, receiver) = std::sync::mpsc::channel();
    let probe = root.clone();
    std::thread::spawn(move || sender.send(prepare_root(&probe, "instance-a")));
    let claimed = receiver
        .recv_timeout(std::time::Duration::from_secs(5))
        .expect("a FIFO marker must not block the claim");
    let error = claimed.expect_err("a FIFO is not a marker");
    assert!(error.to_string().contains("not a regular file"), "{error}");
}

#[test]
fn a_root_is_normalized_from_its_components() {
    assert_eq!(
        normalize_root(Path::new("/srv//at/./t/")).expect("plain"),
        PathBuf::from("/srv/at/t")
    );
    for bad in ["relative/t", "/srv/../t", "/", "//"] {
        assert!(normalize_root(Path::new(bad)).is_err(), "{bad}");
    }
    use std::os::unix::ffi::OsStrExt as _;
    let latin1 = Path::new(std::ffi::OsStr::from_bytes(b"/srv/\xe9t\xe9"));
    let error = normalize_root(latin1).expect_err("not UTF-8");
    assert!(error.to_string().contains("UTF-8"), "{error}");
}

#[test]
fn inspecting_a_root_reports_what_preparing_it_would_refuse() {
    let dir = tempfile::tempdir().expect("dir");
    let root = dir.path().join("t");
    prepare_root(&root, "instance-a").expect("prepare");
    inspect_root(&root, Some("instance-a")).expect("our own claim");
    let error = inspect_root(&root, Some("instance-b")).expect_err("another claim");
    assert!(
        error
            .to_string()
            .contains("used by another Aether instance"),
        "{error}"
    );

    let locked = dir.path().join("locked");
    fs::create_dir(&locked).expect("locked");
    fs::set_permissions(&locked, fs::Permissions::from_mode(0o500)).expect("chmod");
    let inspected = inspect_root(&locked.join("t"), None);
    let prepared = prepare_root(&locked.join("t"), "instance-a");
    fs::set_permissions(&locked, fs::Permissions::from_mode(0o700)).expect("chmod");
    // A superuser writes there regardless, and then both pass.
    assert_eq!(
        inspected.is_err(),
        prepared.is_err(),
        "{inspected:?} {prepared:?}"
    );
}
