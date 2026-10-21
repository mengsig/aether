use super::*;
use aether_core::model::{Message, StopReason, Usage};

fn summary_response() -> aether_model::testing::ScriptedResponse {
    Ok(vec![
        StreamEvent::BlockStart {
            index: 0,
            block: BlockStart::Compaction {
                content: "Remember saffron".into(),
                signature: Some("opaque-signature".into()),
            },
        },
        StreamEvent::BlockStop { index: 0 },
        StreamEvent::Usage {
            usage: Usage {
                input_tokens: 200,
                output_tokens: 20,
                ..Default::default()
            },
        },
        StreamEvent::Stop {
            reason: StopReason::Other {
                value: "compaction".into(),
            },
        },
        StreamEvent::MessageStop,
    ])
}

fn seed_history(h: &Harness, agent: &Agent) {
    let mut commit = Commit::new();
    for i in 0..5 {
        for message in [
            Message::user_text(format!("remember saffron {i}")),
            Message::assistant_text(format!("remembered {i}")),
        ] {
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
                .run(agent.run),
            );
        }
    }
    h.runtime.apply(commit).unwrap();
    assert_eq!(h.store.transcript(agent.id, 0).unwrap().len(), 10);
    assert!(h.store.latest_checkpoint(agent.id).unwrap().is_none());
}

async fn compact(h: &Harness, agent: &Agent, calls: usize) {
    h.runtime
        .request_compaction(agent.id, Actor::user())
        .unwrap();
    h.wait_for("compaction finished", || {
        h.client.requests().len() >= calls
            && h.store.agent(agent.id).unwrap().unwrap().status.kind() == AgentStatusKind::Waiting
    })
    .await;
    assert!(h.client.requests().len() >= calls);
    assert!(types(h.store.as_ref(), agent.id).contains(&"compaction.requested".into()));
}

fn assert_native_summary(h: &Harness, agent: &Agent) -> Usage {
    let requests = h.client.requests();
    assert_eq!(requests.len(), 1);
    assert_eq!(
        requests[0].extensions["anthropic.compaction"],
        json!({"type": "summarize"})
    );
    let transcript = h.store.transcript(agent.id, 0).unwrap();
    assert!(transcript.last().unwrap().message.content[0].is_signed_compaction());
    let updated = h.store.agent(agent.id).unwrap().unwrap();
    assert_eq!(updated.stats.context_tokens, Some(20));
    assert_eq!(updated.stats.usage.input_tokens, 200);
    assert_eq!(updated.stats.usage.output_tokens, 20);
    assert!(updated.stats.usage.cost_usd > 0.0);
    let events = h
        .store
        .events(0, 1000, &EventFilter::own_events(agent.id))
        .unwrap();
    let native = events.iter().find(|e| matches!(&e.kind,
        EventKind::CompactionNative { provider, automatic: false, after_tokens: Some(20), .. } if provider == "anthropic"))
        .expect("compaction.native");
    let request = events
        .iter()
        .find(|e| matches!(e.kind, EventKind::CompactionRequested { .. }));
    assert_eq!(
        native.causation,
        request.map(|e| e.id),
        "the fold names its request"
    );
    updated.stats.usage
}

fn assert_replay_and_cost(h: &Harness, agent: &Agent, summary_usage: Usage) {
    let requests = h.client.requests();
    assert_eq!(requests.len(), 2);
    assert!(requests[1].messages[0].content[0].is_signed_compaction());
    let caps = h
        .runtime
        .models()
        .resolve(&agent.model)
        .unwrap()
        .model
        .capabilities;
    let wire = aether_model::adapters::anthropic::request::build(&requests[1], &caps).unwrap();
    assert_eq!(
        wire.body["messages"][0]["content"][0],
        json!({"type": "compaction", "content": "Remember saffron", "signature": "opaque-signature"})
    );
    assert!(wire.body.get("context_management").is_none());
    assert_eq!(wire.betas, ["compact-2026-09-04"]);
    let invocations = h.store.invocations_for_agent(agent.id).unwrap();
    let expected_cost =
        summary_usage.cost_usd + invocations.iter().map(|i| i.usage.cost_usd).sum::<f64>();
    assert!(
        (h.store
            .agent(agent.id)
            .unwrap()
            .unwrap()
            .stats
            .usage
            .cost_usd
            - expected_cost)
            .abs()
            < 1e-12
    );
}

#[tokio::test]
async fn explicit_anthropic_compaction_keeps_the_signed_block_and_bills_usage() {
    let h = harness(vec![
        summary_response(),
        ScriptedClient::text("after compaction"),
    ])
    .await;
    let (agent, _) = h.runtime.create_agent(idle_spec(), Actor::user()).unwrap();
    h.wait_until(agent.id, |s| s.kind() == AgentStatusKind::Waiting)
        .await;
    seed_history(&h, &agent);
    compact(&h, &agent, 1).await;
    let summary_usage = assert_native_summary(&h, &agent);
    h.runtime
        .send_message(
            agent.id,
            vec![ContentBlock::text("continue")],
            Actor::user(),
        )
        .unwrap();
    h.wait_for("next turn", || h.client.remaining() == 0).await;
    h.wait_until(agent.id, |s| s.kind() == AgentStatusKind::Waiting)
        .await;
    assert_replay_and_cost(&h, &agent, summary_usage);
}

#[tokio::test]
async fn server_refusal_falls_back_to_aethers_compactor() {
    let refusals = vec![
        Ok(vec![
            StreamEvent::Stop {
                reason: StopReason::Refusal { category: None },
            },
            StreamEvent::MessageStop,
        ]),
        Ok(vec![
            StreamEvent::Stop {
                reason: StopReason::EndTurn,
            },
            StreamEvent::MessageStop,
        ]),
        Err(aether_core::model::ModelError::new(
            ModelErrorClass::InvalidRequest,
            "compaction_signature_invalid",
        )),
        Err(aether_core::model::ModelError::new(
            ModelErrorClass::InvalidRequest,
            "compaction_nothing_to_summarize",
        )),
    ];
    for refusal in refusals {
        let checkpoint = json!({"checkpoint": {"objective": "remember saffron", "state": "conversation summarized"}}).to_string();
        let h = harness(vec![refusal, ScriptedClient::text(&checkpoint)]).await;
        let (agent, _) = h.runtime.create_agent(idle_spec(), Actor::user()).unwrap();
        h.wait_until(agent.id, |s| s.kind() == AgentStatusKind::Waiting)
            .await;
        seed_history(&h, &agent);
        compact(&h, &agent, 2).await;
        let requests = h.client.requests();
        assert_eq!(requests.len(), 2);
        assert!(requests[0].extensions.contains_key("anthropic.compaction"));
        assert!(
            requests[1]
                .system
                .iter()
                .any(|s| s.text.contains("compaction service"))
        );
        assert!(h.store.latest_checkpoint(agent.id).unwrap().is_some());
        assert!(types(h.store.as_ref(), agent.id).contains(&"compaction.completed".into()));
        assert!(!types(h.store.as_ref(), agent.id).contains(&"compaction.native".into()));
    }
}

#[tokio::test]
async fn server_unavailable_is_retried_once() {
    let unavailable =
        aether_core::model::ModelError::new(ModelErrorClass::Overloaded, "compaction unavailable")
            .with_status(529)
            .with_provider_code("compaction_unavailable");
    let h = harness(vec![Err(unavailable), summary_response()]).await;
    let (agent, _) = h.runtime.create_agent(idle_spec(), Actor::user()).unwrap();
    h.wait_until(agent.id, |s| s.kind() == AgentStatusKind::Waiting)
        .await;
    seed_history(&h, &agent);
    compact(&h, &agent, 2).await;
    assert_eq!(h.client.requests().len(), 2);
    assert!(
        h.client
            .requests()
            .iter()
            .all(|r| r.extensions.contains_key("anthropic.compaction"))
    );
    assert!(types(h.store.as_ref(), agent.id).contains(&"compaction.native".into()));
}

#[tokio::test]
async fn cancelling_server_compaction_never_calls_aethers_compactor() {
    let h = harness(vec![
        summary_response(),
        ScriptedClient::text("must remain unused"),
    ])
    .await;
    let (agent, _) = h.runtime.create_agent(idle_spec(), Actor::user()).unwrap();
    h.wait_until(agent.id, |s| s.kind() == AgentStatusKind::Waiting)
        .await;
    seed_history(&h, &agent);
    let _gate = h.client.hold_response_at(1);
    h.runtime
        .request_compaction(agent.id, Actor::user())
        .unwrap();
    h.wait_for("server summarize started", || {
        h.client.requests().len() == 1
    })
    .await;
    h.runtime
        .cancel_agent(agent.id, Some("stop compaction".into()), Actor::user())
        .unwrap();
    h.wait_until(agent.id, |s| s.kind() == AgentStatusKind::Cancelled)
        .await;
    h.runtime.shutdown().await;
    assert_eq!(h.client.requests().len(), 1);
    assert!(h.store.latest_checkpoint(agent.id).unwrap().is_none());
    let events = types(h.store.as_ref(), agent.id);
    assert!(!events.contains(&"compaction.native".into()));
    assert!(!events.contains(&"compaction.failed".into()));
}

#[tokio::test]
async fn signed_history_routes_automatic_requests_through_server_compaction() {
    let h = harness(vec![
        summary_response(),
        summary_response(),
        ScriptedClient::text("continued"),
    ])
    .await;
    let (agent, _) = h.runtime.create_agent(idle_spec(), Actor::user()).unwrap();
    h.wait_until(agent.id, |s| s.kind() == AgentStatusKind::Waiting)
        .await;
    seed_history(&h, &agent);
    compact(&h, &agent, 1).await;
    h.runtime
        .apply(
            Commit::new().event(
                EventDraft::new(
                    Actor::System,
                    EventKind::CompactionRequested {
                        trigger: CompactionTrigger::TokenThreshold {
                            tokens: 300_000,
                            threshold: 256_000,
                        },
                    },
                )
                .agent(agent.id)
                .run(agent.run),
            ),
        )
        .unwrap();
    h.runtime
        .send_message(
            agent.id,
            vec![ContentBlock::text("continue")],
            Actor::user(),
        )
        .unwrap();
    h.wait_for("automatic compaction and reply", || {
        h.client.remaining() == 0
    })
    .await;
    h.wait_until(agent.id, |s| s.kind() == AgentStatusKind::Waiting)
        .await;
    let requests = h.client.requests();
    assert_eq!(requests.len(), 3);
    assert!(requests[1].extensions.contains_key("anthropic.compaction"));
    assert!(requests[1].messages[0].content[0].is_signed_compaction());
    assert!(
        h.store
            .events(0, 1000, &EventFilter::own_events(agent.id))
            .unwrap()
            .iter()
            .any(|e| matches!(
                e.kind,
                EventKind::CompactionNative {
                    automatic: true,
                    ..
                }
            ))
    );
}
