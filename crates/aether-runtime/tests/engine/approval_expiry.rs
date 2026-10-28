//! An approval an engine turn waits on ends with that turn: interrupted,
//! cancelled or left by an engine that exited, the turn leaves it denied by
//! the system, never pending.
use super::*;
use aether_core::tool::{ApprovalDecision, ApprovalStatus, SideEffectClass};

/// Asks for one gated call on its first turn and holds it until cancelled,
/// or with `exits` stops taking answers and closes its stream at once;
/// later turns answer at once.
struct AskThenHoldEngine {
    first: std::sync::atomic::AtomicBool,
    exits: bool,
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
        let exits = self.exits;
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
                if exits {
                    // Gone without a word: no answer can reach it now.
                    return;
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

async fn gated_agent_waiting_on_approval(
    exits: bool,
) -> (Harness, AgentId, aether_core::id::ApprovalId) {
    let h = harness_with_engine(
        Arc::new(AskThenHoldEngine {
            first: std::sync::atomic::AtomicBool::new(true),
            exits,
        }),
        Arc::default(),
        Arc::default(),
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
    // Read from the log: an engine that exits may see it expired at once.
    let raised = || {
        h.store
            .events(0, 10_000, &EventFilter::for_agent(agent.id))
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
    assert_eq!(stored.agent_id, agent.id);
    (h, agent.id, approval)
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
    let (h, agent, approval) = gated_agent_waiting_on_approval(false).await;
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
    let (h, agent, approval) = gated_agent_waiting_on_approval(false).await;
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
    let (h, agent, approval) = gated_agent_waiting_on_approval(true).await;
    assert_expired(&h, agent, approval).await;
    h.runtime.shutdown().await;
}
