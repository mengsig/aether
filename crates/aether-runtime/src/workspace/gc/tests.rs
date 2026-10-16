#![allow(clippy::unwrap_used)]

use super::*;
use aether_core::id::AgentId;

struct Fixture {
    _temp: tempfile::TempDir,
    source: PathBuf,
    manifest: WorkspaceManifest,
    candidate: Candidate,
}

fn git_ok(path: &Path, args: &[&str]) -> String {
    let output = git(path).args(args).output().unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8(output.stdout).unwrap().trim().to_owned()
}

impl Fixture {
    fn new() -> Self {
        Self::with_linked_source(false)
    }

    fn with_linked_source(linked: bool) -> Self {
        let temp = tempfile::tempdir().unwrap();
        let source = temp.path().join("source");
        let managed = temp.path().join("managed");
        fs::create_dir(&source).unwrap();
        git_ok(&source, &["init", "-q"]);
        git_ok(&source, &["config", "user.name", "Test User"]);
        git_ok(&source, &["config", "user.email", "test@example.invalid"]);
        fs::write(source.join("tracked"), "saved\n").unwrap();
        fs::write(source.join(".gitignore"), "ignored\nignored-dir/\n").unwrap();
        fs::create_dir(source.join("nested")).unwrap();
        fs::write(source.join("nested/tracked"), "nested\n").unwrap();
        git_ok(&source, &["add", "."]);
        git_ok(&source, &["commit", "-qm", "initial"]);
        git_ok(&source, &["branch", "saved"]);
        let source = if linked {
            let linked = temp.path().join("linked-source");
            git_ok(
                &source,
                &[
                    "worktree",
                    "add",
                    "-qb",
                    "linked",
                    linked.to_str().unwrap(),
                    "HEAD",
                ],
            );
            linked
        } else {
            source
        };
        let manifest = create(&managed, &source, AgentId::new()).unwrap();
        let mut candidate = Candidate::new(manifest.workspace.parent().unwrap().to_path_buf());
        candidate.measure(false, &[]).unwrap();
        assert_eq!(candidate.inspect(&managed).unwrap(), manifest);
        candidate.recheck(&manifest).unwrap();
        Self {
            _temp: temp,
            source,
            manifest,
            candidate,
        }
    }

    fn assert_changed(&self) {
        assert!(self.candidate.recheck_unchanged(&self.manifest).is_err());
        assert!(self.manifest.workspace.is_dir());
    }
}

#[test]
fn unchanged_filesystem_recheck_allows_unforced_removal() {
    let mut fixture = Fixture::new();
    fixture
        .candidate
        .recheck_unchanged(&fixture.manifest)
        .unwrap();
    fixture.candidate.remove(&fixture.manifest).unwrap();
    assert!(!fixture.manifest.workspace.exists());
}

#[test]
fn changed_index_flags_prevent_removal() {
    let fixture = Fixture::new();
    git_ok(
        &fixture.manifest.workspace,
        &["update-index", "--assume-unchanged", "tracked"],
    );
    fixture.assert_changed();
}

#[test]
fn deleted_source_ref_prevents_removal() {
    let fixture = Fixture::new();
    git_ok(&fixture.source, &["update-ref", "-d", "refs/heads/saved"]);
    fixture.assert_changed();
}

#[test]
fn changed_packed_refs_prevent_removal() {
    let fixture = Fixture::new();
    git_ok(&fixture.source, &["pack-refs", "--all"]);
    let managed = fixture.candidate.directory.parent().unwrap();
    let mut candidate = Candidate::new(fixture.candidate.directory.clone());
    candidate.measure(false, &[]).unwrap();
    candidate.inspect(managed).unwrap();
    candidate.recheck(&fixture.manifest).unwrap();
    let packed = fixture.source.join(".git/packed-refs");
    let contents = fs::read_to_string(&packed).unwrap();
    let changed = contents
        .lines()
        .filter(|line| !line.ends_with(" refs/heads/saved"))
        .collect::<Vec<_>>()
        .join("\n")
        + "\n";
    fs::write(packed, changed).unwrap();
    assert!(candidate.recheck_unchanged(&fixture.manifest).is_err());
    assert!(fixture.manifest.workspace.exists());
}

#[test]
fn new_ignored_files_and_directories_prevent_removal() {
    for relative in ["ignored", "ignored-dir/new", "nested/untracked"] {
        let fixture = Fixture::new();
        let path = fixture.manifest.workspace.join(relative);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, "preserve me\n").unwrap();
        fixture.assert_changed();
    }
}

#[test]
fn new_empty_directory_prevents_removal() {
    let fixture = Fixture::new();
    fs::create_dir(fixture.manifest.workspace.join("nested/new")).unwrap();
    fixture.assert_changed();
}

#[test]
fn same_size_edit_with_restored_mtime_prevents_removal() {
    let fixture = Fixture::new();
    let path = fixture.manifest.workspace.join("tracked");
    let before = fs::metadata(&path).unwrap().modified().unwrap();
    fs::write(&path, "other\n").unwrap();
    File::options()
        .write(true)
        .open(path)
        .unwrap()
        .set_modified(before)
        .unwrap();
    fixture.assert_changed();
}

#[test]
fn integration_journal_created_after_probes_prevents_removal() {
    let fixture = Fixture::new();
    fs::write(fixture.candidate.directory.join(JOURNAL), "{}").unwrap();
    fixture.assert_changed();
}

#[test]
fn changed_manifest_prevents_removal() {
    let fixture = Fixture::new();
    let mut changed = fixture.manifest.clone();
    changed.parent = AgentId::new();
    fs::write(
        fixture.candidate.directory.join(MANIFEST),
        serde_json::to_vec(&changed).unwrap(),
    )
    .unwrap();
    fixture.assert_changed();
}

#[test]
fn snapshot_rejects_external_git_configuration() {
    let fixture = Fixture::new();
    git_ok(
        &fixture.source,
        &["config", "include.path", "../external-config"],
    );
    let mut candidate = Candidate::new(fixture.candidate.directory.clone());
    let error = candidate
        .inspect(fixture.candidate.directory.parent().unwrap())
        .unwrap_err();
    assert!(
        error.to_string().contains("configuration includes"),
        "{error}"
    );
}

#[test]
fn snapshot_rejects_symlinked_refs() {
    let fixture = Fixture::new();
    let reference = fixture.source.join(".git/refs/heads/saved");
    let target = fixture.source.join("saved-ref");
    fs::rename(&reference, &target).unwrap();
    std::os::unix::fs::symlink(target, reference).unwrap();
    let mut candidate = Candidate::new(fixture.candidate.directory.clone());
    let error = candidate
        .inspect(fixture.candidate.directory.parent().unwrap())
        .unwrap_err();
    assert!(
        error.to_string().contains("symlinked Git metadata"),
        "{error}"
    );
}

#[test]
fn recheck_requires_successful_inspection() {
    let fixture = Fixture::new();
    let candidate = Candidate::new(fixture.candidate.directory.clone());
    assert!(candidate.recheck_unchanged(&fixture.manifest).is_err());
}

#[test]
fn linked_source_head_changes_prevent_removal() {
    let fixture = Fixture::with_linked_source(true);
    git_ok(
        &fixture.source,
        &["symbolic-ref", "HEAD", "refs/heads/saved"],
    );
    fixture.assert_changed();
}

#[test]
fn snapshot_rejects_cross_worktree_symbolic_refs() {
    let fixture = Fixture::new();
    fs::write(
        fixture.source.join(".git/refs/heads/saved"),
        "ref: worktrees/other/HEAD\n",
    )
    .unwrap();
    let mut candidate = Candidate::new(fixture.candidate.directory.clone());
    let error = candidate
        .inspect(fixture.candidate.directory.parent().unwrap())
        .unwrap_err();
    assert!(
        error.to_string().contains("cross-worktree symbolic refs"),
        "{error}"
    );
}

#[test]
fn snapshot_rejects_parent_traversal_git_directory_aliases() {
    let fixture = Fixture::new();
    let dot_git = fixture.manifest.workspace.join(".git");
    let pointer = fs::read_to_string(&dot_git).unwrap();
    let target = pointer.trim().strip_prefix("gitdir: ").unwrap();
    fs::write(&dot_git, format!("gitdir: {target}/../workspace\n")).unwrap();
    let mut candidate = Candidate::new(fixture.candidate.directory.clone());
    let error = candidate
        .inspect(fixture.candidate.directory.parent().unwrap())
        .unwrap_err();
    assert!(
        error.to_string().contains("parent Git directory pointers"),
        "{error}"
    );
}

#[test]
fn partly_tracked_build_output_is_walked_like_source() {
    let fixture = Fixture::new();
    let workspace = &fixture.manifest.workspace;
    fs::create_dir(workspace.join("ignored-dir")).unwrap();
    fs::write(workspace.join("ignored-dir/kept"), "tracked\n").unwrap();
    git_ok(workspace, &["add", "-f", "ignored-dir/kept"]);
    git_ok(workspace, &["commit", "-qm", "track one build file"]);
    fs::write(workspace.join("ignored-dir/extra"), "precious\n").unwrap();
    let mut candidate = Candidate::new(fixture.candidate.directory.clone());
    candidate
        .measure(false, &["ignored-dir".to_owned()])
        .unwrap();
    assert!(candidate.build_output.is_empty());
    let error = candidate
        .inspect(fixture.candidate.directory.parent().unwrap())
        .unwrap_err();
    assert!(
        error.to_string().contains("dirty, untracked, or ignored"),
        "{error}"
    );
}

#[test]
fn wholly_ignored_build_output_is_skipped_but_its_directory_is_watched() {
    let fixture = Fixture::new();
    let output = fixture.manifest.workspace.join("nested/ignored-dir");
    fs::create_dir(&output).unwrap();
    fs::write(output.join("object"), "build\n").unwrap();
    let mut candidate = Candidate::new(fixture.candidate.directory.clone());
    candidate
        .measure(true, &["ignored-dir".to_owned()])
        .unwrap();
    assert_eq!(candidate.build_output.len(), 1);
    assert_eq!(candidate.build_output[0].path, output);
    assert!(candidate.build_output[0].bytes > 0);
    let manifest = candidate
        .inspect(fixture.candidate.directory.parent().unwrap())
        .unwrap();
    candidate.recheck(&manifest).unwrap();
    fs::write(output.join("new"), "rebuilt\n").unwrap();
    assert!(candidate.recheck_unchanged(&manifest).is_err());
}

#[test]
fn status_records_skip_rename_origins() {
    let output = b"R  renamed\0!! origin/\0!! target/\0?? notes\0!! a/b/__pycache__/\0!! file\0";
    let records = build_output::status_records(output).unwrap();
    assert_eq!(records.len(), 5);
    let ignored = build_output::ignored_directories(output).unwrap();
    let expected: BTreeSet<_> = ["target", "a/b/__pycache__"]
        .into_iter()
        .map(PathBuf::from)
        .collect();
    assert_eq!(ignored, expected);
    assert!(build_output::status_records(b"!!\0").is_err());
    assert!(build_output::status_records(b"R  renamed\0").is_err());
}

fn measured_build_output(fixture: &Fixture) -> (Candidate, PathBuf) {
    let output = fixture.manifest.workspace.join("nested/ignored-dir");
    fs::create_dir_all(output.join("deep")).unwrap();
    fs::write(output.join("deep/object"), "build\n").unwrap();
    let mut candidate = Candidate::new(fixture.candidate.directory.clone());
    candidate
        .measure(false, &["ignored-dir".to_owned()])
        .unwrap();
    assert_eq!(candidate.build_output.len(), 1);
    (candidate, output)
}

#[test]
fn pruning_removes_nested_build_output_through_pinned_handles() {
    let fixture = Fixture::new();
    let (mut candidate, output) = measured_build_output(&fixture);
    candidate.confirm_build_output().unwrap();
    candidate.prune_build_output().unwrap();
    assert!(!output.exists());
    assert!(fixture.manifest.workspace.join("nested/tracked").is_file());
}

#[test]
fn pruning_never_follows_an_ancestor_swapped_for_a_symlink() {
    let fixture = Fixture::new();
    let (mut candidate, _) = measured_build_output(&fixture);
    let nested = fixture.manifest.workspace.join("nested");
    let outside = fixture.source.parent().unwrap().join("outside");
    fs::rename(&nested, &outside).unwrap();
    std::os::unix::fs::symlink(&outside, &nested).unwrap();
    assert!(candidate.prune_build_output().is_err());
    assert!(outside.join("ignored-dir/deep/object").is_file());
}

#[test]
fn pruning_refuses_a_replaced_directory() {
    let fixture = Fixture::new();
    let (mut candidate, output) = measured_build_output(&fixture);
    fs::rename(&output, fixture.manifest.workspace.join("nested/moved")).unwrap();
    fs::create_dir(&output).unwrap();
    fs::write(output.join("precious"), "keep\n").unwrap();
    let error = candidate.prune_build_output().unwrap_err();
    assert!(error.to_string().contains("changed during GC"), "{error}");
    assert!(output.join("precious").is_file());
}

#[test]
fn build_output_force_added_after_measuring_is_not_confirmed() {
    let fixture = Fixture::new();
    let (candidate, output) = measured_build_output(&fixture);
    candidate.confirm_build_output().unwrap();
    git_ok(
        &fixture.manifest.workspace,
        &["add", "-f", "nested/ignored-dir/deep/object"],
    );
    let error = candidate.confirm_build_output().unwrap_err();
    assert!(
        error.to_string().contains("no longer wholly ignored"),
        "{error}"
    );
    assert!(output.join("deep/object").is_file());
}
