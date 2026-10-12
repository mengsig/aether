use std::os::unix::fs::{PermissionsExt as _, symlink};
use std::path::{Path, PathBuf};

use aether_core::agent::{Agent, AgentConfig, AgentStatus, Scope};
use aether_core::event::Actor;
use aether_core::model::Effort;
use aether_core::time::now;
use aether_protocol::remote::{Workspace, WorkspaceValidation};
use aether_protocol::{Command, ErrorCode, Reply};
use aether_runtime::CreateAgentSpec;
use chrono::Duration;

use super::*;
use crate::connection::host_services::dispatch;
use crate::connection::host_services::test_support::{
    commit_file, daemon, init_repo, run_git, running_as_root,
};

fn spec(workspace: &Path, parent: Option<AgentId>) -> CreateAgentSpec {
    CreateAgentSpec {
        role: "worker".into(),
        start: false,
        parent,
        config: Some(AgentConfig {
            effort: Some(Effort::Low),
            ..Default::default()
        }),
        scope: Some(Scope {
            workspace_root: Some(workspace.to_path_buf()),
            ..Default::default()
        }),
        ..Default::default()
    }
}

fn create(daemon: &Daemon, workspace: &Path, parent: Option<AgentId>) -> Agent {
    daemon
        .runtime
        .create_agent(spec(workspace, parent), Actor::user())
        .expect("agent")
        .0
}

/// Rewrite an agent row, the way the scheduler would have left it.
fn amend(daemon: &Daemon, id: AgentId, change: impl FnOnce(&mut Agent)) {
    let mut agent = daemon.runtime.store().agent(id).unwrap().unwrap();
    change(&mut agent);
    daemon
        .runtime
        .apply(aether_store::Commit::new().mutate(aether_store::Mutation::UpsertAgent(agent)))
        .expect("amend");
}

fn stored(daemon: &Daemon) -> Vec<Agent> {
    daemon
        .runtime
        .store()
        .agents(&AgentFilter::default())
        .unwrap()
}

async fn listed(daemon: &Daemon, limit: Option<u32>) -> Vec<Workspace> {
    match dispatch(daemon, Command::ListWorkspaces { limit })
        .await
        .expect("workspaces")
    {
        Reply::Workspaces(listed) => listed,
        other => panic!("wrong reply: {other:?}"),
    }
}

async fn validation(path: &str) -> WorkspaceValidation {
    match validate_workspace(path.into()).await.expect("validation") {
        Reply::WorkspaceValidation(validation) => validation,
        other => panic!("wrong reply: {other:?}"),
    }
}

fn identity(path: &Path) -> (PathBuf, bool) {
    (path.to_path_buf(), true)
}

#[tokio::test]
async fn workspaces_come_most_recently_active_first() {
    let (daemon, _config) = daemon().await;
    let dir = tempfile::tempdir().unwrap();
    let paths: Vec<PathBuf> = ["old", "newest", "middle"]
        .iter()
        .map(|name| {
            let path = dir.path().join(name).canonicalize_or_create();
            create(&daemon, &path, None);
            path
        })
        .collect();
    let base = now();
    for (agent, offset) in stored(&daemon).iter().zip([0, 20, 10]) {
        amend(&daemon, agent.id, |a| {
            a.updated_at = base + Duration::seconds(offset)
        });
    }

    let listed = listed(&daemon, None).await;

    let order: Vec<&Path> = listed.iter().map(|w| w.path.as_path()).collect();
    assert_eq!(
        order,
        [paths[1].as_path(), paths[2].as_path(), paths[0].as_path()]
    );
    assert!(listed.iter().all(|w| w.exists && w.agents == 1));
    daemon.runtime.shutdown().await;
}

trait CreateDir {
    fn canonicalize_or_create(self) -> PathBuf;
}

impl CreateDir for PathBuf {
    fn canonicalize_or_create(self) -> PathBuf {
        std::fs::create_dir_all(&self).unwrap();
        self.canonicalize().unwrap()
    }
}

#[tokio::test]
async fn one_directory_under_two_spellings_is_one_workspace_with_counted_agents() {
    let (daemon, _config) = daemon().await;
    let dir = tempfile::tempdir().unwrap();
    let real = dir.path().join("real").canonicalize_or_create();
    let alias = dir.path().join("alias");
    symlink(&real, &alias).unwrap();
    let root = create(&daemon, &real, None);
    let other_root = create(&daemon, &alias, None);
    let child = create(&daemon, &real, Some(root.id));
    amend(&daemon, child.id, |a| a.status = AgentStatus::Running);

    let listed = listed(&daemon, None).await;

    assert_eq!(listed.len(), 1);
    assert_eq!(listed[0].path, real);
    assert_eq!(listed[0].agents, 3, "two roots and a child");
    assert_eq!(listed[0].running, 1);
    let _ = other_root;
    daemon.runtime.shutdown().await;
}

#[tokio::test]
async fn a_finished_root_still_names_its_workspace_but_counts_no_live_agents() {
    let (daemon, _config) = daemon().await;
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("done").canonicalize_or_create();
    let root = create(&daemon, &path, None);
    amend(&daemon, root.id, |a| {
        a.status = AgentStatus::Completed { summary: None }
    });

    let listed = listed(&daemon, None).await;

    assert_eq!(
        (listed.len(), listed[0].agents, listed[0].running),
        (1, 0, 0)
    );
    assert_eq!(listed[0].path, path);
    daemon.runtime.shutdown().await;
}

#[tokio::test]
async fn a_workspace_that_is_gone_is_flagged_and_has_no_git() {
    let (daemon, _config) = daemon().await;
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("vanishing").canonicalize_or_create();
    create(&daemon, &path, None);
    std::fs::remove_dir(&path).unwrap();

    let listed = listed(&daemon, None).await;

    assert_eq!(listed.len(), 1);
    assert!(!listed[0].exists);
    assert_eq!(listed[0].git, None);
    assert_eq!(listed[0].path, path);
    daemon.runtime.shutdown().await;
}

#[tokio::test]
async fn a_listed_repository_carries_its_git_state() {
    let (daemon, _config) = daemon().await;
    let dir = tempfile::tempdir().unwrap();
    let repo = init_repo(dir.path(), "repo");
    commit_file(&repo, "a.txt", "a");
    std::fs::write(repo.join("b.txt"), "b").unwrap();
    create(&daemon, &repo, None);

    let listed = listed(&daemon, None).await;

    let git = listed[0].git.as_ref().expect("git summary");
    assert!(git.repository && git.dirty);
    assert_eq!(git.branch.as_deref(), Some("main"));
    assert_eq!(git.untracked, 1);
    daemon.runtime.shutdown().await;
}

#[tokio::test]
async fn the_limit_applies_after_ordering_and_zero_is_refused() {
    let (daemon, _config) = daemon().await;
    let dir = tempfile::tempdir().unwrap();
    for n in 0..5 {
        create(
            &daemon,
            &dir.path().join(format!("w{n}")).canonicalize_or_create(),
            None,
        );
    }

    assert_eq!(listed(&daemon, Some(2)).await.len(), 2);
    assert_eq!(listed(&daemon, None).await.len(), 5);
    let error = dispatch(&daemon, Command::ListWorkspaces { limit: Some(0) })
        .await
        .expect_err("zero");
    assert_eq!(error.code, ErrorCode::InvalidArgument);
    daemon.runtime.shutdown().await;
}

#[test]
fn the_daemons_own_root_is_listed_once_and_only_as_recent_as_its_start() {
    let started = now();
    let own = Path::new("/srv/own");
    let alone = known_workspaces(&[], Some(own), started, &identity);
    assert_eq!(alone.len(), 1);
    assert_eq!(
        (alone[0].path.as_path(), alone[0].active_at),
        (own, started)
    );
    assert_eq!(alone[0].agents, 0);
}

#[tokio::test]
async fn the_daemons_own_root_does_not_duplicate_an_agents_workspace() {
    let (daemon, _config) = daemon().await;
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("own").canonicalize_or_create();
    let agent = create(&daemon, &path, None);
    let started = now() - Duration::days(1);

    let known = known_workspaces(&stored(&daemon), Some(&path), started, &identity);

    assert_eq!(known.len(), 1);
    assert_eq!(known[0].agents, 1);
    assert_eq!(
        known[0].active_at, agent.updated_at,
        "the agent's recency wins"
    );
    daemon.runtime.shutdown().await;
}

#[tokio::test]
async fn a_parent_cycle_or_missing_parent_cannot_hang_or_misattribute() {
    let (daemon, _config) = daemon().await;
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("w").canonicalize_or_create();
    let a = create(&daemon, &path, None);
    let b = create(&daemon, &path, Some(a.id));
    amend(&daemon, a.id, |agent| agent.parent = Some(b.id));
    let orphan = create(&daemon, &path, None);
    amend(&daemon, orphan.id, |agent| {
        agent.parent = Some(AgentId::new())
    });

    let agents = stored(&daemon);
    let by_id: HashMap<AgentId, &Agent> = agents.iter().map(|a| (a.id, a)).collect();

    for agent in &agents {
        assert!(
            tree_root(agent, &by_id).is_none(),
            "{} has no root",
            agent.id
        );
    }
    assert!(known_workspaces(&agents, None, now(), &identity).is_empty());
    daemon.runtime.shutdown().await;
}

#[test]
fn home_expansion_covers_only_tilde_and_absolute_paths() {
    let home = Some(Path::new("/home/u"));
    assert_eq!(expand_home("~", home), Some("/home/u".into()));
    assert_eq!(expand_home("~/src/x", home), Some("/home/u/src/x".into()));
    assert_eq!(expand_home("/opt/x", None), Some("/opt/x".into()));
    for bad in ["~root", "~root/x", "src", "./src", ""] {
        assert_eq!(expand_home(bad, home), None, "{bad:?}");
    }
    assert_eq!(expand_home("~/x", None), None);
}

#[tokio::test]
async fn a_usable_directory_validates_to_its_canonical_path_and_git_state() {
    let dir = tempfile::tempdir().unwrap();
    let repo = init_repo(dir.path(), "repo");
    commit_file(&repo, "a.txt", "a");
    std::fs::create_dir(repo.join("sub")).unwrap();
    let alias = dir.path().join("alias");
    symlink(repo.join("sub"), &alias).unwrap();

    let found = validation(alias.to_str().unwrap()).await;

    assert!(found.valid);
    assert_eq!(found.reason, None);
    assert_eq!(found.path.as_deref(), Some(repo.join("sub").as_path()));
    let git = found.git.expect("git");
    assert!(git.repository);
    assert_eq!(git.root.as_deref(), Some(repo.as_path()));
}

#[tokio::test]
async fn a_plain_directory_is_valid_and_not_a_repository() {
    let dir = tempfile::tempdir().unwrap();

    let found = validation(dir.path().to_str().unwrap()).await;

    assert!(found.valid);
    assert!(!found.git.expect("git").repository);
}

#[tokio::test]
async fn things_that_cannot_host_an_agent_say_why_without_being_errors() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().canonicalize().unwrap();
    std::fs::write(root.join("file"), "").unwrap();

    let file = validation(root.join("file").to_str().unwrap()).await;
    assert!(!file.valid);
    assert_eq!(file.reason.as_deref(), Some("not a directory"));
    assert_eq!(
        file.path,
        Some(root.join("file")),
        "it resolved, so it is named"
    );
    assert_eq!(file.git, None);

    let missing = validation(root.join("missing").to_str().unwrap()).await;
    assert!(!missing.valid);
    assert_eq!(missing.reason.as_deref(), Some("no such directory"));
    assert_eq!(missing.path, None);

    for relative in ["relative/dir", "./here", "~someone/else"] {
        let found = validation(relative).await;
        assert!(!found.valid, "{relative}");
        assert!(found.reason.is_some() && found.path.is_none());
    }
}

#[tokio::test]
async fn a_directory_the_daemon_user_cannot_enter_is_not_valid() {
    if running_as_root() {
        return;
    }
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().canonicalize().unwrap();
    let closed = root.join("closed");
    std::fs::create_dir(&closed).unwrap();
    std::fs::set_permissions(&closed, std::fs::Permissions::from_mode(0o600)).unwrap();
    let blocked = root.join("blocked");
    std::fs::create_dir_all(blocked.join("inner")).unwrap();
    std::fs::set_permissions(&blocked, std::fs::Permissions::from_mode(0o000)).unwrap();

    let no_enter = validation(closed.to_str().unwrap()).await;
    let behind = validation(blocked.join("inner").to_str().unwrap()).await;

    std::fs::set_permissions(&blocked, std::fs::Permissions::from_mode(0o755)).unwrap();
    std::fs::set_permissions(&closed, std::fs::Permissions::from_mode(0o755)).unwrap();
    assert!(!no_enter.valid);
    assert_eq!(no_enter.path, Some(closed));
    assert!(!behind.valid);
    assert_eq!(behind.reason.as_deref(), Some("permission denied"));
}

#[tokio::test]
async fn hostile_paths_are_typed_errors_not_validation_replies() {
    for bad in [
        "",
        "/tm\0p",
        "/tmp/\n",
        "/tmp/\u{1b}[0m",
        &format!("/{}", "a".repeat(5000)),
    ] {
        let error = validate_workspace(bad.into()).await.expect_err("refused");
        assert_eq!(error.code, ErrorCode::InvalidArgument, "{bad:?}");
    }
}

#[tokio::test]
async fn the_git_summary_command_checks_its_directory() {
    let dir = tempfile::tempdir().unwrap();
    let repo = init_repo(dir.path(), "repo");
    commit_file(&repo, "a.txt", "a");

    let Reply::GitSummary(summary) = git_summary(repo.clone()).await.expect("summary") else {
        panic!("wrong reply");
    };
    assert_eq!(summary.branch.as_deref(), Some("main"));

    for bad in [
        PathBuf::from("relative"),
        dir.path().join("missing"),
        repo.join("a.txt"),
        PathBuf::from("/tm\0p"),
    ] {
        let error = git_summary(bad.clone()).await.expect_err("refused");
        assert_eq!(error.code, ErrorCode::InvalidArgument, "{bad:?}");
    }
}

#[tokio::test]
async fn no_command_runs_a_filter_hidden_in_work_tree_scoped_config() {
    let (daemon, _config) = daemon().await;
    let dir = tempfile::tempdir().unwrap();
    let repo = init_repo(dir.path(), "repo");
    commit_file(&repo, "a.txt", "a");
    let marker = dir.path().join("filter-ran");
    let filter = format!("touch {}; cat", marker.display());
    run_git(&repo, &["config", "extensions.worktreeConfig", "true"]);
    run_git(
        &repo,
        &["config", "--worktree", "filter.evil.clean", &filter],
    );
    std::fs::write(repo.join(".git/info/attributes"), "*.txt filter=evil\n").unwrap();
    std::fs::write(repo.join("a.txt"), "b").unwrap();
    create(&daemon, &repo, None);

    let listed = listed(&daemon, None).await;
    let validated = validation(repo.to_str().unwrap()).await;
    let Reply::GitSummary(asked) = git_summary(repo.clone()).await.expect("summary") else {
        panic!("wrong reply");
    };

    assert!(!marker.exists(), "the filter command must not have run");
    for summary in [listed[0].git.as_ref(), validated.git.as_ref(), Some(&asked)] {
        assert_eq!(summary.expect("git summary").modified, 1);
    }
    daemon.runtime.shutdown().await;
}
