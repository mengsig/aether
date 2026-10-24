//! cancellation restart scenarios and fixtures.
use super::*;

#[tokio::test]
async fn cancelled_agent_stops_and_receives_no_new_work() {
    let client_delay = Duration::from_millis(400);
    let h = harness(vec![
        ScriptedClient::text("thinking slowly"),
        ScriptedClient::text("should never run"),
    ])
    .await;
    h.client.set_delay(client_delay);
    let (agent, _) = h
        .runtime
        .create_agent(spec("slow work"), Actor::user())
        .expect("create");
    h.wait_until(agent.id, |s| s.kind() == AgentStatusKind::Running)
        .await;
    h.runtime
        .cancel_agent(agent.id, Some("operator".into()), Actor::user())
        .expect("cancel");
    let status = h.wait_until(agent.id, |s| s.is_terminal()).await;
    assert_eq!(status.kind(), AgentStatusKind::Cancelled);
    let task = h
        .store
        .task(agent.current_task.expect("task"))
        .expect("t")
        .expect("exists");
    assert_eq!(task.status.kind(), TaskStatusKind::Cancelled);
    tokio::time::sleep(Duration::from_millis(300)).await;
    assert_eq!(
        h.client.remaining(),
        1,
        "no further invocation after cancel"
    );
    let err = h
        .runtime
        .send_message(agent.id, vec![ContentBlock::text("hi")], Actor::user())
        .unwrap_err();
    assert!(err.to_string().contains("cancelled"));
    assert!(h.store.lease(agent.id).expect("lease").is_none());
}

#[tokio::test]
async fn interrupt_stops_the_turn_and_the_agent_keeps_talking() {
    use aether_core::model::{BlockStart, StreamEvent};
    // A reply that streams word by word, so an interrupt lands mid-sentence.
    let mut slow = vec![
        StreamEvent::MessageStart {
            provider_message_id: Some("scripted".into()),
            model: Some("scripted".into()),
        },
        StreamEvent::BlockStart {
            index: 0,
            block: BlockStart::Text {
                text: String::new(),
            },
        },
    ];
    slow.extend((0..40).map(|i| StreamEvent::TextDelta {
        index: 0,
        text: format!("word{i} "),
    }));
    slow.push(StreamEvent::BlockStop { index: 0 });
    slow.push(StreamEvent::Stop {
        reason: aether_core::model::StopReason::EndTurn,
    });
    slow.push(StreamEvent::MessageStop);
    let h = harness(vec![Ok(slow), ScriptedClient::text("Back again.")]).await;
    h.client.set_delay(Duration::from_millis(60));
    let (agent, _) = h
        .runtime
        .create_agent(
            CreateAgentSpec {
                role: "assistant".into(),
                start: true,
                config: Some(AgentConfig {
                    long_lived: true,
                    ..Default::default()
                }),
                ..Default::default()
            },
            Actor::user(),
        )
        .expect("create");
    h.wait_until(agent.id, |s| s.kind() == AgentStatusKind::Waiting)
        .await;
    assert!(
        !h.runtime
            .interrupt_agent(agent.id, Actor::user())
            .expect("interrupt"),
        "nothing to interrupt while idle"
    );
    h.runtime
        .send_message(agent.id, vec![ContentBlock::text("tell me")], Actor::user())
        .expect("send");
    h.wait_until(agent.id, |s| s.kind() == AgentStatusKind::Running)
        .await;
    // Let a few words stream, then stop it.
    tokio::time::sleep(Duration::from_millis(600)).await;
    assert!(
        h.runtime
            .interrupt_agent(agent.id, Actor::user())
            .expect("interrupt")
    );
    let status = h
        .wait_until(agent.id, |s| s.kind() != AgentStatusKind::Running)
        .await;
    assert_eq!(
        status,
        AgentStatus::Waiting {
            condition: aether_core::agent::WaitCondition::Inbox
        },
        "an interrupted agent idles instead of ending"
    );
    let t = types(h.store.as_ref(), agent.id);
    assert!(t.contains(&"agent.interrupted".to_owned()), "{t:?}");
    assert!(!t.contains(&"agent.cancelled".to_owned()), "{t:?}");
    let transcript = h.store.transcript(agent.id, 0).expect("transcript");
    let partial = transcript.last().expect("partial reply");
    assert_eq!(
        partial.message.role,
        aether_core::model::MessageRole::Assistant
    );
    assert!(
        partial.message.text().starts_with("word0 word1"),
        "streamed text is kept: {:?}",
        partial.message.text()
    );
    assert!(h.store.lease(agent.id).expect("lease").is_none());

    // The conversation continues from the partial reply.
    h.client.set_delay(Duration::ZERO);
    h.runtime
        .send_message(agent.id, vec![ContentBlock::text("go on")], Actor::user())
        .expect("send");
    let deadline = tokio::time::Instant::now() + Duration::from_secs(10);
    while h.client.remaining() > 0 {
        assert!(tokio::time::Instant::now() < deadline);
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    h.wait_until(agent.id, |s| s.kind() == AgentStatusKind::Waiting)
        .await;
    let transcript = h.store.transcript(agent.id, 0).expect("transcript");
    assert_eq!(
        transcript.last().expect("reply").message.text(),
        "Back again."
    );
    let second = &h.client.requests()[1];
    assert!(
        second
            .messages
            .iter()
            .any(|m| m.role == aether_core::model::MessageRole::Assistant
                && m.text().starts_with("word0")),
        "the next request carries the interrupted reply"
    );
}

#[tokio::test]
async fn restart_classifies_interrupted_turns() {
    let recovery_started = now();
    let inspect_id = AgentId::new();
    let retry_id = AgentId::new();
    let h = harness_with(vec![], |store| {
        for (id, tool) in [(inspect_id, "bash"), (retry_id, "read_file")] {
            let at = now();
            let mut agent = aether_core::agent::Agent {
                archived: false,
                id,
                parent: None,
                role: "worker".into(),
                name: "w".into(),
                scope: Scope::default(),
                model: ModelRef::new("anthropic", "claude-sonnet-5-5"),
                fallbacks: Vec::new(),
                status: AgentStatus::Created,
                current_task: None,
                inbox_cursor: 0,
                checkpoint: None,
                engine_session: None,
                engine_usage: None,
                engine_sent: None,
                status_since: None,
                paused: false,
                paused_via: None,
                resume_wait: None,
                restarted_from: None,
                restarted_as: None,
                engine_alive: false,
                last_event_at: None,
                config: AgentConfig::default(),
                stats: Default::default(),
                run: Default::default(),
                created_at: at,
                updated_at: at,
            };
            agent.transition(AgentStatus::Ready, at).expect("ready");
            agent.transition(AgentStatus::Running, at).expect("running");
            agent.status_since = Some(at - chrono::Duration::days(1));
            let call = ToolCall {
                id: Default::default(),
                provider_call_id: "p".into(),
                name: tool.into(),
                input: json!({}),
                agent_id: id,
                task_id: None,
                invocation_id: None,
                resources: vec![],
                idempotency_key: None,
                requested_at: at,
            };
            store
                .commit(
                    Commit::new()
                        .event(
                            EventDraft::new(Actor::System, EventKind::AgentStarted { turn: 1 })
                                .agent(id),
                        )
                        .event(
                            EventDraft::new(
                                Actor::System,
                                EventKind::ToolRequested {
                                    call: call.clone(),
                                    side_effect: aether_engine::side_effect_of(tool),
                                },
                            )
                            .agent(id),
                        )
                        .event(
                            EventDraft::new(
                                Actor::System,
                                EventKind::ToolStarted { call: call.id },
                            )
                            .agent(id),
                        )
                        .mutate(Mutation::UpsertAgent(agent))
                        .mutate(Mutation::SetLease(Lease::new(
                            id,
                            "dead-daemon",
                            1,
                            at,
                            Duration::from_secs(1),
                        ))),
                )
                .expect("seed");
        }
    })
    .await;
    let inspect = h.store.agent(inspect_id).expect("a").expect("exists");
    assert!(
        matches!(inspect.status, AgentStatus::Blocked { .. }),
        "{:?}",
        inspect.status
    );
    assert!(
        inspect
            .status_since
            .is_some_and(|at| at >= recovery_started)
    );
    let t = types(h.store.as_ref(), inspect_id);
    assert!(t.contains(&"recovery.classified".to_string()));
    assert!(t.contains(&"lease.expired".to_string()));
    assert!(t.contains(&"tool.cancelled".to_string()));
    assert!(h.store.lease(inspect_id).expect("lease").is_none());
    // The read-only interruption is safe to retry: the agent is schedulable
    // again (it may already have been parked as idle by the scheduler).
    let retry = h.store.agent(retry_id).expect("a").expect("exists");
    assert!(
        matches!(
            retry.status.kind(),
            AgentStatusKind::Ready | AgentStatusKind::Waiting
        ),
        "{:?}",
        retry.status
    );
    let events = h
        .store
        .events(0, 1000, &EventFilter::for_agent(retry_id))
        .expect("events");
    let class = events.iter().find_map(|e| match &e.kind {
        EventKind::RecoveryClassified { class, .. } => Some(*class),
        _ => None,
    });
    assert_eq!(class, Some(aether_core::event::RecoveryClass::Retry));
}
