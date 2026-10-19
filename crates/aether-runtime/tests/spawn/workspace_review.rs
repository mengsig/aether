use super::*;

fn operator() -> Actor {
    Actor::User {
        name: Some("operator".into()),
        origin: None,
    }
}

fn custom_events(h: &Harness, agent: AgentId, name: &str) -> Vec<serde_json::Value> {
    let filter = aether_store::EventFilter {
        types: vec!["custom".into()],
        ..aether_store::EventFilter::own_events(agent)
    };
    h.store
        .events(0, 1000, &filter)
        .expect("events")
        .into_iter()
        .filter_map(|event| match event.kind {
            aether_core::event::EventKind::Custom { name: n, payload } if n == name => {
                Some(payload)
            }
            _ => None,
        })
        .collect()
}

async fn isolated_child(h: &mut Harness) -> (std::path::PathBuf, BridgeCredentials, AgentId) {
    let root = h
        .runtime
        .config()
        .workspace_root
        .as_ref()
        .expect("root")
        .clone();
    init_repo(&root);
    let parent = start_coordinator(h, None);
    let parent_bridge = h.ready.recv().await.expect("parent bridge");
    assert_eq!(parent_bridge.agent, parent);
    let child = delegate_in_mode(h, &parent_bridge, "change files", "isolated").await;
    let child_bridge = h.ready.recv().await.expect("child bridge");
    assert_eq!(child_bridge.agent, child);
    (root, child_bridge, child)
}

#[tokio::test]
async fn operator_diff_attributes_commits_and_integration_refuses_then_lands() {
    let managed = tempfile::tempdir().expect("managed workspaces");
    let mut h = harness_over(None, 10, "", Some(managed.path().to_path_buf())).await;
    let (root, child_bridge, child) = isolated_child(&mut h).await;
    let child_root = workspace(&h, child);
    std::fs::write(child_root.join("committed.txt"), "kept\n").expect("child file");
    git(&child_root, &["add", "committed.txt"]);
    git(&child_root, &["commit", "-q", "-m", "child commit"]);
    std::fs::write(child_root.join("tracked.txt"), "child edit\n").expect("child edit");
    complete(&h, &child_bridge).await;

    let heads = custom_events(&h, child, "workspace.head");
    assert_eq!(heads.len(), 2, "{heads:?}");
    let diff = h.runtime.workspace_diff(child).expect("diff");
    assert!(diff.isolated && !diff.ambiguous && !diff.integrated);
    let files: Vec<_> = diff.files.iter().map(|f| f.path.as_str()).collect();
    assert_eq!(files, ["committed.txt", "tracked.txt"]);
    assert_eq!((diff.insertions, diff.deletions), (2, 1));
    assert_eq!(diff.commits.len(), 1, "{:?}", diff.commits);
    assert_eq!(diff.commits[0].summary, "child commit");
    assert_eq!(
        diff.commits[0].turn,
        Some(heads[0]["turn"].as_u64().unwrap() as u32)
    );
    assert!(diff.patch.contains("+child edit"), "{}", diff.patch);

    std::fs::write(root.join("tracked.txt"), "parent edit\n").expect("parent edit");
    let refused = h
        .runtime
        .workspace_integrate(child, operator())
        .expect("refusal");
    assert!(!refused.integrated);
    assert_eq!(refused.conflicts, ["tracked.txt"]);
    assert!(!root.join("committed.txt").exists(), "nothing was written");
    assert!(custom_events(&h, child, "workspace.integrated").is_empty());

    std::fs::write(root.join("tracked.txt"), "base\n").expect("parent revert");
    let landed = h
        .runtime
        .workspace_integrate(child, operator())
        .expect("integration");
    assert!(landed.integrated && landed.conflicts.is_empty());
    assert_eq!(landed.changed_paths, ["committed.txt", "tracked.txt"]);
    assert_eq!(
        std::fs::read_to_string(root.join("tracked.txt")).expect("integrated"),
        "child edit\n"
    );
    assert!(h.runtime.workspace_diff(child).expect("diff").integrated);
    assert_eq!(custom_events(&h, child, "workspace.integrated").len(), 1);
    h.runtime.shutdown().await;
}

#[tokio::test]
async fn discard_refuses_a_live_child_unless_forced_and_records_it() {
    let managed = tempfile::tempdir().expect("managed workspaces");
    let mut h = harness_over(None, 10, "", Some(managed.path().to_path_buf())).await;
    let (_root, _child_bridge, child) = isolated_child(&mut h).await;
    let child_root = workspace(&h, child);
    std::fs::write(child_root.join("draft.txt"), "unfinished\n").expect("child edit");

    let error = h
        .runtime
        .workspace_discard(child, false, operator())
        .expect_err("live child");
    assert!(error.to_string().contains("discard refused"), "{error}");
    assert!(child_root.join("draft.txt").exists());

    let discard = h
        .runtime
        .workspace_discard(child, true, operator())
        .expect("forced discard");
    assert!(discard.forced);
    assert_eq!(discard.workspace, child_root);
    assert!(!child_root.exists());
    let events = custom_events(&h, child, "workspace.discarded");
    assert_eq!(events.len(), 1, "{events:?}");
    assert_eq!(events[0]["forced"], true);

    for message in [
        h.runtime.workspace_diff(child).unwrap_err().to_string(),
        h.runtime
            .workspace_discard(child, true, operator())
            .unwrap_err()
            .to_string(),
    ] {
        assert!(
            message.contains(&format!("workspace for agent {child} was discarded")),
            "{message}"
        );
        assert!(!message.contains("cannot inspect"), "{message}");
    }
    h.runtime.shutdown().await;
}

#[tokio::test]
async fn operator_integration_refuses_a_live_child() {
    let managed = tempfile::tempdir().expect("managed workspaces");
    let mut h = harness_over(None, 10, "", Some(managed.path().to_path_buf())).await;
    let (_root, _child_bridge, child) = isolated_child(&mut h).await;

    let error = h
        .runtime
        .workspace_integrate(child, operator())
        .expect_err("live child");
    assert!(
        error.to_string().contains("wait for it to finish"),
        "{error}"
    );
    h.runtime.shutdown().await;
}

#[tokio::test]
async fn a_shared_workspace_diff_covers_its_turns_commits_and_is_ambiguous() {
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
    let child = delegate_in_mode(&h, &parent_bridge, "commit in place", "shared").await;
    let child_bridge = h.ready.recv().await.expect("child bridge");
    assert_eq!(workspace(&h, child), root);
    std::fs::write(root.join("shared.txt"), "from the child\n").expect("child file");
    git(&root, &["add", "shared.txt"]);
    git(&root, &["commit", "-q", "-m", "shared commit"]);
    complete(&h, &child_bridge).await;

    let diff = h.runtime.workspace_diff(child).expect("diff");
    assert!(!diff.isolated && diff.ambiguous && diff.source.is_none());
    let files: Vec<_> = diff.files.iter().map(|f| f.path.as_str()).collect();
    assert_eq!(files, ["shared.txt"]);
    assert_eq!(diff.commits.len(), 1, "{:?}", diff.commits);
    assert_eq!(diff.commits[0].summary, "shared commit");
    assert!(diff.commits[0].turn.is_some());
    let error = h
        .runtime
        .workspace_integrate(child, operator())
        .expect_err("nothing to integrate");
    assert!(error.to_string().contains("integration refused"), "{error}");
    h.runtime.shutdown().await;
}

#[tokio::test]
async fn a_live_restart_successor_protects_its_predecessors_workspace() {
    let managed = tempfile::tempdir().expect("managed workspaces");
    let mut h = harness_over(None, 10, "", Some(managed.path().to_path_buf())).await;
    let (_root, child_bridge, child) = isolated_child(&mut h).await;
    let child_root = workspace(&h, child);
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
        .restart_agent(child, operator())
        .expect("restart child");
    assert_eq!(
        successor.scope.workspace_root.as_deref(),
        Some(child_root.as_path())
    );

    let error = h
        .runtime
        .workspace_discard(child, false, operator())
        .expect_err("successor is live");
    let message = error.to_string();
    assert!(message.contains(&successor.id.to_string()), "{message}");
    assert!(child_root.exists(), "the successor's worktree is kept");
    let error = h
        .runtime
        .workspace_integrate(child, operator())
        .expect_err("successor is live");
    assert!(error.to_string().contains("integration refused"), "{error}");
    h.runtime.shutdown().await;
}
