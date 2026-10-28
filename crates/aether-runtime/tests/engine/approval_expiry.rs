//! An approval an engine turn waits on ends with that turn: interrupted,
//! cancelled or left by an engine that exited, whether it asked natively or
//! through its bridge, the turn leaves it denied by the system, never pending.
use super::*;
use aether_core::tool::{ApprovalDecision, ApprovalStatus, SideEffectClass};

/// How the engine that asked leaves its first turn.
#[derive(Clone, Copy, PartialEq)]
enum Leaves {
    /// Only once cancelled.
    WhenCancelled,
    /// At once, dropping its answer channel with its stream.
    DroppingAnswers,
    /// At once, reporting its result, while another task of it still reads
    /// answers, as an adapter that writes them to its process's stdin does.
    KeepingAnswers,
}

/// Asks for one gated call on its first turn, then leaves as `leaves` says;
/// later turns answer at once.
struct AskThenHoldEngine {
    first: std::sync::atomic::AtomicBool,
    leaves: Leaves,
}

#[async_trait::async_trait]
impl Engine for AskThenHoldEngine {
    fn kind(&self) -> EngineKind {
        EngineKind::CLAUDE_CODE
    }
    fn adapter(&self) -> String {
        "ask-then-hold/1".into()
    }
    async fn run(
        &self,
        _turn: EngineTurn,
        cancel: CancellationToken,
    ) -> Result<EngineRun, EngineError> {
        let (tx, rx) = mpsc::channel(4);
        let (decisions, mut answers) = mpsc::channel::<aether_engine::PermissionDecision>(1);
        let first = self.first.swap(false, std::sync::atomic::Ordering::SeqCst);
        let leaves = self.leaves;
        tokio::spawn(async move {
            if first {
                tx.send(EngineEvent::Permission {
                    request_id: "deploy".into(),
                    tool: "Bash".into(),
                    input: serde_json::json!({"command": "make deploy", "tool_use_id": "deploy-call"}),
                    side_effect: Some(SideEffectClass::External),
                })
                .await
                .unwrap();
                match leaves {
                    // Gone without a word: no answer can reach it now.
                    Leaves::DroppingAnswers => return,
                    Leaves::KeepingAnswers => {
                        tokio::spawn(async move { while answers.recv().await.is_some() {} });
                        let _ = tx
                            .send(EngineEvent::Done(EngineOutcome {
                                is_error: true,
                                error: Some("exited".into()),
                                ..Default::default()
                            }))
                            .await;
                        return;
                    }
                    Leaves::WhenCancelled => {}
                }
                tokio::select! {
                    answer = answers.recv() => assert!(answer.is_none_or(|a| !a.allow), "no answer may allow the call"),
                    () = cancel.cancelled() => {}
                }
                let _ = tx
                    .send(EngineEvent::Done(EngineOutcome {
                        interrupted: true,
                        ..Default::default()
                    }))
                    .await;
                return;
            }
            let _ = tx
                .send(EngineEvent::Done(EngineOutcome {
                    text: Some("done".into()),
                    ..Default::default()
                }))
                .await;
        });
        Ok(EngineRun {
            events: rx,
            input: None,
            decisions: Some(decisions),
            stop: None,
        })
    }
}

/// Asks nothing itself: on its first turn it hands out its bridge
/// credentials, then exits without a word once told to, while a permission
/// prompt its bridge sent still waits. Later turns answer at once.
struct BridgeAskThenExitEngine {
    credentials: Mutex<Option<tokio::sync::oneshot::Sender<(AgentId, String)>>>,
    exit: Arc<tokio::sync::Notify>,
}

#[async_trait::async_trait]
impl Engine for BridgeAskThenExitEngine {
    fn kind(&self) -> EngineKind {
        EngineKind::CLAUDE_CODE
    }
    fn adapter(&self) -> String {
        "bridge-ask-then-exit/1".into()
    }
    async fn run(
        &self,
        turn: EngineTurn,
        _cancel: CancellationToken,
    ) -> Result<EngineRun, EngineError> {
        let (tx, rx) = mpsc::channel(4);
        match self.credentials.lock().unwrap().take() {
            Some(hand_out) => {
                let args = &turn.mcp.as_ref().expect("a bridge for the turn").args;
                let after = |flag: &str| {
                    let at = args.iter().position(|arg| arg == flag).unwrap();
                    args[at + 1].clone()
                };
                hand_out
                    .send((after("--agent").parse().unwrap(), after("--token")))
                    .unwrap();
                let exit = self.exit.clone();
                tokio::spawn(async move {
                    exit.notified().await;
                    drop(tx);
                });
            }
            None => {
                tokio::spawn(async move {
                    let _ = tx
                        .send(EngineEvent::Done(EngineOutcome {
                            text: Some("done".into()),
                            ..Default::default()
                        }))
                        .await;
                });
            }
        }
        Ok(EngineRun {
            events: rx,
            input: None,
            decisions: None,
            stop: None,
        })
    }
}

async fn gated_agent_waiting_on_approval(
    leaves: Leaves,
) -> (Harness, AgentId, aether_core::id::ApprovalId) {
    let engine = Arc::new(AskThenHoldEngine {
        first: std::sync::atomic::AtomicBool::new(true),
        leaves,
    });
    let (h, agent) = gated_agent(engine).await;
    let approval = raised_approval(&h, agent).await;
    (h, agent, approval)
}

/// A started agent whose policy gates the engine's `make deploy`.
async fn gated_agent(engine: Arc<dyn Engine>) -> (Harness, AgentId) {
    let h = harness_with_engine(engine, Arc::default(), Arc::default()).await;
    let (agent, _) = h
        .runtime
        .create_agent(
            CreateAgentSpec {
                role: "assistant".into(),
                start: true,
                config: Some(AgentConfig {
                    long_lived: true,
                    // Not the default policy, which creation reads as unset.
                    approval: ApprovalPolicy {
                        require_for: [SideEffectClass::Mutation, SideEffectClass::External]
                            .into_iter()
                            .collect(),
                    },
                    ..Default::default()
                }),
                ..Default::default()
            },
            Actor::user(),
        )
        .expect("create");
    h.runtime
        .send_message(agent.id, vec![ContentBlock::text("deploy")], Actor::user())
        .unwrap();
    (h, agent.id)
}

async fn raised_approval(h: &Harness, agent: AgentId) -> aether_core::id::ApprovalId {
    // Read from the log: an engine that exits may see it expired at once.
    let raised = || {
        h.store
            .events(0, 10_000, &EventFilter::for_agent(agent))
            .unwrap()
            .into_iter()
            .find_map(|event| match event.kind {
                EventKind::ApprovalRequested { approval, .. } => Some(approval),
                _ => None,
            })
    };
    let approval = tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            if let Some(approval) = raised() {
                break approval;
            }
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    })
    .await
    .expect("approval raised within seconds");
    let stored = h.store.approval(approval).unwrap().expect("stored");
    assert_eq!(stored.agent_id, agent);
    approval
}

/// The approval left the lists, was denied by the system with a reason, and
/// can no longer be granted.
async fn assert_expired(h: &Harness, agent: AgentId, approval: aether_core::id::ApprovalId) {
    tokio::time::timeout(Duration::from_secs(5), async {
        while !h.store.pending_approvals().unwrap().is_empty() {
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    })
    .await
    .expect("the approval leaves the pending list when its turn ends");
    let resolved = h
        .store
        .events(0, 10_000, &EventFilter::for_agent(agent))
        .unwrap()
        .into_iter()
        .find_map(|event| match event.kind {
            EventKind::ApprovalResolved {
                approval: id,
                decision,
                by,
                note,
            } if id == approval => Some((decision, by, note)),
            _ => None,
        })
        .expect("the expiry is recorded");
    assert_eq!(resolved.0, ApprovalDecision::Denied);
    assert_eq!(resolved.1, Actor::System);
    assert!(resolved.2.unwrap().starts_with("turn ended"));
    assert!(matches!(
        h.store.approval(approval).unwrap().unwrap().status,
        ApprovalStatus::Resolved {
            decision: ApprovalDecision::Denied,
            ..
        }
    ));
    let late = h.runtime.resolve_approval(
        approval,
        ApprovalDecision::Approved,
        Actor::user(),
        None,
        None,
    );
    assert!(late.is_err(), "an expired approval cannot be granted later");
}

#[tokio::test]
async fn an_interrupt_expires_the_engine_approval_its_turn_waited_on() {
    let (h, agent, approval) = gated_agent_waiting_on_approval(Leaves::WhenCancelled).await;
    assert!(h.runtime.interrupt_agent(agent, Actor::user()).unwrap());
    assert_expired(&h, agent, approval).await;
    wait_for(h.store.as_ref(), agent, |a| {
        a.status.kind() == AgentStatusKind::Waiting
    })
    .await;
    h.runtime.shutdown().await;
}

#[tokio::test]
async fn cancelling_the_agent_expires_the_engine_approval_its_turn_waited_on() {
    let (h, agent, approval) = gated_agent_waiting_on_approval(Leaves::WhenCancelled).await;
    h.runtime
        .cancel_agent(agent, Some("stop".into()), Actor::user())
        .unwrap();
    assert_expired(&h, agent, approval).await;
    wait_for(h.store.as_ref(), agent, |a| {
        a.status.kind() == AgentStatusKind::Cancelled
    })
    .await;
    h.runtime.shutdown().await;
}

#[tokio::test]
async fn an_engine_that_exits_mid_wait_expires_the_approval_it_asked_for() {
    let (h, agent, approval) = gated_agent_waiting_on_approval(Leaves::DroppingAnswers).await;
    assert_expired(&h, agent, approval).await;
    h.runtime.shutdown().await;
}

#[tokio::test]
async fn an_engine_that_leaves_but_keeps_reading_answers_expires_the_approval() {
    let (h, agent, approval) = gated_agent_waiting_on_approval(Leaves::KeepingAnswers).await;
    assert_expired(&h, agent, approval).await;
    h.runtime.shutdown().await;
}

#[tokio::test]
async fn an_engine_that_exits_while_its_bridge_waits_expires_the_approval() {
    let (credentials, handed_out) = tokio::sync::oneshot::channel();
    let exit = Arc::new(tokio::sync::Notify::new());
    let engine = Arc::new(BridgeAskThenExitEngine {
        credentials: Mutex::new(Some(credentials)),
        exit: exit.clone(),
    });
    let (h, agent) = gated_agent(engine).await;
    let (bridged, token) = tokio::time::timeout(Duration::from_secs(5), handed_out)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(bridged, agent);
    // Like the daemon's dispatch, this keeps waiting for the reply after the
    // engine is gone, so only the engine leaving can end the approval.
    let runtime = h.runtime.clone();
    let prompt = tokio::spawn(async move {
        runtime
            .engine_tool(
                bridged,
                &token,
                "permission".into(),
                serde_json::json!({
                    "tool_name": "Bash",
                    "tool_use_id": "deploy-call",
                    "input": {"command": "make deploy"}
                }),
            )
            .await
    });
    let approval = raised_approval(&h, agent).await;
    exit.notify_one();
    assert_expired(&h, agent, approval).await;
    let (reply, _) = tokio::time::timeout(Duration::from_secs(5), prompt)
        .await
        .expect("the waiting bridge call is answered")
        .unwrap()
        .expect("answered, not dropped");
    assert_eq!(
        serde_json::from_str::<serde_json::Value>(&reply).unwrap()["behavior"],
        "deny"
    );
    h.runtime.shutdown().await;
}
