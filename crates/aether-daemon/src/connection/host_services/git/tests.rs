use std::ffi::OsStr;
use std::time::Duration;

use aether_protocol::ErrorCode;

use super::head::distance;
use super::work_tree::{MAX_STATUS_ENTRIES, StatusParser};
use super::{summarize, summarize_with};
use crate::connection::host_services::test_support::{
    commit_file, init_repo, run_git, running_as_root, script,
};

const WAIT: Duration = Duration::from_secs(20);

#[tokio::test]
async fn a_clean_checkout_reports_branch_commit_and_root() {
    let dir = tempfile::tempdir().unwrap();
    let repo = init_repo(dir.path(), "repo");
    commit_file(&repo, "a.txt", "a");
    let full = run_git(&repo, &["rev-parse", "HEAD"]);

    let summary = summarize(&repo, WAIT).await.expect("summary");

    assert!(summary.repository);
    assert_eq!(summary.root.as_deref(), Some(repo.as_path()));
    assert_eq!(summary.branch.as_deref(), Some("main"));
    assert_eq!(summary.head.as_deref(), Some(&full.trim()[..7]));
    assert!(!summary.detached);
    assert!(!summary.dirty);
    assert_eq!(
        (summary.staged, summary.modified, summary.untracked),
        (0, 0, 0)
    );
    assert_eq!((summary.ahead, summary.behind), (None, None));
}

#[tokio::test]
async fn a_subdirectory_reports_the_top_of_the_work_tree() {
    let dir = tempfile::tempdir().unwrap();
    let repo = init_repo(dir.path(), "repo");
    commit_file(&repo, "a.txt", "a");
    std::fs::create_dir_all(repo.join("deep/er")).unwrap();

    let summary = summarize(&repo.join("deep/er"), WAIT)
        .await
        .expect("summary");

    assert_eq!(summary.root.as_deref(), Some(repo.as_path()));
}

#[tokio::test]
async fn changes_are_counted_by_kind_and_ignored_files_are_not() {
    let dir = tempfile::tempdir().unwrap();
    let repo = init_repo(dir.path(), "repo");
    commit_file(&repo, ".gitignore", "*.log\n");
    commit_file(&repo, "tracked.txt", "one");
    commit_file(&repo, "other.txt", "one");
    std::fs::write(repo.join("tracked.txt"), "two").unwrap();
    std::fs::write(repo.join("staged.txt"), "new").unwrap();
    run_git(&repo, &["add", "staged.txt"]);
    std::fs::write(repo.join("untracked.txt"), "x").unwrap();
    std::fs::write(repo.join("noise.log"), "x").unwrap();
    std::fs::write(repo.join("other.txt"), "two").unwrap();
    run_git(&repo, &["add", "other.txt"]);
    std::fs::write(repo.join("other.txt"), "three").unwrap();

    let summary = summarize(&repo, WAIT).await.expect("summary");

    assert!(summary.dirty);
    assert_eq!(summary.staged, 2, "staged.txt and other.txt");
    assert_eq!(summary.modified, 2, "tracked.txt and other.txt again");
    assert_eq!(summary.untracked, 1, "the ignored log is not counted");
}

#[tokio::test]
async fn a_detached_head_has_a_commit_and_no_branch() {
    let dir = tempfile::tempdir().unwrap();
    let repo = init_repo(dir.path(), "repo");
    commit_file(&repo, "a.txt", "a");
    run_git(&repo, &["checkout", "-q", "--detach"]);

    let summary = summarize(&repo, WAIT).await.expect("summary");

    assert!(summary.repository && summary.detached);
    assert_eq!(summary.branch, None);
    assert_eq!(summary.head.as_deref().map(str::len), Some(7));
}

#[tokio::test]
async fn a_repository_with_no_commits_has_a_branch_and_no_head() {
    let dir = tempfile::tempdir().unwrap();
    let repo = init_repo(dir.path(), "repo");

    let clean = summarize(&repo, WAIT).await.expect("summary");
    assert!(clean.repository && !clean.dirty && !clean.detached);
    assert_eq!(clean.branch.as_deref(), Some("main"));
    assert_eq!(clean.head, None);

    std::fs::write(repo.join("new.txt"), "x").unwrap();
    let dirty = summarize(&repo, WAIT).await.expect("summary");
    assert!(dirty.dirty);
    assert_eq!(dirty.untracked, 1);
}

#[tokio::test]
async fn a_directory_outside_any_repository_is_not_one() {
    let dir = tempfile::tempdir().unwrap();

    let summary = summarize(dir.path(), WAIT).await.expect("summary");

    assert!(!summary.repository);
    assert_eq!(summary, Default::default());
}

#[tokio::test]
async fn the_inside_of_a_git_directory_and_a_bare_repository_are_not_work_trees() {
    let dir = tempfile::tempdir().unwrap();
    let repo = init_repo(dir.path(), "repo");
    commit_file(&repo, "a.txt", "a");
    let bare = dir.path().join("bare.git");
    run_git(
        dir.path(),
        &["init", "-q", "--bare", bare.to_str().unwrap()],
    );

    assert!(
        !summarize(&repo.join(".git"), WAIT)
            .await
            .unwrap()
            .repository
    );
    assert!(!summarize(&bare, WAIT).await.unwrap().repository);
}

#[tokio::test]
async fn distance_from_the_upstream_is_reported_when_there_is_one() {
    let dir = tempfile::tempdir().unwrap();
    let origin = dir.path().join("origin.git");
    run_git(
        dir.path(),
        &[
            "init",
            "-q",
            "--bare",
            "-b",
            "main",
            origin.to_str().unwrap(),
        ],
    );
    let first = init_repo(dir.path(), "first");
    commit_file(&first, "a.txt", "a");
    run_git(
        &first,
        &["remote", "add", "origin", origin.to_str().unwrap()],
    );
    run_git(&first, &["push", "-q", "-u", "origin", "main"]);
    let second = dir.path().join("second");
    run_git(
        dir.path(),
        &[
            "clone",
            "-q",
            origin.to_str().unwrap(),
            second.to_str().unwrap(),
        ],
    );
    commit_file(&second, "b.txt", "b");
    run_git(&second, &["push", "-q"]);

    let level = summarize(&first, WAIT).await.unwrap();
    assert_eq!((level.ahead, level.behind), (Some(0), Some(0)));

    run_git(&first, &["fetch", "-q"]);
    commit_file(&first, "c.txt", "c");
    let diverged = summarize(&first, WAIT).await.unwrap();
    assert_eq!((diverged.ahead, diverged.behind), (Some(1), Some(1)));
}

#[tokio::test]
async fn a_missing_git_is_a_visible_error() {
    let dir = tempfile::tempdir().unwrap();

    let error = summarize_with(OsStr::new("/nonexistent/git"), dir.path(), WAIT)
        .await
        .expect_err("git is missing");

    assert_eq!(error.code, ErrorCode::Unavailable);
    assert!(error.message.contains("not installed"), "{}", error.message);
}

#[tokio::test]
async fn git_that_hangs_is_stopped_at_the_deadline() {
    let dir = tempfile::tempdir().unwrap();
    let hang = script(dir.path(), "git", "sleep 60");

    let started = std::time::Instant::now();
    let error = summarize_with(hang.as_os_str(), dir.path(), Duration::from_millis(200))
        .await
        .expect_err("times out");

    assert_eq!(error.code, ErrorCode::Unavailable);
    assert!(error.message.contains("too long"), "{}", error.message);
    assert!(started.elapsed() < Duration::from_secs(5));
}

#[tokio::test]
async fn git_runs_with_a_scrubbed_environment() {
    let dir = tempfile::tempdir().unwrap();
    let report = dir.path().join("env.txt");
    let spy = script(
        dir.path(),
        "git",
        &format!("env > {}; echo 'fatal: spy' >&2; exit 2", report.display()),
    );

    summarize_with(spy.as_os_str(), dir.path(), WAIT)
        .await
        .expect_err("the spy fails");

    let seen = std::fs::read_to_string(&report).unwrap();
    let names: Vec<&str> = seen.lines().filter_map(|l| l.split('=').next()).collect();
    for name in &names {
        let allowed = *name == "PATH"
            || *name == "LC_ALL"
            || *name == "PWD"
            || *name == "SHLVL"
            || *name == "_"
            || name.starts_with("GIT_");
        assert!(allowed, "unexpected variable {name} reached git");
    }
    assert!(seen.contains("GIT_CONFIG_GLOBAL=/dev/null"));
    assert!(seen.contains("GIT_TERMINAL_PROMPT=0"));
}

#[tokio::test]
async fn an_endless_status_is_cut_off_and_counted_as_a_lower_bound() {
    let dir = tempfile::tempdir().unwrap();
    let endless = script(
        dir.path(),
        "git",
        r#"case "$*" in
  *rev-parse*) printf 'true\n%s\n%s/.git\n.git\nsha1\n' "$PWD" "$PWD" ;;
  *symbolic-ref*) echo refs/heads/main ;;
  *for-each-ref*) ;;
  *status*) while :; do printf '? file\0'; done ;;
  *) exit 1 ;;
esac"#,
    );
    let work = dir.path().canonicalize().unwrap();

    let summary = summarize_with(endless.as_os_str(), &work, WAIT)
        .await
        .expect("summary");

    assert_eq!(summary.untracked as usize, MAX_STATUS_ENTRIES);
    assert_eq!(summary.branch.as_deref(), Some("main"));
    assert!(summary.dirty);
    assert_eq!(summary.head, None);
}

#[tokio::test]
async fn a_git_that_fails_for_another_reason_says_why() {
    let dir = tempfile::tempdir().unwrap();
    let broken = script(
        dir.path(),
        "git",
        "echo 'fatal: detected dubious ownership' >&2; exit 128",
    );

    let error = summarize_with(broken.as_os_str(), dir.path(), WAIT)
        .await
        .expect_err("fails");

    assert_eq!(error.code, ErrorCode::Unavailable);
    assert!(
        error.message.contains("dubious ownership"),
        "{}",
        error.message
    );
}

#[tokio::test]
async fn an_unreadable_directory_is_an_error_not_a_non_repository() {
    if running_as_root() {
        return;
    }
    use std::os::unix::fs::PermissionsExt as _;
    let dir = tempfile::tempdir().unwrap();
    let locked = dir.path().join("locked");
    std::fs::create_dir(&locked).unwrap();
    std::fs::set_permissions(&locked, std::fs::Permissions::from_mode(0o000)).unwrap();

    let result = summarize(&locked, WAIT).await;

    std::fs::set_permissions(&locked, std::fs::Permissions::from_mode(0o755)).unwrap();
    assert!(result.is_err_and(|e| e.code == ErrorCode::Unavailable));
}

#[test]
fn status_records_are_counted_by_side() {
    let mut parser = StatusParser::default();
    for record in [
        "# branch.oid 0123456789abcdef",
        "1 M. N... 100644 100644 100644 a b staged.txt",
        "1 .M N... 100644 100644 100644 a b modified.txt",
        "1 MM N... 100644 100644 100644 a b both.txt",
        "1 .D N... 100644 100644 000000 a b deleted.txt",
        "u UU N... 100644 100644 100644 100644 a b c conflict.txt",
        "? new file.txt",
    ] {
        parser.entry(record);
    }
    let changes = parser.changes;
    assert_eq!(changes.staged, 2);
    assert_eq!(
        changes.modified, 4,
        "modified, both, deleted and the conflict"
    );
    assert_eq!(changes.untracked, 1);
}

#[test]
fn distance_from_an_upstream_is_read_from_either_side() {
    assert_eq!(distance("").unwrap(), Some((0, 0)));
    assert_eq!(distance("ahead 3").unwrap(), Some((3, 0)));
    assert_eq!(distance("behind 2").unwrap(), Some((0, 2)));
    assert_eq!(distance("ahead 3, behind 1").unwrap(), Some((3, 1)));
    assert_eq!(distance("gone").unwrap(), None);
    for nonsense in ["sideways 3", "ahead", "ahead x", "ahead 1, gone"] {
        assert!(distance(nonsense).is_err(), "{nonsense}");
    }
}

#[tokio::test]
async fn a_linked_work_tree_reports_its_own_branch_and_changes() {
    let dir = tempfile::tempdir().unwrap();
    let repo = init_repo(dir.path(), "repo");
    commit_file(&repo, "a.txt", "a");
    let linked = dir.path().canonicalize().unwrap().join("linked");
    run_git(
        &repo,
        &[
            "worktree",
            "add",
            "-q",
            "-b",
            "side",
            linked.to_str().unwrap(),
        ],
    );
    std::fs::write(linked.join("a.txt"), "changed").unwrap();
    std::fs::write(linked.join("new.txt"), "x").unwrap();

    let summary = summarize(&linked, WAIT).await.expect("summary");

    assert_eq!(summary.root.as_deref(), Some(linked.as_path()));
    assert_eq!(summary.branch.as_deref(), Some("side"));
    assert_eq!(
        (summary.staged, summary.modified, summary.untracked),
        (0, 1, 1)
    );
    let main = summarize(&repo, WAIT).await.expect("summary");
    assert!(!main.dirty, "the main work tree has an index of its own");
}

#[tokio::test]
async fn a_sha256_repository_is_read_in_its_own_object_format() {
    let dir = tempfile::tempdir().unwrap();
    let repo = dir.path().canonicalize().unwrap().join("repo");
    std::fs::create_dir(&repo).unwrap();
    run_git(
        &repo,
        &["init", "-q", "-b", "main", "--object-format=sha256"],
    );
    commit_file(&repo, "a.txt", "a");
    std::fs::write(repo.join("a.txt"), "changed").unwrap();
    std::fs::write(repo.join("b.txt"), "b").unwrap();
    run_git(&repo, &["add", "b.txt"]);
    let full = run_git(&repo, &["rev-parse", "HEAD"]);
    assert_eq!(full.trim().len(), 64);

    let summary = summarize(&repo, WAIT).await.expect("summary");

    assert_eq!(summary.head.as_deref(), Some(&full.trim()[..7]));
    assert_eq!(
        (summary.staged, summary.modified, summary.untracked),
        (1, 1, 0)
    );
}

#[tokio::test]
async fn a_split_index_is_read_whole() {
    let dir = tempfile::tempdir().unwrap();
    let repo = init_repo(dir.path(), "repo");
    commit_file(&repo, "a.txt", "a");
    run_git(&repo, &["update-index", "--split-index"]);
    std::fs::write(repo.join("b.txt"), "b").unwrap();
    run_git(&repo, &["add", "b.txt"]);
    std::fs::write(repo.join("a.txt"), "changed").unwrap();

    let summary = summarize(&repo, WAIT).await.expect("summary");

    assert_eq!(
        (summary.staged, summary.modified, summary.untracked),
        (1, 1, 0)
    );
}

#[tokio::test]
async fn settings_that_decide_what_counts_as_changed_are_carried_over() {
    use std::os::unix::fs::PermissionsExt as _;
    let dir = tempfile::tempdir().unwrap();
    let repo = init_repo(dir.path(), "repo");
    commit_file(&repo, "a.txt", "a");
    let executable = std::fs::Permissions::from_mode(0o755);
    std::fs::set_permissions(repo.join("a.txt"), executable).unwrap();
    assert_eq!(summarize(&repo, WAIT).await.unwrap().modified, 1);

    run_git(&repo, &["config", "core.fileMode", "false"]);

    let summary = summarize(&repo, WAIT).await.expect("summary");
    assert!(!summary.dirty, "the mode is not compared: {summary:?}");
}

#[tokio::test]
async fn a_comparison_setting_that_is_not_a_word_is_refused() {
    let dir = tempfile::tempdir().unwrap();
    let repo = init_repo(dir.path(), "repo");
    commit_file(&repo, "a.txt", "a");
    run_git(&repo, &["config", "core.eol", "lf -c core.fsmonitor=true"]);

    let error = summarize(&repo, WAIT).await.expect_err("refused");

    assert_eq!(error.code, ErrorCode::Unavailable);
    assert!(error.message.contains("core.eol"), "{}", error.message);
}

#[tokio::test]
async fn the_repository_exclude_file_still_hides_untracked_files() {
    let dir = tempfile::tempdir().unwrap();
    let repo = init_repo(dir.path(), "repo");
    commit_file(&repo, "a.txt", "a");
    std::fs::write(repo.join("scratch.tmp"), "x").unwrap();
    std::fs::write(repo.join("seen.txt"), "x").unwrap();
    std::fs::write(repo.join(".git/info/exclude"), "*.tmp\n").unwrap();

    let summary = summarize(&repo, WAIT).await.expect("summary");

    assert_eq!(summary.untracked, 1, "only seen.txt");
}

#[tokio::test]
async fn an_upstream_that_is_gone_leaves_the_distance_out() {
    let dir = tempfile::tempdir().unwrap();
    let repo = init_repo(dir.path(), "repo");
    commit_file(&repo, "a.txt", "a");
    run_git(&repo, &["config", "branch.main.remote", "origin"]);
    run_git(&repo, &["config", "branch.main.merge", "refs/heads/main"]);
    run_git(&repo, &["config", "remote.origin.url", "/nonexistent"]);
    let fetch = "+refs/heads/*:refs/remotes/origin/*";
    run_git(&repo, &["config", "remote.origin.fetch", fetch]);

    let summary = summarize(&repo, WAIT).await.expect("summary");

    assert_eq!(summary.branch.as_deref(), Some("main"));
    assert_eq!((summary.ahead, summary.behind), (None, None));
}
