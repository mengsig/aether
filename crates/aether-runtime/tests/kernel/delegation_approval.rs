use super::*;
use aether_core::agent::NetworkPolicy;
use aether_core::model::Effort;
use aether_core::tool::ToolExposure;
use aether_engine::{Engine, EngineError, EngineKind, EngineRun, EngineTurn};
use aether_model::testing::ScriptedResponse;
use tokio_util::sync::CancellationToken;

fn mutation_gates() -> ApprovalPolicy {
    ApprovalPolicy {
        require_for: [
            SideEffectClass::Mutation,
            SideEffectClass::IdempotentMutation,
        ]
        .into(),
    }
}

fn delegate_to(model: &str) -> ScriptedResponse {
    ScriptedClient::tool_call(
        "delegate",
        json!({"role":"worker", "objective":"check", "model":model, "effort":"low",
               "network":false, "wait":true}),
    )
}

#[tokio::test]
async fn api_delegate_inherits_gates_without_asking_for_the_delegation() {
    let h = harness(vec![
        delegate_to("anthropic/claude-sonnet-5-5"),
        ScriptedClient::tool_call("write_file", json!({"path":"note.txt", "content":"hello"})),
    ])
    .await;
    let release = h.client.hold_next_response();
    let tools = ToolExposure::Only {
        names: ["delegate".into(), "write_file".into()].into(),
    };
    let mut spec = spec("delegate a worker");
    spec.approval = Some(mutation_gates());
    spec.scope = Some(Scope {
        workspace_root: Some(h.workspace_root.clone()),
        network: NetworkPolicy::Allowed,
        ..Default::default()
    });
    let (parent, _) = h.runtime.create_agent(spec, Actor::user()).unwrap();
    h.wait_for("parent's model request", || !h.client.requests().is_empty())
        .await;
    let current = h
        .runtime
        .update_agent(
            parent.id,
            AgentUpdate {
                tools: Some(tools.clone()),
                ..Default::default()
            },
            Actor::user(),
        )
        .unwrap();
    assert_eq!(current.config.tools, tools);
    release.cancel();
    let child = child_of(&h, parent.id).await;
    h.wait_until(child, |s| s.kind() == AgentStatusKind::Waiting)
        .await;
    let stored = h.store.agent(child).unwrap().unwrap();
    assert_eq!(stored.run, parent.run);
    assert_eq!(stored.scope.workspace_root, parent.scope.workspace_root);
    assert_eq!(stored.scope.network, NetworkPolicy::Denied);
    assert_eq!(
        stored.model,
        ModelRef::new("anthropic", "claude-sonnet-5-5")
    );
    assert_eq!(stored.config.effort, Some(Effort::Low));
    assert_eq!(stored.config.tools, tools);
    assert_eq!(stored.config.approval, parent.config.approval);
    let pending = h.store.pending_approvals().unwrap();
    assert_eq!(pending.len(), 1, "only the child's write needs approval");
    assert_eq!(pending[0].agent_id, child);
    assert_eq!(pending[0].tool, "write_file");
    assert!(!h.workspace_root.join("note.txt").exists());
    h.runtime.shutdown().await;
}

#[tokio::test]
async fn waking_or_restarting_a_delegated_child_rechecks_parent_approvals() {
    let h = harness(vec![
        delegate_to("anthropic/claude-sonnet-5-5"),
        ScriptedClient::tool_call("fail_task", json!({"error":"fixture failure"})),
        ScriptedClient::tool_call("wait_for", json!({"timeout_seconds":3600})),
    ])
    .await;
    let mut spec = spec("delegate a worker");
    spec.config.as_mut().unwrap().long_lived = true;
    let (parent, _) = h.runtime.create_agent(spec, Actor::user()).unwrap();
    let child = child_of(&h, parent.id).await;
    h.wait_for("parent's turn after the child failed", || {
        h.client.requests().len() >= 3
    })
    .await;
    let original = h.store.agent(child).unwrap().unwrap();
    assert_eq!(original.status.kind(), AgentStatusKind::Failed);
    assert!(original.config.approval.is_yolo());

    h.runtime
        .set_approvals(parent.id, mutation_gates(), false, Actor::user())
        .unwrap();
    for result in [
        h.runtime.wake_agent(child, Actor::user()),
        h.runtime.restart_agent(child, Actor::user()),
    ] {
        let error = result.unwrap_err().to_string();
        assert!(
            error.contains("approval") && error.contains(&parent.id.to_string()),
            "{error}"
        );
    }
    assert_eq!(
        h.store
            .agents(&AgentFilter {
                parent: Some(parent.id),
                ..Default::default()
            })
            .unwrap()
            .len(),
        1
    );
    let stored = h.store.agent(child).unwrap().unwrap();
    assert_eq!(stored.status.kind(), AgentStatusKind::Failed);
    assert!(stored.restarted_as.is_none());
    h.runtime.shutdown().await;
}

#[tokio::test]
async fn resuming_a_legacy_child_checks_ancestor_gates_before_a_turn() {
    let h = harness(vec![ScriptedClient::tool_call(
        "write_file",
        json!({"path":"note.txt", "content":"hello"}),
    )])
    .await;
    let mut root_spec = spec("hold staff");
    root_spec.approval = Some(mutation_gates());
    root_spec.start = false;
    let (root, _) = h.runtime.create_agent(root_spec, Actor::user()).unwrap();
    let mut child_spec = spec("review a note");
    child_spec.parent = Some(root.id);
    child_spec.config.as_mut().unwrap().approval = mutation_gates();
    child_spec.start = false;
    let (mut legacy, _) = h
        .runtime
        .create_agent(child_spec, Actor::agent(root.id))
        .unwrap();
    assert_eq!(legacy.config.approval, root.config.approval);
    legacy.config.approval = ApprovalPolicy::none();
    legacy.status = AgentStatus::Waiting {
        condition: aether_core::agent::WaitCondition::Inbox,
    };
    h.store
        .commit(Commit::new().mutate(Mutation::UpsertAgent(legacy.clone())))
        .unwrap();

    let resumed = h
        .runtime
        .resume_agent(legacy.id, false, Actor::user())
        .unwrap();
    assert_eq!(resumed[0].status.kind(), AgentStatusKind::Ready);
    let status = h.wait_until(legacy.id, AgentStatus::is_terminal).await;
    assert!(
        matches!(status, AgentStatus::Failed { error } if error.contains("approval") && error.contains(&root.id.to_string()))
    );
    assert!(h.client.requests().is_empty());
    assert!(!h.workspace_root.join("note.txt").exists());
    h.runtime.shutdown().await;
}

struct UngatedEngine;

#[async_trait::async_trait]
impl Engine for UngatedEngine {
    fn kind(&self) -> EngineKind {
        EngineKind::CODEX
    }

    fn adapter(&self) -> String {
        "ungated-test/1".into()
    }

    fn gates_permissions(&self) -> bool {
        false
    }

    async fn run(&self, _: EngineTurn, _: CancellationToken) -> Result<EngineRun, EngineError> {
        panic!("scripted model must never launch a native engine")
    }
}

#[tokio::test]
async fn api_delegate_refuses_a_child_whose_engine_cannot_gate() {
    let engines = [(
        EngineKind::CODEX,
        Arc::new(UngatedEngine) as Arc<dyn Engine>,
    )]
    .into();
    let h = harness_full_with_engines(
        vec![
            delegate_to("codex/gpt-6-luna"),
            ScriptedClient::tool_call("complete_task", json!({"summary":"done"})),
        ],
        |_| {},
        |_| {},
        None,
        None,
        None,
        |_| {},
        engines,
    )
    .await;
    let mut spec = spec("delegate a worker");
    spec.approval = Some(mutation_gates());
    let (parent, _) = h.runtime.create_agent(spec, Actor::user()).unwrap();
    h.wait_until(parent.id, AgentStatus::is_terminal).await;
    assert_eq!(h.store.agents(&AgentFilter::default()).unwrap().len(), 1);
    assert_eq!(h.store.tasks(&Default::default()).unwrap().len(), 1);
    assert!(h.store.pending_approvals().unwrap().is_empty());
    let events = h
        .store
        .events(0, 1000, &EventFilter::for_agent(parent.id))
        .unwrap();
    let outcome = settled_outcome(&events, "delegate");
    assert!(outcome.is_error, "{}", outcome.content);
    assert!(
        !outcome.content.contains("codex/default"),
        "{}",
        outcome.content
    );
    assert!(
        outcome.content.contains("required approvals"),
        "{}",
        outcome.content
    );
    h.runtime.shutdown().await;
}
