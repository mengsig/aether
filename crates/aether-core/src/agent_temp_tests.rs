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
    let link = dir.path().join("link");
    std::os::unix::fs::symlink(&real, &link).expect("symlink");
    let error = prepare_root(&link, "instance-a").expect_err("symlinked root");
    assert!(
        error.to_string().contains("not a real directory"),
        "{error}"
    );
    assert!(!real.join(MARKER).exists());

    let shared = dir.path().join("shared");
    fs::create_dir(&shared).expect("shared");
    fs::set_permissions(&shared, fs::Permissions::from_mode(0o777)).expect("chmod");
    let error = prepare_root(&shared, "instance-a").expect_err("world-writable root");
    assert!(
        error.to_string().contains("writable by group or others"),
        "{error}"
    );
    assert_eq!(mode(&shared), 0o777, "an existing root is never chmodded");

    let meta = fs::symlink_metadata(&real).expect("metadata");
    let error = check_root(&real, &meta, our_uid() + 1).expect_err("foreign owner");
    assert!(error.to_string().contains("is owned by uid"), "{error}");
    assert!(inspect_root(&link).is_err() && inspect_root(&shared).is_err());
    assert!(inspect_root(&real).is_ok() && inspect_root(&dir.path().join("absent")).is_ok());
    let marker_dir = dir.path().join("marked");
    fs::create_dir(&marker_dir).expect("marked");
    fs::create_dir(marker_dir.join(MARKER)).expect("marker dir");
    assert!(prepare_root(&marker_dir, "instance-a").is_err());
}

#[test]
fn a_leaf_is_created_lazily_and_recreated_after_deletion() {
    let dir = tempfile::tempdir().expect("dir");
    let root = dir.path().join("t");
    prepare_root(&root, "instance-a").expect("prepare");
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
    let meta = fs::symlink_metadata(&ours).expect("metadata");
    let error = check_leaf(&ours, &meta, our_uid() + 1).expect_err("foreign owner");
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
