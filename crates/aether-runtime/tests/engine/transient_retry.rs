//! Native retries require a rejected prompt and no observed model or tool activity.
use super::*;
use aether_core::model::Usage;

const STARTUP_TIMEOUT: &str = "codex app-server did not start the turn in time";
const CAPACITY: &str = "Selected model is at capacity. Please try a different model.";

fn failure(error: &str, prompt_not_submitted: bool) -> EngineEvent {
    EngineEvent::Done(EngineOutcome {
        is_error: true,
        error: Some(error.into()),
        prompt_not_submitted,
        ..Default::default()
    })
}

async fn queued(runs: Vec<Vec<EngineEvent>>) -> (Harness, Arc<QueuedEngine>) {
    let engine = QueuedEngine::new(runs);
    let h = harness_with_engine(engine.clone(), Arc::default(), Arc::default()).await;
    (h, engine)
}

fn failures(h: &Harness, id: AgentId) -> Vec<(aether_core::model::ModelErrorClass, bool)> {
    h.store
        .events(0, 10_000, &EventFilter::own_events(id))
        .unwrap()
        .into_iter()
        .filter_map(|event| match event.kind {
            EventKind::InvocationFailed {
                class: Some(class),
                retryable,
                ..
            } => Some((class, retryable)),
            _ => None,
        })
        .collect()
}

#[tokio::test]
async fn startup_retry_preserves_resumed_session_and_prompt() {
    let mut first = vec![EngineEvent::Session {
        id: "retained".into(),
        model: None,
        tools: vec![],
    }];
    first.extend(answered("ready"));
    let (h, engine) = queued(vec![
        first,
        vec![failure(STARTUP_TIMEOUT, true)],
        answered("recovered"),
    ])
    .await;
    let id = chat_agent(&h);
    engine_turn(&h, id, "first").await;
    engine_turn(&h, id, "retry this resumed prompt").await;
    assert_eq!(
        *engine.sessions.lock().unwrap(),
        [None, Some("retained".into()), Some("retained".into())]
    );
    let prompts = engine.prompts.lock().unwrap().clone();
    assert_eq!(prompts.len(), 3);
    assert_eq!(prompts[1], prompts[2]);
    assert_eq!(
        failures(&h, id),
        [(aether_core::model::ModelErrorClass::Timeout, true)]
    );
    assert_eq!(count(h.store.as_ref(), id, "snapshot.created"), 2);
    assert_eq!(
        spoken(h.store.as_ref(), id)
            .iter()
            .filter(|(role, _)| *role == MessageRole::Assistant)
            .count(),
        2
    );
    h.runtime.shutdown().await;
}

#[tokio::test]
async fn capacity_before_any_activity_is_retried() {
    let (h, engine) = queued(vec![vec![failure(CAPACITY, false)], answered("recovered")]).await;
    let id = chat_agent(&h);
    engine_turn(&h, id, "go").await;
    assert_eq!(engine.prompts.lock().unwrap().len(), 2);
    assert_eq!(
        failures(&h, id),
        [(aether_core::model::ModelErrorClass::Overloaded, true)]
    );
    h.runtime.shutdown().await;
}

#[tokio::test]
async fn repeated_startup_failures_are_bounded_and_reach_the_root() {
    use aether_core::model::ModelErrorClass;
    for (error, class) in [
        (STARTUP_TIMEOUT, ModelErrorClass::Timeout),
        ("API Error: Connection error.", ModelErrorClass::Network),
    ] {
        give_up_reaches_the_root(error, class).await;
    }
}

async fn give_up_reaches_the_root(error: &str, class: aether_core::model::ModelErrorClass) {
    let engine = QueuedEngine::new((0..1_000).map(|_| vec![failure(error, true)]).collect());
    let h = harness_with_engine_config(engine.clone(), |config| {
        config.engine_transient_retry_window = Duration::from_millis(200);
    })
    .await;
    let mut parent = None;
    let mut ancestors = Vec::new();
    for name in ["root", "parent"] {
        let (agent, _) = h
            .runtime
            .create_agent(
                CreateAgentSpec {
                    role: "worker".into(),
                    parent,
                    config: Some(AgentConfig {
                        effort: Some(Effort::Low),
                        ..Default::default()
                    }),
                    objective: Some(name.into()),
                    start: false,
                    ..Default::default()
                },
                parent
                    .map(|id| Actor::Agent { id })
                    .unwrap_or_else(Actor::user),
            )
            .unwrap();
        parent = Some(agent.id);
        ancestors.push(agent.id);
    }
    let (child, task) = h
        .runtime
        .create_agent(
            CreateAgentSpec {
                role: "worker".into(),
                parent,
                config: Some(AgentConfig {
                    effort: Some(Effort::Low),
                    ..Default::default()
                }),
                objective: Some("fail visibly".into()),
                start: true,
                ..Default::default()
            },
            Actor::Agent {
                id: parent.unwrap(),
            },
        )
        .unwrap();
    let failed = wait_for(h.store.as_ref(), child.id, |a| a.status.is_terminal()).await;
    assert!(
        matches!(failed.status, AgentStatus::Failed { error: ref terminal } if terminal.contains(error))
    );
    assert!(matches!(
        h.store.task(task.unwrap().id).unwrap().unwrap().status,
        aether_core::task::TaskStatus::Failed { .. }
    ));
    let launches = engine.prompts.lock().unwrap().len();
    assert!((2..1_000).contains(&launches), "{launches}");
    let recorded = failures(&h, child.id);
    // Retryable per launch; the give-up is terminal, recorded on its own when
    // the window ran out during a wait rather than during the last launch.
    assert!(
        [launches, launches + 1].contains(&recorded.len()),
        "{recorded:?}"
    );
    let (last, retried) = recorded.split_last().unwrap();
    assert!(!last.1 && retried.iter().all(|f| f.1));
    assert!(recorded.iter().all(|f| f.0 == class));
    for ancestor in ancestors {
        let errors: Vec<_> = h
            .store
            .inbox(ancestor, 0)
            .unwrap()
            .into_iter()
            .filter_map(|item| match item.kind {
                aether_core::inbox::InboxKind::ChildFailed { agent, error }
                    if agent == child.id =>
                {
                    Some(error)
                }
                _ => None,
            })
            .collect();
        assert_eq!(
            errors.len(),
            1,
            "each ancestor receives the terminal failure once"
        );
        assert!(errors[0].contains(error));
    }
    h.runtime.shutdown().await;
}

#[tokio::test]
async fn ambiguous_startup_and_capacity_after_activity_are_not_replayed() {
    use aether_core::model::StreamEvent;
    let cases = vec![
        vec![failure(STARTUP_TIMEOUT, false)],
        vec![
            EngineEvent::Delta(StreamEvent::TextDelta {
                index: 0,
                text: "partial".into(),
            }),
            failure(CAPACITY, false),
        ],
        vec![
            EngineEvent::Delta(StreamEvent::ThinkingDelta {
                index: 0,
                text: "thinking".into(),
            }),
            failure(CAPACITY, false),
        ],
        vec![
            EngineEvent::Assistant {
                content: vec![ContentBlock::ToolUse {
                    id: "tool".into(),
                    name: "Bash".into(),
                    input: serde_json::json!({"command":"true"}),
                }],
                usage: None,
                model: None,
            },
            failure(CAPACITY, false),
        ],
        vec![
            EngineEvent::UsageDelta {
                usage: Usage {
                    input_tokens: 1,
                    ..Default::default()
                },
            },
            failure(CAPACITY, false),
        ],
        vec![EngineEvent::Done(EngineOutcome {
            is_error: true,
            error: Some(CAPACITY.into()),
            cost_usd: Some(0.1),
            ..Default::default()
        })],
        vec![EngineEvent::Done(EngineOutcome {
            is_error: true,
            error: Some(CAPACITY.into()),
            text: Some("partial".into()),
            ..Default::default()
        })],
        vec![failure("Weekly limit reached; model is overloaded", true)],
    ];
    for events in cases {
        let quota = matches!(events.last(), Some(EngineEvent::Done(o)) if o.error.as_deref().unwrap().contains("Weekly"));
        let (h, engine) = queued(vec![events, answered("must not run")]).await;
        let (id, _) = engine_worker(&h, "do not replay effects");
        wait_for(h.store.as_ref(), id, |a| {
            a.status.is_terminal() || (quota && matches!(a.status, AgentStatus::Blocked { .. }))
        })
        .await;
        assert_eq!(engine.prompts.lock().unwrap().len(), 1);
        assert!(!failures(&h, id)[0].1);
        h.runtime.shutdown().await;
    }
}

#[tokio::test]
async fn a_bridge_request_prevents_replay_even_without_native_tool_events() {
    let (handover, ready) = tokio::sync::oneshot::channel();
    let (finish, finished) = tokio::sync::oneshot::channel();
    let h = harness_with_engine(
        Arc::new(SilentBridgeEngine {
            outcome: EngineOutcome {
                is_error: true,
                error: Some(CAPACITY.into()),
                ..Default::default()
            },
            handover: Mutex::new(Some(handover)),
            finish: Mutex::new(Some(finished)),
        }),
        Arc::default(),
        Arc::default(),
    )
    .await;
    let (id, _) = engine_worker(&h, "observe bridge activity");
    let (agent, token) = ready.await.unwrap();
    h.runtime
        .engine_tool(agent, &token, "inspect_state".into(), serde_json::json!({}))
        .await
        .unwrap();
    finish.send(()).unwrap();
    wait_for(h.store.as_ref(), id, |a| a.status.is_terminal()).await;
    assert_eq!(
        failures(&h, id),
        [(aether_core::model::ModelErrorClass::Overloaded, false)]
    );
    h.runtime.shutdown().await;
}

#[tokio::test]
async fn resume_exhaustion_never_restarts_the_session() {
    for error in [
        "Weekly limit reached",
        "insufficient_quota",
        "context_length_exceeded",
        "prompt is too long",
    ] {
        let mut first = vec![EngineEvent::Session {
            id: "retained".into(),
            model: None,
            tools: vec![],
        }];
        first.extend(answered("ready"));
        let (h, engine) = queued(vec![
            first,
            vec![EngineEvent::Done(EngineOutcome {
                is_error: true,
                error: Some(error.into()),
                session_lost: true,
                ..Default::default()
            })],
            answered("must not restart"),
        ])
        .await;
        let id = chat_agent(&h);
        engine_turn(&h, id, "first").await;
        h.runtime
            .send_message(id, vec![ContentBlock::text("resume")], Actor::user())
            .unwrap();
        wait_for(h.store.as_ref(), id, |agent| {
            agent.status.is_terminal() || matches!(agent.status, AgentStatus::Blocked { .. })
        })
        .await;
        assert_eq!(engine.prompts.lock().unwrap().len(), 2, "{error}");
        assert!(!failures(&h, id)[0].1);
        h.runtime.shutdown().await;
    }
}

#[tokio::test]
async fn a_claude_zero_cost_overload_receipt_is_metadata_and_can_retry() {
    let events = aether_engine::claude_code::parse_line(
        &serde_json::json!({"type":"result", "subtype":"error_during_execution", "is_error":true,
            "result":"Model is overloaded", "session_id":"empty", "total_cost_usd":0.0, "usage":{}})
        .to_string(),
        &mut aether_engine::claude_code::ParseState::default(),
    );
    assert!(
        matches!(&events[..], [EngineEvent::ProtocolNote(_), EngineEvent::Done(outcome)]
        if outcome.cost_usd.is_none() && outcome.cost_usage.is_none())
    );
    assert!(
        !events
            .iter()
            .any(|event| matches!(event, EngineEvent::SessionUsage { .. }))
    );
    let (h, engine) = queued(vec![events, answered("recovered")]).await;
    let id = chat_agent(&h);
    engine_turn(&h, id, "go").await;
    assert_eq!(engine.prompts.lock().unwrap().len(), 2);
    assert_eq!(
        failures(&h, id),
        [(aether_core::model::ModelErrorClass::Overloaded, true)]
    );
    h.runtime.shutdown().await;
}

struct CleanupEngine {
    phases: Mutex<std::collections::VecDeque<(BridgeHandover, tokio::sync::oneshot::Receiver<()>)>>,
    attempts: std::sync::atomic::AtomicUsize,
}

#[async_trait::async_trait]
impl Engine for CleanupEngine {
    fn kind(&self) -> EngineKind {
        EngineKind::CLAUDE_CODE
    }
    fn adapter(&self) -> String {
        "cleanup-test/1".into()
    }
    async fn run(
        &self,
        turn: EngineTurn,
        cancel: CancellationToken,
    ) -> Result<EngineRun, EngineError> {
        let attempt = self
            .attempts
            .fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        let (handover, finish) = self
            .phases
            .lock()
            .unwrap()
            .pop_front()
            .expect("expected attempt");
        let args = &turn.mcp.unwrap().args;
        let agent = args[4].parse().unwrap();
        let token = args[6].clone();
        let (tx, rx) = mpsc::channel(8);
        tokio::spawn(async move {
            if attempt == 0 {
                tx.send(failure(CAPACITY, false)).await.unwrap();
                cancel.cancelled().await;
                handover.send((agent, token)).unwrap();
                finish.await.unwrap();
            } else {
                handover.send((agent, token)).unwrap();
                finish.await.unwrap();
                for event in answered("recovered") {
                    tx.send(event).await.unwrap();
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

struct CleanupFixture {
    engine: Arc<CleanupEngine>,
    ready: Vec<tokio::sync::oneshot::Receiver<(AgentId, String)>>,
    finishes: Vec<tokio::sync::oneshot::Sender<()>>,
}

fn cleanup_engine() -> CleanupFixture {
    let mut phases = std::collections::VecDeque::new();
    let mut ready = Vec::new();
    let mut finishes = Vec::new();
    for _ in 0..2 {
        let (handover, receive) = tokio::sync::oneshot::channel();
        let (finish, finished) = tokio::sync::oneshot::channel();
        phases.push_back((handover, finished));
        ready.push(receive);
        finishes.push(finish);
    }
    CleanupFixture {
        engine: Arc::new(CleanupEngine {
            phases: Mutex::new(phases),
            attempts: std::sync::atomic::AtomicUsize::new(0),
        }),
        ready,
        finishes,
    }
}

#[tokio::test]
async fn a_bridge_request_after_done_is_refused_and_prevents_retry() {
    let CleanupFixture {
        engine,
        mut ready,
        mut finishes,
    } = cleanup_engine();
    let h = harness_with_engine(engine.clone(), Arc::default(), Arc::default()).await;
    let (id, _) = engine_worker(&h, "fence late bridge activity");
    let (agent, token) = ready.remove(0).await.unwrap();
    let (reply, error) = h
        .runtime
        .engine_tool(agent, &token, "inspect_state".into(), serde_json::json!({}))
        .await
        .unwrap();
    assert!(error, "late bridge call was served: {reply}");
    finishes.remove(0).send(()).unwrap();
    wait_for(h.store.as_ref(), id, |a| a.status.is_terminal()).await;
    assert_eq!(engine.attempts.load(std::sync::atomic::Ordering::SeqCst), 1);
    assert_eq!(
        failures(&h, id),
        [(aether_core::model::ModelErrorClass::Overloaded, false)]
    );
    h.runtime.shutdown().await;
}

#[tokio::test]
async fn retry_waits_for_cleanup_and_retires_previous_bridge_credentials() {
    let CleanupFixture {
        engine,
        mut ready,
        mut finishes,
    } = cleanup_engine();
    let h = harness_with_engine(engine.clone(), Arc::default(), Arc::default()).await;
    let id = chat_agent(&h);
    h.runtime
        .send_message(id, vec![ContentBlock::text("go")], Actor::user())
        .unwrap();
    let (agent, old_token) = ready.remove(0).await.unwrap();
    assert_eq!(engine.attempts.load(std::sync::atomic::Ordering::SeqCst), 1);
    finishes.remove(0).send(()).unwrap();
    let (_, new_token) = ready.remove(0).await.unwrap();
    assert_ne!(old_token, new_token);
    assert!(
        h.runtime
            .engine_tool(
                agent,
                &old_token,
                "inspect_state".into(),
                serde_json::json!({})
            )
            .await
            .is_err()
    );
    finishes.remove(0).send(()).unwrap();
    wait_for(h.store.as_ref(), id, |a| {
        a.status.kind() == AgentStatusKind::Waiting
    })
    .await;
    assert_eq!(engine.attempts.load(std::sync::atomic::Ordering::SeqCst), 2);
    assert_eq!(
        failures(&h, id),
        [(aether_core::model::ModelErrorClass::Overloaded, true)]
    );
    h.runtime.shutdown().await;
}

#[tokio::test]
async fn cleanup_preserves_late_exhaustion_and_never_retries_it() {
    for error in ["Weekly limit reached", "context_length_exceeded"] {
        let (h, engine) = queued(vec![
            vec![failure(CAPACITY, false), failure(error, false)],
            answered("must not run"),
        ])
        .await;
        let (id, _) = engine_worker(&h, "retain late exhaustion");
        let stopped = wait_for(h.store.as_ref(), id, |agent| {
            agent.status.is_terminal() || matches!(agent.status, AgentStatus::Blocked { .. })
        })
        .await;
        assert_eq!(engine.prompts.lock().unwrap().len(), 1);
        assert!(format!("{:?}", stopped.status).contains(error));
        assert!(!failures(&h, id)[0].1);
        h.runtime.shutdown().await;
    }
}

#[tokio::test]
async fn cleanup_never_lowers_a_failed_turns_billing_receipt() {
    let (h, engine) = queued(vec![vec![
        EngineEvent::Done(EngineOutcome {
            is_error: true,
            error: Some(CAPACITY.into()),
            cost_usd: Some(0.4),
            usage: Usage {
                input_tokens: 100,
                ..Default::default()
            },
            ..Default::default()
        }),
        EngineEvent::Done(EngineOutcome {
            is_error: true,
            error: Some("cancelled".into()),
            ..Default::default()
        }),
    ]])
    .await;
    let (id, _) = engine_worker(&h, "retain billing");
    let failed = wait_for(h.store.as_ref(), id, |a| a.status.is_terminal()).await;
    assert_eq!(engine.prompts.lock().unwrap().len(), 1);
    assert_eq!(failed.stats.usage.input_tokens, 100);
    assert_eq!(failed.stats.usage.cost_usd, 0.4);
    assert!(matches!(failed.status, AgentStatus::Failed { ref error } if error.contains(CAPACITY)));
    h.runtime.shutdown().await;
}

#[tokio::test]
async fn cancellation_during_cleanup_never_starts_another_attempt() {
    let CleanupFixture {
        engine,
        mut ready,
        mut finishes,
    } = cleanup_engine();
    let h = harness_with_engine(engine.clone(), Arc::default(), Arc::default()).await;
    let (id, _) = engine_worker(&h, "cancel before replay");
    ready.remove(0).await.unwrap();
    h.runtime
        .cancel_agent(id, Some("stop".into()), Actor::user())
        .unwrap();
    finishes.remove(0).send(()).unwrap();
    let cancelled = wait_for(h.store.as_ref(), id, |a| a.status.is_terminal()).await;
    assert!(matches!(cancelled.status, AgentStatus::Cancelled { .. }));
    assert_eq!(engine.attempts.load(std::sync::atomic::Ordering::SeqCst), 1);
    assert!(failures(&h, id).is_empty());
    h.runtime.shutdown().await;
}
