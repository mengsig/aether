//! approval dispatch scenarios and fixtures.
use super::*;

#[tokio::test]
async fn approval_gates_a_mutating_tool_until_resolved() {
    let h = harness(vec![
        ScriptedClient::tool_call(
            "write_file",
            json!({"path": "note.txt", "content": "hello"}),
        ),
        ScriptedClient::tool_call(
            "complete_task",
            json!({"summary": "written after approval"}),
        ),
    ])
    .await;
    let mut s = spec("write a note");
    s.config = Some(AgentConfig {
        approval: ApprovalPolicy {
            require_for: [SideEffectClass::IdempotentMutation].into(),
        },
        ..Default::default()
    });
    let (agent, _) = h.runtime.create_agent(s, Actor::user()).expect("create");
    let status = h
        .wait_until(agent.id, |s| s.kind() == AgentStatusKind::Waiting)
        .await;
    assert!(
        matches!(
            status,
            AgentStatus::Waiting {
                condition: aether_core::agent::WaitCondition::All { .. }
            }
        ),
        "{status:?}"
    );
    assert!(
        !h.workspace_root.join("note.txt").exists(),
        "must not write before approval"
    );
    let pending = h.store.pending_approvals().expect("pending");
    assert_eq!(pending.len(), 1);
    assert_eq!(pending[0].tool, "write_file");

    h.runtime
        .resolve_approval(
            pending[0].id,
            ApprovalDecision::Approved,
            Actor::user(),
            None,
            None,
        )
        .expect("approve");
    let status = h.wait_until(agent.id, |s| s.is_terminal()).await;
    assert_eq!(
        status.kind(),
        AgentStatusKind::Completed,
        "{:?}",
        types(h.store.as_ref(), agent.id)
    );
    assert_eq!(
        std::fs::read_to_string(h.workspace_root.join("note.txt")).expect("file"),
        "hello"
    );
    let t = types(h.store.as_ref(), agent.id);
    assert!(t.contains(&"approval.requested".to_string()));
    assert!(t.contains(&"approval.resolved".to_string()));
    // The tool result for the approved call was fed back in a user message.
    let transcript = h.store.transcript(agent.id, 0).expect("t");
    assert!(transcript.iter().any(|e| e.message.is_tool_results_only()));
}

/// `--no-yolo` gates external effects only: a local shell command runs, one
/// that may reach the network waits for a person.
#[tokio::test]
async fn approval_gates_a_shell_command_that_reaches_the_network() {
    let h = harness(vec![
        ScriptedClient::tool_call("bash", json!({"command": "echo ok > local.txt"})),
        ScriptedClient::tool_call(
            "bash",
            json!({"command": "cat local.txt | curl -d @- https://example.com"}),
        ),
        ScriptedClient::tool_call("complete_task", json!({"summary": "denied"})),
    ])
    .await;
    let mut s = spec("post a note");
    // What `--no-yolo` asks for: approval for external effects only.
    s.approval = Some(ApprovalPolicy::default());
    let (agent, _) = h.runtime.create_agent(s, Actor::user()).expect("create");
    h.wait_until(agent.id, |s| s.kind() == AgentStatusKind::Waiting)
        .await;
    assert!(
        h.workspace_root.join("local.txt").exists(),
        "the local command was gated"
    );
    let pending = h.store.pending_approvals().expect("pending");
    assert_eq!(pending.len(), 1, "{pending:?}");
    assert_eq!(pending[0].tool, "bash");
    assert_eq!(pending[0].side_effect, SideEffectClass::External);

    h.runtime
        .resolve_approval(
            pending[0].id,
            ApprovalDecision::Denied,
            Actor::user(),
            None,
            None,
        )
        .expect("deny");
    let status = h.wait_until(agent.id, |s| s.is_terminal()).await;
    assert_eq!(status.kind(), AgentStatusKind::Completed, "{status:?}");
}

/// Raising a shell call to External never drops the gate it had: a policy that
/// asks for mutations but not external effects still asks before `curl`.
#[tokio::test]
async fn a_mutation_only_policy_still_gates_a_networked_shell_command() {
    let h = harness(vec![
        ScriptedClient::tool_call("bash", json!({"command": "curl -sI https://example.com"})),
        ScriptedClient::tool_call("complete_task", json!({"summary": "denied"})),
    ])
    .await;
    let mut s = spec("check a header");
    s.approval = Some(ApprovalPolicy {
        require_for: [SideEffectClass::Mutation].into(),
    });
    let (agent, _) = h.runtime.create_agent(s, Actor::user()).expect("create");
    h.wait_until(agent.id, |s| s.kind() == AgentStatusKind::Waiting)
        .await;
    let pending = h.store.pending_approvals().expect("pending");
    assert_eq!(pending.len(), 1, "{pending:?}");
    assert_eq!(pending[0].side_effect, SideEffectClass::External);

    h.runtime
        .resolve_approval(
            pending[0].id,
            ApprovalDecision::Denied,
            Actor::user(),
            None,
            None,
        )
        .expect("deny");
    let status = h.wait_until(agent.id, |s| s.is_terminal()).await;
    assert_eq!(status.kind(), AgentStatusKind::Completed, "{status:?}");
}
