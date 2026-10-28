//! An agent admitted with approvals never runs on an engine that cannot ask
//! before acting, as a Codex CLI replaced by an exec-only build after the
//! agent was created would be.
use super::*;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};

/// Answers every turn; `gates` says whether it asks before acting.
struct SwappableEngine {
    gates: AtomicBool,
    runs: AtomicUsize,
}

#[async_trait::async_trait]
impl Engine for SwappableEngine {
    fn kind(&self) -> EngineKind {
        EngineKind::CLAUDE_CODE
    }
    fn adapter(&self) -> String {
        "swappable/1".into()
    }
    fn gates_permissions(&self) -> bool {
        self.gates.load(Ordering::SeqCst)
    }
    async fn run(
        &self,
        _turn: EngineTurn,
        _cancel: CancellationToken,
    ) -> Result<EngineRun, EngineError> {
        self.runs.fetch_add(1, Ordering::SeqCst);
        let (tx, rx) = mpsc::channel(4);
        tokio::spawn(async move {
            for event in answered("done") {
                if tx.send(event).await.is_err() {
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

#[tokio::test]
async fn a_gated_agent_is_refused_by_an_engine_that_cannot_ask() {
    let engine = Arc::new(SwappableEngine {
        gates: AtomicBool::new(true),
        runs: AtomicUsize::new(0),
    });
    let h = harness_with_engine(engine.clone(), Arc::default(), Arc::default()).await;
    let gated = ApprovalPolicy::default();
    assert!(!gated.is_yolo(), "the default policy gates calls");
    let (agent, _) = h
        .runtime
        .create_agent(
            CreateAgentSpec {
                role: "assistant".into(),
                start: true,
                approval: Some(gated.clone()),
                config: Some(AgentConfig {
                    long_lived: true,
                    ..Default::default()
                }),
                ..Default::default()
            },
            Actor::user(),
        )
        .expect("create");
    assert_eq!(
        agent.config.approval, gated,
        "a gating engine keeps the policy"
    );

    // The CLI is replaced by one that runs every call without asking.
    engine.gates.store(false, Ordering::SeqCst);
    h.runtime
        .send_message(agent.id, vec![ContentBlock::text("deploy")], Actor::user())
        .expect("send");
    // Settled either way: refused, or back waiting after a turn that ran.
    let stopped = wait_for(h.store.as_ref(), agent.id, |a| {
        !a.is_live() || (a.stats.turns > 0 && a.status.kind() == AgentStatusKind::Waiting)
    })
    .await;
    let AgentStatus::Failed { error } = &stopped.status else {
        panic!("the turn is refused, not {:?}", stopped.status);
    };
    assert!(error.contains("cannot ask before acting"), "{error}");
    assert!(error.contains("--approval none"), "{error}");
    assert_eq!(
        engine.runs.load(Ordering::SeqCst),
        0,
        "the engine never ran"
    );
    assert_eq!(stopped.config.approval, gated, "the policy is kept");
}
