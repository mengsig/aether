#![allow(clippy::unwrap_used)]

use super::*;
use aether_core::agent::{AgentConfig, AgentStats, AgentStatus, Role};
use aether_core::id::{AgentId, RunId};
use aether_core::model::ModelRef;
use aether_store::{Commit, Mutation};
use std::os::unix::fs::{PermissionsExt, symlink};
use std::process::Command;
use std::sync::Arc;

struct Fixture {
    _temp: tempfile::TempDir,
    _config: tempfile::TempDir,
    runtime: Arc<Runtime>,
    source: PathBuf,
    managed: PathBuf,
    manifest: workspace::WorkspaceManifest,
}

fn git(repo: &Path, args: &[&str]) -> String {
    let output = Command::new("git")
        .arg("-C")
        .arg(repo)
        .args(args)
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8(output.stdout).unwrap().trim().to_owned()
}

async fn fixture() -> Fixture {
    let temp = tempfile::tempdir().unwrap();
    let source = temp.path().join("source");
    let managed = temp.path().join("managed");
    fs::create_dir(&source).unwrap();
    git(&source, &["init", "-q"]);
    git(&source, &["config", "user.name", "Test User"]);
    git(&source, &["config", "user.email", "test@example.invalid"]);
    fs::write(source.join("tracked"), "saved\n").unwrap();
    fs::write(source.join(".gitignore"), "ignored\n").unwrap();
    git(&source, &["add", "."]);
    git(&source, &["commit", "-qm", "initial"]);
    let manifest = workspace::create(&managed, &source, AgentId::new()).unwrap();
    let (runtime, config) = crate::test_runtime(|config| {
        config.delegated_workspace_root = Some(managed.clone());
    })
    .await;
    runtime.shutdown.cancel();
    Fixture {
        _temp: temp,
        _config: config,
        runtime,
        source,
        managed,
        manifest,
    }
}

impl Fixture {
    fn collect(&self, dry_run: bool) -> WorkspaceGcReport {
        self.runtime.gc_workspaces(Duration::ZERO, dry_run).unwrap()
    }

    fn owner(&self, status: AgentStatus) -> Agent {
        let at = aether_core::time::now() - chrono::Duration::days(30);
        Agent {
            id: AgentId::new(),
            parent: Some(self.manifest.parent),
            role: Role::new("worker"),
            name: "gc-test".into(),
            scope: Scope {
                workspace_root: Some(self.manifest.workspace.clone()),
                ..Default::default()
            },
            model: ModelRef::new("mock", "test"),
            fallbacks: vec![],
            status,
            current_task: None,
            inbox_cursor: 0,
            checkpoint: None,
            engine_session: None,
            engine_usage: None,
            engine_sent: None,
            status_since: None,
            archived: false,
            paused: false,
            paused_via: None,
            resume_wait: None,
            restarted_from: None,
            restarted_as: None,
            config: AgentConfig::default(),
            stats: AgentStats::default(),
            run: RunId::new(),
            created_at: at,
            updated_at: at,
        }
    }

    fn store(&self, agent: Agent) {
        self.runtime
            .apply(Commit::new().mutate(Mutation::UpsertAgent(agent)))
            .unwrap();
    }

    fn assert_kept(&self, needle: &str) {
        let report = self.collect(false);
        assert_eq!(report.workspaces.len(), 1);
        let entry = &report.workspaces[0];
        assert_eq!(entry.action, WorkspaceGcAction::Kept, "{entry:?}");
        assert!(entry.reason.contains(needle), "{entry:?}");
        assert!(self.manifest.workspace.exists());
        assert_eq!(report.bytes_removed, 0);
    }
}

#[tokio::test]
async fn gc_removes_only_provably_orphaned_snapshot_references() {
    let f = fixture().await;
    let head = git(&f.source, &["rev-parse", "HEAD"]);
    let tree = git(&f.source, &["rev-parse", "HEAD^{tree}"]);
    let snapshot = |date: &str, email: &str, id: &str| {
        let output = std::process::Command::new("git")
            .current_dir(&f.source)
            .env("GIT_COMMITTER_DATE", date)
            .env("GIT_COMMITTER_EMAIL", email)
            .env("GIT_AUTHOR_EMAIL", email)
            .args(["commit-tree", &tree, "-m"])
            .arg(format!("Aether child workspace snapshot {id}"))
            .output()
            .unwrap();
        assert!(output.status.success());
        String::from_utf8(output.stdout).unwrap().trim().to_owned()
    };
    let aether = "aether@local.invalid";
    let (long_ago, now) = (
        "2020-01-01T00:00:00Z",
        aether_core::time::now().to_rfc3339(),
    );
    let new_id = || ulid::Ulid::generate().to_string();
    let (orphan_id, flight_id, user_id, borrowed_id, plain_id) =
        (new_id(), new_id(), new_id(), new_id(), new_id());
    let old = snapshot(long_ago, aether, &orphan_id);
    let fresh = snapshot(&now, aether, &flight_id);
    let users_commit = snapshot(long_ago, "someone@example.invalid", &user_id);
    let name = |id: &str| format!("refs/aether/workspaces/{id}");
    let (orphan, in_flight, users_ulid, borrowed, plain) = (
        name(&orphan_id),
        name(&flight_id),
        name(&user_id),
        name(&borrowed_id),
        name(&plain_id),
    );
    let foreign = "refs/aether/workspaces/not-a-ulid";
    let branch = "refs/heads/aether-workspaces";
    for (name, commit) in [
        (orphan.as_str(), &old),
        (in_flight.as_str(), &fresh),
        (users_ulid.as_str(), &users_commit),
        (borrowed.as_str(), &old),
        (plain.as_str(), &head),
        (foreign, &old),
        (branch, &old),
    ] {
        git(&f.source, &["update-ref", name, commit]);
    }

    f.collect(true);
    assert!(
        !git(&f.source, &["for-each-ref", &orphan]).is_empty(),
        "a preview deletes nothing"
    );
    f.collect(false);

    assert!(git(&f.source, &["for-each-ref", &orphan]).is_empty());
    for kept in [&in_flight, &users_ulid, &borrowed, &plain, foreign, branch] {
        assert!(
            !git(&f.source, &["for-each-ref", kept]).is_empty(),
            "{kept}"
        );
    }
}

#[tokio::test]
async fn dry_run_reports_bytes_then_completed_owner_is_collected() {
    let f = fixture().await;
    f.store(f.owner(AgentStatus::Completed { summary: None }));
    let before_refs = git(&f.source, &["show-ref"]);
    let report = f.collect(true);
    assert_eq!(
        report.workspaces[0].action,
        WorkspaceGcAction::WouldRemove,
        "{report:?}"
    );
    assert!(report.workspaces[0].bytes_complete);
    assert!(report.bytes_eligible > 0);
    assert_eq!(report.bytes_removed, 0);
    assert_eq!(report.bytes_kept, report.bytes_eligible);
    assert!(f.manifest.workspace.exists());
    assert!(!f.managed.join(".integration.lock").exists());
    serde_json::to_string(&report).unwrap();
    let removed = f.collect(false);
    assert_eq!(
        removed.workspaces[0].action,
        WorkspaceGcAction::Removed,
        "{removed:?}"
    );
    assert_eq!(removed.bytes_removed, report.bytes_eligible);
    assert!(!f.manifest.workspace.exists());
    assert_eq!(git(&f.source, &["show-ref"]), before_refs);
    assert_eq!(
        git(&f.source, &["worktree", "list", "--porcelain"])
            .matches("worktree ")
            .count(),
        1
    );
}

#[tokio::test]
async fn dirty_untracked_and_ignored_files_are_preserved() {
    let f = fixture().await;
    fs::write(f.manifest.workspace.join("tracked"), "unsaved\n").unwrap();
    f.assert_kept("unsaved file bytes");
    fs::write(f.manifest.workspace.join("tracked"), "saved\n").unwrap();
    for name in ["untracked", "ignored"] {
        fs::write(f.manifest.workspace.join(name), "keep me").unwrap();
        f.assert_kept("dirty, untracked, or ignored");
        fs::remove_file(f.manifest.workspace.join(name)).unwrap();
    }
}

#[tokio::test]
async fn assume_unchanged_cannot_hide_unsaved_bytes() {
    let f = fixture().await;
    git(
        &f.manifest.workspace,
        &["update-index", "--assume-unchanged", "tracked"],
    );
    fs::write(f.manifest.workspace.join("tracked"), "unsaved\n").unwrap();
    assert!(git(&f.manifest.workspace, &["status", "--porcelain"]).is_empty());
    f.assert_kept("unsaved file bytes");
}

#[tokio::test]
async fn clean_index_flags_keep_workspaces_in_preview_and_collection() {
    for flag in ["--assume-unchanged", "--skip-worktree"] {
        let f = fixture().await;
        git(&f.manifest.workspace, &["update-index", flag, "tracked"]);
        let preview = f.collect(true);
        assert_eq!(preview.workspaces[0].action, WorkspaceGcAction::Kept);
        assert!(preview.workspaces[0].reason.contains("workspace index"));
        f.assert_kept("workspace index");
        assert_eq!(
            fs::read_to_string(f.manifest.workspace.join("tracked")).unwrap(),
            "saved\n"
        );
    }
}

#[tokio::test]
async fn recheck_preserves_edits_hidden_by_new_index_flags() {
    for flag in ["--assume-unchanged", "--skip-worktree"] {
        let f = fixture().await;
        let mut candidate =
            workspace::gc::Candidate::new(f.manifest.workspace.parent().unwrap().to_owned());
        let manifest = candidate.inspect(&f.managed).unwrap();
        git(&manifest.workspace, &["update-index", flag, "tracked"]);
        fs::write(manifest.workspace.join("tracked"), "concurrent edit\n").unwrap();
        assert!(git(&manifest.workspace, &["status", "--porcelain"]).is_empty());
        let error = candidate.recheck(&manifest).unwrap_err();
        assert!(error.to_string().contains("changed during GC"), "{error}");
        assert_eq!(
            fs::read_to_string(manifest.workspace.join("tracked")).unwrap(),
            "concurrent edit\n"
        );
        f.assert_kept("unsaved file bytes");
    }
}

#[tokio::test]
async fn live_paused_waiting_and_restartable_owners_are_preserved() {
    let f = fixture().await;
    let mut agent = f.owner(AgentStatus::Created);
    f.store(agent.clone());
    f.assert_kept("live or resumable");
    agent.paused = true;
    agent.status = AgentStatus::Waiting {
        condition: aether_core::agent::WaitCondition::Inbox,
    };
    f.store(agent.clone());
    f.assert_kept("live or resumable");
    agent.status = AgentStatus::Cancelled { reason: None };
    f.store(agent.clone());
    f.assert_kept("live or resumable");
    agent.status = AgentStatus::Failed {
        error: "restartable".into(),
    };
    f.store(agent);
    f.assert_kept("live or resumable");
}

#[tokio::test]
async fn completed_projection_with_running_turn_is_preserved() {
    let f = fixture().await;
    let agent = f.owner(AgentStatus::Completed { summary: None });
    f.store(agent.clone());
    let cancel = tokio_util::sync::CancellationToken::new();
    f.runtime.sched.lock().running.insert(
        agent.id,
        crate::RunningTurn {
            task: None,
            interrupt: cancel.child_token(),
            cancel,
            cancel_request: None,
            stop_reason: None,
            addressed: false,
            provider: "mock".into(),
            engine: true,
            on_provider: false,
        },
    );
    f.assert_kept("live or resumable");
}

#[tokio::test]
async fn unique_commits_require_another_source_ref() {
    let f = fixture().await;
    fs::write(f.manifest.workspace.join("tracked"), "committed\n").unwrap();
    git(&f.manifest.workspace, &["commit", "-qam", "child work"]);
    f.assert_kept("commits missing");
    let head = git(&f.manifest.workspace, &["rev-parse", "HEAD"]);
    git(&f.source, &["update-ref", "refs/heads/saved-child", &head]);
    assert_eq!(
        f.collect(false).workspaces[0].action,
        WorkspaceGcAction::Removed
    );
}

#[tokio::test]
async fn unsaved_source_snapshot_and_integration_journals_are_preserved() {
    let f = fixture().await;
    let journal = f
        .manifest
        .workspace
        .parent()
        .unwrap()
        .join("integration.json");
    fs::write(&journal, "{}").unwrap();
    f.assert_kept("integration journal");
    fs::remove_file(&journal).unwrap();
    symlink(f.source.join("missing"), &journal).unwrap();
    f.assert_kept("integration journal");
    fs::remove_file(&journal).unwrap();
    // The second candidate captures source bytes absent from every saved ref.
    fs::write(f.source.join("tracked"), "unsaved source\n").unwrap();
    let second = workspace::create(&f.managed, &f.source, AgentId::new()).unwrap();
    let report = f.collect(false);
    let entry = report
        .workspaces
        .iter()
        .find(|entry| entry.workspace == second.workspace)
        .unwrap();
    assert_eq!(entry.action, WorkspaceGcAction::Kept);
    assert!(entry.reason.contains("baseline is not saved"), "{entry:?}");
}

fn preview(grace: Duration, now: SystemTime) -> Sweep {
    Sweep {
        grace,
        dry_run: true,
        prune_kept: false,
        now,
    }
}

#[tokio::test]
async fn grace_boundary_is_inclusive_and_future_timestamps_stay_protected() {
    let f = fixture().await;
    let mut candidate =
        workspace::gc::Candidate::new(f.manifest.workspace.parent().unwrap().to_owned());
    candidate.measure(true, &[]).unwrap();
    let latest = candidate.modified;
    let grace = Duration::from_secs(100);
    let just_before = latest + grace - Duration::from_nanos(1);
    let report = f
        .runtime
        .gc_workspaces_at(preview(grace, just_before))
        .unwrap();
    assert_eq!(report.workspaces[0].action, WorkspaceGcAction::Kept);
    let report = f
        .runtime
        .gc_workspaces_at(preview(grace, latest + grace))
        .unwrap();
    assert_eq!(
        report.workspaces[0].action,
        WorkspaceGcAction::WouldRemove,
        "{report:?}"
    );
    assert!(!grace_elapsed(
        latest,
        latest + Duration::from_secs(1),
        Duration::ZERO
    ));
}

#[cfg(target_os = "linux")]
#[tokio::test]
async fn mode000_directories_and_files_are_removed_without_following_symlinks() {
    let f = fixture().await;
    let outside = f.source.join("outside");
    fs::write(&outside, "outside remains").unwrap();
    fs::set_permissions(&outside, fs::Permissions::from_mode(0o000)).unwrap();
    // Track a symlink so it is saved work, without granting access to its target.
    symlink(&outside, f.manifest.workspace.join("link")).unwrap();
    git(&f.manifest.workspace, &["add", "link"]);
    git(&f.manifest.workspace, &["commit", "-qm", "saved symlink"]);
    let head = git(&f.manifest.workspace, &["rev-parse", "HEAD"]);
    git(&f.source, &["update-ref", "refs/heads/saved-child", &head]);
    let empty = f.manifest.workspace.join("empty");
    fs::create_dir(&empty).unwrap();
    for path in [
        &empty,
        &f.manifest.workspace.join("tracked"),
        &f.manifest.workspace,
    ] {
        fs::set_permissions(path, fs::Permissions::from_mode(0o000)).unwrap();
    }
    let preview = f.collect(true);
    assert_eq!(preview.workspaces[0].action, WorkspaceGcAction::Kept);
    assert!(!preview.workspaces[0].bytes_complete);
    assert_eq!(
        fs::metadata(&f.manifest.workspace)
            .unwrap()
            .permissions()
            .mode()
            & 0o777,
        0
    );
    let report = f.collect(false);
    assert_eq!(
        report.workspaces[0].action,
        WorkspaceGcAction::Removed,
        "{report:?}"
    );
    assert_eq!(
        fs::metadata(&outside).unwrap().permissions().mode() & 0o777,
        0
    );
    fs::set_permissions(&outside, fs::Permissions::from_mode(0o600)).unwrap();
    assert_eq!(fs::read_to_string(outside).unwrap(), "outside remains");
}

#[cfg(target_os = "linux")]
#[tokio::test]
async fn permission_repair_is_restored_when_dirty_candidate_is_kept() {
    let f = fixture().await;
    let tracked = f.manifest.workspace.join("tracked");
    fs::write(&tracked, "unsaved").unwrap();
    fs::set_permissions(&tracked, fs::Permissions::from_mode(0o000)).unwrap();
    f.assert_kept("unsaved file bytes");
    assert_eq!(
        fs::metadata(tracked).unwrap().permissions().mode() & 0o777,
        0
    );
}

#[tokio::test]
async fn copied_manifest_cannot_redirect_collection_to_a_live_worktree() {
    let f = fixture().await;
    f.store(f.owner(AgentStatus::Created));
    let alias = f.managed.join(ulid::Ulid::generate().to_string());
    fs::create_dir(&alias).unwrap();
    fs::write(
        alias.join("manifest.json"),
        serde_json::to_vec(&f.manifest).unwrap(),
    )
    .unwrap();
    symlink(&f.manifest.workspace, alias.join("workspace")).unwrap();
    let before_worktrees = git(&f.source, &["worktree", "list", "--porcelain"]);
    for dry_run in [true, false] {
        let report = f.collect(dry_run);
        assert_eq!(report.workspaces.len(), 2);
        assert!(
            report
                .workspaces
                .iter()
                .all(|entry| entry.action == WorkspaceGcAction::Kept)
        );
        let redirected = report
            .workspaces
            .iter()
            .find(|entry| entry.workspace == alias.join("workspace"))
            .unwrap();
        assert!(
            redirected.reason.contains("real directory"),
            "{redirected:?}"
        );
        assert_eq!(report.bytes_removed, 0);
        assert!(f.manifest.workspace.exists());
        assert_eq!(
            git(&f.source, &["worktree", "list", "--porcelain"]),
            before_worktrees
        );
    }
}

#[tokio::test]
async fn symlink_candidate_is_kept_and_admission_rechecks_deleted_workspace() {
    let f = fixture().await;
    let alias = f.managed.join(ulid::Ulid::generate().to_string());
    symlink(&f.source, &alias).unwrap();
    let scope = Scope {
        workspace_root: Some(f.manifest.workspace.clone()),
        ..Default::default()
    };
    let scope_alias = f._temp.path().join("workspace-alias");
    symlink(&f.manifest.workspace, &scope_alias).unwrap();
    let aliased = Scope {
        workspace_root: Some(scope_alias),
        ..Default::default()
    };
    assert!(f.runtime.recheck_workspace_gc_admission(&scope).is_ok());
    let report = f.collect(false);
    assert!(
        report
            .workspaces
            .iter()
            .any(|entry| entry.reason.contains("real directory"))
    );
    assert!(f.source.join("tracked").exists());
    assert!(f.runtime.recheck_workspace_gc_admission(&scope).is_err());
    assert!(f.runtime.recheck_workspace_gc_admission(&aliased).is_err());
    let before = f
        .runtime
        .store
        .agents(&AgentFilter::default())
        .unwrap()
        .len();
    let error = f
        .runtime
        .create_agent(
            crate::CreateAgentSpec {
                model: Some(ModelRef::new("claude-code", "haiku")),
                scope: Some(scope),
                ..Default::default()
            },
            aether_core::event::Actor::user(),
        )
        .unwrap_err();
    assert!(
        error
            .to_string()
            .contains("disappeared before agent admission"),
        "{error}"
    );
    assert_eq!(
        f.runtime
            .store
            .agents(&AgentFilter::default())
            .unwrap()
            .len(),
        before
    );
}

#[tokio::test]
async fn recent_final_owner_and_quarantined_resources_prevent_collection() {
    let f = fixture().await;
    let mut agent = f.owner(AgentStatus::Completed { summary: None });
    agent.updated_at = aether_core::time::now();
    f.store(agent.clone());
    let report = f
        .runtime
        .gc_workspaces(Duration::from_secs(3600), false)
        .unwrap();
    assert_eq!(report.workspaces[0].action, WorkspaceGcAction::Kept);
    assert!(report.workspaces[0].reason.contains("grace period"));
    f.runtime.resources.quarantine(
        agent.id,
        vec![aether_core::tool::ResourceKey("file:test".into())],
    );
    f.assert_kept("live or resumable");
    f.runtime.resources.release_quarantine(agent.id);
}

#[tokio::test]
async fn candidate_limit_reports_partial_byte_scope() {
    let f = fixture().await;
    for _ in 0..MAX_CANDIDATES {
        fs::create_dir(f.managed.join(ulid::Ulid::generate().to_string())).unwrap();
    }
    let report = f.collect(true);
    assert!(report.truncated);
    assert_eq!(report.workspaces.len(), MAX_CANDIDATES);
    assert_eq!(report.bytes_removed, 0);
    let actual = f.collect(false);
    assert_eq!(
        actual
            .workspaces
            .iter()
            .map(|entry| &entry.workspace)
            .collect::<Vec<_>>(),
        report
            .workspaces
            .iter()
            .map(|entry| &entry.workspace)
            .collect::<Vec<_>>()
    );
    let next = f.collect(false);
    assert!(!next.truncated);
    assert_eq!(next.workspaces.len(), 1);
    assert!(
        !report
            .workspaces
            .iter()
            .any(|entry| entry.workspace == next.workspaces[0].workspace)
    );
}

#[tokio::test]
async fn candidate_branch_is_not_an_independent_saved_ref() {
    let f = fixture().await;
    git(&f.manifest.workspace, &["checkout", "-qb", "private-child"]);
    fs::write(f.manifest.workspace.join("tracked"), "private commit\n").unwrap();
    git(
        &f.manifest.workspace,
        &["commit", "-qam", "private child work"],
    );
    f.assert_kept("attached to a branch");
}

#[tokio::test]
async fn active_gc_reservation_rejects_creation_until_released() {
    let f = fixture().await;
    let scope = Scope {
        workspace_root: Some(f.manifest.workspace.clone()),
        ..Default::default()
    };
    *f.runtime.workspace_gc_active.lock() = scope.workspace_root.clone();
    let error = f
        .runtime
        .create_agent(
            crate::CreateAgentSpec {
                model: Some(ModelRef::new("claude-code", "haiku")),
                scope: Some(scope.clone()),
                ..Default::default()
            },
            aether_core::event::Actor::user(),
        )
        .unwrap_err();
    assert!(
        error.to_string().contains("being inspected for collection"),
        "{error}"
    );
    f.runtime.workspace_gc_active.lock().take();
    assert!(f.runtime.recheck_workspace_gc_admission(&scope).is_ok());
}

#[tokio::test]
async fn preview_does_not_reserve_admission_or_take_maintenance_locks() {
    let f = fixture().await;
    let integration = workspace::gc::integration_lock(&f.managed, false).unwrap();
    let sweep = f.runtime.storage_gc_lock.lock();
    let hold = f.runtime.hold();
    let existing_reservation = f.source.clone();
    *f.runtime.workspace_gc_active.lock() = Some(existing_reservation.clone());
    let runtime = f.runtime.clone();
    let (tx, rx) = std::sync::mpsc::channel();
    let worker = std::thread::spawn(move || {
        let preview = runtime.gc_workspaces(Duration::ZERO, true);
        let status = runtime.storage_gc(true);
        tx.send((preview, status)).unwrap();
    });
    let result = rx.recv_timeout(Duration::from_secs(10));
    // Release every lock before joining even if the regression times out.
    drop(hold);
    drop(sweep);
    drop(integration);
    worker.join().unwrap();
    let (preview, status) = result.expect("preview must not wait for mutation locks");
    assert_eq!(
        preview.unwrap().workspaces[0].action,
        WorkspaceGcAction::WouldRemove
    );
    assert!(status.unwrap()["dry_run"].as_bool().unwrap());
    assert_eq!(
        *f.runtime.workspace_gc_active.lock(),
        Some(existing_reservation)
    );
    assert!(f.manifest.workspace.exists());
}

#[tokio::test]
async fn unrelated_home_scope_does_not_own_completed_workspace() {
    let f = fixture().await;
    f.store(f.owner(AgentStatus::Completed { summary: None }));
    let mut broad = f.owner(AgentStatus::Created);
    broad.parent = None;
    broad.scope.workspace_root = Some(f._temp.path().to_path_buf());
    broad.scope.allowed_paths = vec![f.manifest.workspace.clone()];
    f.store(broad);
    assert_eq!(
        f.collect(true).workspaces[0].action,
        WorkspaceGcAction::WouldRemove
    );
    assert_eq!(
        f.collect(false).workspaces[0].action,
        WorkspaceGcAction::Removed
    );
}

#[tokio::test]
async fn live_agent_inside_workspace_protects_it_without_delegation_relation() {
    let f = fixture().await;
    let mut agent = f.owner(AgentStatus::Created);
    agent.parent = None;
    // A missing descendant is still a scoped path, resolved by existing prefix.
    agent.scope.workspace_root = Some(f.manifest.workspace.join("nested"));
    f.store(agent);
    f.assert_kept("live or resumable");
}

#[tokio::test]
async fn manifest_parent_and_delegation_chain_protect_workspace() {
    let f = fixture().await;
    let mut parent = f.owner(AgentStatus::Created);
    parent.id = f.manifest.parent;
    parent.parent = None;
    parent.scope.workspace_root = Some(f.source.clone());
    f.store(parent.clone());
    f.assert_kept("live or resumable");
    parent.status = AgentStatus::Completed { summary: None };
    f.store(parent.clone());
    let mut descendant = f.owner(AgentStatus::Created);
    descendant.scope.workspace_root = Some(f.source.clone());
    f.store(descendant.clone());
    f.assert_kept("live or resumable");
    descendant.status = AgentStatus::Completed { summary: None };
    f.store(descendant);
    let mut ancestor = f.owner(AgentStatus::Created);
    ancestor.parent = None;
    ancestor.scope.workspace_root = Some(f.source.clone());
    parent.parent = Some(ancestor.id);
    f.store(parent);
    f.store(ancestor);
    f.assert_kept("live or resumable");
}

#[tokio::test]
async fn explicit_workspace_resource_lease_protects_unrelated_owner() {
    let f = fixture().await;
    let mut broad = f.owner(AgentStatus::Completed { summary: None });
    broad.parent = None;
    broad.scope.workspace_root = Some(f._temp.path().to_path_buf());
    f.store(broad.clone());
    f.runtime.resources.quarantine(
        broad.id,
        vec![aether_core::tool::ResourceKey::file(
            f.manifest.workspace.join("tracked"),
        )],
    );
    f.assert_kept("explicit resource lease");
    f.runtime.resources.release_quarantine(broad.id);
    assert_eq!(
        f.collect(false).workspaces[0].action,
        WorkspaceGcAction::Removed
    );
}

#[tokio::test]
async fn completed_workspace_owner_with_durable_turn_lease_is_kept() {
    let f = fixture().await;
    let owner = f.owner(AgentStatus::Completed { summary: None });
    f.store(owner.clone());
    let lease = aether_core::lease::Lease::new(
        owner.id,
        "test-holder",
        1,
        aether_core::time::now(),
        Duration::from_secs(60),
    );
    f.runtime
        .apply(Commit::new().mutate(Mutation::SetLease(lease)))
        .unwrap();
    f.assert_kept("live or resumable");
}

#[tokio::test]
async fn preview_reports_each_workspace_protection_reason() {
    let f = fixture().await;
    let mut owner = f.owner(AgentStatus::Created);
    f.store(owner.clone());
    let report = f.collect(true);
    assert_eq!(report.workspaces[0].action, WorkspaceGcAction::Kept);
    assert!(report.workspaces[0].reason.contains("live or resumable"));
    owner.status = AgentStatus::Completed { summary: None };
    f.store(owner);
    fs::write(f.manifest.workspace.join("ignored"), "precious").unwrap();
    let report = f.collect(true);
    assert!(
        report.workspaces[0]
            .reason
            .contains("dirty, untracked, or ignored")
    );
    fs::remove_file(f.manifest.workspace.join("ignored")).unwrap();
    fs::write(f.manifest.workspace.join("tracked"), "unique\n").unwrap();
    git(&f.manifest.workspace, &["commit", "-qam", "unique work"]);
    let report = f.collect(true);
    assert_eq!(report.workspaces[0].action, WorkspaceGcAction::Kept);
    assert!(report.workspaces[0].reason.contains("commits missing"));
    assert!(f.manifest.workspace.exists());
}

mod build_output;
