//! Delegation and waits with managed Git worktrees.
use super::*;

fn child_of(store: &dyn Store, parent: AgentId) -> aether_core::agent::Agent {
    let children = store
        .agents(&AgentFilter {
            parent: Some(parent),
            ..Default::default()
        })
        .expect("agents");
    assert_eq!(children.len(), 1, "one delegated child");
    children.into_iter().next().expect("child")
}

pub(super) fn init_repository(root: &std::path::Path) {
    git_in(root, &["init", "-q"]);
    git_in(root, &["config", "user.email", "fixture@example.com"]);
    git_in(root, &["config", "user.name", "fixture"]);
    std::fs::write(root.join("tracked.txt"), "committed\n").expect("base file");
    git_in(root, &["add", "tracked.txt"]);
    git_in(root, &["commit", "-q", "-m", "base"]);
}

#[tokio::test]
async fn omitted_workspace_mode_keeps_a_nongit_shared_workspace() {
    let h = harness(vec![
        ScriptedClient::tool_call(
            "delegate",
            json!({
                "role": "worker", "objective": "check", "model": "anthropic/claude-sonnet-5-5",
                "effort": "low", "wait": true
            }),
        ),
        ScriptedClient::tool_call("complete_task", json!({"summary": "child done"})),
        ScriptedClient::tool_call("complete_task", json!({"summary": "parent done"})),
    ])
    .await;
    let (parent, _) = h
        .runtime
        .create_agent(spec("delegate without Git"), Actor::user())
        .expect("parent");
    h.wait_until(parent.id, |status| status.is_terminal()).await;

    let child = child_of(h.store.as_ref(), parent.id);
    assert_eq!(child.scope.workspace_root, parent.scope.workspace_root);
    assert_eq!(child.status.kind(), AgentStatusKind::Completed);
    assert_eq!(h.client.remaining(), 0);
}

#[tokio::test]
async fn nested_git_source_allows_managed_storage_elsewhere_in_broad_parent() {
    let h = harness_configured(
        vec![
            ScriptedClient::tool_call(
                "delegate",
                json!({
                    "role": "worker", "objective": "edit nested repo",
                    "model": "anthropic/claude-sonnet-5-5", "effort": "low",
                    "workspace_mode": "isolated", "workspace_root": "project",
                    "wait": true
                }),
            ),
            ScriptedClient::tool_call("complete_task", json!({"summary": "child done"})),
            ScriptedClient::tool_call("complete_task", json!({"summary": "parent done"})),
        ],
        |_| {},
        |config| {
            let parent_root = config.workspace_root.as_ref().expect("parent root");
            config.delegated_workspace_root = Some(parent_root.join(".aether-workspaces"));
        },
    )
    .await;
    let source = h.workspace_root.join("project");
    std::fs::create_dir(&source).expect("nested source");
    init_repository(&source);

    let (parent, _) = h
        .runtime
        .create_agent(spec("delegate nested repository"), Actor::user())
        .expect("parent");
    h.wait_until(parent.id, |status| status.is_terminal()).await;
    let child = child_of(h.store.as_ref(), parent.id);
    assert_eq!(child.status.kind(), AgentStatusKind::Completed);
    assert!(
        child
            .scope
            .workspace_root
            .as_ref()
            .expect("child worktree")
            .starts_with(h.workspace_root.join(".aether-workspaces"))
    );
    assert!(source.join("tracked.txt").exists());
    assert_eq!(h.client.remaining(), 0);
}

#[tokio::test]
async fn isolated_delegate_refuses_container_backend_before_creating_a_worktree() {
    let managed = tempfile::tempdir().expect("managed worktree directory");
    let h = harness_configured(
        vec![
            ScriptedClient::tool_call(
                "delegate",
                json!({
                    "role": "worker", "objective": "edit in isolation",
                    "model": "anthropic/claude-sonnet-5-5", "effort": "low",
                    "workspace_mode": "isolated", "wait": false
                }),
            ),
            ScriptedClient::tool_call("complete_task", json!({"summary": "done"})),
        ],
        |_| {},
        |config| config.delegated_workspace_root = Some(managed.path().to_path_buf()),
    )
    .await;
    init_repository(&h.workspace_root);
    let mut parent_spec = spec("delegate from a container");
    parent_spec.scope = Some(Scope {
        workspace_root: Some(h.workspace_root.clone()),
        backend: aether_core::agent::ExecutionBackend::Container {
            image: "fixture:latest".into(),
        },
        ..Default::default()
    });
    let (parent, _) = h
        .runtime
        .create_agent(parent_spec, Actor::user())
        .expect("container parent");
    h.wait_until(parent.id, |status| status.is_terminal()).await;

    let events = h
        .store
        .events(0, 10_000, &EventFilter::for_agent(parent.id))
        .expect("events");
    let outcome = settled_outcome(&events, "delegate");
    assert!(outcome.is_error, "{outcome:?}");
    assert!(
        outcome
            .content
            .contains("requires the host execution backend"),
        "{outcome:?}"
    );
    assert!(
        h.store
            .agents(&AgentFilter {
                parent: Some(parent.id),
                ..Default::default()
            })
            .expect("children")
            .is_empty()
    );
    assert_eq!(
        std::fs::read_dir(managed.path())
            .expect("managed directory")
            .count(),
        0,
        "refusal must not create a worktree"
    );
}

#[tokio::test]
async fn isolated_delegate_snapshots_dirty_parent_and_retains_child_result() {
    let managed = tempfile::tempdir().expect("managed worktree directory");
    let h = harness_configured(
        vec![
            ScriptedClient::tool_call(
                "delegate",
                json!({
                    "role": "worker", "objective": "edit in isolation",
                    "model": "anthropic/claude-sonnet-5-5", "effort": "low",
                    "workspace_mode": "isolated", "wait": true
                }),
            ),
            ScriptedClient::tool_call(
                "bash",
                json!({"command": "printf 'child result\\n' > result.txt"}),
            ),
            ScriptedClient::tool_call("ask_user", json!({"question": "Ready to finish?"})),
            ScriptedClient::tool_call("complete_task", json!({"summary": "child done"})),
            ScriptedClient::tool_call("complete_task", json!({"summary": "parent done"})),
        ],
        |_| {},
        |config| config.delegated_workspace_root = Some(managed.path().to_path_buf()),
    )
    .await;
    init_repository(&h.workspace_root);
    std::fs::write(h.workspace_root.join("tracked.txt"), "dirty parent\n")
        .expect("unstaged change");
    std::fs::write(h.workspace_root.join("untracked.txt"), "new parent file\n")
        .expect("untracked file");

    let (parent, _) = h
        .runtime
        .create_agent(spec("delegate"), Actor::user())
        .expect("parent");
    h.wait_for("isolated child", || {
        !h.store
            .agents(&AgentFilter {
                parent: Some(parent.id),
                ..Default::default()
            })
            .expect("agents")
            .is_empty()
    })
    .await;
    let child = child_of(h.store.as_ref(), parent.id);
    let child_root = child.scope.workspace_root.as_ref().expect("worktree");
    assert_ne!(child_root, &h.workspace_root);
    h.wait_until(child.id, |status| {
        matches!(status, AgentStatus::Waiting { .. })
    })
    .await;
    h.wait_until(parent.id, |status| {
        matches!(status, AgentStatus::Waiting { .. })
    })
    .await;

    assert_eq!(
        std::fs::read_to_string(child_root.join("tracked.txt")).expect("snapshot tracked"),
        "dirty parent\n"
    );
    assert_eq!(
        std::fs::read_to_string(child_root.join("untracked.txt")).expect("snapshot untracked"),
        "new parent file\n"
    );
    assert_eq!(
        std::fs::read_to_string(child_root.join("result.txt")).expect("child edit"),
        "child result\n"
    );
    assert!(!h.workspace_root.join("result.txt").exists());

    h.runtime
        .send_message(child.id, vec![ContentBlock::text("yes")], Actor::user())
        .expect("answer child");
    h.wait_until(parent.id, |status| status.is_terminal()).await;
    assert_eq!(
        child_of(h.store.as_ref(), parent.id).status.kind(),
        AgentStatusKind::Completed
    );
    assert!(
        child_root.exists(),
        "settlement retains the child's result tree"
    );
    let parent_events = types(h.store.as_ref(), parent.id);
    assert_eq!(
        parent_events
            .iter()
            .filter(|kind| *kind == "agent.resumed")
            .count(),
        1,
        "the delegated wait wakes once for the child result: {parent_events:?}"
    );
    assert_eq!(h.client.remaining(), 0);
}
