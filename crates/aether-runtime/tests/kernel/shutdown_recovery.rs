//! A daemon stop interrupts work; it does not end it. The next start resumes
//! what the stop cut short, while a cancel someone asked for stays a cancel.
use super::*;

/// The agent's task, as the store holds it now.
fn task_of(h: &Harness, id: AgentId) -> aether_core::task::Task {
    let task = h
        .store
        .agent(id)
        .expect("agent")
        .expect("exists")
        .current_task
        .expect("the agent carries its task");
    h.store.task(task).expect("task").expect("exists")
}

/// A stopped shell can have partial effects; restart requires inspection.
#[tokio::test]
async fn a_daemon_stop_mid_mutating_tool_requires_inspection() {
    let h = harness(vec![ScriptedClient::tool_call(
        "bash",
        json!({"command": "echo applied >> partial.txt; sleep 3600"}),
    )])
    .await;
    let (agent, _) = h
        .runtime
        .create_agent(spec("slow command"), Actor::user())
        .expect("create");
    let marker = h.workspace_root.join("partial.txt");
    h.wait_for("the shell applied its effect", || marker.exists())
        .await;

    let h2 = restart_over(
        &h,
        vec![ScriptedClient::tool_call(
            "complete_task",
            json!({"summary": "finished after the restart"}),
        )],
    )
    .await;
    let status = h2
        .store
        .agent(agent.id)
        .expect("agent")
        .expect("exists")
        .status;
    assert!(
        matches!(
            status,
            AgentStatus::Blocked {
                reason: BlockReason::Inspection { .. }
            }
        ),
        "{status:?}"
    );
    assert_eq!(task_of(&h2, agent.id).status.kind(), TaskStatusKind::Active);
    assert_eq!(h2.client.remaining(), 1, "no automatic retry");
    assert!(
        !h2.runtime.resource_holdings().is_empty(),
        "partial effects keep their keys fenced"
    );
    assert_eq!(
        recovery_class(h2.store.as_ref(), agent.id),
        Some(aether_core::event::RecoveryClass::Inspect)
    );
    h2.runtime
        .resume_agent(agent.id, false, Actor::user())
        .expect("inspection cleared");
    let status = h2.wait_until(agent.id, |s| s.is_terminal()).await;
    assert_eq!(
        std::fs::read_to_string(marker).expect("partial effect"),
        "applied\n"
    );
    let seen = types(h2.store.as_ref(), agent.id);
    assert_eq!(status.kind(), AgentStatusKind::Completed, "{seen:?}");
    assert_eq!(
        task_of(&h2, agent.id).status.kind(),
        TaskStatusKind::Completed
    );
    for ended in ["agent.cancelled", "task.cancelled"] {
        assert!(!seen.iter().any(|t| t == ended), "{ended} in {seen:?}");
    }
    assert!(seen.iter().any(|t| t == "tool.cancelled"), "{seen:?}");
    assert_eq!(seen.iter().filter(|t| *t == "tool.cancelled").count(), 1);
    h2.runtime.shutdown().await;
}

/// A cancel someone asked for is still a cancel when the daemon stops before
/// the turn has settled it: nothing resumes it on the next start.
#[tokio::test]
async fn a_cancel_racing_the_daemon_stop_still_cancels() {
    let h = harness(vec![ScriptedClient::tool_call(
        "bash",
        json!({"command": "sleep 30"}),
    )])
    .await;
    let (agent, _) = h
        .runtime
        .create_agent(spec("slow command"), Actor::user())
        .expect("create");
    wait_for_event(h.store.as_ref(), agent.id, "tool.started").await;

    h.runtime
        .cancel_agent(agent.id, Some("not needed".into()), Actor::user())
        .expect("cancel");
    // The turn settles the cancel only after the stop has begun.
    let h2 = restart_over(&h, vec![ScriptedClient::text("never asked")]).await;
    let status = h2.wait_until(agent.id, |s| s.is_terminal()).await;
    assert_eq!(
        status,
        AgentStatus::Cancelled {
            reason: Some("not needed".into())
        }
    );
    assert_eq!(
        task_of(&h2, agent.id).status.kind(),
        TaskStatusKind::Cancelled
    );
    assert_eq!(recovery_class(h2.store.as_ref(), agent.id), None);
    h2.runtime.shutdown().await;
}

#[tokio::test]
async fn a_daemon_stop_mid_model_retries_without_cancelling_the_task() {
    let h = harness(vec![ScriptedClient::text("held")]).await;
    let _gate = h.client.hold_response_at(1);
    let (agent, _) = h
        .runtime
        .create_agent(spec("finish after restart"), Actor::user())
        .expect("create");
    h.wait_for("model entered the gate", || h.client.requests().len() == 1)
        .await;
    let h2 = restart_over(
        &h,
        vec![ScriptedClient::tool_call(
            "complete_task",
            json!({"summary": "done"}),
        )],
    )
    .await;
    assert_eq!(
        h2.wait_until(agent.id, |s| s.is_terminal()).await.kind(),
        AgentStatusKind::Completed
    );
    assert_eq!(
        recovery_class(h2.store.as_ref(), agent.id),
        Some(aether_core::event::RecoveryClass::Retry)
    );
    assert!(!types(h2.store.as_ref(), agent.id).contains(&"agent.cancelled".to_owned()));
    h2.runtime.shutdown().await;
}

#[tokio::test]
async fn cancelled_read_only_and_unstarted_calls_remain_retryable() {
    for (tool, started) in [("read_file", true), ("bash", false)] {
        let id = AgentId::new();
        let call = ToolCallId::new();
        let h = harness_with(vec![], |store| {
            seed_crashed_turn(store, id, None, Vec::new(), false);
            let draft = |kind| EventDraft::new(Actor::System, kind).agent(id);
            let mut commit = Commit::new().event(draft(EventKind::ToolRequested {
                call: ToolCall {
                    id: call,
                    provider_call_id: "p".into(),
                    name: tool.into(),
                    input: json!({}),
                    agent_id: id,
                    task_id: None,
                    invocation_id: None,
                    resources: vec![],
                    idempotency_key: None,
                    requested_at: now(),
                },
                side_effect: if started {
                    SideEffectClass::ReadOnly
                } else {
                    SideEffectClass::Mutation
                },
            }));
            if started {
                commit.push_event(draft(EventKind::ToolStarted { call }));
            }
            commit.push_event(draft(EventKind::ToolCancelled { call }));
            store.commit(commit).expect("seed cancelled call");
        })
        .await;
        assert_eq!(
            recovery_class(h.store.as_ref(), id),
            Some(aether_core::event::RecoveryClass::Retry),
            "{tool}"
        );
        assert_eq!(
            types(h.store.as_ref(), id)
                .iter()
                .filter(|t| *t == "tool.cancelled")
                .count(),
            1
        );
        h.runtime.shutdown().await;
    }
}

#[tokio::test]
async fn restart_composes_a_committed_tool_outcome_before_calling_the_model() {
    let id = AgentId::new();
    let call = ToolCallId::new();
    let invocation = InvocationId::new();
    let h = harness_with(vec![ScriptedClient::text("continued")], |store| {
        seed_crashed_turn(store, id, None, Vec::new(), false);
        let draft = |kind| EventDraft::new(Actor::System, kind).agent(id);
        store
            .commit(
                Commit::new()
                    .event(draft(EventKind::AgentMessage {
                        role: aether_core::model::MessageRole::Assistant,
                        content: vec![ContentBlock::ToolUse {
                            id: "provider-call".into(),
                            name: "write_file".into(),
                            input: json!({"path": "note.txt", "content": "applied"}),
                        }],
                        invocation: Some(invocation),
                    }))
                    .event(draft(EventKind::ToolRequested {
                        call: ToolCall {
                            id: call,
                            provider_call_id: "provider-call".into(),
                            name: "write_file".into(),
                            input: json!({"path": "note.txt", "content": "applied"}),
                            agent_id: id,
                            task_id: None,
                            invocation_id: Some(invocation),
                            resources: vec![],
                            idempotency_key: None,
                            requested_at: now(),
                        },
                        side_effect: SideEffectClass::IdempotentMutation,
                    }))
                    .event(draft(EventKind::ToolStarted { call }))
                    .event(draft(EventKind::ToolSucceeded {
                        call,
                        outcome: ToolOutcome::ok("already applied"),
                    })),
            )
            .expect("committed outcome without its transcript message");
    })
    .await;
    h.wait_for("the resumed model request", || {
        !h.client.requests().is_empty()
    })
    .await;
    let requests = h.client.requests();
    assert!(requests[0].messages.iter().any(|message| message.content.iter().any(|block| matches!(block,
        ContentBlock::ToolResult { tool_use_id, content, is_error: false }
            if tool_use_id == "provider-call" && content == &vec![aether_core::model::ToolResultBlock::Text { text: "already applied".into() }]
    ))), "the committed outcome was not sent: {:?}", requests[0].messages);
    assert_eq!(
        types(h.store.as_ref(), id)
            .iter()
            .filter(|t| *t == "tool.started")
            .count(),
        1
    );
    h.runtime.shutdown().await;
}

#[tokio::test]
async fn a_durable_cancel_is_applied_before_restart_classifies_the_turn() {
    let id = AgentId::new();
    let child_id = AgentId::new();
    let h = harness_with(vec![], |store| {
        seed_crashed_turn(store, id, None, Vec::new(), false);
        seed_crashed_turn(store, child_id, None, Vec::new(), false);
        let mut child = store.agent(child_id).expect("child").expect("exists");
        child.parent = Some(id);
        store
            .commit(
                Commit::new()
                    .mutate(Mutation::UpsertAgent(child))
                    .mutate(Mutation::SetMeta {
                    key: "runtime.pending_cancellations".into(),
                    value:
                        json!({id.to_string(): {"by": Actor::user(), "reason": "operator cancel"}})
                            .to_string(),
                }),
            )
            .expect("cancel intent committed before shutdown settled the turn");
    })
    .await;
    assert_eq!(
        h.wait_until(id, |status| status.is_terminal()).await,
        AgentStatus::Cancelled {
            reason: Some("operator cancel".into())
        }
    );
    assert_eq!(
        recovery_class(h.store.as_ref(), id),
        None,
        "an explicit cancel is not a retry"
    );
    assert!(
        h.store.inbox(id, 0).expect("inbox").is_empty(),
        "no recovery wake for cancelled work"
    );
    assert_eq!(
        h.wait_until(child_id, |status| status.is_terminal())
            .await
            .kind(),
        AgentStatusKind::Cancelled
    );
    assert_eq!(recovery_class(h.store.as_ref(), child_id), None);
    assert!(h.store.inbox(child_id, 0).expect("child inbox").is_empty());
    h.runtime.shutdown().await;
}

#[tokio::test]
async fn an_approved_call_stopped_while_waiting_for_keys_never_started() {
    let h = harness_configured(
        vec![
            ScriptedClient::tool_call(
                "bash",
                json!({"command": "echo held > held.txt; sleep 3600"}),
            ),
            ScriptedClient::tool_call(
                "bash",
                json!({"command": "echo should-not-run > queued.txt"}),
            ),
        ],
        |_| {},
        |config| config.max_concurrent_turns = 2,
    )
    .await;
    let (holder, _) = h
        .runtime
        .create_agent(spec("hold keys"), Actor::user())
        .expect("holder");
    h.wait_for("holder running", || {
        h.workspace_root.join("held.txt").exists()
    })
    .await;
    let mut request = spec("wait for approved tool");
    request.config.as_mut().expect("config").approval = ApprovalPolicy {
        require_for: [SideEffectClass::Mutation].into(),
    };
    let (waiter, _) = h
        .runtime
        .create_agent(request, Actor::user())
        .expect("waiter");
    h.wait_until(waiter.id, |s| matches!(s, AgentStatus::Waiting { .. }))
        .await;
    let approval = h.store.pending_approvals().expect("approvals").remove(0);
    h.runtime
        .resolve_approval(
            approval.id,
            ApprovalDecision::Approved,
            Actor::user(),
            None,
            None,
        )
        .expect("approve");
    h.wait_until(waiter.id, |s| {
        matches!(
            s,
            AgentStatus::Blocked {
                reason: BlockReason::Resource { .. }
            }
        )
    })
    .await;
    h.runtime.shutdown().await;
    assert!(
        !types(h.store.as_ref(), waiter.id).contains(&"tool.started".to_owned()),
        "a call queued for keys has not run"
    );
    assert!(!h.workspace_root.join("queued.txt").exists());
    let h2 = harness_full(
        vec![ScriptedClient::tool_call(
            "complete_task",
            json!({"summary": "cancelled call handled"}),
        )],
        |_| {},
        |_| {},
        None,
        None,
        Some(h.store.clone()),
        |_| {},
    )
    .await;
    assert_eq!(
        recovery_class(h2.store.as_ref(), holder.id),
        Some(aether_core::event::RecoveryClass::Inspect)
    );
    assert_eq!(
        recovery_class(h2.store.as_ref(), waiter.id),
        Some(aether_core::event::RecoveryClass::Retry)
    );
    assert_eq!(
        h2.wait_until(waiter.id, |s| s.is_terminal()).await.kind(),
        AgentStatusKind::Completed
    );
    h2.runtime.shutdown().await;
}

#[tokio::test]
async fn clearing_an_inspection_wakes_the_interrupted_turn() {
    let id = AgentId::new();
    let h = harness_with(
        vec![ScriptedClient::text("continued after inspection")],
        |store| {
            seed_crashed_turn(store, id, Some("bash"), Vec::new(), false);
            store
                .commit(
                    Commit::new().event(
                        EventDraft::new(
                            Actor::System,
                            EventKind::AgentMessage {
                                role: aether_core::model::MessageRole::Assistant,
                                content: vec![ContentBlock::ToolUse {
                                    id: "p".into(),
                                    name: "bash".into(),
                                    input: json!({"command": "unknown partial effects"}),
                                }],
                                invocation: None,
                            },
                        )
                        .agent(id),
                    ),
                )
                .expect("assistant tool use was the last committed message");
        },
    )
    .await;
    h.runtime
        .resume_agent(id, false, Actor::user())
        .expect("clear inspection");
    assert!(
        h.store
            .inbox(id, 0)
            .expect("inbox")
            .iter()
            .any(|item| matches!(&item.kind,
                aether_core::inbox::InboxKind::Wake { reason } if reason.contains("inspection")
            )),
        "clearing inspection must wake the interrupted work"
    );
    h.wait_for("model resumed after inspection", || {
        h.client.requests().len() == 1
    })
    .await;
    h.runtime.shutdown().await;
}

/// A stop is not a compactor failure: the request outlives it, as after a crash.
#[tokio::test]
async fn a_daemon_stop_mid_compaction_leaves_the_request_for_the_next_start() {
    let checkpoint = json!({
        "checkpoint": {
            "objective": "keep chatting",
            "state": "six exchanges so far",
            "decisions": [],
            "constraints": [],
            "open_loops": []
        },
        "memory_candidates": []
    })
    .to_string();
    let h = harness(vec![ScriptedClient::text(&checkpoint)]).await;
    let (agent, _) = h
        .runtime
        .create_agent(idle_spec(), Actor::user())
        .expect("create");
    h.wait_until(agent.id, |s| s.kind() == AgentStatusKind::Waiting)
        .await;
    let mut history = Commit::new();
    for i in 0..6 {
        for (role, text) in [
            (
                aether_core::model::MessageRole::User,
                format!("message {i}"),
            ),
            (
                aether_core::model::MessageRole::Assistant,
                format!("reply {i}"),
            ),
        ] {
            history.push_event(
                EventDraft::new(
                    Actor::System,
                    EventKind::AgentMessage {
                        role,
                        content: vec![ContentBlock::text(text)],
                        invocation: None,
                    },
                )
                .agent(agent.id)
                .run(agent.run),
            );
        }
    }
    h.runtime.apply(history).expect("seed history");
    let _gate = h.client.hold_response_at(1);
    h.runtime
        .request_compaction(agent.id, Actor::user())
        .expect("request compaction");
    h.wait_for("compactor entered the gate", || {
        h.client.requests().len() == 1
    })
    .await;

    let h2 = restart_over(
        &h,
        vec![
            ScriptedClient::text(&checkpoint),
            ScriptedClient::text("after compaction"),
        ],
    )
    .await;
    h2.wait_for("the restarted turn compacted", || {
        h2.store
            .latest_checkpoint(agent.id)
            .expect("checkpoint")
            .is_some()
    })
    .await;
    let seen = types(h2.store.as_ref(), agent.id);
    assert!(
        !seen.iter().any(|t| t == "compaction.failed"),
        "the stop was recorded as a compactor failure: {seen:?}"
    );
    h2.runtime.shutdown().await;
}

/// The batch settled the task before the stop cut its sibling short: that
/// ending stands, rather than reviving a worker whose task is already closed.
#[tokio::test]
async fn a_task_completed_in_the_stopped_batch_ends_the_agent() {
    let h = harness(vec![ScriptedClient::tool_calls(
        &[
            ("complete_task", json!({"summary": "done before the stop"})),
            (
                "bash",
                json!({"command": "echo started > started.txt; sleep 3600"}),
            ),
        ],
        None,
    )])
    .await;
    let (agent, _) = h
        .runtime
        .create_agent(spec("finish, then a slow command"), Actor::user())
        .expect("create");
    let marker = h.workspace_root.join("started.txt");
    h.wait_for("the sibling command started", || marker.exists())
        .await;

    let h2 = restart_over(&h, vec![ScriptedClient::text("never asked")]).await;
    let status = h2
        .store
        .agent(agent.id)
        .expect("agent")
        .expect("exists")
        .status;
    assert_eq!(
        status,
        AgentStatus::Completed {
            summary: Some("done before the stop".into())
        }
    );
    assert_eq!(
        task_of(&h2, agent.id).status.kind(),
        TaskStatusKind::Completed
    );
    assert_eq!(recovery_class(h2.store.as_ref(), agent.id), None);
    assert_eq!(h2.client.remaining(), 1, "no turn after the task closed");
    h2.runtime.shutdown().await;
}
