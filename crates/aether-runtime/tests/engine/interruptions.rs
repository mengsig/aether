//! Bounded diagnostics for interrupted engines.
use super::*;
/// Ends the turn with `interrupted: true` and whatever error text the test
/// hands it - an adapter reporting it stopped itself for a harness boundary,
/// unrelated to a forced wind-down (`Park`).
struct InterruptingEngine {
    error: String,
}

#[async_trait::async_trait]
impl Engine for InterruptingEngine {
    fn kind(&self) -> EngineKind {
        EngineKind::CLAUDE_CODE
    }
    fn adapter(&self) -> String {
        "interrupting/1".into()
    }
    async fn run(
        &self,
        _turn: EngineTurn,
        _cancel: CancellationToken,
    ) -> Result<EngineRun, EngineError> {
        let (tx, rx) = mpsc::channel(1);
        tx.send(EngineEvent::Done(EngineOutcome {
            interrupted: true,
            error: Some(self.error.clone()),
            ..Default::default()
        }))
        .await
        .expect("fixture channel capacity");
        Ok(EngineRun {
            events: rx,
            input: None,
            decisions: None,
            stop: None,
        })
    }
}

/// `engine.interrupted`'s `reason` is an adapter error string
/// with no length contract of its own - it must reach the durable event
/// clamped, the same as its sibling `engine.parked`, not verbatim.
#[tokio::test]
async fn engine_interrupted_clamps_an_over_long_adapter_error() {
    // Longer than turn_engine's private MAX_ENGINE_ERROR (1024 chars); this
    // test pins the clamp at the emit site, not the pre-existing helper.
    let long_error = "x".repeat(1024 + 500);
    let h = harness_with_engine(
        Arc::new(InterruptingEngine {
            error: long_error.clone(),
        }),
        Arc::new(Mutex::new(Vec::new())),
        Arc::new(Mutex::new(Vec::new())),
    )
    .await;
    let id = chat_agent(&h);
    h.runtime
        .send_message(id, vec![ContentBlock::text("go")], Actor::user())
        .expect("send");
    let agent = wait_for(h.store.as_ref(), id, |a| {
        a.status.kind() == AgentStatusKind::Waiting
    })
    .await;
    assert!(
        !agent.status.is_terminal(),
        "an interruption is not a failure"
    );

    let events = h
        .store
        .events(0, 10_000, &EventFilter::for_agent(id))
        .expect("events");
    let reason = events
        .iter()
        .find_map(|e| match &e.kind {
            EventKind::Custom { name, payload } if name == "engine.interrupted" => Some(
                payload["reason"]
                    .as_str()
                    .expect("reason is a string")
                    .to_owned(),
            ),
            _ => None,
        })
        .expect("the interruption left its trace");
    assert_eq!(
        reason.chars().count(),
        1024,
        "an over-long adapter error reached the event unclamped: {} chars",
        reason.chars().count()
    );
    assert!(
        long_error.starts_with(&reason),
        "the clamp must keep the error's own text, not replace it"
    );
    h.runtime.shutdown().await;
}

const SYNTHETIC_DENIAL: &str =
    "The user doesn't want to proceed with this tool use. The tool use was rejected.";

/// Emits Claude's killed-tool result, or Codex's interrupted turn without a result.
struct InFlightEngine {
    prompts: Arc<Mutex<Vec<String>>>,
    result: Option<(&'static str, bool)>,
    wait_for_stop: bool,
    tool: &'static str,
}

#[async_trait::async_trait]
impl Engine for InFlightEngine {
    fn kind(&self) -> EngineKind {
        EngineKind::CLAUDE_CODE
    }
    fn adapter(&self) -> String {
        "in-flight/1".into()
    }
    async fn run(
        &self,
        turn: EngineTurn,
        cancel: CancellationToken,
    ) -> Result<EngineRun, EngineError> {
        let first = {
            let mut prompts = self.prompts.lock().unwrap();
            prompts.push(turn.prompt);
            prompts.len() == 1
        };
        let (tx, rx) = mpsc::channel(8);
        let (result, wait_for_stop, tool) = (self.result, self.wait_for_stop, self.tool);
        tokio::spawn(async move {
            tx.send(EngineEvent::Session {
                id: "in-flight-session".into(),
                model: None,
                tools: vec![],
            })
            .await
            .unwrap();
            if first {
                tx.send(EngineEvent::Assistant {
                    content: vec![ContentBlock::ToolUse {
                        id: "running-call".into(),
                        name: tool.into(),
                        input: serde_json::json!({"command":"make ci"}),
                    }],
                    usage: None,
                    model: None,
                })
                .await
                .unwrap();
                if wait_for_stop {
                    cancel.cancelled().await;
                }
                if let Some((content, is_error)) = result {
                    tx.send(EngineEvent::ToolResult {
                        tool_use_id: "running-call".into(),
                        content: content.into(),
                        is_error,
                    })
                    .await
                    .unwrap();
                    tx.send(EngineEvent::Assistant {
                        content: vec![ContentBlock::text("I will avoid running that command.")],
                        usage: None,
                        model: None,
                    })
                    .await
                    .unwrap();
                }
            } else {
                tx.send(EngineEvent::Assistant {
                    content: vec![ContentBlock::text("resumed")],
                    usage: None,
                    model: None,
                })
                .await
                .unwrap();
            }
            let _ = tx
                .send(EngineEvent::Done(EngineOutcome {
                    interrupted: first && wait_for_stop,
                    text: (!first || !wait_for_stop).then(|| "done".into()),
                    ..Default::default()
                }))
                .await;
        });
        Ok(EngineRun {
            events: rx,
            input: None,
            decisions: None,
            stop: None,
        })
    }
}

async fn in_flight(denial: bool, wait_for_stop: bool) -> Harness {
    in_flight_tool(denial, wait_for_stop, "Bash").await
}

async fn in_flight_tool(denial: bool, wait_for_stop: bool, tool: &'static str) -> Harness {
    in_flight_result(
        denial.then_some((SYNTHETIC_DENIAL, true)),
        wait_for_stop,
        tool,
    )
    .await
}

async fn in_flight_result(
    result: Option<(&'static str, bool)>,
    wait_for_stop: bool,
    tool: &'static str,
) -> Harness {
    let prompts = Arc::new(Mutex::new(Vec::new()));
    let h = harness_with_engine(
        Arc::new(InFlightEngine {
            prompts: prompts.clone(),
            result,
            wait_for_stop,
            tool,
        }),
        prompts,
        Arc::default(),
    )
    .await;
    let id = chat_agent(&h);
    h.runtime
        .send_message(id, vec![ContentBlock::text("go")], Actor::user())
        .unwrap();
    tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            if h.store
                .events(0, 1000, &EventFilter::own_events(id))
                .unwrap()
                .iter()
                .any(|e| matches!(e.kind, EventKind::ToolStarted { .. }))
            {
                break;
            }
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    })
    .await
    .expect("tool starts in seconds");
    h
}

#[tokio::test]
async fn interrupt_corrects_synthetic_denial_and_delivers_note_on_resume() {
    for (tool, result) in [
        ("Bash", None),
        ("Bash", Some((SYNTHETIC_DENIAL, true))),
        ("mcp__aether__shell", Some(("cancelled", true))),
        ("bash", Some(("[exit code ?]", false))),
    ] {
        let h = in_flight_result(result, true, tool).await;
        let id = h
            .store
            .agents(&aether_store::AgentFilter::default())
            .unwrap()[0]
            .id;
        h.runtime.interrupt_agent(id, Actor::user()).unwrap();
        wait_for(h.store.as_ref(), id, |a| {
            a.status.kind() == AgentStatusKind::Waiting
        })
        .await;
        let events = h
            .store
            .events(0, 1000, &EventFilter::own_events(id))
            .unwrap();
        assert!(events.iter().any(|e| matches!(&e.kind, EventKind::Custom { name, payload } if name == "tool.interrupted" && payload["cause"] == "turn interrupt")));
        assert!(
            !events
                .iter()
                .any(|e| matches!(e.kind, EventKind::ToolDenied { .. }))
        );
        assert!(!events.iter().any(|e| matches!(&e.kind, EventKind::AgentMessage { content, .. } if content.iter().any(|b| matches!(b, ContentBlock::ToolResult { content, .. } if format!("{content:?}").contains(SYNTHETIC_DENIAL))))));
        h.runtime
            .send_message(id, vec![ContentBlock::text("continue")], Actor::user())
            .unwrap();
        wait_for(h.store.as_ref(), id, |a| {
            a.stats.turns >= 2 && a.status.kind() == AgentStatusKind::Waiting
        })
        .await;
        let prompts = h.prompts.lock().unwrap().clone();
        assert!(prompts[1].contains("not denied"));
        assert!(prompts[1].contains("make ci"));
        assert!(prompts[1].contains("turn interrupt"));
        h.runtime
            .send_message(id, vec![ContentBlock::text("one more")], Actor::user())
            .unwrap();
        wait_for(h.store.as_ref(), id, |a| {
            a.stats.turns >= 3 && a.status.kind() == AgentStatusKind::Waiting
        })
        .await;
        assert!(!h.prompts.lock().unwrap()[2].contains("not denied"));
        h.runtime.shutdown().await;
    }
}

#[tokio::test]
async fn cancellation_names_the_reason_and_real_denial_stays_denied() {
    let h = in_flight(true, true).await;
    let id = h
        .store
        .agents(&aether_store::AgentFilter::default())
        .unwrap()[0]
        .id;
    h.runtime
        .cancel_agent(id, Some("stage deadline exceeded".into()), Actor::user())
        .unwrap();
    wait_for(h.store.as_ref(), id, |a| a.status.is_terminal()).await;
    assert!(h.store.events(0, 1000, &EventFilter::own_events(id)).unwrap().iter().any(|e| matches!(&e.kind, EventKind::Custom { name, payload } if name == "tool.interrupted" && payload["cause"].as_str().unwrap().contains("stage deadline exceeded"))));
    h.runtime.shutdown().await;

    let h = in_flight(true, false).await;
    let id = h
        .store
        .agents(&aether_store::AgentFilter::default())
        .unwrap()[0]
        .id;
    wait_for(h.store.as_ref(), id, |a| {
        a.status.kind() == AgentStatusKind::Waiting
    })
    .await;
    let events = h
        .store
        .events(0, 1000, &EventFilter::own_events(id))
        .unwrap();
    assert!(
        events
            .iter()
            .any(|e| matches!(e.kind, EventKind::ToolDenied { .. }))
    );
    assert!(
        !events.iter().any(
            |e| matches!(&e.kind, EventKind::Custom { name, .. } if name == "tool.interrupted")
        )
    );
    h.runtime.shutdown().await;
}

#[tokio::test]
async fn daemon_restart_delivers_interrupted_tool_note_to_retained_session() {
    let h = in_flight_tool(true, true, "Read").await;
    let id = h
        .store
        .agents(&aether_store::AgentFilter::default())
        .unwrap()[0]
        .id;
    assert!(!h.prompts.lock().unwrap()[0].contains("not denied"));
    h.runtime.shutdown().await;
    let engine: Arc<dyn Engine> = Arc::new(InFlightEngine {
        prompts: h.prompts.clone(),
        result: Some((SYNTHETIC_DENIAL, true)),
        wait_for_stop: true,
        tool: "Read",
    });
    let restarted = Runtime::start_with_engines(
        h.runtime.config().clone(),
        h.store.clone(),
        h.runtime.models().clone(),
        h.runtime.tools().clone(),
        None,
        HashMap::from([(EngineKind::CLAUDE_CODE, engine)]),
    )
    .await
    .unwrap();
    wait_for(h.store.as_ref(), id, |a| {
        a.stats.turns >= 2 && a.status.kind() == AgentStatusKind::Waiting
    })
    .await;
    let prompts = h.prompts.lock().unwrap().clone();
    assert!(prompts[1].contains("daemon stop or restart"));
    assert!(prompts[1].contains("not denied"));
    restarted.shutdown().await;
}

#[tokio::test]
async fn scheduler_deadline_retains_the_cause_of_an_in_flight_interruption() {
    let h = in_flight(true, true).await;
    let id = h
        .store
        .agents(&aether_store::AgentFilter::default())
        .unwrap()[0]
        .id;
    h.runtime
        .update_agent(
            id,
            aether_runtime::AgentUpdate {
                deadline: Some(aether_core::agent::DeadlineUpdate::Set(
                    aether_core::time::now(),
                )),
                ..Default::default()
            },
            Actor::user(),
        )
        .unwrap();
    tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            let events = h.store.events(0, 1000, &EventFilter::own_events(id)).unwrap();
            if events.iter().any(|event| matches!(&event.kind, EventKind::Custom { name, payload }
                if name == "tool.interrupted" && payload["cause"].as_str().unwrap().contains("scheduler stop: deadline of"))) {
                break;
            }
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    }).await.expect("deadline interruption records its cause within seconds");
    assert!(matches!(
        h.store.agent(id).unwrap().unwrap().status,
        AgentStatus::Failed { .. }
    ));
    h.runtime.shutdown().await;
}

#[tokio::test]
async fn completed_command_racing_an_interrupt_keeps_its_success() {
    let h = in_flight_result(Some(("done\n[exit code 0]", false)), true, "bash").await;
    let id = h
        .store
        .agents(&aether_store::AgentFilter::default())
        .unwrap()[0]
        .id;
    h.runtime.interrupt_agent(id, Actor::user()).unwrap();
    wait_for(h.store.as_ref(), id, |a| {
        a.status.kind() == AgentStatusKind::Waiting
    })
    .await;
    let events = h
        .store
        .events(0, 1000, &EventFilter::own_events(id))
        .unwrap();
    assert!(
        events
            .iter()
            .any(|event| matches!(event.kind, EventKind::ToolSucceeded { .. }))
    );
    assert!(!events.iter().any(
        |event| matches!(&event.kind, EventKind::Custom { name, .. } if name == "tool.interrupted")
    ));
    h.runtime.shutdown().await;
}
