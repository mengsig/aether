//! A native turn that goes silent is interrupted and resumed, then fails
//! visibly; a running tool keeps the watchdog quiet.
use super::network_retry::{failure, session};
use super::*;

/// What one launch of the stall engine does after sending its events.
#[derive(Default)]
enum Ending {
    /// Close the stream: the run is over.
    #[default]
    Finish,
    /// Stay silent, stream open, until the runtime stops the process.
    Silent,
    /// Stay silent for this long, then send the rest and finish.
    Pause(Duration, Vec<EngineEvent>),
}

struct StallEngine {
    runs: Mutex<std::collections::VecDeque<(Vec<EngineEvent>, Ending)>>,
    sessions: Mutex<Vec<Option<String>>>,
    prompts: Mutex<Vec<String>>,
}

impl StallEngine {
    fn new(runs: Vec<(Vec<EngineEvent>, Ending)>) -> Arc<Self> {
        Arc::new(Self {
            runs: Mutex::new(runs.into()),
            sessions: Mutex::default(),
            prompts: Mutex::default(),
        })
    }
}

#[async_trait::async_trait]
impl Engine for StallEngine {
    fn kind(&self) -> EngineKind {
        EngineKind::CLAUDE_CODE
    }

    fn adapter(&self) -> String {
        "stall/1".into()
    }

    async fn run(
        &self,
        turn: EngineTurn,
        cancel: CancellationToken,
    ) -> Result<EngineRun, EngineError> {
        let (tx, rx) = mpsc::channel(64);
        self.sessions.lock().unwrap().push(turn.session);
        self.prompts.lock().unwrap().push(turn.prompt);
        let (events, ending) = self.runs.lock().unwrap().pop_front().unwrap_or_default();
        tokio::spawn(async move {
            for event in events {
                if tx.send(event).await.is_err() {
                    return;
                }
            }
            match ending {
                Ending::Finish => {}
                Ending::Silent => cancel.cancelled().await,
                Ending::Pause(pause, rest) => {
                    tokio::time::sleep(pause).await;
                    for event in rest {
                        if tx.send(event).await.is_err() {
                            return;
                        }
                    }
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

async fn stalling(runs: Vec<(Vec<EngineEvent>, Ending)>) -> (Harness, Arc<StallEngine>, AgentId) {
    let engine = StallEngine::new(runs);
    let h = harness_with_engine_config(engine.clone(), |config| {
        config.engine_stall_timeout = Duration::from_millis(150);
    })
    .await;
    let id = chat_agent(&h);
    (h, engine, id)
}

fn stalls(h: &Harness, id: AgentId) -> Vec<serde_json::Value> {
    h.store
        .events(0, 10_000, &EventFilter::own_events(id))
        .unwrap()
        .into_iter()
        .filter_map(|event| match event.kind {
            EventKind::Custom { name, payload } if name == "agent.stalled" => Some(payload),
            _ => None,
        })
        .collect()
}

fn tool_call(id: &str) -> EngineEvent {
    EngineEvent::Assistant {
        content: vec![ContentBlock::ToolUse {
            id: id.into(),
            name: "Bash".into(),
            input: serde_json::json!({"command": "make ci"}),
        }],
        usage: None,
        model: None,
    }
}

#[tokio::test]
async fn a_silent_engine_is_interrupted_and_its_session_resumed() {
    let mut finish = answered("recovered");
    finish.insert(0, session("kept"));
    let (h, engine, id) = stalling(vec![
        (vec![session("kept")], Ending::Silent),
        (finish, Ending::Finish),
    ])
    .await;
    engine_turn(&h, id, "do the work").await;
    let stalled = stalls(&h, id);
    assert_eq!(stalled.len(), 1, "{stalled:?}");
    assert_eq!(stalled[0]["engine"], "claude-code");
    assert_eq!(stalled[0]["attempt"], 1);
    assert_eq!(stalled[0]["agent"], id.to_string());
    assert!(stalled[0]["silent_secs"].is_u64() && stalled[0]["model"].is_string());
    assert_eq!(
        *engine.sessions.lock().unwrap(),
        [None, Some("kept".to_owned())],
        "the retry resumes the session the stalled run opened"
    );
    assert!(engine.prompts.lock().unwrap()[1].contains("do the work"));
    h.runtime.shutdown().await;
}

#[tokio::test]
async fn an_engine_that_keeps_stalling_ends_the_turn_with_an_actionable_error() {
    let runs = (0..10)
        .map(|_| (vec![session("kept")], Ending::Silent))
        .collect();
    let (h, engine, id) = stalling(runs).await;
    h.runtime
        .send_message(id, vec![ContentBlock::text("do the work")], Actor::user())
        .unwrap();
    let ended = wait_for(h.store.as_ref(), id, |a| {
        a.stats.turns > 0 && a.status.kind() != AgentStatusKind::Running
    })
    .await;
    assert_eq!(stalls(&h, id).len(), 3, "the first stall plus two retries");
    assert_eq!(engine.prompts.lock().unwrap().len(), 3);
    let events = h
        .store
        .events(0, 10_000, &EventFilter::own_events(id))
        .unwrap();
    let (said, class) = events
        .iter()
        .filter_map(|event| match &event.kind {
            EventKind::InvocationFailed {
                error,
                class,
                retryable: false,
                ..
            } => Some((error.clone(), *class)),
            _ => None,
        })
        .next_back()
        .expect("a terminal failure is recorded");
    assert_eq!(class, Some(aether_core::model::ModelErrorClass::Timeout));
    assert!(
        said.contains("engine stalled") && said.contains("message it to resume"),
        "{said}"
    );
    assert_eq!(
        ended.status.kind(),
        AgentStatusKind::Waiting,
        "the agent stays recoverable"
    );
    h.runtime.shutdown().await;
}

#[tokio::test]
async fn a_stalled_agent_leaves_no_problem_once_cancelled() {
    let runs = (0..10)
        .map(|_| (vec![session("kept")], Ending::Silent))
        .collect();
    let (h, _engine, id) = stalling(runs).await;
    h.runtime
        .send_message(id, vec![ContentBlock::text("do the work")], Actor::user())
        .unwrap();
    wait_for(h.store.as_ref(), id, |a| {
        a.stats.turns > 0 && a.status.kind() != AgentStatusKind::Running
    })
    .await;
    let mentions = |h: &Harness| {
        let problems = h.runtime.problems().unwrap();
        problems.iter().any(|p| p.contains(&id.to_string()))
    };
    assert!(
        mentions(&h),
        "the gave-up stall is reported while the agent lives"
    );
    h.runtime
        .cancel_agent(id, Some("gave up".into()), Actor::user())
        .unwrap();
    let deadline = std::time::Instant::now() + Duration::from_secs(5);
    while mentions(&h) {
        assert!(
            std::time::Instant::now() < deadline,
            "a cancelled agent's stall outlived it: {:?}",
            h.runtime.problems()
        );
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    h.runtime.shutdown().await;
}

#[tokio::test]
async fn a_tool_that_runs_longer_than_the_timeout_is_not_a_stall() {
    let mut finish = vec![EngineEvent::ToolResult {
        tool_use_id: "slow".into(),
        content: "done".into(),
        is_error: false,
    }];
    finish.extend(answered("built"));
    let (h, _engine, id) = stalling(vec![(
        vec![session("kept"), tool_call("slow")],
        Ending::Pause(Duration::from_millis(600), finish),
    )])
    .await;
    engine_turn(&h, id, "run the build").await;
    assert!(stalls(&h, id).is_empty());
    h.runtime.shutdown().await;
}

#[tokio::test]
async fn a_failure_the_engine_reports_itself_is_not_a_stall() {
    let (h, _engine, id) = stalling(vec![(
        vec![session("kept"), failure("boom")],
        Ending::Finish,
    )])
    .await;
    h.runtime
        .send_message(id, vec![ContentBlock::text("go")], Actor::user())
        .unwrap();
    wait_for(h.store.as_ref(), id, |a| {
        a.stats.turns > 0 && a.status.kind() != AgentStatusKind::Running
    })
    .await;
    assert!(stalls(&h, id).is_empty());
    h.runtime.shutdown().await;
}
