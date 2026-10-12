//! Repositories built to make git run a program of theirs. Each plants a
//! command that creates a marker file, proves an unguarded git runs it where
//! that can be shown, and then asserts a summary never does.

use std::ffi::OsStr;
use std::path::{Path, PathBuf};
use std::time::Duration;

use aether_protocol::ErrorReply;
use aether_protocol::remote::GitSummary;

use super::{summarize, summarize_with};
use crate::connection::host_services::test_support::{commit_file, init_repo, run_git, script};

const WAIT: Duration = Duration::from_secs(20);
/// Attributes that send every text file through the planted drivers.
const ATTRIBUTES: &str = "*.txt filter=evil diff=evil\n";

/// A repository with one committed text file, changed since, and the marker
/// a planted program would leave.
struct Hostile {
    dir: tempfile::TempDir,
    repo: PathBuf,
    marker: PathBuf,
}

impl Hostile {
    fn new() -> Self {
        let dir = tempfile::tempdir().unwrap();
        let repo = init_repo(dir.path(), "repo");
        commit_file(&repo, "a.txt", "a");
        // The same size as before, so git must read the file to compare it.
        std::fs::write(repo.join("a.txt"), "b").unwrap();
        let marker = dir.path().canonicalize().unwrap().join("it-ran");
        Self { dir, repo, marker }
    }

    /// Shell that leaves the marker and then passes its input through.
    fn payload(&self) -> String {
        format!("touch {}; cat", self.marker.display())
    }

    /// A program that leaves the marker, for settings that name a path.
    fn program(&self, name: &str) -> String {
        let body = format!("touch {}", self.marker.display());
        let path = script(self.dir.path(), name, &body);
        path.to_str().unwrap().to_owned()
    }

    fn config(&self, key: &str, value: &str) {
        run_git(&self.repo, &["config", key, value]);
    }

    /// Prove the plant is live: git run as a person would runs it.
    fn assert_armed(&self, args: &[&str]) {
        let ran = std::process::Command::new("git")
            .env("GIT_CONFIG_GLOBAL", "/dev/null")
            .env("GIT_CONFIG_NOSYSTEM", "1")
            .arg("-C")
            .arg(&self.repo)
            .args(args)
            .output()
            .expect("git runs");
        assert!(
            self.marker.exists(),
            "git {args:?} did not run the planted program: {}",
            String::from_utf8_lossy(&ran.stderr)
        );
        std::fs::remove_file(&self.marker).unwrap();
    }

    /// Summarise `dir` with `program` as git; the plant must stay asleep.
    async fn summarize_as(&self, program: &OsStr, dir: &Path) -> Result<GitSummary, ErrorReply> {
        let summary = summarize_with(program, dir, WAIT).await;
        assert!(
            !self.marker.exists(),
            "a program the repository configured was run: {summary:?}"
        );
        summary
    }

    async fn summary(&self) -> GitSummary {
        let summary = summarize(&self.repo, WAIT).await;
        assert!(
            !self.marker.exists(),
            "a program the repository configured was run: {summary:?}"
        );
        summary.expect("summary")
    }
}

#[tokio::test]
async fn a_clean_filter_in_the_repository_config_never_runs() {
    let hostile = Hostile::new();
    hostile.config("filter.evil.clean", &hostile.payload());
    hostile.config("filter.evil.smudge", &hostile.payload());
    hostile.config("filter.evil.required", "true");
    std::fs::write(hostile.repo.join(".gitattributes"), ATTRIBUTES).unwrap();
    hostile.assert_armed(&["status"]);

    let summary = hostile.summary().await;

    assert_eq!(summary.modified, 1, "the change is still seen");
    assert_eq!(summary.untracked, 1, "and so is .gitattributes");
}

#[tokio::test]
async fn a_filter_in_work_tree_scoped_config_never_runs() {
    let hostile = Hostile::new();
    hostile.config("extensions.worktreeConfig", "true");
    run_git(
        &hostile.repo,
        &[
            "config",
            "--worktree",
            "filter.evil.clean",
            &hostile.payload(),
        ],
    );
    std::fs::write(hostile.repo.join(".git/info/attributes"), ATTRIBUTES).unwrap();
    assert!(hostile.repo.join(".git/config.worktree").exists());
    let local = run_git(&hostile.repo, &["config", "--local", "--list"]);
    assert!(
        !local.contains("filter."),
        "invisible to `--local`: {local}"
    );
    hostile.assert_armed(&["status"]);

    let summary = hostile.summary().await;

    assert_eq!((summary.modified, summary.untracked), (1, 0));
}

#[tokio::test]
async fn a_filter_process_never_runs() {
    let hostile = Hostile::new();
    hostile.config("filter.evil.process", &hostile.program("process.sh"));
    std::fs::write(hostile.repo.join(".git/info/attributes"), ATTRIBUTES).unwrap();
    hostile.assert_armed(&["status"]);

    assert_eq!(hostile.summary().await.modified, 1);
}

#[tokio::test]
async fn config_pulled_in_by_an_include_never_runs_its_programs() {
    let hostile = Hostile::new();
    let included = format!(
        "[filter \"evil\"]\n\tclean = {}\n[core]\n\tfsmonitor = {}\n",
        hostile.payload(),
        hostile.program("monitor.sh")
    );
    std::fs::write(hostile.repo.join(".git/evil.inc"), included).unwrap();
    hostile.config("includeIf.gitdir:**.path", "evil.inc");
    hostile.config("include.path", "evil.inc");
    std::fs::write(hostile.repo.join(".git/info/attributes"), ATTRIBUTES).unwrap();
    hostile.assert_armed(&["status"]);

    assert_eq!(hostile.summary().await.modified, 1);
}

#[tokio::test]
async fn a_filesystem_monitor_never_runs_from_any_config_scope() {
    let hostile = Hostile::new();
    hostile.config("core.fsmonitor", &hostile.program("monitor.sh"));
    hostile.assert_armed(&["status"]);
    assert_eq!(hostile.summary().await.modified, 1);

    run_git(&hostile.repo, &["config", "--unset", "core.fsmonitor"]);
    hostile.config("extensions.worktreeConfig", "true");
    let monitor = hostile.program("monitor.sh");
    run_git(
        &hostile.repo,
        &["config", "--worktree", "core.fsmonitor", &monitor],
    );
    hostile.assert_armed(&["status"]);

    assert_eq!(hostile.summary().await.modified, 1);
}

#[tokio::test]
async fn hooks_never_run_from_the_repository_or_a_configured_path() {
    let hostile = Hostile::new();
    let elsewhere = hostile.dir.path().join("hooks");
    std::fs::create_dir(&elsewhere).unwrap();
    let body = format!("touch {}", hostile.marker.display());
    let names = [
        "post-index-change",
        "reference-transaction",
        "pre-auto-gc",
        "post-checkout",
        "fsmonitor-watchman",
    ];
    for name in names {
        script(&hostile.repo.join(".git/hooks"), name, &body);
        script(&elsewhere, name, &body);
    }
    hostile.assert_armed(&["add", "a.txt"]);
    hostile.config("core.hooksPath", elsewhere.to_str().unwrap());
    std::fs::write(hostile.repo.join("a.txt"), "changed again").unwrap();
    hostile.assert_armed(&["add", "a.txt"]);
    std::fs::write(hostile.repo.join("a.txt"), "and again").unwrap();

    let summary = hostile.summary().await;

    assert_eq!((summary.staged, summary.modified), (1, 1));
}

#[tokio::test]
async fn diff_drivers_and_text_conversion_never_run() {
    let hostile = Hostile::new();
    let program = hostile.program("differ.sh");
    hostile.config("diff.external", &program);
    hostile.config("diff.evil.command", &program);
    hostile.config("diff.evil.textconv", &program);
    hostile.config("status.showStash", "true");
    std::fs::write(hostile.repo.join(".git/info/attributes"), ATTRIBUTES).unwrap();
    hostile.assert_armed(&["diff"]);

    assert_eq!(hostile.summary().await.modified, 1);
}

#[tokio::test]
async fn pagers_and_aliases_never_run() {
    let hostile = Hostile::new();
    let program = hostile.program("pager.sh");
    hostile.config("core.pager", &program);
    let commands = [
        "status",
        "rev-parse",
        "symbolic-ref",
        "for-each-ref",
        "config",
    ];
    for command in commands {
        hostile.config(&format!("pager.{command}"), &program);
        hostile.config(&format!("alias.{command}"), &format!("!{program}"));
    }
    hostile.config("alias.planted", &format!("!{program}"));
    hostile.assert_armed(&["planted"]);

    assert_eq!(hostile.summary().await.modified, 1);
}

/// Point the branch at a promisor remote reached by a planted `ssh`, then
/// lose the commit, so that reading it would mean fetching it.
fn plant_lazy_fetch(hostile: &Hostile) {
    let program = hostile.program("ssh.sh");
    hostile.config("core.sshCommand", &program);
    hostile.config("core.gitProxy", &program);
    hostile.config("credential.helper", &format!("!{program}"));
    hostile.config("gpg.program", &program);
    hostile.config("gpg.ssh.program", &program);
    hostile.config("remote.origin.url", "ssh://example.invalid/repo.git");
    hostile.config("remote.origin.fetch", "+refs/heads/*:refs/remotes/origin/*");
    hostile.config("remote.origin.uploadpack", &program);
    hostile.config("branch.main.remote", "origin");
    hostile.config("branch.main.merge", "refs/heads/main");
    hostile.assert_armed(&["fetch", "origin"]);
    hostile.config("remote.origin.promisor", "true");
    hostile.config("remote.origin.partialclonefilter", "blob:none");
    hostile.config("extensions.partialClone", "origin");
    let commit = run_git(&hostile.repo, &["rev-parse", "HEAD"]);
    let (fan, rest) = commit.trim().split_at(2);
    let object = hostile.repo.join(".git/objects").join(fan).join(rest);
    std::fs::remove_file(object).expect("the commit is a loose object");
}

#[tokio::test]
async fn a_missing_object_is_never_fetched_through_a_configured_transport() {
    let hostile = Hostile::new();
    plant_lazy_fetch(&hostile);
    hostile.assert_armed(&["status"]);

    let result = hostile.summarize_as(OsStr::new("git"), &hostile.repo).await;

    assert!(result.is_err(), "the commit cannot be read: {result:?}");
}

#[tokio::test]
async fn a_git_too_old_to_refuse_lazy_fetches_is_still_denied_every_transport() {
    let hostile = Hostile::new();
    plant_lazy_fetch(&hostile);
    let old_git = script(
        hostile.dir.path(),
        "git",
        "unset GIT_NO_LAZY_FETCH\nexec git \"$@\"",
    );

    let result = hostile
        .summarize_as(old_git.as_os_str(), &hostile.repo)
        .await;

    assert!(result.is_err(), "the commit cannot be read: {result:?}");
}

#[tokio::test]
async fn a_hostile_submodule_is_never_entered() {
    let hostile = Hostile::new();
    let inner = init_repo(hostile.dir.path(), "inner");
    commit_file(&inner, "b.txt", "b");
    run_git(
        &hostile.repo,
        &["submodule", "add", "-q", inner.to_str().unwrap(), "sub"],
    );
    run_git(&hostile.repo, &["commit", "-q", "-m", "sub"]);
    let sub = hostile.repo.join("sub");
    run_git(
        &sub,
        &["config", "core.fsmonitor", &hostile.program("m.sh")],
    );
    std::fs::write(sub.join("b.txt"), "changed").unwrap();
    hostile.config("status.submoduleSummary", "true");
    hostile.config("diff.ignoreSubmodules", "none");
    hostile.config("submodule.sub.ignore", "none");
    hostile.config("submodule.recurse", "true");
    hostile.assert_armed(&["status"]);

    let outer = hostile.summary().await;
    assert_eq!(outer.modified, 1, "a.txt; the submodule is not looked into");
    let inside = hostile
        .summarize_as(OsStr::new("git"), &sub)
        .await
        .expect("summary");

    assert_eq!(inside.root.as_deref(), Some(sub.as_path()));
    assert_eq!(inside.modified, 1);
}

#[tokio::test]
async fn config_turned_hostile_between_two_git_runs_never_runs_a_program() {
    let hostile = Hostile::new();
    std::fs::write(hostile.repo.join(".git/info/attributes"), ATTRIBUTES).unwrap();
    let planted = format!(
        "[filter \"evil\"]\n\tclean = {}\n[core]\n\tfsmonitor = {}\n",
        hostile.payload(),
        hostile.program("monitor.sh")
    );
    let plant = hostile.dir.path().join("plant");
    std::fs::write(&plant, planted).unwrap();
    // Clean through every check, hostile the moment `status` is asked for.
    let flipping = script(
        hostile.dir.path(),
        "git",
        &format!(
            "case \"$*\" in *status*) cat {} >> {}/.git/config ;; esac\nexec git \"$@\"",
            plant.display(),
            hostile.repo.display()
        ),
    );

    let summary = hostile
        .summarize_as(flipping.as_os_str(), &hostile.repo)
        .await
        .expect("summary");

    assert_eq!(summary.modified, 1);
    let config = std::fs::read_to_string(hostile.repo.join(".git/config")).unwrap();
    assert!(config.contains("filter \"evil\""), "the flip happened");
    hostile.assert_armed(&["status"]);
}

#[tokio::test]
async fn a_repository_someone_else_owns_is_refused_though_it_calls_itself_safe() {
    let hostile = Hostile::new();
    hostile.config("safe.directory", "*");
    hostile.config("filter.evil.clean", &hostile.payload());
    std::fs::write(hostile.repo.join(".git/info/attributes"), ATTRIBUTES).unwrap();
    let foreign = script(
        hostile.dir.path(),
        "git",
        "GIT_TEST_ASSUME_DIFFERENT_OWNER=1 exec git \"$@\"",
    );

    let error = hostile
        .summarize_as(foreign.as_os_str(), &hostile.repo)
        .await
        .expect_err("refused");

    assert!(
        error.message.contains("dubious ownership"),
        "{}",
        error.message
    );
}
