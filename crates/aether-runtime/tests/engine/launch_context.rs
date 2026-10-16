//! Identity reaches ordinary, resumed and checkpoint launches through the additive hook.
use super::*;
use aether_core::agent::Agent;
use aether_engine::EngineLaunchContext;

#[derive(Clone, Copy)]
struct Launch {
    summary_only: bool,
    resumed: bool,
    context: EngineLaunchContext,
}

struct ContextProbe {
    inner: CheckpointEngine,
    launches: Arc<Mutex<Vec<Launch>>>,
}

#[async_trait::async_trait]
impl Engine for ContextProbe {
    fn kind(&self) -> EngineKind {
        EngineKind::CLAUDE_CODE
    }
    fn adapter(&self) -> String {
        "context-probe/1".into()
    }
    async fn run(
        &self,
        _turn: EngineTurn,
        _cancel: CancellationToken,
    ) -> Result<EngineRun, EngineError> {
        panic!("runtime must use the context hook")
    }
    async fn run_with_context(
        &self,
        turn: EngineTurn,
        context: EngineLaunchContext,
        cancel: CancellationToken,
    ) -> Result<EngineRun, EngineError> {
        self.launches.lock().unwrap().push(Launch {
            summary_only: turn.summary_only,
            resumed: turn.session.is_some(),
            context,
        });
        self.inner.run(turn, cancel).await
    }
}

#[tokio::test]
async fn launch_identity_survives_resume_and_compaction() {
    let launches = Arc::new(Mutex::new(Vec::new()));
    let engine = Arc::new(ContextProbe {
        inner: CheckpointEngine {
            turns: Arc::default(),
            valid: true,
        },
        launches: launches.clone(),
    });
    let h = harness_with_engine(engine, Arc::default(), Arc::default()).await;
    let (agent, task) = create_task_agent(&h);
    let before = wait_for(h.store.as_ref(), agent.id, |a| {
        a.stats.turns >= 1 && a.status.kind() == AgentStatusKind::Waiting
    })
    .await;
    engine_turn(&h, agent.id, "remember ginger").await;
    h.runtime
        .request_compaction(agent.id, Actor::user())
        .unwrap();
    wait_for(h.store.as_ref(), agent.id, |a| {
        a.stats.turns > before.stats.turns + 1 && a.status.kind() == AgentStatusKind::Waiting
    })
    .await;
    let expected = EngineLaunchContext {
        run_id: agent.run,
        agent_id: agent.id,
        task_id: Some(task.unwrap().id),
    };
    let recorded = launches.lock().unwrap().clone();
    assert!(recorded.iter().any(|launch| launch.summary_only));
    assert!(
        recorded
            .iter()
            .any(|launch| !launch.summary_only && launch.resumed)
    );
    assert!(recorded.iter().all(|launch| launch.context == expected));
    h.runtime.shutdown().await;
}

fn create_task_agent(h: &Harness) -> (Agent, Option<aether_core::task::Task>) {
    let (agent, task) = h
        .runtime
        .create_agent(
            CreateAgentSpec {
                role: "assistant".into(),
                objective: Some("remember saffron".into()),
                start: true,
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
    assert!(task.is_some());
    assert!(agent.config.long_lived);
    (agent, task)
}
