use super::*;
use crate::app::transcript::{CallStatus, QuestionState};
use aether_core::id::{ApprovalId, InboxItemId, MessageId};
use aether_core::inbox::{InboxItem, InboxKind};
use aether_core::task::{Task, TaskResult, TaskStatus};
use aether_core::tool::{
    ApprovalDecision, ApprovalRequest, ApprovalStatus, SideEffectClass, ToolCall, ToolOutcome,
};

#[tokio::test]
async fn reconnect_offline_agent_states_match_fresh_tree() {
    let mut fixture = Fixture::new().await;
    let mut live = fixture.fresh().await;
    for (status, paused) in [
        (AgentStatus::Running, false),
        (
            AgentStatus::Waiting {
                condition: WaitCondition::Inbox,
            },
            false,
        ),
        (
            AgentStatus::Waiting {
                condition: WaitCondition::Inbox,
            },
            true,
        ),
        (
            AgentStatus::Completed {
                summary: Some("done".into()),
            },
            false,
        ),
        (
            AgentStatus::Failed {
                error: "fixture failure".into(),
            },
            false,
        ),
    ] {
        live.reduce(AppEvent::Disconnected { because: None });
        fixture.agent.status = status.clone();
        fixture.agent.paused = paused;
        fixture.agent.status_since = Some(now());
        fixture.put_agent(fixture.agent.clone());
        fixture.reconnect(&mut live).await;
        let fresh = fixture.fresh().await;
        assert_eq!(live.tree, fresh.tree);
        assert_eq!(live.selected_agent().unwrap().status, status);
        assert_eq!(live.selected_agent().unwrap().paused, paused);
    }
    fixture.stop().await;
}

#[tokio::test]
async fn reconnect_offline_child_and_task_creation_and_completion_match_fresh_tree() {
    let mut fixture = Fixture::new().await;
    let mut live = fixture.fresh().await;
    live.reduce(AppEvent::Disconnected { because: None });
    let mut child = fixture.agent.clone();
    child.id = AgentId::new();
    child.parent = Some(fixture.agent.id);
    let mut task = Task::new("offline child task", Actor::user(), child.run, now());
    task.owner = Some(child.id);
    task.status = TaskStatus::Active;
    child.current_task = Some(task.id);
    fixture.put_agent(child.clone());
    fixture
        .store
        .commit(Commit::new().mutate(Mutation::UpsertTask(task.clone())))
        .unwrap();
    fixture.reconnect(&mut live).await;
    let fresh = fixture.fresh().await;
    assert_eq!(live.tree, fresh.tree);
    assert_eq!(live.tree.nodes[&fixture.agent.id].children, vec![child.id]);
    assert_eq!(live.tree.nodes[&child.id].task, Some(task.clone()));
    live.reduce(AppEvent::Disconnected { because: None });
    task.status = TaskStatus::Completed {
        result: TaskResult::summary("child done"),
    };
    child.status = AgentStatus::Completed {
        summary: Some("child done".into()),
    };
    fixture.put_agent(child.clone());
    fixture
        .store
        .commit(Commit::new().mutate(Mutation::UpsertTask(task.clone())))
        .unwrap();
    fixture.reconnect(&mut live).await;
    let fresh = fixture.fresh().await;
    assert_eq!(live.tree, fresh.tree);
    assert_eq!(live.tree.nodes[&child.id].task, Some(task));
    assert!(live.tree.nodes[&child.id].agent.status.is_terminal());
    fixture.stop().await;
}

#[tokio::test]
async fn reconnect_offline_billing_and_budget_match_fresh_snapshots() {
    let mut fixture = Fixture::new().await;
    let mut live = fixture.fresh().await;
    live.reduce(AppEvent::Disconnected { because: None });
    let usage = Usage {
        input_tokens: 42_000,
        output_tokens: 321,
        cost_usd: 1.25,
        ..Default::default()
    };
    fixture.agent.stats.usage = usage;
    fixture.agent.config.token_budget = Some(80_000);
    fixture.put_agent(fixture.agent.clone());
    fixture.commit(vec![EventKind::AgentUsageUpdated {
        total: usage,
        turn_usage: usage,
    }]);
    fixture.reconnect(&mut live).await;
    let fresh = fixture.fresh().await;
    assert_eq!(live.tree, fresh.tree);
    assert_eq!(
        live.status.as_ref().unwrap().usage_total,
        fresh.status.as_ref().unwrap().usage_total
    );
    assert_eq!(live.tree.nodes[&fixture.agent.id].subtree_usage, usage);
    assert_eq!(
        live.selected_agent().unwrap().config.token_budget,
        Some(80_000)
    );
    assert!(crate::testing::render_to(&live, 180, 30).contains("$1.25"));
    assert!(crate::testing::render_to(&fresh, 180, 30).contains("$1.25"));
    fixture.stop().await;
}

#[tokio::test]
async fn reconnect_offline_context_usage_matches_fresh_capacity_display() {
    let mut fixture = Fixture::new().await;
    let mut live = fixture.fresh().await;
    let model = live
        .models
        .iter()
        .find(|m| m.capabilities.context_window >= 100_000)
        .unwrap();
    let threshold = live.compaction_threshold(&model.model).unwrap();
    fixture.agent.model = model.model.clone();
    fixture.agent.stats.context_tokens = Some(threshold / 2);
    fixture.put_agent(fixture.agent.clone());
    fixture.reconnect(&mut live).await;
    assert_eq!(live.context_tokens[&fixture.agent.id], threshold / 2);
    live.reduce(AppEvent::Disconnected { because: None });
    fixture.agent.stats.context_tokens = Some(threshold / 4);
    fixture.put_agent(fixture.agent.clone());
    fixture.commit(vec![EventKind::Custom {
        name: "context.usage".into(),
        payload: serde_json::json!({"tokens": threshold / 4}),
    }]);
    fixture.reconnect(&mut live).await;
    let fresh = fixture.fresh().await;
    assert_eq!(live.context_tokens, fresh.context_tokens);
    assert_eq!(live.context_tokens[&fixture.agent.id], threshold / 4);
    assert!(crate::testing::render_to(&live, 180, 30).contains("75% until compaction"));
    assert!(crate::testing::render_to(&fresh, 180, 30).contains("75% until compaction"));
    fixture.stop().await;
}

fn approval(fixture: &Fixture) -> ApprovalRequest {
    ApprovalRequest {
        id: ApprovalId::new(),
        agent_id: fixture.agent.id,
        task_id: None,
        call_id: ToolCallId::new(),
        tool: "bash".into(),
        description: "echo approved".into(),
        argument: Some("echo approved".into()),
        resources: vec![],
        side_effect: SideEffectClass::Mutation,
        requested_at: now(),
        status: ApprovalStatus::Pending,
    }
}

fn operator_reply(agent: AgentId) -> [EventKind; 2] {
    let item = InboxItem {
        id: InboxItemId::new(),
        agent_id: agent,
        seq: 1,
        kind: InboxKind::Message {
            message_id: MessageId::new(),
            from: Actor::user(),
            content: vec![ContentBlock::text("yes")],
        },
        delivered_at: now(),
    };
    let items = vec![item.id];
    [
        EventKind::InboxDelivered { item },
        EventKind::InboxConsumed { up_to: 1, items },
    ]
}

fn pending_attention(fixture: &Fixture) -> ApprovalRequest {
    let request = approval(fixture);
    fixture
        .store
        .commit(Commit::new().mutate(Mutation::PutApproval(request.clone())))
        .unwrap();
    fixture.commit(vec![
        EventKind::ApprovalRequested {
            approval: request.id,
            call: request.call_id,
            tool: request.tool.clone(),
            description: request.description.clone(),
            argument: request.argument.clone(),
            side_effect: request.side_effect,
        },
        EventKind::QuestionAsked {
            question: "Continue?".into(),
        },
    ]);
    assert_eq!(
        fixture.store.approval(request.id).unwrap(),
        Some(request.clone())
    );
    assert_eq!(request.agent_id, fixture.agent.id);
    request
}

#[tokio::test]
async fn reconnect_offline_resolved_approval_and_answered_question_are_not_pending() {
    let mut fixture = Fixture::new().await;
    let mut request = pending_attention(&fixture);
    let mut live = fixture.fresh().await;
    assert_eq!(live.approvals.len(), 1);
    assert!(models(&live, fixture.agent.id).iter().any(|m| matches!(
        m,
        CellModel::Question {
            state: QuestionState::Pending,
            ..
        }
    )));
    live.reduce(AppEvent::Disconnected { because: None });
    request.status = ApprovalStatus::Resolved {
        decision: ApprovalDecision::Approved,
        by: Actor::user(),
        at: now(),
        note: None,
    };
    fixture
        .store
        .commit(Commit::new().mutate(Mutation::PutApproval(request.clone())))
        .unwrap();
    let mut resolved = vec![EventKind::ApprovalResolved {
        approval: request.id,
        decision: ApprovalDecision::Approved,
        by: Actor::user(),
        note: None,
    }];
    resolved.extend(operator_reply(fixture.agent.id));
    fixture.commit(resolved);
    fixture.reconnect(&mut live).await;
    let fresh = fixture.fresh().await;
    assert!(live.approvals.is_empty());
    assert_eq!(live.approvals, fresh.approvals);
    assert_eq!(
        models(&live, fixture.agent.id),
        models(&fresh, fixture.agent.id)
    );
    assert!(!models(&live, fixture.agent.id).iter().any(|m| matches!(
        m,
        CellModel::Question {
            state: QuestionState::Pending,
            ..
        }
    )));
    fixture.stop().await;
}

fn tool_call(agent: AgentId) -> ToolCall {
    ToolCall {
        id: ToolCallId::new(),
        provider_call_id: "one".into(),
        name: "bash".into(),
        input: serde_json::json!({"command": "echo one"}),
        agent_id: agent,
        task_id: None,
        invocation_id: None,
        resources: vec![],
        idempotency_key: None,
        requested_at: now(),
    }
}

#[tokio::test]
async fn reconnect_offline_tools_and_tool_running_at_disconnect_match_fresh() {
    let mut fixture = Fixture::new().await;
    let call = tool_call(fixture.agent.id);
    fixture.commit(vec![
        EventKind::ToolRequested {
            call: call.clone(),
            side_effect: SideEffectClass::ReadOnly,
        },
        EventKind::ToolStarted { call: call.id },
    ]);
    let mut live = fixture.fresh().await;
    assert!(models(&live, fixture.agent.id).iter().any(|m| matches!(
        m,
        CellModel::Exec {
            status: CallStatus::Running,
            ..
        }
    )));
    live.reduce(AppEvent::Disconnected { because: None });
    let mut offline = call.clone();
    offline.id = ToolCallId::new();
    fixture.commit(vec![
        EventKind::ToolSucceeded {
            call: call.id,
            outcome: ToolOutcome::ok("one"),
        },
        EventKind::ToolRequested {
            call: offline.clone(),
            side_effect: SideEffectClass::ReadOnly,
        },
        EventKind::ToolStarted { call: offline.id },
        EventKind::ToolSucceeded {
            call: offline.id,
            outcome: ToolOutcome::ok("two"),
        },
    ]);
    fixture.reconnect(&mut live).await;
    let fresh = fixture.fresh().await;
    assert_eq!(
        models(&live, fixture.agent.id),
        models(&fresh, fixture.agent.id)
    );
    assert_eq!(
        models(&live, fixture.agent.id)
            .iter()
            .filter(|m| matches!(
                m,
                CellModel::Exec {
                    status: CallStatus::Succeeded,
                    ..
                }
            ))
            .count(),
        2
    );
    fixture.stop().await;
}
