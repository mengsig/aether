//! Delegation under a mutation-gated parent: the child inherits the gate and
//! the run, and an engine that cannot ask before acting is refused outright.
use super::*;
use aether_core::tool::{ApprovalDecision, SideEffectClass};

fn mutation_policy() -> ApprovalPolicy {
    ApprovalPolicy {
        require_for: [SideEffectClass::Mutation].into(),
    }
}

fn start_gated_coordinator(h: &Harness) -> AgentId {
    let (agent, task) = h
        .runtime
        .create_agent(
            CreateAgentSpec {
                role: "coordinator".into(),
                objective: Some("delegate gated work".into()),
                start: true,
                config: Some(AgentConfig {
                    long_lived: true,
                    approval: mutation_policy(),
                    ..Default::default()
                }),
                ..Default::default()
            },
            Actor::user(),
        )
        .expect("create");
    assert_eq!(agent.config.approval, mutation_policy());
    assert_eq!(agent.current_task, Some(task.expect("coordinator task").id));
    agent.id
}

fn bash_prompt(command: &str) -> serde_json::Value {
    json!({"tool_name": "Bash", "input": {"command": command}, "tool_use_id": "tu-1"})
}

async fn ready_bridge(h: &mut Harness) -> BridgeCredentials {
    tokio::time::timeout(Duration::from_secs(5), h.ready.recv())
        .await
        .expect("agent started promptly")
        .expect("bridge credentials")
}

async fn delegate_gated_child(h: &Harness, bridge: &BridgeCredentials) -> AgentId {
    let (child, reply) =
        tokio::time::timeout(Duration::from_secs(5), delegate(h, bridge, "gated child"))
            .await
            .expect("delegate replied without an approval wait");
    assert_ne!(child, bridge.agent, "{reply}");
    assert!(h.store.pending_approvals().unwrap().is_empty());
    child
}

#[tokio::test]
async fn a_gated_parent_delegates_without_an_approval_and_the_child_inherits_the_gate() {
    let mut h = harness(10).await;
    let parent = start_gated_coordinator(&h);
    let bridge = ready_bridge(&mut h).await;
    assert_eq!(bridge.agent, parent);

    let child = delegate_gated_child(&h, &bridge).await;
    let child_record = h.store.agent(child).unwrap().expect("child");
    let parent_record = h.store.agent(parent).unwrap().expect("parent");
    assert_eq!(child_record.config.approval, mutation_policy());
    assert_eq!(child_record.run, parent_record.run);
    h.runtime.shutdown().await;
}

#[tokio::test]
async fn a_gated_childs_mutation_waits_for_an_approval() {
    let mut h = harness(10).await;
    start_gated_coordinator(&h);
    let parent_bridge = ready_bridge(&mut h).await;
    let child = delegate_gated_child(&h, &parent_bridge).await;
    let child_bridge = ready_bridge(&mut h).await;
    assert_eq!(child_bridge.agent, child);

    let runtime = h.runtime.clone();
    let asking = tokio::spawn(async move {
        runtime
            .engine_tool(
                child_bridge.agent,
                &child_bridge.token,
                "permission".into(),
                bash_prompt("touch out.txt"),
            )
            .await
    });
    let pending = wait_until_pending(&h).await;
    assert_eq!(pending.agent_id, child);
    assert_eq!(pending.side_effect, SideEffectClass::Mutation);
    h.runtime
        .resolve_approval(
            pending.id,
            ApprovalDecision::Denied,
            Actor::user(),
            Some("not now".into()),
            None,
        )
        .expect("deny");
    let (reply, _) = tokio::time::timeout(Duration::from_secs(5), asking)
        .await
        .expect("permission answered promptly")
        .expect("join")
        .expect("answered");
    assert!(reply.contains("\"deny\""), "{reply}");
    h.runtime.shutdown().await;
}

async fn wait_until_pending(h: &Harness) -> aether_core::tool::ApprovalRequest {
    let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
    loop {
        let mut pending = h.store.pending_approvals().unwrap();
        assert!(
            pending.len() <= 1,
            "a single call raised multiple approvals"
        );
        if let Some(approval) = pending.pop() {
            return approval;
        }
        assert!(
            tokio::time::Instant::now() < deadline,
            "no approval was raised"
        );
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
}

#[tokio::test]
async fn an_engine_that_cannot_ask_is_refused_before_any_child_or_task() {
    let mut h = harness(10).await;
    let parent = start_gated_coordinator(&h);
    let bridge = ready_bridge(&mut h).await;
    let tasks_before = h.store.tasks(&Default::default()).unwrap().len();

    h.gates.store(false, Ordering::SeqCst);
    let (reply, is_error) = tokio::time::timeout(
        Duration::from_secs(5),
        h.runtime.engine_tool(
            parent,
            &bridge.token,
            "delegate".into(),
            json!({"model":"claude-code/haiku", "effort":"low", "role":"worker",
                   "objective":"ungated", "wait": false}),
        ),
    )
    .await
    .expect("delegate replied promptly")
    .expect("answered");
    assert!(
        is_error,
        "an ungated engine admitted a gated child: {reply}"
    );
    assert!(reply.contains("required approvals"), "{reply}");
    assert!(children_of(h.store.as_ref(), parent).is_empty());
    assert_eq!(
        h.store.tasks(&Default::default()).unwrap().len(),
        tasks_before
    );
    h.runtime.shutdown().await;
}

#[tokio::test]
async fn a_repeated_key_returns_the_same_isolated_child_without_approval() {
    let managed = tempfile::tempdir().expect("managed workspaces");
    let mut h = harness_over(None, 10, "", Some(managed.path().to_path_buf())).await;
    let root = h.runtime.config().workspace_root.as_deref().expect("root");
    workspace_integration::init_repo(root);
    let parent = start_gated_coordinator(&h);
    let bridge = ready_bridge(&mut h).await;
    assert_eq!(bridge.agent, parent);
    let input = json!({"model":"claude-code/haiku", "effort":"low", "role":"worker",
                       "objective":"once", "workspace_mode":"isolated",
                       "wait":false, "idempotency_key":"gated-1"});
    let call = || {
        tokio::time::timeout(
            Duration::from_secs(5),
            h.runtime.engine_tool(
                bridge.agent,
                &bridge.token,
                "delegate".into(),
                input.clone(),
            ),
        )
    };
    let (first, first_error) = call()
        .await
        .expect("first reply promptly")
        .expect("first reply");
    let (second, second_error) = call()
        .await
        .expect("repeat replied promptly")
        .expect("repeat reply");
    assert!(!first_error && !second_error, "{first} / {second}");
    let first: serde_json::Value = serde_json::from_str(first.lines().last().unwrap()).unwrap();
    let second: serde_json::Value = serde_json::from_str(second.lines().last().unwrap()).unwrap();
    assert_eq!(first["agent_id"], second["agent_id"]);
    assert_eq!(first["task_id"], second["task_id"]);
    assert_eq!(second["replayed"], true);
    let children = children_of(h.store.as_ref(), parent);
    assert_eq!(children.len(), 1);
    let child = h.store.agent(children[0]).unwrap().expect("child");
    assert_eq!(child.config.approval, mutation_policy());
    let workspace = child.scope.workspace_root.expect("isolated root");
    assert!(workspace.starts_with(managed.path().canonicalize().unwrap()));
    assert_eq!(std::fs::read_dir(managed.path()).unwrap().count(), 1);
    assert!(h.store.pending_approvals().unwrap().is_empty());
    h.runtime.shutdown().await;
}
