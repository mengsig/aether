use super::*;

const TARGET_FILES: usize = 16_400;

impl Fixture {
    /// Ignore `pattern` through the shared exclude file, so the workspace
    /// needs no commit of its own.
    fn exclude(&self, pattern: &str) {
        let path = self.source.join(".git/info/exclude");
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        let mut existing = fs::read_to_string(&path).unwrap_or_default();
        existing.push_str(&format!("{pattern}\n"));
        fs::write(path, existing).unwrap();
    }

    fn build_output(&self, relative: &str, files: usize) -> PathBuf {
        let directory = self.manifest.workspace.join(relative);
        fs::create_dir_all(directory.join("debug/deps")).unwrap();
        for index in 0..files {
            fs::write(directory.join(format!("debug/deps/{index}.o")), "object").unwrap();
        }
        directory
    }

    fn prune(&self, dry_run: bool) -> WorkspaceGcReport {
        self.runtime
            .gc_workspaces_pruning_build_output(Duration::ZERO, dry_run)
            .unwrap()
    }
}

fn single(report: &WorkspaceGcReport) -> &WorkspaceGcEntry {
    assert_eq!(report.workspaces.len(), 1, "{report:?}");
    &report.workspaces[0]
}

#[tokio::test]
async fn finished_workspace_with_only_build_output_is_pruned_then_removed() {
    let f = fixture().await;
    f.store(f.owner(AgentStatus::Completed { summary: None }));
    f.exclude("target/");
    let target = f.build_output("target", 3);
    let preview = f.collect(true);
    let entry = single(&preview);
    assert_eq!(entry.action, WorkspaceGcAction::WouldRemove, "{entry:?}");
    assert_eq!(entry.build_output.len(), 1, "{entry:?}");
    assert_eq!(entry.build_output[0].path, target);
    assert_eq!(entry.build_output[0].action, BuildOutputAction::WouldPrune);
    assert!(entry.build_output[0].bytes > 0);
    assert!(target.is_dir());
    let report = f.collect(false);
    let entry = single(&report);
    assert_eq!(entry.action, WorkspaceGcAction::Removed, "{entry:?}");
    assert_eq!(entry.build_output[0].action, BuildOutputAction::Pruned);
    assert_eq!(report.bytes_removed, preview.bytes_eligible);
    assert!(!f.manifest.workspace.exists());
}

#[tokio::test]
async fn build_output_past_the_entry_limit_does_not_keep_the_workspace() {
    let f = fixture().await;
    f.store(f.owner(AgentStatus::Completed { summary: None }));
    f.exclude("target/");
    f.build_output("target", TARGET_FILES);
    let preview = f.collect(true);
    let entry = single(&preview);
    assert_eq!(entry.action, WorkspaceGcAction::WouldRemove, "{entry:?}");
    assert!(entry.bytes_complete);
    assert!(!entry.reason.contains("limit"), "{entry:?}");
    let entry = f.collect(false).workspaces.remove(0);
    assert_eq!(entry.action, WorkspaceGcAction::Removed, "{entry:?}");
    assert!(!f.manifest.workspace.exists());
}

#[tokio::test]
async fn untracked_source_keeps_the_workspace_and_only_prune_mode_clears_build_output() {
    let f = fixture().await;
    f.store(f.owner(AgentStatus::Completed { summary: None }));
    f.exclude("target/");
    let target = f.build_output("target", 2);
    let source = f.manifest.workspace.join("notes.rs");
    fs::write(&source, "fn unsaved() {}\n").unwrap();
    f.assert_kept("dirty, untracked, or ignored");
    assert!(target.is_dir());
    let preview = f.prune(true);
    let entry = single(&preview);
    assert_eq!(entry.action, WorkspaceGcAction::Kept);
    assert!(entry.reason.contains("dirty, untracked, or ignored"));
    assert_eq!(entry.build_output[0].action, BuildOutputAction::WouldPrune);
    assert_eq!(preview.bytes_eligible, entry.build_output[0].bytes);
    assert!(target.is_dir());
    let report = f.prune(false);
    let entry = single(&report);
    assert_eq!(entry.action, WorkspaceGcAction::Kept);
    assert!(entry.reason.contains("dirty, untracked, or ignored"));
    assert_eq!(entry.build_output[0].action, BuildOutputAction::Pruned);
    assert_eq!(report.bytes_removed, preview.bytes_eligible);
    assert!(!target.exists());
    assert_eq!(fs::read_to_string(source).unwrap(), "fn unsaved() {}\n");
}

#[tokio::test]
async fn undeclared_partly_tracked_and_symlinked_output_is_never_pruned() {
    let f = fixture().await;
    f.store(f.owner(AgentStatus::Completed { summary: None }));
    f.exclude("build/");
    f.exclude("target");
    let build = f.build_output("build", 1);
    let outside = f._temp.path().join("outside");
    fs::create_dir(&outside).unwrap();
    fs::write(outside.join("keep"), "outside").unwrap();
    symlink(&outside, f.manifest.workspace.join("target")).unwrap();
    let entry = f.prune(false).workspaces.remove(0);
    assert_eq!(entry.action, WorkspaceGcAction::Kept, "{entry:?}");
    assert!(entry.build_output.is_empty(), "{entry:?}");
    assert!(build.is_dir());
    assert_eq!(fs::read_to_string(outside.join("keep")).unwrap(), "outside");
}

#[tokio::test]
async fn live_or_resumable_owners_keep_their_build_output() {
    let cases: [fn(&Fixture); 6] = [
        |f| f.store(f.owner(AgentStatus::Created)),
        |f| {
            let mut agent = f.owner(AgentStatus::Waiting {
                condition: aether_core::agent::WaitCondition::Inbox,
            });
            agent.paused = true;
            f.store(agent);
        },
        // A stopped child stays resumable while its parent lives.
        |f| {
            f.store(f.parent(AgentStatus::Ready));
            f.store(f.owner(AgentStatus::Cancelled { reason: None }));
        },
        |f| {
            f.store(f.parent(AgentStatus::Ready));
            f.store(f.owner(AgentStatus::Failed {
                error: "restartable".into(),
            }));
        },
        |f| {
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
        },
        |f| {
            let agent = f.owner(AgentStatus::Completed { summary: None });
            f.store(agent.clone());
            let lease = aether_core::lease::Lease::new(
                agent.id,
                "test-holder",
                1,
                aether_core::time::now(),
                Duration::from_secs(60),
            );
            f.runtime
                .apply(Commit::new().mutate(Mutation::SetLease(lease)))
                .unwrap();
        },
    ];
    for (index, owner) in cases.into_iter().enumerate() {
        let f = fixture().await;
        f.exclude("target/");
        let target = f.build_output("target", 1);
        fs::write(f.manifest.workspace.join("untracked"), "keep").unwrap();
        owner(&f);
        for report in [f.collect(false), f.prune(false)] {
            let entry = single(&report);
            assert_eq!(
                entry.action,
                WorkspaceGcAction::Kept,
                "case {index}: {entry:?}"
            );
            assert!(
                entry
                    .build_output
                    .iter()
                    .all(|output| output.action == BuildOutputAction::Kept),
                "case {index}: {entry:?}"
            );
        }
        assert!(target.join("debug/deps/0.o").is_file(), "case {index}");
    }
}

#[tokio::test]
async fn build_output_within_grace_is_kept() {
    let f = fixture().await;
    f.store(f.owner(AgentStatus::Completed { summary: None }));
    f.exclude("target/");
    let target = f.build_output("target", 1);
    fs::write(f.manifest.workspace.join("untracked"), "keep").unwrap();
    let report = f
        .runtime
        .gc_workspaces_pruning_build_output(Duration::from_secs(3600), false)
        .unwrap();
    let entry = single(&report);
    assert!(entry.reason.contains("grace period"), "{entry:?}");
    assert_eq!(entry.build_output[0].action, BuildOutputAction::Kept);
    assert!(target.is_dir());
}
