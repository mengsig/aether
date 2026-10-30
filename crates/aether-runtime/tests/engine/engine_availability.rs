//! An agent's engine that is briefly unavailable - a launch the OS refused
//! for now, a probe that failed under load - is waited out, not failed.
use super::*;

/// Refuses its first `refusals` launches with `error`, then runs `inner`.
struct RefusingEngine {
    refusals: std::sync::atomic::AtomicUsize,
    error: fn() -> std::io::Error,
    inner: Arc<QueuedEngine>,
}

#[async_trait::async_trait]
impl Engine for RefusingEngine {
    fn kind(&self) -> EngineKind {
        EngineKind::CLAUDE_CODE
    }

    fn adapter(&self) -> String {
        "refusing/1".into()
    }

    async fn run(
        &self,
        turn: EngineTurn,
        cancel: CancellationToken,
    ) -> Result<EngineRun, EngineError> {
        let left = self.refusals.load(std::sync::atomic::Ordering::SeqCst);
        if left > 0 {
            self.refusals
                .store(left - 1, std::sync::atomic::Ordering::SeqCst);
            return Err(EngineError::Spawn {
                program: "claude".into(),
                source: (self.error)(),
            });
        }
        self.inner.run(turn, cancel).await
    }
}

async fn refused_launch(error: fn() -> std::io::Error) -> AgentStatus {
    let inner = QueuedEngine::new(vec![answered("done")]);
    let engine = Arc::new(RefusingEngine {
        refusals: 2.into(),
        error,
        inner: inner.clone(),
    });
    let h = harness_with_engine_config(engine, |_| {}).await;
    let id = chat_agent(&h);
    h.runtime
        .send_message(id, vec![ContentBlock::text("go")], Actor::user())
        .unwrap();
    let ended = wait_for(h.store.as_ref(), id, |a| {
        a.stats.turns > 0 && a.status.kind() != AgentStatusKind::Running
    })
    .await;
    let launched = inner.prompts.lock().unwrap().len();
    h.runtime.shutdown().await;
    assert!(launched <= 1, "{launched}");
    ended.status
}

#[tokio::test]
async fn a_launch_refused_for_now_is_retried() {
    for error in [
        || std::io::Error::from(std::io::ErrorKind::WouldBlock),
        || std::io::Error::from(std::io::ErrorKind::ExecutableFileBusy),
        || std::io::Error::from(std::io::ErrorKind::OutOfMemory),
    ] {
        let status = refused_launch(error).await;
        assert_eq!(status.kind(), AgentStatusKind::Waiting, "{status:?}");
    }
}

#[tokio::test]
async fn a_launch_denied_outright_still_fails() {
    let status =
        refused_launch(|| std::io::Error::from(std::io::ErrorKind::PermissionDenied)).await;
    assert!(matches!(status, AgentStatus::Failed { .. }), "{status:?}");
}
