//! checkpoints scenarios and fixtures.
use super::*;

fn use_aether_compactor(config: &mut RuntimeConfig) {
    config.default_model = Some(ModelRef::new("openai", "gpt-6.1-sol"));
}

async fn harness(responses: Vec<aether_model::testing::ScriptedResponse>) -> Harness {
    super::harness_configured(responses, |_| {}, use_aether_compactor).await
}

#[tokio::test]
async fn pressure_compaction_preserves_completed_tool_results() {
    use aether_core::model::Message;

    for batch in [false, true] {
        let checkpoint = json!({"checkpoint": {
            "objective": "inspect results", "state": "inspection in progress"
        }})
        .to_string();
        let h = harness(vec![
            ScriptedClient::text(&checkpoint),
            ScriptedClient::text("after compaction"),
        ])
        .await;
        let (agent, _) = h.runtime.create_agent(idle_spec(), Actor::user()).unwrap();
        h.wait_until(agent.id, |s| s.kind() == AgentStatusKind::Waiting)
            .await;

        let calls: Vec<_> = (0..6)
            .map(|i| ContentBlock::ToolUse {
                id: format!("c{i}"),
                name: "shell".into(),
                input: json!({}),
            })
            .collect();
        let results: Vec<_> = (0..6)
            .map(|i| ContentBlock::tool_result_text(format!("c{i}"), format!("OUTPUT-{i}"), false))
            .collect();
        let mut messages = vec![Message::user_text("inspect results")];
        if batch {
            messages.push(Message::assistant_blocks(calls.clone()));
        }
        for (call, result) in calls.into_iter().zip(&results) {
            if !batch {
                messages.push(Message::assistant_blocks(vec![call]));
            }
            messages.push(Message::user_blocks(vec![result.clone()]));
        }
        let mut commit = Commit::new();
        for message in messages {
            commit.push_event(
                EventDraft::new(
                    Actor::agent(agent.id),
                    EventKind::AgentMessage {
                        role: message.role,
                        content: message.content,
                        invocation: None,
                    },
                )
                .agent(agent.id)
                .run(agent.run)
                .run(agent.run),
            );
        }
        let trigger = CompactionTrigger::TokenThreshold {
            tokens: 800,
            threshold: 750,
        };
        commit.push_event(
            EventDraft::new(
                Actor::agent(agent.id),
                EventKind::CompactionRequested {
                    trigger: trigger.clone(),
                },
            )
            .agent(agent.id)
            .run(agent.run)
            .run(agent.run),
        );
        h.runtime.apply(commit).unwrap();
        h.runtime
            .send_message(
                agent.id,
                vec![ContentBlock::text("continue")],
                Actor::user(),
            )
            .unwrap();
        h.wait_for("compacted and answered", || h.client.remaining() == 0)
            .await;
        h.wait_until(agent.id, |s| s.kind() == AgentStatusKind::Waiting)
            .await;

        let checkpoint = h
            .store
            .latest_checkpoint(agent.id)
            .unwrap()
            .expect("checkpoint");
        assert_eq!(checkpoint.provenance.trigger, Some(trigger));
        let requests = h.client.requests();
        assert_eq!(requests.len(), 2);
        assert!(
            requests[0]
                .system
                .iter()
                .any(|block| block.text.contains("compaction service"))
        );
        let folded = requests[0].messages[0].text();
        let raw: Vec<_> = requests[1]
            .messages
            .iter()
            .flat_map(|message| &message.content)
            .collect();
        for (i, result) in results.iter().enumerate() {
            if batch || i >= 2 {
                assert!(raw.contains(&result), "lost raw result {i}, batch={batch}");
            } else {
                assert!(
                    folded.contains(&format!("OUTPUT-{i}")),
                    "lost folded result {i}"
                );
            }
        }
    }
}

#[tokio::test]
async fn explicit_checkpoint_request_compacts_older_transcript() {
    let checkpoint_json = serde_json::json!({
        "checkpoint": {
            "objective": "keep chatting",
            "state": "four exchanges so far",
            "decisions": ["answer briefly"],
            "constraints": ["stay polite"],
            "open_loops": []
        },
        "memory_candidates": [{
            "scope": "global", "kind": "preference",
            "text": "The user prefers brief answers in chat sessions",
            "importance": 0.7, "confidence": 0.8, "tags": ["style"]
        }]
    })
    .to_string();
    let checkpoint_response = ScriptedClient::text(&checkpoint_json);
    let mut checkpoint_usage = checkpoint_response
        .as_ref()
        .unwrap()
        .iter()
        .find_map(|event| match event {
            aether_core::model::StreamEvent::Usage { usage } => Some(*usage),
            _ => None,
        })
        .expect("fixture usage");
    let h = harness(vec![
        ScriptedClient::text("reply one"),
        ScriptedClient::text("reply two"),
        ScriptedClient::text("reply three"),
        ScriptedClient::text("reply four"),
        ScriptedClient::tool_call("request_checkpoint", json!({"reason": "long chat"})),
        // The compactor's turn, then the agent's next reply.
        checkpoint_response,
        ScriptedClient::text("after compaction"),
    ])
    .await;
    let (agent, _) = h
        .runtime
        .create_agent(
            CreateAgentSpec {
                role: "assistant".into(),
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
        .expect("create");
    h.wait_until(agent.id, |s| s.kind() == AgentStatusKind::Waiting)
        .await;
    for i in 1..=5 {
        let before = h.client.remaining();
        h.runtime
            .send_message(
                agent.id,
                vec![ContentBlock::text(format!("message {i}"))],
                Actor::user(),
            )
            .expect("send");
        let deadline = tokio::time::Instant::now() + Duration::from_secs(10);
        while h.client.remaining() == before {
            assert!(
                tokio::time::Instant::now() < deadline,
                "turn {i} did not run"
            );
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
        // Let the turn settle before the next message.
        let deadline = tokio::time::Instant::now() + Duration::from_secs(10);
        loop {
            let a = h.store.agent(agent.id).expect("a").expect("exists");
            let settled = a.status.kind() == AgentStatusKind::Waiting
                || (i == 5
                    && a.status.kind() != AgentStatusKind::Running
                    && h.client.remaining() == 0);
            if settled {
                break;
            }
            assert!(tokio::time::Instant::now() < deadline);
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    }
    // After the checkpoint request the agent takes one more turn, which
    // compacts first and then answers.
    let deadline = tokio::time::Instant::now() + Duration::from_secs(10);
    while h.client.remaining() > 0 {
        assert!(
            tokio::time::Instant::now() < deadline,
            "compaction turn did not run"
        );
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    h.wait_until(agent.id, |s| s.kind() == AgentStatusKind::Waiting)
        .await;

    let checkpoint = h
        .store
        .latest_checkpoint(agent.id)
        .expect("cp")
        .expect("exists");
    assert_eq!(checkpoint.version, 1);
    assert_eq!(checkpoint.draft.objective, "keep chatting");
    assert!(checkpoint.provenance.to_seq > checkpoint.provenance.from_seq);
    let updated = h.store.agent(agent.id).expect("a").expect("exists");
    assert_eq!(updated.checkpoint, Some(checkpoint.id));
    let t = types(h.store.as_ref(), agent.id);
    assert!(t.contains(&"compaction.requested".to_string()));
    assert!(t.contains(&"checkpoint.created".to_string()));
    assert!(t.contains(&"compaction.completed".to_string()));
    assert!(t.contains(&"memory.accepted".to_string()), "{t:?}");

    // The turn after compaction saw the checkpoint layer and a shorter tail.
    let turns = h.store.invocations_for_agent(agent.id).expect("inv");
    let compactor_model = checkpoint
        .provenance
        .compactor_model
        .as_ref()
        .expect("compactor model");
    let resolved = h
        .runtime
        .models()
        .resolve(compactor_model)
        .expect("registered compactor");
    let price = resolved
        .model
        .capabilities
        .cost
        .as_ref()
        .expect("priced API compactor");
    checkpoint_usage.set_cost(Some(price.cost_of(&checkpoint_usage)));
    checkpoint_usage.cost_basis = aether_core::model::CostBasis::ListPrice;
    let mut expected = checkpoint_usage;
    for invocation in &turns {
        expected += invocation.usage;
    }
    // The independent sum visits compaction first, so floating-point addition
    // can differ in its last bit from the chronological runtime total.
    assert!((updated.stats.usage.cost_usd - expected.cost_usd).abs() < 1e-12);
    expected.cost_usd = updated.stats.usage.cost_usd;
    assert_eq!(
        updated.stats.usage, expected,
        "API compaction is charged once in addition to ordinary calls"
    );
    let events = h
        .store
        .events(0, 10_000, &EventFilter::for_agent(agent.id))
        .unwrap();
    let billed = events
        .iter()
        .position(|event| matches!(event.kind, EventKind::AgentUsageUpdated { .. }))
        .expect("billing event");
    let completed = events
        .iter()
        .position(|event| matches!(event.kind, EventKind::CompactionCompleted { .. }))
        .expect("checkpoint event");
    assert!(
        billed < completed,
        "usage is persisted before accepting the checkpoint"
    );
    let EventKind::CompactionCompleted {
        before_tokens,
        after_tokens,
        ..
    } = events[completed].kind
    else {
        unreachable!()
    };
    let history = h.store.transcript(agent.id, 0).expect("transcript");
    let history: Vec<_> = history
        .iter()
        .filter(|entry| entry.seq < events[completed].seq)
        .collect();
    let estimate = aether_context::TokenEstimator;
    let expected_before: u32 = history
        .iter()
        .map(|entry| estimate.message(&entry.message))
        .sum();
    let retained: u32 = history
        .iter()
        .filter(|entry| entry.seq > checkpoint.provenance.to_seq)
        .map(|entry| estimate.message(&entry.message))
        .sum();
    let expected_after = estimate.text(&aether_context::render::checkpoint(&checkpoint)) + retained;
    assert_eq!(before_tokens, Some(expected_before));
    assert_eq!(after_tokens, Some(expected_after));
    let report = aether_core::report::build(updated.run, &events, aether_core::time::now());
    let mut reported = report
        .body
        .agents
        .iter()
        .find(|a| a.id == agent.id)
        .unwrap()
        .usage;
    assert!((reported.cost_usd - expected.cost_usd).abs() < 1e-12);
    reported.cost_usd = expected.cost_usd;
    assert_eq!(reported, expected);

    let last = h
        .store
        .snapshot(turns.last().expect("turn").snapshot_id)
        .expect("snap")
        .expect("exists");
    let cp_layer = last
        .layers
        .iter()
        .find(|l| l.kind == aether_core::snapshot::LayerKind::Checkpoint)
        .expect("checkpoint layer");
    assert!(
        cp_layer
            .text
            .as_deref()
            .unwrap_or("")
            .contains("four exchanges so far")
    );
    let all_entries = h.store.transcript(agent.id, 0).expect("t").len();
    assert!(
        last.messages.len() < all_entries,
        "tail {} should be shorter than full transcript {all_entries}",
        last.messages.len()
    );
    assert!(
        !last.messages.iter().any(|m| m.text().contains("message 1")),
        "folded turns must leave the tail"
    );
    // The compactor request carried the transcript and the schema.
    let requests = h.client.requests();
    let compactor_req = requests
        .iter()
        .find(|r| {
            r.system
                .iter()
                .any(|b| b.text.contains("compaction service"))
        })
        .expect("compactor request");
    assert!(compactor_req.messages[0].text().contains("message 1"));
}

/// A compactor configured by alias is sent its canonical id: the API knows
/// `gpt-6-luna`, not Aether's `luna`.
#[tokio::test]
async fn an_aliased_compactor_is_sent_its_canonical_id() {
    let checkpoint = json!({
        "checkpoint": {
            "objective": "answer the operator",
            "state": "one exchange so far",
            "decisions": [],
            "constraints": [],
            "open_loops": []
        },
        "memory_candidates": []
    })
    .to_string();
    let h = harness_full(
        vec![
            ScriptedClient::text("hello"),
            ScriptedClient::text(&checkpoint),
            ScriptedClient::text("ready"),
        ],
        |_| {},
        use_aether_compactor,
        None,
        Some("[model]\ncompactor = \"openai-responses/luna\"\n"),
        None,
        |_| {},
    )
    .await;
    let (agent, _) = h
        .runtime
        .create_agent(
            CreateAgentSpec {
                role: "assistant".into(),
                model: Some(ModelRef::new("anthropic", "claude-sonnet-5-5")),
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
        .expect("create");
    for (i, text) in ["hi", "hello after switching"].into_iter().enumerate() {
        if i == 1 {
            // Leaving the API for an engine owes a handoff, folded by the compactor.
            h.runtime
                .update_agent(
                    agent.id,
                    AgentUpdate {
                        model: Some(ModelRef::new("claude-code", "haiku")),
                        ..Default::default()
                    },
                    Actor::user(),
                )
                .expect("switch");
        }
        h.runtime
            .send_message(agent.id, vec![ContentBlock::text(text)], Actor::user())
            .expect("send");
        h.wait_for("turn", || {
            h.store.agent(agent.id).expect("agent").is_some_and(|a| {
                a.status.kind() == AgentStatusKind::Waiting && a.stats.turns > i as u32
            })
        })
        .await;
    }

    let requests = h.client.requests();
    let compactor_req = requests
        .iter()
        .find(|r| {
            r.system
                .iter()
                .any(|b| b.text.contains("compaction service"))
        })
        .expect("compactor request");
    assert_eq!(
        compactor_req.model,
        ModelRef::new("openai-responses", "gpt-6-luna")
    );
}

#[tokio::test]
async fn manual_compaction_never_requests_a_reply_without_pending_work() {
    for scenario in ["success", "failure", "empty"] {
        let response = if scenario == "failure" {
            Err(aether_core::model::ModelError::new(
                ModelErrorClass::Auth,
                "compactor refused",
            ))
        } else {
            ScriptedClient::text(
                &json!({"checkpoint": {
                "objective": "keep chatting", "state": "remember saffron", "decisions": [],
                "constraints": [], "open_loops": []
            }, "memory_candidates": []})
                .to_string(),
            )
        };
        let h = harness_full(
            vec![response, ScriptedClient::text("unrequested reply")],
            |_| {},
            use_aether_compactor,
            None,
            Some("[model]\ncompactor = \"openai/gpt-6.1-sol\"\n"),
            None,
            |_| {},
        )
        .await;
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
            .unwrap();
        h.wait_until(agent.id, |status| status.kind() == AgentStatusKind::Waiting)
            .await;
        if scenario != "empty" {
            let mut commit = Commit::new();
            for i in 0..8 {
                for (role, text) in [
                    (
                        aether_core::model::MessageRole::User,
                        format!("message {i}"),
                    ),
                    (
                        aether_core::model::MessageRole::Assistant,
                        format!("answer {i}"),
                    ),
                ] {
                    commit.push_event(
                        EventDraft::new(
                            Actor::agent(agent.id),
                            EventKind::AgentMessage {
                                role,
                                content: vec![ContentBlock::text(text)],
                                invocation: None,
                            },
                        )
                        .agent(agent.id)
                        .run(agent.run)
                        .run(agent.run),
                    );
                }
            }
            h.runtime.apply(commit).unwrap();
        }
        let boundary = h.store.last_seq().unwrap();
        h.runtime
            .request_compaction(agent.id, Actor::user())
            .unwrap();
        h.wait_for("manual compaction settled", || {
            let settled = h
                .store
                .events(boundary, 1000, &EventFilter::own_events(agent.id))
                .unwrap()
                .iter()
                .any(|event| {
                    matches!(
                        event.kind,
                        EventKind::CompactionCompleted { .. } | EventKind::CompactionFailed { .. }
                    )
                });
            settled
                && h.store.agent(agent.id).unwrap().unwrap().status.kind()
                    == AgentStatusKind::Waiting
        })
        .await;
        h.runtime.shutdown().await;
        let events = h
            .store
            .events(boundary, 1000, &EventFilter::own_events(agent.id))
            .unwrap();
        let outcome = events
            .iter()
            .find(|event| {
                matches!(
                    event.kind,
                    EventKind::CompactionCompleted { .. } | EventKind::CompactionFailed { .. }
                )
            })
            .unwrap_or_else(|| panic!("{scenario}: {events:?}"));
        let request = events
            .iter()
            .find(|event| matches!(event.kind, EventKind::CompactionRequested { .. }))
            .unwrap_or_else(|| panic!("{scenario}: {events:?}"));
        assert_eq!(
            outcome.causation,
            Some(request.id),
            "{scenario}: the outcome names the request it settles"
        );
        assert!(
            !events
                .iter()
                .any(|event| matches!(event.kind, EventKind::AgentMessage { .. })),
            "{scenario}: compaction must not add a reply or a user wake message"
        );
        assert_eq!(
            h.client.requests().len(),
            usize::from(scenario != "empty"),
            "{scenario}: only the compactor may be invoked"
        );
        assert_eq!(
            h.store.latest_checkpoint(agent.id).unwrap().is_some(),
            scenario == "success"
        );
    }
}

/// A retired compactor must fail with its successor, not yield to the
/// agent's own model.
#[tokio::test]
async fn a_retired_compactor_fails_with_its_successor() {
    let checkpoint = json!({"checkpoint": {
        "objective": "keep chatting", "state": "unused", "decisions": [],
        "constraints": [], "open_loops": []
    }, "memory_candidates": []})
    .to_string();
    let h = harness_full(
        vec![ScriptedClient::text(&checkpoint)],
        |_| {},
        use_aether_compactor,
        None,
        Some("[model]\ncompactor = \"anthropic/claude-haiku-4-5\"\n"),
        None,
        |_| {},
    )
    .await;
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
        .unwrap();
    h.wait_until(agent.id, |status| status.kind() == AgentStatusKind::Waiting)
        .await;
    let mut commit = Commit::new();
    for i in 0..8 {
        for (role, text) in [
            (
                aether_core::model::MessageRole::User,
                format!("message {i}"),
            ),
            (
                aether_core::model::MessageRole::Assistant,
                format!("answer {i}"),
            ),
        ] {
            commit.push_event(
                EventDraft::new(
                    Actor::agent(agent.id),
                    EventKind::AgentMessage {
                        role,
                        content: vec![ContentBlock::text(text)],
                        invocation: None,
                    },
                )
                .agent(agent.id)
                .run(agent.run)
                .run(agent.run),
            );
        }
    }
    h.runtime.apply(commit).unwrap();
    let boundary = h.store.last_seq().unwrap();
    h.runtime
        .request_compaction(agent.id, Actor::user())
        .unwrap();
    let settled = || {
        h.store
            .events(boundary, 1000, &EventFilter::own_events(agent.id))
            .unwrap()
            .into_iter()
            .find_map(|event| match event.kind {
                EventKind::CompactionFailed { error, models } => Some(Ok((error, models))),
                EventKind::CompactionCompleted { .. } => Some(Err(())),
                _ => None,
            })
    };
    h.wait_for("compaction settled", || settled().is_some())
        .await;
    h.runtime.shutdown().await;
    let compactor = ModelRef::new("anthropic", "claude-haiku-4-5");
    assert_eq!(
        settled()
            .unwrap()
            .expect("compaction must fail, not fall through"),
        (
            "compactor model unavailable: anthropic/claude-haiku-4-5 was retired; \
             use anthropic/claude-haiku-5-5"
                .to_string(),
            vec![compactor]
        )
    );
    assert!(h.client.requests().is_empty(), "no model may be invoked");
    assert!(h.store.latest_checkpoint(agent.id).unwrap().is_none());
}

#[tokio::test]
async fn manual_compaction_still_answers_other_pending_inbox_work() {
    let h = harness(vec![ScriptedClient::text("requested reply")]).await;
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
        .unwrap();
    h.wait_until(agent.id, |status| status.kind() == AgentStatusKind::Waiting)
        .await;
    h.runtime
        .pause_agent(agent.id, false, Actor::user())
        .unwrap();
    h.runtime
        .request_compaction(agent.id, Actor::user())
        .unwrap();
    h.runtime
        .send_message(
            agent.id,
            vec![ContentBlock::text("please reply")],
            Actor::user(),
        )
        .unwrap();
    h.runtime
        .resume_agent(agent.id, false, Actor::user())
        .unwrap();
    h.wait_until(agent.id, |status| status.kind() == AgentStatusKind::Waiting)
        .await;
    h.runtime.shutdown().await;
    assert_eq!(h.client.requests().len(), 1);
    assert!(
        h.store
            .transcript(agent.id, 0)
            .unwrap()
            .iter()
            .any(|entry| entry.message.text() == "requested reply")
    );
}

#[tokio::test]
async fn manual_compaction_preserves_an_unsatisfied_wait() {
    use aether_core::agent::WaitCondition;
    use aether_core::model::MessageRole;
    let h = harness(vec![ScriptedClient::text("unrequested reply")]).await;
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
        .unwrap();
    h.wait_until(agent.id, |status| status.kind() == AgentStatusKind::Waiting)
        .await;
    let condition = WaitCondition::Deadline {
        at: now() + chrono::Duration::hours(1),
    };
    let mut waiting = h.store.agent(agent.id).unwrap().unwrap();
    waiting.status = AgentStatus::Waiting {
        condition: condition.clone(),
    };
    h.runtime
        .apply(
            Commit::new()
                .event(
                    EventDraft::new(
                        Actor::agent(agent.id),
                        EventKind::AgentMessage {
                            role: MessageRole::User,
                            content: vec![ContentBlock::tool_result_text(
                                "wait",
                                "Waiting until later",
                                false,
                            )],
                            invocation: None,
                        },
                    )
                    .agent(agent.id)
                    .run(agent.run),
                )
                .mutate(Mutation::UpsertAgent(waiting)),
        )
        .unwrap();
    h.runtime
        .request_compaction(agent.id, Actor::user())
        .unwrap();
    h.wait_until(agent.id, |status| status.kind() == AgentStatusKind::Waiting)
        .await;
    h.runtime.shutdown().await;
    assert!(h.client.requests().is_empty());
    assert!(
        types(h.store.as_ref(), agent.id)
            .iter()
            .any(|kind| kind == "compaction.failed"),
        "the manual request must settle even while the original wait is unsatisfied"
    );
    assert_eq!(
        h.store.agent(agent.id).unwrap().unwrap().status,
        AgentStatus::Waiting { condition }
    );
}

/// A message the wait defers stays deferred: the request alone woke the agent.
#[tokio::test]
async fn manual_compaction_leaves_items_the_wait_defers_unread() {
    use aether_core::agent::WaitCondition;
    let h = harness(vec![ScriptedClient::text("unrequested reply")]).await;
    let (agent, _) = h.runtime.create_agent(idle_spec(), Actor::user()).unwrap();
    h.wait_until(agent.id, |status| status.kind() == AgentStatusKind::Waiting)
        .await;
    let condition = WaitCondition::Deadline {
        at: now() + chrono::Duration::hours(1),
    };
    let mut waiting = h.store.agent(agent.id).unwrap().unwrap();
    waiting.status = AgentStatus::Waiting {
        condition: condition.clone(),
    };
    h.runtime
        .apply(Commit::new().mutate(Mutation::UpsertAgent(waiting)))
        .unwrap();
    // Not this agent's parent, so the wait holds it until the deadline.
    h.runtime
        .send_message(
            agent.id,
            vec![ContentBlock::text("progress note")],
            Actor::agent(AgentId::new()),
        )
        .unwrap();
    h.runtime
        .request_compaction(agent.id, Actor::user())
        .unwrap();
    h.wait_until(agent.id, |status| status.kind() == AgentStatusKind::Waiting)
        .await;
    h.runtime.shutdown().await;
    assert!(
        h.client.requests().is_empty(),
        "the deferred message must not get an early reply"
    );
    let after = h.store.agent(agent.id).unwrap().unwrap();
    assert_eq!(after.status, AgentStatus::Waiting { condition });
    assert!(
        h.store
            .inbox(agent.id, after.inbox_cursor)
            .unwrap()
            .iter()
            .any(|item| matches!(item.kind, aether_core::inbox::InboxKind::Message { .. })),
        "the deferred message must stay unread for the restored wait"
    );
}

/// An operator stop ends the request's turn; it is not retried at once.
#[tokio::test]
async fn interrupting_a_manual_compaction_does_not_rerun_it() {
    let checkpoint = json!({"checkpoint": {
        "objective": "keep chatting", "state": "six exchanges", "decisions": [],
        "constraints": [], "open_loops": []
    }, "memory_candidates": []})
    .to_string();
    let h = harness(vec![
        ScriptedClient::text(&checkpoint),
        ScriptedClient::text(&checkpoint),
    ])
    .await;
    let (agent, _) = h.runtime.create_agent(idle_spec(), Actor::user()).unwrap();
    h.wait_until(agent.id, |status| status.kind() == AgentStatusKind::Waiting)
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
    h.runtime.apply(history).unwrap();
    let boundary = h.store.last_seq().unwrap();
    let _gate = h.client.hold_response_at(1);
    h.runtime
        .request_compaction(agent.id, Actor::user())
        .unwrap();
    h.wait_for("compactor entered the gate", || {
        h.client.requests().len() == 1
    })
    .await;
    assert!(h.runtime.interrupt_agent(agent.id, Actor::user()).unwrap());
    h.wait_for("the interrupted turn settled with its request read", || {
        let after = h.store.agent(agent.id).unwrap().unwrap();
        after.status.kind() == AgentStatusKind::Waiting
            && h.store.inbox_last_seq(agent.id).unwrap() <= after.inbox_cursor
    })
    .await;
    h.runtime.shutdown().await;
    let started = h
        .store
        .events(boundary, 1000, &EventFilter::own_events(agent.id))
        .unwrap()
        .iter()
        .filter(|event| matches!(event.kind, EventKind::AgentStarted { .. }))
        .count();
    assert_eq!(started, 1, "the stop must not start another turn");
    assert_eq!(h.client.requests().len(), 1);
    assert!(
        types(h.store.as_ref(), agent.id)
            .iter()
            .any(|kind| kind == "agent.interrupted")
    );
}
