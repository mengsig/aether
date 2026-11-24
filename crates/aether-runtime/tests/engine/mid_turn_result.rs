//! The 2026-11-21 incident: Claude Code sent a `result` mid-turn and streamed
//! a tool call 2 s later. That call must be served, never lost to a closed stdin.
use super::*;
use std::sync::atomic::{AtomicBool, Ordering};

const LATE_TOOL: &str = "mcp__aether__mcp__archon__model_policy";

/// Replays the incident: a call, a `result`, then 2 s later another call. Like
/// Claude Code, a call made after its stdin closed fails with "Connection closed".
struct MidTurnResultEngine {
    stdin_closed_before_late_call: Arc<AtomicBool>,
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

fn result(text: &str) -> EngineEvent {
    EngineEvent::Done(EngineOutcome {
        closing_accounting: true,
        text: Some(text.into()),
        ..Default::default()
    })
}

#[async_trait::async_trait]
impl Engine for MidTurnResultEngine {
    fn kind(&self) -> EngineKind {
        EngineKind::CLAUDE_CODE
    }

    fn adapter(&self) -> String {
        "mid-turn-result/1".into()
    }

    async fn run(
        &self,
        _turn: EngineTurn,
        _cancel: CancellationToken,
    ) -> Result<EngineRun, EngineError> {
        let (events, rx) = mpsc::channel(16);
        let (input, mut stdin) = mpsc::channel::<aether_engine::EngineInput>(16);
        let closed_early = self.stdin_closed_before_late_call.clone();
        tokio::spawn(async move {
            let early = [
                tool_use("call-1", "mcp__aether__mcp__archon__autonomy_policy"),
                tool_result("call-1", "Autonomy: defaults", false),
                result("mid-turn"),
            ];
            for event in early {
                if events.send(event).await.is_err() {
                    return;
                }
            }
            tokio::time::sleep(Duration::from_secs(2)).await;
            let closed = stdin.is_closed();
            closed_early.store(closed, Ordering::SeqCst);
            let late = match closed {
                true => tool_result("call-2", "Connection closed", true),
                false => tool_result("call-2", "Allowed models: any", false),
            };
            for event in [tool_use("call-2", LATE_TOOL), late, result("done")] {
                if events.send(event).await.is_err() {
                    return;
                }
            }
            // The process exits once its stdin closes.
            while stdin.recv().await.is_some() {}
        });
        Ok(EngineRun {
            events: rx,
            input: Some(input),
            decisions: None,
            stop: None,
        })
    }
}

#[tokio::test]
async fn a_tool_call_streamed_after_a_mid_turn_result_is_served_not_lost() {
    let closed_early = Arc::new(AtomicBool::new(false));
    let engine = Arc::new(MidTurnResultEngine {
        stdin_closed_before_late_call: closed_early.clone(),
    });
    let h = harness_with_engine(engine, Arc::default(), Arc::default()).await;
    let (agent, _) = h
        .runtime
        .create_agent(
            CreateAgentSpec {
                role: "orchestrator".into(),
                start: true,
                model: Some(ModelRef::new("claude-code", "haiku")),
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
    h.runtime
        .send_message(
            id,
            vec![ContentBlock::text("check the policy")],
            Actor::user(),
        )
        .expect("send");
    wait_for(h.store.as_ref(), id, |a| {
        a.status.kind() == AgentStatusKind::Waiting && a.stats.turns >= 1
    })
    .await;
    assert!(
        !closed_early.load(Ordering::SeqCst),
        "stdin stayed open for the call streamed after the result"
    );

    let events = h
        .store
        .events(0, 10_000, &EventFilter::for_agent(id))
        .expect("events");
    let late_call = events
        .iter()
        .find_map(|event| match &event.kind {
            EventKind::ToolRequested { call, .. } if call.name == LATE_TOOL => Some(call.id),
            _ => None,
        })
        .expect("the late call is on the record");
    let position =
        |wanted: &dyn Fn(&EventKind) -> bool| events.iter().position(|e| wanted(&e.kind));
    let served = position(&|kind| {
        matches!(kind, EventKind::ToolSucceeded { call, outcome }
            if *call == late_call && !outcome.is_error && outcome.content.contains("Allowed models"))
    })
    .expect("the late call completed");
    let ended = position(&|kind| matches!(kind, EventKind::AgentTurnCompleted { .. }))
        .expect("the turn ended");
    assert!(served < ended, "the turn ended only after the late call");
    let turns_ended = events
        .iter()
        .filter(|e| matches!(e.kind, EventKind::AgentTurnCompleted { .. }))
        .count();
    assert_eq!(turns_ended, 1, "one turn, not one per result");
    let reported = events.iter().any(|e| {
        matches!(&e.kind, EventKind::Custom { name, .. }
            if name == aether_core::event::TURN_ENDED_AFTER_TOOL_ERROR)
    });
    assert!(!reported, "nothing failed, so nothing is reported");
}
