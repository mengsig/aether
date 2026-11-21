//! A turn that ends on a failed tool call says so with one event, whichever
//! engine ran it, so a host can wake the agent rather than lose the failure.
use super::*;
use std::sync::atomic::{AtomicUsize, Ordering};

const FAILED_TOOL: &str = "mcp__aether__mcp__archon__model_policy";

/// Its first turn ends right after a failed call, as the incident's did; every
/// later turn retries the call, which works, before it answers. As Codex, the
/// failed call is what Codex's own output parser makes of a real line.
struct FailingCallEngine {
    kind: EngineKind,
    runs: AtomicUsize,
}

/// A failed call as the engine reports it: Codex marks an MCP tool's error
/// answer only in its result's `isError`, leaving the item `completed`.
fn failed_call(kind: EngineKind) -> Vec<EngineEvent> {
    if kind != EngineKind::CODEX {
        return vec![
            tool_use("call-1", FAILED_TOOL),
            tool_result("call-1", "Connection closed", true),
        ];
    }
    let line = serde_json::json!({"type": "item.completed", "item": {
        "id": "call-1", "type": "mcp_tool_call", "server": "aether",
        "tool": "mcp__archon__model_policy", "arguments": {"repo": "example"},
        "result": {"content": [{"type": "text", "text": "Connection closed"}], "isError": true},
        "status": "completed"}});
    let events = aether_engine::codex::parse_line(
        &line.to_string(),
        &mut aether_engine::codex::ParseState::default(),
    );
    assert!(
        matches!(
            events.last(),
            Some(EngineEvent::ToolResult { is_error: true, .. })
        ),
        "Codex's parser keeps the error: {events:?}"
    );
    events
}

fn tool_use(id: &str, name: &str) -> EngineEvent {
    EngineEvent::Assistant {
        content: vec![ContentBlock::ToolUse {
            id: id.into(),
            name: name.into(),
            input: serde_json::json!({"repo": "example"}),
        }],
        usage: None,
        model: None,
    }
}

fn tool_result(id: &str, content: &str, is_error: bool) -> EngineEvent {
    EngineEvent::ToolResult {
        tool_use_id: id.into(),
        content: content.into(),
        is_error,
    }
}

#[async_trait::async_trait]
impl Engine for FailingCallEngine {
    fn kind(&self) -> EngineKind {
        self.kind
    }

    fn adapter(&self) -> String {
        "failing-call/1".into()
    }

    async fn run(
        &self,
        _turn: EngineTurn,
        _cancel: CancellationToken,
    ) -> Result<EngineRun, EngineError> {
        let first = self.runs.fetch_add(1, Ordering::SeqCst) == 0;
        let (events, rx) = mpsc::channel(16);
        let mut script = failed_call(self.kind);
        if !first {
            script.push(tool_use("call-2", FAILED_TOOL));
            script.push(tool_result("call-2", "Allowed models: any", false));
        }
        script.push(EngineEvent::Assistant {
            content: vec![ContentBlock::text("done")],
            usage: None,
            model: None,
        });
        script.push(EngineEvent::Done(EngineOutcome {
            closing_accounting: true,
            text: Some("done".into()),
            ..Default::default()
        }));
        tokio::spawn(async move {
            for event in script {
                if events.send(event).await.is_err() {
                    return;
                }
            }
        });
        Ok(EngineRun {
            events: rx,
            input: None,
            decisions: None,
            stop: None,
        })
    }
}

/// Every `turn_ended_after_tool_error` payload recorded for `id`, in order.
fn unanswered_errors(store: &dyn Store, id: AgentId) -> Vec<serde_json::Value> {
    store
        .events(0, 10_000, &EventFilter::for_agent(id))
        .expect("events")
        .into_iter()
        .filter_map(|event| match event.kind {
            EventKind::Custom { name, payload }
                if name == aether_core::event::TURN_ENDED_AFTER_TOOL_ERROR =>
            {
                Some(payload)
            }
            _ => None,
        })
        .collect()
}

/// Asserts each `turn_ended_after_tool_error` comes right after its turn's
/// `agent.turn_completed`, with nothing between them; returns how many.
fn assert_adjacent_to_turn_end(store: &dyn Store, id: AgentId) -> usize {
    let events = store
        .events(0, 10_000, &EventFilter::for_agent(id))
        .expect("events");
    let mut reported = 0;
    for (at, event) in events.iter().enumerate() {
        let EventKind::Custom { name, payload } = &event.kind else {
            continue;
        };
        if name != aether_core::event::TURN_ENDED_AFTER_TOOL_ERROR {
            continue;
        }
        let previous = at.checked_sub(1).map(|before| &events[before].kind);
        assert!(
            matches!(previous, Some(EventKind::AgentTurnCompleted { turn, .. }) if payload["turn"] == *turn),
            "right after its turn_completed, got {previous:?}"
        );
        reported += 1;
    }
    reported
}

async fn run_turn(h: &Harness, id: AgentId, turns: u32) {
    h.runtime
        .send_message(
            id,
            vec![ContentBlock::text("check the policy")],
            Actor::user(),
        )
        .expect("send");
    wait_for(h.store.as_ref(), id, |a| {
        a.status.kind() == AgentStatusKind::Waiting && a.stats.turns >= turns
    })
    .await;
}

#[tokio::test]
async fn a_turn_ending_on_a_failed_call_is_reported_once_for_every_engine() {
    for (kind, model) in [
        (
            EngineKind::CLAUDE_CODE,
            ModelRef::new("claude-code", "haiku"),
        ),
        (EngineKind::CODEX, ModelRef::new("codex", "gpt-6-luna")),
    ] {
        let engine = Arc::new(FailingCallEngine {
            kind,
            runs: AtomicUsize::new(0),
        });
        let h = harness_with_engine(engine, Arc::default(), Arc::default()).await;
        let (agent, _) = h
            .runtime
            .create_agent(
                CreateAgentSpec {
                    role: "orchestrator".into(),
                    start: true,
                    model: Some(model.clone()),
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
        let id = agent.id;
        wait_for(h.store.as_ref(), id, |a| {
            a.status.kind() == AgentStatusKind::Waiting
        })
        .await;
        assert!(
            unanswered_errors(h.store.as_ref(), id).is_empty(),
            "{model}: no turn yet"
        );

        run_turn(&h, id, 1).await;
        let reported = unanswered_errors(h.store.as_ref(), id);
        assert_eq!(reported.len(), 1, "{model}: exactly once: {reported:?}");
        let payload = &reported[0];
        assert_eq!(payload["agent_id"], id.to_string(), "{model}");
        assert_eq!(payload["tool"], FAILED_TOOL, "{model}");
        let error = payload["error"].as_str().expect("the error text");
        assert!(error.contains("Connection closed"), "{model}: {error}");
        assert!(payload["at"].is_string(), "{model}: {payload}");
        let ended_turn = h
            .store
            .events(0, 10_000, &EventFilter::for_agent(id))
            .expect("events")
            .into_iter()
            .find_map(|event| match event.kind {
                EventKind::AgentTurnCompleted { turn, .. } => Some(turn),
                _ => None,
            })
            .expect("the turn ended");
        assert_eq!(payload["turn"], ended_turn, "{model}: the turn that ended");
        assert_eq!(
            assert_adjacent_to_turn_end(h.store.as_ref(), id),
            1,
            "{model}"
        );

        // The next turn retries the call and it works: nothing more to report.
        run_turn(&h, id, 2).await;
        assert_eq!(
            unanswered_errors(h.store.as_ref(), id).len(),
            1,
            "{model}: a failure a later call answered is not reported"
        );
    }
}

#[tokio::test]
async fn a_final_turn_ending_on_a_failed_call_reports_it_right_after_the_turn() {
    let engine = Arc::new(FailingCallEngine {
        kind: EngineKind::CLAUDE_CODE,
        runs: AtomicUsize::new(0),
    });
    let h = harness_with_engine(engine, Arc::default(), Arc::default()).await;
    let (agent, _) = h
        .runtime
        .create_agent(
            CreateAgentSpec {
                role: "worker".into(),
                start: true,
                model: Some(ModelRef::new("claude-code", "haiku")),
                objective: Some("check the policy".into()),
                config: Some(AgentConfig {
                    approval: ApprovalPolicy::none(),
                    ..Default::default()
                }),
                ..Default::default()
            },
            Actor::user(),
        )
        .expect("create");
    let ended = wait_for(h.store.as_ref(), agent.id, |a| a.status.is_terminal()).await;
    assert!(ended.status.is_terminal(), "{:?}", ended.status);
    assert_eq!(
        assert_adjacent_to_turn_end(h.store.as_ref(), agent.id),
        1,
        "a terminal end reports it once, before the agent's own end events"
    );
}
