//! Workspaces of stopped children whose parent ended or whose source is gone.

use super::*;
use aether_core::event::Actor;

fn worktree_list(repo: &Path) -> String {
    git(repo, &["worktree", "list", "--porcelain"])
}

fn listed(list: &str, path: &Path) -> bool {
    let wanted = format!("worktree {}", path.display());
    list.lines().any(|line| line == wanted)
}

/// The source is a linked worktree of `main`, as a host's run checkout is.
async fn linked_fixture() -> (Fixture, PathBuf) {
    let temp = tempfile::tempdir().unwrap();
    let main = temp.path().join("main");
    let source = temp.path().join("source");
    let managed = temp.path().join("managed");
    fs::create_dir(&main).unwrap();
    git(&main, &["init", "-q"]);
    git(&main, &["config", "user.name", "Test User"]);
    git(&main, &["config", "user.email", "test@example.invalid"]);
    fs::write(main.join("tracked"), "saved\n").unwrap();
    git(&main, &["add", "."]);
    git(&main, &["commit", "-qm", "initial"]);
    let source_arg = source.to_str().unwrap();
    git(&main, &["worktree", "add", "-q", "--detach", source_arg]);
    let manifest = workspace::create(&managed, &source, AgentId::new()).unwrap();
    let (runtime, config) = crate::test_runtime(|config| {
        config.delegated_workspace_root = Some(managed.clone());
    })
    .await;
    runtime.shutdown.cancel();
    let main = fs::canonicalize(main).unwrap();
    let fixture = Fixture {
        _temp: temp,
        _config: config,
        runtime,
        source: manifest.source.clone(),
        managed,
        manifest,
    };
    (fixture, main)
}

#[tokio::test]
async fn killed_child_workspace_is_collected_and_pruned_once_parent_ends() {
    let f = fixture().await;
    let mut ancestor = f.parent(AgentStatus::Ready);
    ancestor.id = AgentId::new();
    let mut parent = f.parent(AgentStatus::Ready);
    parent.parent = Some(ancestor.id);
    f.store(ancestor);
    f.store(parent.clone());
    f.store(f.owner(AgentStatus::Failed {
        error: "deadline passed".into(),
    }));
    f.assert_kept("live or resumable");
    // A stale entry shows the source's worktree list is pruned after removal.
    let stale = f._temp.path().join("stale");
    git(
        &f.source,
        &["worktree", "add", "-q", "--detach", stale.to_str().unwrap()],
    );
    fs::remove_dir_all(&stale).unwrap();

    parent.status = AgentStatus::Completed { summary: None };
    f.store(parent);
    fs::write(f.manifest.workspace.join("tracked"), "unsaved\n").unwrap();
    f.assert_kept("unsaved file bytes");
    fs::write(f.manifest.workspace.join("tracked"), "saved\n").unwrap();

    let preview = f.collect(true);
    assert_eq!(preview.workspaces[0].action, WorkspaceGcAction::WouldRemove);
    assert!(f.manifest.workspace.exists());
    assert!(listed(&worktree_list(&f.source), &f.manifest.workspace));

    let report = f.collect(false);
    assert_eq!(
        report.workspaces[0].action,
        WorkspaceGcAction::Removed,
        "{report:?}"
    );
    assert!(!f.manifest.workspace.exists());
    let after = worktree_list(&f.source);
    assert!(!listed(&after, &f.manifest.workspace), "{after}");
    assert!(!after.contains(stale.to_str().unwrap()), "{after}");
}

#[tokio::test]
async fn stopped_child_cannot_wake_or_restart_into_a_collected_workspace() {
    let f = fixture().await;
    let mut parent = f.parent(AgentStatus::Failed {
        error: "stage failed".into(),
    });
    f.store(parent.clone());
    let child = f.owner(AgentStatus::Failed {
        error: "deadline passed".into(),
    });
    f.store(child.clone());
    assert_eq!(
        f.collect(false).workspaces[0].action,
        WorkspaceGcAction::Removed
    );
    // The operator wakes the parent; its child must still not resume.
    parent.status = AgentStatus::Ready;
    f.store(parent);
    let woken = f.runtime.wake_agent(child.id, Actor::user()).unwrap_err();
    assert!(woken.to_string().contains("was collected"), "{woken}");
    let restarted = f
        .runtime
        .restart_agent(child.id, Actor::user())
        .unwrap_err();
    assert!(
        restarted.to_string().contains("was collected"),
        "{restarted}"
    );
}

#[tokio::test]
async fn workspace_of_a_gone_source_is_collected_while_its_parent_lives() {
    let (f, main) = linked_fixture().await;
    f.store(f.parent(AgentStatus::Ready));
    let mut child = f.owner(AgentStatus::Running);
    f.store(child.clone());
    git(&main, &["worktree", "remove", f.source.to_str().unwrap()]);
    assert!(listed(&worktree_list(&main), &f.manifest.workspace));
    f.assert_kept("live or resumable");

    child.status = AgentStatus::Cancelled { reason: None };
    f.store(child);
    fs::write(f.manifest.workspace.join("untracked"), "keep me").unwrap();
    f.assert_kept("dirty, untracked, or ignored");
    fs::remove_file(f.manifest.workspace.join("untracked")).unwrap();

    let preview = f.collect(true);
    assert_eq!(preview.workspaces[0].action, WorkspaceGcAction::WouldRemove);
    assert!(f.manifest.workspace.exists());
    let report = f.collect(false);
    let entry = &report.workspaces[0];
    assert_eq!(entry.action, WorkspaceGcAction::Removed, "{entry:?}");
    assert!(
        entry.reason.contains("source checkout is gone"),
        "{entry:?}"
    );
    assert!(!f.manifest.workspace.exists());
    assert!(!listed(&worktree_list(&main), &f.manifest.workspace));
}

#[tokio::test]
async fn gone_source_keeps_workspace_commits_missing_elsewhere() {
    let (f, main) = linked_fixture().await;
    fs::write(f.manifest.workspace.join("tracked"), "child work\n").unwrap();
    git(&f.manifest.workspace, &["commit", "-qam", "child work"]);
    git(&main, &["worktree", "remove", f.source.to_str().unwrap()]);
    f.assert_kept("commits missing");
    let preview = f.collect(true);
    assert_eq!(preview.workspaces[0].action, WorkspaceGcAction::Kept);
    assert!(listed(&worktree_list(&main), &f.manifest.workspace));
}
