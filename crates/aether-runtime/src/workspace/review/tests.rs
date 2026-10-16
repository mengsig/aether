use super::*;

fn git_ok(repo: &Path, args: &[&str]) -> String {
    let output = Command::new("git")
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .args(["-c", "commit.gpgsign=false", "-C"])
        .arg(repo)
        .args(args)
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "git {args:?}: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8(output.stdout).unwrap().trim().to_owned()
}

fn commit_all(repo: &Path, message: &str) -> String {
    git_ok(repo, &["add", "-A"]);
    git_ok(repo, &["commit", "-qm", message]);
    git_ok(repo, &["rev-parse", "HEAD"])
}

fn repo() -> (tempfile::TempDir, PathBuf, PathBuf) {
    let temp = tempfile::tempdir().unwrap();
    let source = temp.path().join("source");
    let managed = temp.path().join("managed");
    fs::create_dir(&source).unwrap();
    git_ok(&source, &["init", "-q"]);
    git_ok(&source, &["config", "user.name", "Test User"]);
    git_ok(&source, &["config", "user.email", "test@example.invalid"]);
    fs::write(source.join("kept.txt"), b"one\ntwo\n").unwrap();
    fs::write(source.join("removed.txt"), b"bye\n").unwrap();
    commit_all(&source, "initial");
    (temp, source, managed)
}

#[test]
fn a_worktree_diff_lists_statuses_line_counts_and_the_patch() {
    let (_temp, source, managed) = repo();
    let manifest = create(&managed, &source, AgentId::new()).unwrap();
    let workspace = &manifest.workspace;
    fs::write(workspace.join("kept.txt"), b"one\nTWO\nthree\n").unwrap();
    fs::remove_file(workspace.join("removed.txt")).unwrap();
    fs::write(workspace.join("image.bin"), b"\0\x01binary\0").unwrap();
    let loaded = load(&managed, workspace).unwrap();
    let tree = current_tree(&managed, &loaded).unwrap();

    let diff = tree_diff(workspace, &loaded.baseline_tree, &tree).unwrap();
    let summary: Vec<_> = diff
        .files
        .iter()
        .map(|f| (f.path.as_str(), f.status, f.insertions, f.deletions))
        .collect();
    assert_eq!(
        summary,
        vec![
            ("image.bin", FileStatus::Added, None, None),
            ("kept.txt", FileStatus::Modified, Some(2), Some(1)),
            ("removed.txt", FileStatus::Deleted, Some(0), Some(1)),
        ]
    );
    assert_eq!((diff.insertions, diff.deletions), (2, 2));
    assert!(!diff.patch.truncated);
    assert!(diff.patch.text.contains("+TWO"), "{}", diff.patch.text);
    assert_eq!(diff.patch.bytes, diff.patch.text.len() as u64);
}

#[test]
fn a_patch_over_the_limit_is_cut_and_marked_with_its_full_size() {
    let (_temp, source, managed) = repo();
    let manifest = create(&managed, &source, AgentId::new()).unwrap();
    let line = "x".repeat(99) + "\n";
    fs::write(
        manifest.workspace.join("big.txt"),
        line.repeat(PATCH_LIMIT / 50),
    )
    .unwrap();
    let tree = current_tree(&managed, &manifest).unwrap();

    let diff = tree_diff(&manifest.workspace, &manifest.baseline_tree, &tree).unwrap();
    assert!(diff.patch.truncated);
    assert!(diff.patch.bytes > 2 * PATCH_LIMIT as u64);
    let marker = aether_core::workspace_review::truncation_marker(PATCH_LIMIT, diff.patch.bytes);
    assert!(diff.patch.text.ends_with(&marker));
    assert_eq!(diff.patch.text.len(), PATCH_LIMIT + marker.len());
    assert_eq!(diff.files[0].insertions, Some((PATCH_LIMIT / 50) as u64));
}

#[test]
fn commits_since_the_snapshot_are_listed_newest_first() {
    let (_temp, source, managed) = repo();
    let manifest = create(&managed, &source, AgentId::new()).unwrap();
    let workspace = &manifest.workspace;
    git_ok(workspace, &["config", "user.name", "Child Agent"]);
    git_ok(
        workspace,
        &["config", "user.email", "child@example.invalid"],
    );
    fs::write(workspace.join("first.txt"), b"1\n").unwrap();
    let first = commit_all(workspace, "first change");
    fs::write(workspace.join("second.txt"), b"2\n").unwrap();
    let second = commit_all(workspace, "second change");

    let start = snapshot_commit(&manifest).unwrap();
    let head = head(workspace).unwrap().unwrap();
    assert_eq!(head, second);
    let commits = log(workspace, &start, &head, COMMIT_LIMIT + 1).unwrap();
    let listed: Vec<_> = commits
        .iter()
        .map(|c| (c.id.as_str(), c.summary.as_str(), c.author.as_str()))
        .collect();
    assert_eq!(
        listed,
        vec![
            (second.as_str(), "second change", "Child Agent"),
            (first.as_str(), "first change", "Child Agent"),
        ]
    );
    assert!(log(workspace, &first, &first, 1).unwrap().is_empty());
}

#[test]
fn head_is_absent_outside_a_repository() {
    let temp = tempfile::tempdir().unwrap();
    assert_eq!(head(temp.path()).unwrap(), None);
    assert!(log(temp.path(), "not-a-commit", "also-not", 1).is_err());
}

#[test]
fn abandon_removes_an_edited_worktree_and_its_refs() {
    let (_temp, source, managed) = repo();
    let manifest = create(&managed, &source, AgentId::new()).unwrap();
    fs::write(manifest.workspace.join("kept.txt"), b"unsaved edit\n").unwrap();
    fs::write(manifest.workspace.join("new.txt"), b"untracked\n").unwrap();
    let id = workspace_id(&manifest).unwrap();
    assert!(discard(&managed, &manifest).is_err(), "edits block cleanup");

    abandon(&managed, &manifest).unwrap();
    assert!(!manifest.workspace.parent().unwrap().exists());
    let refs = git_ok(&source, &["for-each-ref", "refs/aether"]);
    assert!(!refs.contains(&id), "{refs}");
    let worktrees = git_ok(&source, &["worktree", "list", "--porcelain"]);
    assert!(!worktrees.contains(&id), "{worktrees}");
    assert_eq!(fs::read(source.join("kept.txt")).unwrap(), b"one\ntwo\n");
}

#[test]
fn abandon_refuses_a_workspace_mid_integration() {
    let (_temp, source, managed) = repo();
    let manifest = create(&managed, &source, AgentId::new()).unwrap();
    let journal = manifest.workspace.parent().unwrap().join(JOURNAL);
    fs::write(&journal, b"{}").unwrap();

    let error = abandon(&managed, &manifest).unwrap_err().to_string();
    assert!(error.contains("unfinished integration"), "{error}");
    assert!(manifest.workspace.exists());
}

#[test]
fn malformed_git_listings_are_errors() {
    assert!(parse_name_status(b"M\0").is_err());
    assert!(parse_name_status(b"R100\0a\0").is_err());
    assert!(parse_numstat(b"3\tpath-without-count\0").is_err());
    assert_eq!(
        parse_numstat(b"-\t-\tbin\0").unwrap()[Path::new("bin")],
        None
    );
}
