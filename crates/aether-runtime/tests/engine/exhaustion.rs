//! Quota exhaustion, fallback and committed task outcomes.
use super::*;
/// A native engine's own "interrupted" verdict does not change what a usage
/// limit means; it suspends the agent instead of failing it,
/// and the reset prose is kept as detail, never parsed into a probe time.
#[tokio::test]
async fn weekly_exhaustion_suspends_even_when_native_outcome_says_interrupted() {
    for interrupted in [false, true] {
        let engine = QueuedEngine::new(vec![vec![EngineEvent::Done(EngineOutcome {
            is_error: true,
            interrupted,
            error: Some(
                "You've hit your limit · resets Sep 18 at 2pm; sk-123456789012345678901234".into(),
            ),
            session: Some("retained-session".into()),
            ..Default::default()
        })]]);
        let h = harness_with_engine(
            engine.clone(),
            Arc::new(Mutex::new(vec![])),
            Arc::new(Mutex::new(vec![])),
        )
        .await;
        let (agent, task) = h
            .runtime
            .create_agent(
                CreateAgentSpec {
                    role: "worker".into(),
                    objective: Some("keep recoverable work".into()),
                    start: true,
                    ..Default::default()
                },
                Actor::user(),
            )
            .unwrap();
        let suspended = wait_for(h.store.as_ref(), agent.id, |a| {
            matches!(
                a.status,
                AgentStatus::Blocked {
                    reason: BlockReason::Exhausted { .. }
                }
            )
        })
        .await;
        let AgentStatus::Blocked {
            reason:
                BlockReason::Exhausted {
                    class,
                    detail,
                    reported_reset,
                    ..
                },
        } = suspended.status
        else {
            panic!("expected a suspension")
        };
        assert_eq!(class, ExhaustionClass::UsageLimit);
        assert!(detail.contains("resets Sep 18 at 2pm"), "{detail}");
        assert!(detail.contains("claude-code"), "{detail}");
        assert!(!detail.contains("sk-123456789012345678901234"), "{detail}");
        assert_eq!(
            reported_reset, None,
            "a reset named in prose is never parsed into a probe time"
        );
        assert!(
            !h.store
                .task(task.unwrap().id)
                .unwrap()
                .unwrap()
                .status
                .is_terminal(),
            "a suspended agent keeps its task"
        );
        assert_eq!(engine.prompts.lock().unwrap().len(), 1);
        h.runtime.shutdown().await;
    }
}

#[tokio::test]
async fn a_committed_task_outcome_survives_late_exhaustion_during_park() {
    for (long_lived, error, class) in [
        (false, "Weekly limit reached", "usage_limit"),
        (true, "Weekly limit reached", "usage_limit"),
        (false, "context_length_exceeded", "context_limit"),
        (true, "context_length_exceeded", "context_limit"),
    ] {
        let (handover, ready) = tokio::sync::oneshot::channel();
        let (finish, finished) = tokio::sync::oneshot::channel();
        let engine = Arc::new(SilentBridgeEngine {
            handover: Mutex::new(Some(handover)),
            finish: Mutex::new(Some(finished)),
            outcome: EngineOutcome {
                is_error: true,
                error: Some(error.into()),
                ..Default::default()
            },
        });
        let h = harness_with_engine(
            engine,
            Arc::new(Mutex::new(vec![])),
            Arc::new(Mutex::new(vec![])),
        )
        .await;
        let mut parent = None;
        let mut ancestors = Vec::new();
        for role in ["root", "parent"] {
            let (ancestor, _) = h
                .runtime
                .create_agent(
                    CreateAgentSpec {
                        role: role.into(),
                        parent,
                        start: false,
                        config: parent.map(|_| AgentConfig {
                            effort: Some(Effort::Low),
                            ..Default::default()
                        }),
                        ..Default::default()
                    },
                    parent.map(Actor::agent).unwrap_or_else(Actor::user),
                )
                .unwrap();
            parent = Some(ancestor.id);
            ancestors.push(ancestor.id);
        }
        let (agent, task) = h
            .runtime
            .create_agent(
                CreateAgentSpec {
                    role: "worker".into(),
                    parent,
                    objective: Some("finish before provider wind-down".into()),
                    start: true,
                    config: Some(AgentConfig {
                        effort: Some(Effort::Low),
                        long_lived,
                        approval: ApprovalPolicy::none(),
                        ..Default::default()
                    }),
                    ..Default::default()
                },
                Actor::user(),
            )
            .unwrap();
        let (id, token) = tokio::time::timeout(Duration::from_secs(20), ready)
            .await
            .unwrap()
            .unwrap();
        let (reply, is_error) = h
            .runtime
            .engine_tool(
                id,
                &token,
                "complete_task".into(),
                serde_json::json!({"summary":"done"}),
            )
            .await
            .unwrap();
        assert!(!is_error, "{reply}");
        finish.send(()).unwrap();
        let ended = wait_for(h.store.as_ref(), agent.id, |agent| {
            agent.status.is_terminal()
                || (agent.stats.turns >= 1 && matches!(agent.status, AgentStatus::Waiting { .. }))
        })
        .await;
        assert_eq!(
            ended.status.kind(),
            if long_lived {
                AgentStatusKind::Waiting
            } else {
                AgentStatusKind::Completed
            }
        );
        assert!(matches!(
            h.store.task(task.unwrap().id).unwrap().unwrap().status,
            aether_core::task::TaskStatus::Completed { .. }
        ));
        assert_eq!(exhaustion_events(h.store.as_ref(), id), 0);
        for ancestor in ancestors {
            let notices: Vec<_> = h
                .store
                .inbox(ancestor, 0)
                .unwrap()
                .into_iter()
                .filter_map(|item| match item.kind {
                    aether_core::inbox::InboxKind::ExternalTrigger {
                        source,
                        payload,
                        operator: false,
                    } if source == "exhaustion" => Some(payload),
                    _ => None,
                })
                .collect();
            assert_eq!(notices.len(), 1);
            assert_eq!(notices[0]["source_agent"], serde_json::json!(id));
            assert_eq!(notices[0]["category"], class);
            assert_eq!(
                notices[0]["verdict"],
                if class == "context_limit" {
                    "context limit exceeded; task outcome unchanged"
                } else {
                    "provider exhausted; task outcome unchanged"
                }
            );
        }

        h.runtime.shutdown().await;
    }
}

#[tokio::test]
async fn engine_exhaustion_uses_fallback_without_crossing_native_sessions() {
    let mut first = vec![EngineEvent::Session {
        id: "claude-session".into(),
        model: None,
        tools: vec![],
    }];
    first.push(EngineEvent::Done(EngineOutcome {
        is_error: true,
        error: Some("Weekly limit reached; resets tomorrow 14:00".into()),
        ..Default::default()
    }));
    let mut second = vec![EngineEvent::Session {
        id: "codex-session".into(),
        model: None,
        tools: vec![],
    }];
    second.extend(answered("fallback answered"));
    let engine = QueuedEngine::new(vec![
        first,
        second,
        answered("new primary session answered"),
    ]);
    let h = harness_with_engine(
        engine.clone(),
        Arc::new(Mutex::new(vec![])),
        Arc::new(Mutex::new(vec![])),
    )
    .await;
    let (agent, _) = h
        .runtime
        .create_agent(
            CreateAgentSpec {
                role: "assistant".into(),
                start: true,
                model: Some(ModelRef::new("claude-code", "haiku")),
                fallbacks: vec![ModelRef::new("codex", "gpt-6.1-sol")],
                ..Default::default()
            },
            Actor::user(),
        )
        .unwrap();
    h.runtime
        .send_message(
            agent.id,
            vec![ContentBlock::text("first turn")],
            Actor::user(),
        )
        .unwrap();
    let waited = wait_for(h.store.as_ref(), agent.id, |agent| {
        agent.stats.turns == 1 && matches!(agent.status, AgentStatus::Waiting { .. })
    })
    .await;
    assert_eq!(engine.prompts.lock().unwrap().len(), 2);
    assert_eq!(waited.stats.turns, 1, "fallback is not another turn");
    assert_eq!(*engine.sessions.lock().unwrap(), vec![None, None]);
    h.runtime
        .send_message(
            agent.id,
            vec![ContentBlock::text("next turn")],
            Actor::user(),
        )
        .unwrap();
    wait_for(h.store.as_ref(), agent.id, |agent| {
        agent.stats.turns == 2 && matches!(agent.status, AgentStatus::Waiting { .. })
    })
    .await;
    assert_eq!(
        *engine.sessions.lock().unwrap(),
        vec![None, None, None],
        "primary must not resume the fallback's session"
    );
    assert_eq!(engine.prompts.lock().unwrap().len(), 3);
    h.runtime.shutdown().await;
}

#[tokio::test]
async fn an_exhausted_engine_cause_survives_an_unrelated_fallback_failure() {
    let engine = QueuedEngine::new(vec![
        vec![EngineEvent::Done(EngineOutcome {
            is_error: true,
            error: Some("Weekly limit reached; resets tomorrow 14:00".into()),
            ..Default::default()
        })],
        vec![EngineEvent::Done(EngineOutcome {
            is_error: true,
            error: Some("unrelated engine error".into()),
            ..Default::default()
        })],
    ]);
    let h = harness_with_engine(
        engine.clone(),
        Arc::new(Mutex::new(vec![])),
        Arc::new(Mutex::new(vec![])),
    )
    .await;
    let (agent, _) = h
        .runtime
        .create_agent(
            CreateAgentSpec {
                role: "worker".into(),
                objective: Some("retain the primary cause".into()),
                start: true,
                model: Some(ModelRef::new("claude-code", "haiku")),
                fallbacks: vec![ModelRef::new("codex", "gpt-6.1-sol")],
                ..Default::default()
            },
            Actor::user(),
        )
        .unwrap();
    let failed = wait_for(h.store.as_ref(), agent.id, |agent| {
        agent.status.is_terminal()
    })
    .await;
    let AgentStatus::Failed { error } = failed.status else {
        panic!("expected failure")
    };
    assert!(error.contains("usage_limit"), "{error}");
    assert!(error.contains("haiku"));
    assert!(error.contains("resets tomorrow 14:00"));
    assert!(error.contains("unrelated engine error"));
    assert_eq!(engine.prompts.lock().unwrap().len(), 2);
    h.runtime.shutdown().await;
}

fn exhaustion_events(store: &dyn Store, id: AgentId) -> usize {
    store
        .events(0, 1000, &EventFilter::own_events(id))
        .unwrap()
        .iter()
        .filter(|event| {
            matches!(&event.kind, EventKind::Custom { name, .. } if name == "exhaustion.failed")
        })
        .count()
}

fn fallback_reasons(store: &dyn Store, id: AgentId) -> Vec<String> {
    store
        .events(0, 10_000, &EventFilter::own_events(id))
        .unwrap()
        .into_iter()
        .filter_map(|event| match event.kind {
            EventKind::ProviderFallback { reason, .. } => Some(reason),
            _ => None,
        })
        .collect()
}

/// A silent-turn stop is the runtime's own verdict on the task, not a link
/// failure: it keeps its message and never moves to a configured fallback.
#[tokio::test]
async fn a_silent_turn_stop_is_final_with_or_without_a_fallback() {
    for fallbacks in [vec![], vec![ModelRef::new("codex", "gpt-6.1-sol")]] {
        let engine = QueuedEngine::new(vec![
            never_invoked(),
            never_invoked(),
            never_invoked(),
            answered("a fallback must not answer a silent-turn stop"),
        ]);
        let h = harness_with_engine(
            engine.clone(),
            Arc::new(Mutex::new(Vec::new())),
            Arc::new(Mutex::new(Vec::new())),
        )
        .await;
        let (agent, _) = h
            .runtime
            .create_agent(
                CreateAgentSpec {
                    role: "worker".into(),
                    start: true,
                    objective: Some("watch the build".into()),
                    model: Some(ModelRef::new("claude-code", "haiku")),
                    fallbacks: fallbacks.clone(),
                    config: Some(AgentConfig {
                        approval: ApprovalPolicy::none(),
                        ..Default::default()
                    }),
                    ..Default::default()
                },
                Actor::user(),
            )
            .unwrap();
        let ended = wait_for(h.store.as_ref(), agent.id, |agent| {
            agent.status.is_terminal()
        })
        .await;
        let AgentStatus::Failed { error } = ended.status else {
            panic!("expected the silent-turn stop, got {:?}", ended.status)
        };
        assert!(error.contains("ended without an answer"), "{error}");
        assert!(!error.starts_with("model unavailable"), "{error}");
        assert!(fallback_reasons(h.store.as_ref(), agent.id).is_empty());
        assert_eq!(exhaustion_events(h.store.as_ref(), agent.id), 0);
        assert_eq!(engine.prompts.lock().unwrap().len(), 3, "{fallbacks:?}");
        h.runtime.shutdown().await;
    }
}

/// First turn over the bridge (so a harness tool can settle its task), then
/// scripted runs.
struct BridgeThenQueued {
    first: SilentBridgeEngine,
    later: Arc<QueuedEngine>,
}

#[async_trait::async_trait]
impl Engine for BridgeThenQueued {
    fn kind(&self) -> EngineKind {
        EngineKind::CLAUDE_CODE
    }

    fn adapter(&self) -> String {
        "bridge-then-queued/1".into()
    }

    async fn run(
        &self,
        turn: EngineTurn,
        cancel: CancellationToken,
    ) -> Result<EngineRun, EngineError> {
        if self.first.handover.lock().unwrap().is_some() {
            return self.first.run(turn, cancel).await;
        }
        self.later.run(turn, cancel).await
    }
}

/// A long-lived agent keeps its settled task as `current_task`. A provider
/// limit on a later turn is that turn's, so the chain still falls back.
#[tokio::test]
async fn a_long_lived_agent_falls_back_on_a_limit_after_its_task_settled() {
    let (handover, ready) = tokio::sync::oneshot::channel();
    let (finish, finished) = tokio::sync::oneshot::channel();
    let later = QueuedEngine::new(vec![
        vec![EngineEvent::Done(EngineOutcome {
            is_error: true,
            error: Some("Weekly limit reached; resets tomorrow 14:00".into()),
            ..Default::default()
        })],
        answered("fallback answered the later turn"),
    ]);
    let engine = Arc::new(BridgeThenQueued {
        first: SilentBridgeEngine {
            outcome: EngineOutcome {
                text: Some("task done".into()),
                ..Default::default()
            },
            handover: Mutex::new(Some(handover)),
            finish: Mutex::new(Some(finished)),
        },
        later: later.clone(),
    });
    let h = harness_with_engine(
        engine,
        Arc::new(Mutex::new(vec![])),
        Arc::new(Mutex::new(vec![])),
    )
    .await;
    let (agent, task) = h
        .runtime
        .create_agent(
            CreateAgentSpec {
                role: "assistant".into(),
                objective: Some("first objective".into()),
                start: true,
                model: Some(ModelRef::new("claude-code", "haiku")),
                fallbacks: vec![ModelRef::new("codex", "gpt-6.1-sol")],
                config: Some(AgentConfig {
                    long_lived: true,
                    approval: ApprovalPolicy::none(),
                    ..Default::default()
                }),
                ..Default::default()
            },
            Actor::user(),
        )
        .unwrap();
    let task = task.expect("task").id;
    let (id, token) = tokio::time::timeout(Duration::from_secs(20), ready)
        .await
        .unwrap()
        .unwrap();
    let (reply, is_error) = h
        .runtime
        .engine_tool(
            id,
            &token,
            "complete_task".into(),
            serde_json::json!({"summary": "done"}),
        )
        .await
        .unwrap();
    assert!(!is_error, "{reply}");
    finish.send(()).unwrap();
    let settled = wait_for(h.store.as_ref(), agent.id, |agent| {
        agent.status.is_terminal()
            || (agent.stats.turns >= 1 && matches!(agent.status, AgentStatus::Waiting { .. }))
    })
    .await;
    assert_eq!(settled.status.kind(), AgentStatusKind::Waiting);
    assert_eq!(
        settled.current_task,
        Some(task),
        "the settled task stays current"
    );

    h.runtime
        .send_message(
            agent.id,
            vec![ContentBlock::text("next turn")],
            Actor::user(),
        )
        .unwrap();
    let ended = wait_for(h.store.as_ref(), agent.id, |agent| {
        agent.status.is_terminal()
            || (agent.stats.turns >= 2 && matches!(agent.status, AgentStatus::Waiting { .. }))
    })
    .await;
    assert_eq!(
        ended.status.kind(),
        AgentStatusKind::Waiting,
        "{:?}",
        ended.status
    );
    let reasons = fallback_reasons(h.store.as_ref(), agent.id);
    assert_eq!(reasons.len(), 1, "{reasons:?}");
    assert!(reasons[0].contains("Weekly limit reached"), "{reasons:?}");
    assert_eq!(
        later.prompts.lock().unwrap().len(),
        2,
        "limit, then fallback"
    );
    assert_eq!(exhaustion_events(h.store.as_ref(), agent.id), 0);
    assert!(matches!(
        h.store.task(task).unwrap().unwrap().status,
        aether_core::task::TaskStatus::Completed { .. }
    ));
    h.runtime.shutdown().await;
}

#[tokio::test]
async fn native_context_limit_overrides_an_open_wait_and_interruption() {
    let (handover, ready) = tokio::sync::oneshot::channel();
    let (finish, finished) = tokio::sync::oneshot::channel();
    let engine = Arc::new(SilentBridgeEngine {
        handover: Mutex::new(Some(handover)),
        finish: Mutex::new(Some(finished)),
        outcome: EngineOutcome {
            is_error: true,
            interrupted: true,
            error: Some("context_length_exceeded".into()),
            ..Default::default()
        },
    });
    let h = harness_with_engine(
        engine,
        Arc::new(Mutex::new(vec![])),
        Arc::new(Mutex::new(vec![])),
    )
    .await;
    let (agent, _) = h
        .runtime
        .create_agent(
            CreateAgentSpec {
                role: "worker".into(),
                objective: Some("keep work".into()),
                start: true,
                config: Some(AgentConfig {
                    approval: ApprovalPolicy::none(),
                    ..Default::default()
                }),
                ..Default::default()
            },
            Actor::user(),
        )
        .unwrap();
    let (id, token) = ready.await.unwrap();
    let (reply, is_error) = h
        .runtime
        .engine_tool(
            id,
            &token,
            "wait_for".into(),
            serde_json::json!({"timeout_seconds":3600}),
        )
        .await
        .unwrap();
    assert!(!is_error, "{reply}");
    finish.send(()).unwrap();
    let ended = wait_for(h.store.as_ref(), agent.id, |agent| {
        agent.status.is_terminal()
    })
    .await;
    assert!(
        matches!(ended.status, AgentStatus::Failed { error } if error.contains("context_length_exceeded"))
    );
    let events = h
        .store
        .latest_events(100, &EventFilter::own_events(id))
        .unwrap();
    assert!(events.iter().any(|event| matches!(
        event.kind,
        EventKind::InvocationFailed {
            class: Some(aether_core::model::ModelErrorClass::ContextLimit),
            ..
        }
    )));
    // Parking keeps the adapter's interruption as provenance; context exhaustion wins the task.
    let interrupted = events.iter().filter(|event| {
        matches!(&event.kind, EventKind::Custom { name, .. } if name == "engine.interrupted")
    }).count();
    assert_eq!(interrupted, 1);
    let task = h.store.task(agent.current_task.unwrap()).unwrap().unwrap();
    assert!(
        matches!(task.status, aether_core::task::TaskStatus::Failed { error, .. }
        if error.contains("context_length_exceeded"))
    );
    h.runtime.shutdown().await;
}

/// A refused sign-in is as final as a spent quota: it overrides an open wait
/// and the adapter's interruption, and suspends the agent on `credentials`.
#[tokio::test]
async fn a_refused_sign_in_overrides_an_open_wait_and_interruption() {
    let (handover, ready) = tokio::sync::oneshot::channel();
    let (finish, finished) = tokio::sync::oneshot::channel();
    let engine = Arc::new(SilentBridgeEngine {
        handover: Mutex::new(Some(handover)),
        finish: Mutex::new(Some(finished)),
        outcome: EngineOutcome {
            is_error: true,
            interrupted: true,
            error: Some("OAuth token has expired · Please run /login".into()),
            ..Default::default()
        },
    });
    let h = harness_with_engine(
        engine,
        Arc::new(Mutex::new(vec![])),
        Arc::new(Mutex::new(vec![])),
    )
    .await;
    let (agent, _) = h
        .runtime
        .create_agent(
            CreateAgentSpec {
                role: "worker".into(),
                objective: Some("keep work".into()),
                start: true,
                config: Some(AgentConfig {
                    approval: ApprovalPolicy::none(),
                    ..Default::default()
                }),
                ..Default::default()
            },
            Actor::user(),
        )
        .unwrap();
    let (id, token) = ready.await.unwrap();
    let (reply, is_error) = h
        .runtime
        .engine_tool(
            id,
            &token,
            "wait_for".into(),
            serde_json::json!({"timeout_seconds":3600}),
        )
        .await
        .unwrap();
    assert!(!is_error, "{reply}");
    finish.send(()).unwrap();
    let ended = wait_for(h.store.as_ref(), agent.id, |agent| {
        agent.status.kind() == AgentStatusKind::Blocked || agent.status.is_terminal()
    })
    .await;
    assert!(
        matches!(
            ended.status,
            AgentStatus::Blocked {
                reason: BlockReason::Exhausted {
                    class: ExhaustionClass::Credentials,
                    ..
                }
            }
        ),
        "{:?}",
        ended.status
    );
    let task = h.store.task(agent.current_task.unwrap()).unwrap().unwrap();
    assert!(
        !task.status.is_terminal(),
        "a suspended agent keeps its task"
    );
    h.runtime.shutdown().await;
}

#[tokio::test]
async fn native_exhaustion_reaches_root_on_cancel_and_budget_exits() {
    for budget in [false, true] {
        let (handover, ready) = tokio::sync::oneshot::channel();
        let (finish, finished) = tokio::sync::oneshot::channel();
        let engine = Arc::new(SilentBridgeEngine {
            handover: Mutex::new(Some(handover)),
            finish: Mutex::new(Some(finished)),
            outcome: EngineOutcome {
                is_error: true,
                error: Some("Weekly limit reached".into()),
                ..Default::default()
            },
        });
        let h = harness_with_engine(
            engine,
            Arc::new(Mutex::new(vec![])),
            Arc::new(Mutex::new(vec![])),
        )
        .await;
        let (root, _) = h
            .runtime
            .create_agent(
                CreateAgentSpec {
                    role: "root".into(),
                    start: false,
                    ..Default::default()
                },
                Actor::user(),
            )
            .unwrap();
        let (agent, _) = h
            .runtime
            .create_agent(
                CreateAgentSpec {
                    role: "worker".into(),
                    parent: Some(root.id),
                    objective: Some("work".into()),
                    start: true,
                    config: Some(AgentConfig {
                        effort: Some(Effort::Low),
                        token_budget: budget.then_some(40_000),
                        approval: ApprovalPolicy::none(),
                        ..Default::default()
                    }),
                    ..Default::default()
                },
                Actor::agent(root.id),
            )
            .unwrap();
        let (id, token) = ready.await.unwrap();
        if budget {
            let (mut child, _) = h
                .runtime
                .create_agent(
                    CreateAgentSpec {
                        role: "spender".into(),
                        parent: Some(id),
                        start: false,
                        config: Some(AgentConfig {
                            effort: Some(Effort::Low),
                            ..Default::default()
                        }),
                        ..Default::default()
                    },
                    Actor::user(),
                )
                .unwrap();
            child.stats.usage.input_tokens = 41_000;
            h.store
                .commit(
                    aether_store::Commit::new().mutate(aether_store::Mutation::UpsertAgent(child)),
                )
                .unwrap();
            let (reply, is_error) = h
                .runtime
                .engine_tool(
                    id,
                    &token,
                    "remember".into(),
                    serde_json::json!({"text":"unused", "kind":"fact", "scope":"agent"}),
                )
                .await
                .unwrap();
            assert!(is_error && reply.contains("token budget"), "{reply}");
        } else {
            h.runtime
                .cancel_agent(id, Some("operator stopped".into()), Actor::user())
                .unwrap();
        }
        finish.send(()).unwrap();
        wait_for(h.store.as_ref(), agent.id, |agent| {
            agent.stats.turns > 0 && agent.status.is_terminal()
        })
        .await;
        let events = h
            .store
            .latest_events(100, &EventFilter::own_events(id))
            .unwrap();
        assert!(
            events.iter().any(|event| matches!(&event.kind,
            EventKind::Custom { name, .. } if name == "exhaustion.reported")),
            "budget={budget}"
        );
        let inbox = h.store.inbox(root.id, 0).unwrap();
        assert!(inbox.iter().any(|item| matches!(&item.kind,
            aether_core::inbox::InboxKind::ExternalTrigger { source, payload, operator: false }
                if source == "exhaustion" && payload["category"] == "usage_limit")));
        let ended = h.store.agent(id).unwrap().unwrap();
        if budget {
            assert!(
                matches!(ended.status, AgentStatus::Failed { error } if error.contains("token budget"))
            );
        } else {
            assert_eq!(ended.status.kind(), AgentStatusKind::Cancelled);
        }
        h.runtime.shutdown().await;
    }
}
