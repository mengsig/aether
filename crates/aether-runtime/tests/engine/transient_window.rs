//! The transient retry window bounds one outage: real progress between two
//! failures starts a fresh one, Claude Code's synthetic error message does not.
use super::network_retry::{failures, session};
use super::*;

const SERVER: &str = "API Error: 500 Internal server error";

fn server_failure(prompt_not_submitted: bool) -> EngineEvent {
    EngineEvent::Done(EngineOutcome {
        is_error: true,
        error: Some(SERVER.into()),
        prompt_not_submitted,
        ..Default::default()
    })
}

/// A run that streamed a message from `model`, then hit the outage again.
fn progressed_then_failed(model: Option<&str>) -> Vec<EngineEvent> {
    vec![
        session("s"),
        EngineEvent::Assistant {
            content: vec![ContentBlock::text("working on it")],
            usage: None,
            model: model.map(str::to_owned),
        },
        server_failure(false),
    ]
}

/// The window outlasts one floor wait but not two, so only a reset after the
/// second failure leaves room for a third launch.
async fn run_twice_failing(second: Vec<EngineEvent>) -> (Harness, Arc<QueuedEngine>, AgentId) {
    let engine = QueuedEngine::new(vec![vec![server_failure(true)], second, answered("done")]);
    let h = harness_with_engine_config(engine.clone(), |config| {
        config.engine_transient_retry_window = Duration::from_millis(400);
        config.engine_transient_retry_floor = Duration::from_millis(300);
    })
    .await;
    let id = chat_agent(&h);
    h.runtime
        .send_message(id, vec![ContentBlock::text("go")], Actor::user())
        .unwrap();
    wait_for(h.store.as_ref(), id, |a| {
        a.stats.turns > 0 && a.status.kind() != AgentStatusKind::Running
    })
    .await;
    (h, engine, id)
}

#[tokio::test]
async fn progress_after_a_failure_gives_the_next_outage_its_own_window() {
    let (h, engine, id) = run_twice_failing(progressed_then_failed(None)).await;
    assert_eq!(engine.prompts.lock().unwrap().len(), 3);
    assert_eq!(
        failures(&h, id),
        [
            (aether_core::model::ModelErrorClass::Server, true),
            (aether_core::model::ModelErrorClass::Server, true),
        ]
    );
    let agent = h.store.agent(id).unwrap().unwrap();
    assert_eq!(agent.status.kind(), AgentStatusKind::Waiting);
    h.runtime.shutdown().await;
}

#[tokio::test]
async fn a_synthetic_error_message_is_not_progress() {
    let (h, engine, id) = run_twice_failing(progressed_then_failed(Some("<synthetic>"))).await;
    assert_eq!(
        engine.prompts.lock().unwrap().len(),
        2,
        "the window that opened at the first failure ran out"
    );
    let recorded = failures(&h, id);
    assert!(
        recorded.last().is_some_and(|(_, retryable)| !retryable),
        "{recorded:?}"
    );
    h.runtime.shutdown().await;
}

/// Progress ends at most eight outages of one turn (`MAX_RECOVERIES`): an
/// engine that answers a little and then fails every attempt still gives up.
#[tokio::test]
async fn an_engine_that_fails_after_every_bit_of_progress_still_gives_up() {
    const RECOVERIES: usize = 8;
    let runs = (0..40).map(|_| progressed_then_failed(None)).collect();
    let engine = QueuedEngine::new(runs);
    let h = harness_with_engine_config(engine.clone(), |config| {
        config.engine_transient_retry_window = Duration::from_millis(80);
        config.engine_transient_retry_floor = Duration::from_millis(20);
    })
    .await;
    let id = chat_agent(&h);
    h.runtime
        .send_message(id, vec![ContentBlock::text("go")], Actor::user())
        .unwrap();
    wait_for(h.store.as_ref(), id, |a| {
        a.stats.turns > 0 && a.status.kind() != AgentStatusKind::Running
    })
    .await;
    let launches = engine.prompts.lock().unwrap().len();
    assert!(
        (RECOVERIES + 1..=RECOVERIES + 6).contains(&launches),
        "{launches} launches"
    );
    let recorded = failures(&h, id);
    assert!(
        recorded.last().is_some_and(|(_, retryable)| !retryable),
        "{recorded:?}"
    );
    h.runtime.shutdown().await;
}
