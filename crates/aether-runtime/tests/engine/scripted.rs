//! Scripted native engine shared by runtime integration tests.
use super::*;
#[derive(Clone, Copy)]
pub(super) enum Reply {
    Accept,
    Reject,
    /// One reply per turn, each priced from `costs`, then the run ends. An
    /// exhausted queue is a turn the engine reported no price for.
    OneTurn,
    /// `OneTurn` on a session the engine keeps, so the next turn resumes it.
    Resumable,
    Quota,
    /// Runs one world tool of its own and reports the result, as an engine
    /// that owns its loop does.
    ToolRun,
}

/// Behaves like Claude Code: a message handed over mid-turn is folded in
/// after the current result, acknowledged as it is, then answered. With
/// `Reject` it refuses every steered message instead.
pub(super) struct ScriptedEngine {
    pub(super) reply: Reply,
    pub(super) prompts: Arc<Mutex<Vec<String>>>,
    pub(super) costs: Arc<Mutex<std::collections::VecDeque<Option<f64>>>>,
    pub(super) systems: Arc<Mutex<Vec<String>>>,
    pub(super) tool_gate: Option<Arc<tokio::sync::Notify>>,
}

#[async_trait::async_trait]
impl Engine for ScriptedEngine {
    fn kind(&self) -> EngineKind {
        EngineKind::CLAUDE_CODE
    }

    fn adapter(&self) -> String {
        "scripted/1".into()
    }

    async fn run(
        &self,
        turn: EngineTurn,
        _cancel: CancellationToken,
    ) -> Result<EngineRun, EngineError> {
        self.prompts.lock().unwrap().push(turn.prompt.clone());
        self.systems
            .lock()
            .unwrap()
            .push(turn.system_prompt.clone());
        let (events, rx) = mpsc::channel(64);
        let reply = self.reply;
        let tool_gate = self.tool_gate.clone();
        let cost = self.costs.lock().unwrap().pop_front().flatten();
        if matches!(reply, Reply::OneTurn | Reply::Resumable | Reply::Quota) {
            tokio::spawn(async move {
                if matches!(reply, Reply::Resumable) {
                    let _ = events
                        .send(EngineEvent::Session {
                            id: "s-resumable".into(),
                            model: None,
                            tools: vec![],
                        })
                        .await;
                }
                if matches!(reply, Reply::Quota) {
                    events
                        .send(EngineEvent::RateLimit {
                            payload: serde_json::json!({
                                "unifiedWindows": {
                                    "five_hour": {"utilization": 0.42, "resetsAt": aether_core::time::now().timestamp() + 3600},
                                    "seven_day": {"utilization": 0.73}
                                }
                            }),
                        })
                        .await
                        .expect("quota event");
                }
                let _ = events
                    .send(EngineEvent::Assistant {
                        content: vec![ContentBlock::text("done")],
                        usage: None,
                        model: None,
                    })
                    .await;
                let _ = events
                    .send(EngineEvent::Done(EngineOutcome {
                        closing_accounting: true,
                        text: Some("done".into()),
                        cost_usd: cost,
                        // Tokens as an engine reports them: carried on the
                        // outcome's `usage`, with the price beside it rather
                        // than in it. Folding both must not make a turn the
                        // engine *did* price read as a floor.
                        usage: aether_core::model::Usage {
                            input_tokens: 70,
                            output_tokens: 17,
                            ..Default::default()
                        },
                        ..Default::default()
                    }))
                    .await;
            });
            return Ok(EngineRun {
                events: rx,
                input: None,
                decisions: None,
                stop: None,
            });
        }
        let (input_tx, mut input) = mpsc::channel::<aether_engine::EngineInput>(16);
        tokio::spawn(async move {
            let say = |text: &str| EngineEvent::Assistant {
                content: vec![ContentBlock::text(text)],
                usage: None,
                model: None,
            };
            let done = |text: &str| {
                EngineEvent::Done(EngineOutcome {
                    closing_accounting: true,
                    text: Some(text.to_owned()),
                    ..Default::default()
                })
            };
            let _ = events
                .send(EngineEvent::Session {
                    id: "s1".into(),
                    model: None,
                    tools: vec![],
                })
                .await;
            if matches!(reply, Reply::ToolRun) {
                let _ = events
                    .send(EngineEvent::Assistant {
                        content: vec![ContentBlock::ToolUse {
                            id: "call-1".into(),
                            name: "Bash".into(),
                            input: serde_json::json!({"command": "ls"}),
                        }],
                        usage: None,
                        model: None,
                    })
                    .await;
                if let Some(gate) = tool_gate {
                    gate.notified().await;
                }
                let _ = events
                    .send(EngineEvent::ToolResult {
                        tool_use_id: "call-1".into(),
                        content: "src/ tests/\nANTHROPIC_API_KEY=sk-ant-api03-NOTAREALKEYbutkeyshaped0123456789".into(),
                        is_error: false,
                    })
                    .await;
                let _ = events.send(say("two directories")).await;
                let _ = events.send(done("two directories")).await;
                return;
            }
            let _ = events.send(say("first reply")).await;
            // Whatever arrives while the first result is being produced is
            // queued, exactly as the CLI queues stdin.
            let mut queued = Vec::new();
            let window = tokio::time::sleep(Duration::from_millis(400));
            tokio::pin!(window);
            loop {
                tokio::select! {
                    Some(message) = input.recv() => queued.push(message.text),
                    () = &mut window => break,
                }
            }
            let _ = events.send(done("first reply")).await;
            let mut queued = queued.into_iter();
            loop {
                let text = match queued.next() {
                    Some(t) => t,
                    None => match input.recv().await {
                        Some(message) => message.text,
                        None => break,
                    },
                };
                match reply {
                    Reply::Accept => {
                        let _ = events.send(EngineEvent::InputAccepted).await;
                        let answer = format!("heard: {}", text.trim());
                        let _ = events.send(say(&answer)).await;
                        let _ = events.send(done(&answer)).await;
                    }
                    Reply::Reject | Reply::ToolRun => {
                        let _ = events
                            .send(EngineEvent::InputRejected {
                                reason: "turn is over".into(),
                            })
                            .await;
                    }
                    Reply::OneTurn | Reply::Resumable | Reply::Quota => {
                        unreachable!("answered above")
                    }
                }
            }
        });
        Ok(EngineRun {
            events: rx,
            input: Some(input_tx),
            decisions: None,
            stop: None,
        })
    }
}
