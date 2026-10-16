use super::*;

fn git(root: &std::path::Path, args: &[&str]) {
    let output = std::process::Command::new("git")
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .env("GIT_CONFIG_SYSTEM", "/dev/null")
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .env_remove("GIT_DIR")
        .env_remove("GIT_WORK_TREE")
        .args([
            "-c",
            "commit.gpgsign=false",
            "-c",
            "core.hooksPath=/dev/null",
        ])
        .arg("-C")
        .arg(root)
        .args(args)
        .output()
        .expect("Git is available");
    assert!(
        output.status.success(),
        "git {args:?} failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
}

fn init_repo(root: &std::path::Path) {
    git(root, &["init", "-q"]);
    git(root, &["config", "user.name", "fixture"]);
    git(root, &["config", "user.email", "fixture@example.com"]);
    std::fs::write(root.join("tracked.txt"), "base\n").expect("base file");
    git(root, &["add", "tracked.txt"]);
    git(root, &["commit", "-q", "-m", "base"]);
}

async fn delegate_in_mode(
    h: &Harness,
    bridge: &BridgeCredentials,
    objective: &str,
    workspace_mode: &str,
) -> AgentId {
    delegate_with_source(h, bridge, objective, workspace_mode, None).await
}

async fn delegate_with_source(
    h: &Harness,
    bridge: &BridgeCredentials,
    objective: &str,
    workspace_mode: &str,
    source: Option<&std::path::Path>,
) -> AgentId {
    let before = children_of(h.store.as_ref(), bridge.agent);
    let mut input = json!({
        "model": "claude-code/haiku", "effort": "low", "role": "worker",
        "objective": objective, "workspace_mode": workspace_mode, "wait": false
    });
    if let Some(source) = source {
        input["workspace_root"] = json!(source.to_string_lossy());
    }
    let (reply, is_error) = h
        .runtime
        .engine_tool(bridge.agent, &bridge.token, "delegate".into(), input)
        .await
        .expect("bridge reply");
    assert!(!is_error, "{reply}");
    children_of(h.store.as_ref(), bridge.agent)
        .into_iter()
        .find(|id| !before.contains(id))
        .unwrap_or_else(|| panic!("delegate created no child: {reply}"))
}

#[tokio::test]
async fn isolated_delegate_can_select_a_nested_repository_root() {
    let managed = tempfile::tempdir().expect("managed workspaces");
    let mut h = harness_over(None, 10, "", Some(managed.path().to_path_buf())).await;
    let root = h
        .runtime
        .config()
        .workspace_root
        .as_ref()
        .expect("root")
        .clone();
    let source = root.join("project");
    std::fs::create_dir(&source).expect("nested project");
    init_repo(&source);

    let parent = start_coordinator(&h, None);
    let parent_bridge = h.ready.recv().await.expect("parent bridge");
    assert_eq!(parent_bridge.agent, parent);
    let child = delegate_with_source(
        &h,
        &parent_bridge,
        "edit selected project",
        "isolated",
        Some(&source),
    )
    .await;
    let child_bridge = h.ready.recv().await.expect("child bridge");
    assert_eq!(child_bridge.agent, child);
    let child_root = workspace(&h, child);
    assert_eq!(
        std::fs::read_to_string(child_root.join("tracked.txt")).expect("source snapshot"),
        "base\n"
    );
    std::fs::write(child_root.join("chosen.txt"), "selected result\n").expect("child edit");
    complete(&h, &child_bridge).await;
    let (reply, is_error) = integrate(&h, &parent_bridge, child).await;
    assert!(!is_error, "{reply}");
    assert_eq!(
        std::fs::read_to_string(source.join("chosen.txt")).expect("selected source result"),
        "selected result\n"
    );
    assert!(!root.join("chosen.txt").exists());
    h.runtime.shutdown().await;
}

#[cfg(unix)]
#[tokio::test]
async fn isolated_source_symlink_cannot_escape_parent_workspace() {
    let managed = tempfile::tempdir().expect("managed workspaces");
    let outside = tempfile::tempdir().expect("outside repository");
    init_repo(outside.path());
    let mut h = harness_over(None, 10, "", Some(managed.path().to_path_buf())).await;
    let root = h
        .runtime
        .config()
        .workspace_root
        .as_ref()
        .expect("root")
        .clone();
    let shortcut = root.join("shortcut");
    std::os::unix::fs::symlink(outside.path(), &shortcut).expect("source symlink");

    let parent = start_coordinator(&h, None);
    let parent_bridge = h.ready.recv().await.expect("parent bridge");
    assert_eq!(parent_bridge.agent, parent);
    let (reply, is_error) = h
        .runtime
        .engine_tool(
            parent,
            &parent_bridge.token,
            "delegate".into(),
            json!({
                "model": "claude-code/haiku", "effort": "low", "role": "worker",
                "objective": "must not escape", "workspace_mode": "isolated",
                "workspace_root": shortcut.to_string_lossy(), "wait": false
            }),
        )
        .await
        .expect("bridge reply");
    assert!(is_error, "{reply}");
    assert!(children_of(h.store.as_ref(), parent).is_empty());
    assert_eq!(
        std::fs::read_dir(managed.path())
            .expect("managed directory")
            .count(),
        0,
        "refused delegation leaves no worktree"
    );
    h.runtime.shutdown().await;
}

fn workspace(h: &Harness, agent: AgentId) -> std::path::PathBuf {
    h.store
        .agent(agent)
        .expect("agent row")
        .expect("agent exists")
        .scope
        .workspace_root
        .expect("workspace root")
}

async fn complete(h: &Harness, bridge: &BridgeCredentials) {
    let (reply, is_error) = h
        .runtime
        .engine_tool(
            bridge.agent,
            &bridge.token,
            "complete_task".into(),
            json!({"summary": "done"}),
        )
        .await
        .expect("completion reply");
    assert!(!is_error, "{reply}");
    wait_until(h.store.as_ref(), bridge.agent, |status| {
        status.is_terminal()
    })
    .await;
}

async fn integrate(h: &Harness, parent: &BridgeCredentials, child: AgentId) -> (String, bool) {
    h.runtime
        .engine_tool(
            parent.agent,
            &parent.token,
            "integrate_workspace".into(),
            json!({"child_agent_id": child.to_string()}),
        )
        .await
        .expect("integration reply")
}

#[tokio::test]
async fn parent_can_work_then_explicitly_wait_for_isolated_child() {
    let managed = tempfile::tempdir().expect("managed workspaces");
    let mut h = harness_over(None, 10, "", Some(managed.path().to_path_buf())).await;
    let root = h
        .runtime
        .config()
        .workspace_root
        .as_ref()
        .expect("root")
        .clone();
    init_repo(&root);

    let parent = start_coordinator(&h, None);
    let parent_bridge = h.ready.recv().await.expect("parent bridge");
    assert_eq!(parent_bridge.agent, parent);
    let child = delegate_in_mode(&h, &parent_bridge, "write independently", "isolated").await;
    let child_bridge = h.ready.recv().await.expect("child bridge");
    assert_eq!(child_bridge.agent, child);
    let child_root = workspace(&h, child);
    std::fs::write(root.join("later.txt"), "parent continued\n").expect("parent work");
    assert!(
        !child_root.join("later.txt").exists(),
        "child sees delegation snapshot"
    );

    let task = h
        .store
        .tasks(&aether_store::TaskFilter {
            owner: Some(child),
            ..Default::default()
        })
        .expect("tasks")
        .remove(0);
    let (reply, is_error) = h
        .runtime
        .engine_tool(
            parent,
            &parent_bridge.token,
            "wait_for".into(),
            json!({"tasks": [task.id.to_string()]}),
        )
        .await
        .expect("wait reply");
    assert!(!is_error, "{reply}");
    wait_until(h.store.as_ref(), parent, |status| {
        matches!(status, AgentStatus::Waiting { .. })
    })
    .await;
    std::fs::write(child_root.join("result.txt"), "child result\n").expect("child work");
    complete(&h, &child_bridge).await;

    let resumed = h
        .ready
        .recv()
        .await
        .expect("parent resumed after child completion");
    assert_eq!(resumed.agent, parent);
    assert!(
        !root.join("result.txt").exists(),
        "wait does not integrate files"
    );
    let (reply, is_error) = integrate(&h, &resumed, child).await;
    assert!(!is_error, "{reply}");
    assert_eq!(
        std::fs::read_to_string(root.join("result.txt")).expect("integrated result"),
        "child result\n"
    );
    assert_eq!(
        std::fs::read_to_string(root.join("later.txt")).expect("parent work preserved"),
        "parent continued\n"
    );
    h.runtime.shutdown().await;
}

#[tokio::test]
async fn isolated_child_can_restart_and_live_child_blocks_parent_scope_tightening() {
    let managed = tempfile::tempdir().expect("managed workspaces");
    let mut h = harness_over(None, 10, "", Some(managed.path().to_path_buf())).await;
    let root = h
        .runtime
        .config()
        .workspace_root
        .as_ref()
        .expect("root")
        .clone();
    init_repo(&root);
    let (parent, _) = h
        .runtime
        .create_agent(
            CreateAgentSpec {
                role: "coordinator".into(),
                objective: Some("delegate a worker".into()),
                model: Some(ModelRef::new("claude-code", "haiku")),
                scope: Some(aether_core::agent::Scope {
                    workspace_root: Some(root.clone()),
                    network: aether_core::agent::NetworkPolicy::Allowed,
                    ..Default::default()
                }),
                start: true,
                config: Some(AgentConfig {
                    long_lived: true,
                    approval: ApprovalPolicy::none(),
                    ..Default::default()
                }),
                ..Default::default()
            },
            Actor::user(),
        )
        .expect("parent");
    let parent_bridge = h.ready.recv().await.expect("parent bridge");
    assert_eq!(parent_bridge.agent, parent.id);
    let child = delegate_in_mode(&h, &parent_bridge, "might fail", "isolated").await;
    let child_bridge = h.ready.recv().await.expect("child bridge");
    assert_eq!(child_bridge.agent, child);
    let child_root = workspace(&h, child);

    let refused = h
        .runtime
        .update_agent(
            parent.id,
            aether_runtime::AgentUpdate {
                network: Some(aether_core::agent::NetworkPolicy::Denied),
                ..Default::default()
            },
            Actor::user(),
        )
        .expect_err("live child still has network access");
    assert!(refused.to_string().contains("descendant"), "{refused}");

    let (reply, is_error) = h
        .runtime
        .engine_tool(
            child,
            &child_bridge.token,
            "fail_task".into(),
            json!({"error": "fixture failure"}),
        )
        .await
        .expect("failure reply");
    assert!(!is_error, "{reply}");
    wait_until(h.store.as_ref(), child, |status| status.is_terminal()).await;
    let successor = h
        .runtime
        .restart_agent(child, Actor::user())
        .expect("restart child");
    assert_eq!(successor.parent, Some(parent.id));
    assert_eq!(
        successor.scope.workspace_root.as_deref(),
        Some(child_root.as_path())
    );
    assert!(child_root.exists(), "recovery keeps the isolated workspace");
    h.runtime.shutdown().await;
}

#[tokio::test]
async fn terminal_child_integration_preserves_parent_edits_and_is_repeatable() {
    let managed = tempfile::tempdir().expect("managed workspaces");
    let mut h = harness_over(None, 10, "", Some(managed.path().to_path_buf())).await;
    let root = h
        .runtime
        .config()
        .workspace_root
        .as_ref()
        .expect("root")
        .clone();
    init_repo(&root);
    std::fs::write(root.join("tracked.txt"), "dirty before delegation\n")
        .expect("dirty tracked file");

    let parent = start_coordinator(&h, None);
    let parent_bridge = h.ready.recv().await.expect("parent bridge");
    assert_eq!(parent_bridge.agent, parent);
    let child = delegate_in_mode(&h, &parent_bridge, "write result", "isolated").await;
    let child_bridge = h.ready.recv().await.expect("child bridge");
    assert_eq!(child_bridge.agent, child);
    let child_root = workspace(&h, child);
    assert_ne!(child_root, root);
    assert_eq!(
        std::fs::read_to_string(child_root.join("tracked.txt")).expect("snapshot"),
        "dirty before delegation\n"
    );

    std::fs::write(root.join("parent_only.txt"), "keep parent\n").expect("parent edit");
    std::fs::write(child_root.join("child_only.txt"), "from child\n").expect("child edit");
    complete(&h, &child_bridge).await;
    assert!(
        child_root.exists(),
        "child worktree survives task completion"
    );
    assert!(
        !root.join("child_only.txt").exists(),
        "no automatic integration"
    );

    let (first, is_error) = integrate(&h, &parent_bridge, child).await;
    assert!(!is_error, "{first}");
    assert_eq!(
        std::fs::read_to_string(root.join("child_only.txt")).expect("integrated file"),
        "from child\n"
    );
    assert_eq!(
        std::fs::read_to_string(root.join("parent_only.txt")).expect("preserved parent file"),
        "keep parent\n"
    );
    assert_eq!(
        std::fs::read_to_string(root.join("tracked.txt")).expect("preserved dirty baseline"),
        "dirty before delegation\n"
    );
    let (second, is_error) = integrate(&h, &parent_bridge, child).await;
    assert!(!is_error, "{second}");
    let repeated: serde_json::Value = serde_json::from_str(&second).expect("report JSON");
    assert_eq!(repeated["already_integrated"], json!(true));
    assert!(
        child_root.exists(),
        "integration retains the child worktree"
    );
    h.runtime.shutdown().await;
}

#[tokio::test]
async fn integration_refuses_overlapping_parent_edits_and_untracked_collisions() {
    let managed = tempfile::tempdir().expect("managed workspaces");
    let mut h = harness_over(None, 10, "", Some(managed.path().to_path_buf())).await;
    let root = h
        .runtime
        .config()
        .workspace_root
        .as_ref()
        .expect("root")
        .clone();
    init_repo(&root);

    let parent = start_coordinator(&h, None);
    let parent_bridge = h.ready.recv().await.expect("parent bridge");
    assert_eq!(parent_bridge.agent, parent);
    let child = delegate_in_mode(&h, &parent_bridge, "edit same paths", "isolated").await;
    let child_bridge = h.ready.recv().await.expect("child bridge");
    assert_eq!(child_bridge.agent, child);
    let child_root = workspace(&h, child);

    std::fs::write(root.join("tracked.txt"), "parent later\n").expect("parent tracked edit");
    std::fs::write(child_root.join("tracked.txt"), "child later\n").expect("child tracked edit");
    std::fs::write(root.join("new.txt"), "parent new\n").expect("parent untracked edit");
    std::fs::write(child_root.join("new.txt"), "child new\n").expect("child untracked edit");
    complete(&h, &child_bridge).await;

    let (reply, is_error) = integrate(&h, &parent_bridge, child).await;
    assert!(
        is_error && (reply.contains("parent changed") || reply.contains("conflict")),
        "{reply}"
    );
    assert_eq!(
        std::fs::read_to_string(root.join("tracked.txt")).expect("parent tracked"),
        "parent later\n"
    );
    assert_eq!(
        std::fs::read_to_string(root.join("new.txt")).expect("parent untracked"),
        "parent new\n"
    );
    assert_eq!(
        std::fs::read_to_string(child_root.join("tracked.txt")).expect("child tracked"),
        "child later\n"
    );
    assert_eq!(
        std::fs::read_to_string(child_root.join("new.txt")).expect("child new"),
        "child new\n"
    );
    h.runtime.shutdown().await;
}

#[tokio::test]
async fn nested_isolation_integrates_in_order_and_shared_grandchild_does_not_block() {
    let managed = tempfile::tempdir().expect("managed workspaces");
    let mut h = harness_over(None, 10, "", Some(managed.path().to_path_buf())).await;
    let root = h
        .runtime
        .config()
        .workspace_root
        .as_ref()
        .expect("root")
        .clone();
    init_repo(&root);

    let parent = start_coordinator(&h, None);
    let parent_bridge = h.ready.recv().await.expect("parent bridge");
    assert_eq!(parent_bridge.agent, parent);
    let middle = delegate_in_mode(&h, &parent_bridge, "manage workers", "isolated").await;
    let middle_bridge = h.ready.recv().await.expect("middle bridge");
    assert_eq!(middle_bridge.agent, middle);
    let middle_root = workspace(&h, middle);
    let leaf = delegate_in_mode(&h, &middle_bridge, "write leaf result", "isolated").await;
    let leaf_bridge = h.ready.recv().await.expect("leaf bridge");
    assert_eq!(leaf_bridge.agent, leaf);
    let leaf_root = workspace(&h, leaf);
    assert_ne!(middle_root, leaf_root);
    std::fs::write(leaf_root.join("leaf.txt"), "nested result\n").expect("leaf edit");
    complete(&h, &leaf_bridge).await;
    assert!(!middle_root.join("leaf.txt").exists());
    let (reply, is_error) = integrate(&h, &middle_bridge, leaf).await;
    assert!(!is_error, "{reply}");
    assert_eq!(
        std::fs::read_to_string(middle_root.join("leaf.txt")).expect("nested integration"),
        "nested result\n"
    );

    let shared = delegate_in_mode(&h, &middle_bridge, "review merged result", "shared").await;
    let shared_bridge = h.ready.recv().await.expect("shared grandchild bridge");
    assert_eq!(shared_bridge.agent, shared);
    assert_eq!(workspace(&h, shared), middle_root);
    complete(&h, &shared_bridge).await;
    complete(&h, &middle_bridge).await;

    let (reply, is_error) = integrate(&h, &parent_bridge, middle).await;
    assert!(!is_error, "{reply}");
    assert_eq!(
        std::fs::read_to_string(root.join("leaf.txt")).expect("root integration"),
        "nested result\n"
    );
    h.runtime.shutdown().await;
}

#[tokio::test]
async fn parent_can_integrate_child_without_accepting_isolated_grandchild() {
    let managed = tempfile::tempdir().expect("managed workspaces");
    let mut h = harness_over(None, 10, "", Some(managed.path().to_path_buf())).await;
    let root = h
        .runtime
        .config()
        .workspace_root
        .as_ref()
        .expect("root")
        .clone();
    init_repo(&root);

    let parent = start_coordinator(&h, None);
    let parent_bridge = h.ready.recv().await.expect("parent bridge");
    assert_eq!(parent_bridge.agent, parent);
    let child = delegate_in_mode(&h, &parent_bridge, "prepare accepted result", "isolated").await;
    let child_bridge = h.ready.recv().await.expect("child bridge");
    assert_eq!(child_bridge.agent, child);
    let child_root = workspace(&h, child);
    std::fs::write(child_root.join("accepted.txt"), "accepted result\n").expect("child edit");

    let grandchild =
        delegate_in_mode(&h, &child_bridge, "prepare optional result", "isolated").await;
    let grandchild_bridge = h.ready.recv().await.expect("grandchild bridge");
    assert_eq!(grandchild_bridge.agent, grandchild);
    let grandchild_root = workspace(&h, grandchild);
    std::fs::write(grandchild_root.join("deferred.txt"), "deferred result\n")
        .expect("grandchild edit");
    complete(&h, &grandchild_bridge).await;
    complete(&h, &child_bridge).await;

    let (reply, is_error) = integrate(&h, &parent_bridge, child).await;
    assert!(!is_error, "{reply}");
    assert_eq!(
        std::fs::read_to_string(root.join("accepted.txt")).expect("accepted result in parent"),
        "accepted result\n"
    );
    assert!(!root.join("deferred.txt").exists());
    assert_eq!(
        std::fs::read_to_string(grandchild_root.join("deferred.txt"))
            .expect("deferred result remains available"),
        "deferred result\n"
    );
    h.runtime.shutdown().await;
}

#[path = "workspace_review.rs"]
mod review;
