//! A native tool call that never returns is bounded: past the bound the turn
//! is interrupted, the reason recorded, and the inbox served by a fresh turn.
//! A call the engine keeps showing life for is left alone.
use std::collections::VecDeque;
use std::time::Instant;

use super::network_retry::session;
use super::*;

const BOUND: Duration = Duration::from_millis(400);

/// One step of a scripted launch.
enum Step {
    Send(Box<EngineEvent>),
    Sleep(Duration),
    /// Hold the stream open, silent, until the runtime stops the process.
    Hang,
    /// Like `Hang`, then take this long to exit once stopped.
    HangThenLinger(Duration),
}

fn send(event: EngineEvent) -> Step {
    Step::Send(Box::new(event))
}

/// Runs one script per launch. It takes steered input and never answers it,
/// like a CLI stuck inside a tool call.
struct BoundEngine {
    runs: Mutex<VecDeque<Vec<Step>>>,
    prompts: Mutex<Vec<String>>,
    /// When each launch started, and when each one's stream closed.
    launched: Mutex<Vec<Instant>>,
    closed: Arc<Mutex<Vec<Instant>>>,
}

#[async_trait::async_trait]
impl Engine for BoundEngine {
    fn kind(&self) -> EngineKind {
        EngineKind::CLAUDE_CODE
    }

    fn adapter(&self) -> String {
        "bound/1".into()
    }

    async fn run(
        &self,
        turn: EngineTurn,
        cancel: CancellationToken,
    ) -> Result<EngineRun, EngineError> {
        let (tx, rx) = mpsc::channel(64);
        let (input, mut steered) = mpsc::channel(8);
        self.prompts.lock().unwrap().push(turn.prompt);
        self.launched.lock().unwrap().push(Instant::now());
        let closed = self.closed.clone();
        let steps = self
            .runs
            .lock()
            .unwrap()
            .pop_front()
            .unwrap_or_else(|| answered("unscripted").into_iter().map(send).collect());
        tokio::spawn(async move {
            // Read and dropped: the engine never acknowledges what it is handed.
            tokio::spawn(async move { while steered.recv().await.is_some() {} });
            for step in steps {
                match step {
                    Step::Send(event) => {
                        if tx.send(*event).await.is_err() {
                            break;
                        }
                    }
                    Step::Sleep(pause) => tokio::time::sleep(pause).await,
                    Step::Hang => cancel.cancelled().await,
                    Step::HangThenLinger(exit) => {
                        cancel.cancelled().await;
                        tokio::time::sleep(exit).await;
                    }
                }
            }
            drop(tx);
            closed.lock().unwrap().push(Instant::now());
        });
        Ok(EngineRun {
            events: rx,
            input: Some(input),
            decisions: None,
            stop: None,
        })
    }
}

async fn bounded(runs: Vec<Vec<Step>>) -> (Harness, Arc<BoundEngine>, AgentId) {
    let engine = Arc::new(BoundEngine {
        runs: Mutex::new(runs.into()),
        prompts: Mutex::default(),
        launched: Mutex::default(),
        closed: Arc::default(),
    });
    let h = harness_with_engine_config(engine.clone(), |config| {
        config.tool_call_bound = BOUND;
    })
    .await;
    let id = chat_agent(&h);
    (h, engine, id)
}

fn customs(h: &Harness, id: AgentId, name: &str) -> Vec<serde_json::Value> {
    h.store
        .events(0, 10_000, &EventFilter::own_events(id))
        .unwrap()
        .into_iter()
        .filter_map(|event| match event.kind {
            EventKind::Custom { name: n, payload } if n == name => Some(payload),
            _ => None,
        })
        .collect()
}

fn overdue(h: &Harness, id: AgentId) -> Vec<serde_json::Value> {
    customs(h, id, "agent.tool_call_overdue")
}

fn tool_use(id: &str, name: &str) -> EngineEvent {
    EngineEvent::Assistant {
        content: vec![ContentBlock::ToolUse {
            id: id.into(),
            name: name.into(),
            input: serde_json::json!({"command": "make ci"}),
        }],
        usage: None,
        model: None,
    }
}

fn result(id: &str) -> EngineEvent {
    EngineEvent::ToolResult {
        tool_use_id: id.into(),
        content: "done".into(),
        is_error: false,
    }
}

fn job(id: &str, running: bool) -> EngineEvent {
    EngineEvent::BackgroundTask {
        id: id.into(),
        running,
    }
}

fn hung(call: &str) -> Vec<Step> {
    vec![send(session("s")), send(tool_use(call, "Bash")), Step::Hang]
}

/// A launch that runs `steps` and then answers `text`.
fn then_answers(mut steps: Vec<Step>, text: &str) -> Vec<Step> {
    steps.extend(answered(text).into_iter().map(send));
    steps
}

async fn turn_count_settles(h: &Harness, id: AgentId, turns: u32) -> aether_core::agent::Agent {
    wait_for(h.store.as_ref(), id, |a| {
        a.stats.turns >= turns && a.status.kind() == AgentStatusKind::Waiting
    })
    .await
}

#[tokio::test]
async fn a_call_that_never_returns_is_stopped_and_the_inbox_served_in_a_fresh_turn() {
    let (h, engine, id) = bounded(vec![hung("hung"), then_answers(Vec::new(), "recovered")]).await;
    let started = Instant::now();
    h.runtime
        .send_message(id, vec![ContentBlock::text("run the build")], Actor::user())
        .unwrap();
    wait_until("the hung call", || {
        (count(h.store.as_ref(), id, "tool.requested") > 0).then_some(())
    })
    .await;
    h.runtime
        .send_message(
            id,
            vec![ContentBlock::text("also check the logs")],
            Actor::user(),
        )
        .unwrap();
    let agent = turn_count_settles(&h, id, 2).await;
    assert!(started.elapsed() >= BOUND, "stopped before the bound");

    let stopped = overdue(&h, id);
    assert_eq!(stopped.len(), 1, "{stopped:?}");
    assert_eq!(stopped[0]["tool"], "Bash");
    assert_eq!(stopped[0]["resumed"], true);
    assert_eq!(stopped[0]["agent"], id.to_string());
    let interrupted = customs(&h, id, "engine.interrupted");
    assert_eq!(interrupted.len(), 1, "{interrupted:?}");
    let reason = interrupted[0]["reason"].as_str().unwrap();
    assert!(
        reason.contains("tool-call bound") && reason.contains("Bash"),
        "{reason}"
    );
    let cancelled: Vec<String> = h
        .store
        .events(0, 10_000, &EventFilter::own_events(id))
        .unwrap()
        .into_iter()
        .filter_map(|event| match event.kind {
            EventKind::ToolCancelled { call } => Some(call.to_string()),
            _ => None,
        })
        .collect();
    assert_eq!(
        cancelled,
        [stopped[0]["call"].as_str().unwrap()],
        "the hung call is closed"
    );

    let prompts = engine.prompts.lock().unwrap().clone();
    assert_eq!(prompts.len(), 2, "{prompts:?}");
    assert!(!prompts[0].contains("also check the logs"));
    assert!(
        prompts[1].contains("also check the logs")
            && prompts[1].contains("Aether stopped your turn"),
        "the fresh turn carries the queued message and the wake: {}",
        prompts[1]
    );
    assert_eq!(
        agent.inbox_cursor,
        h.store.inbox_last_seq(id).unwrap(),
        "nothing left unread"
    );
    h.runtime.shutdown().await;
}

#[tokio::test]
async fn a_turn_that_hangs_again_on_its_own_wake_waits_for_a_message() {
    let (h, engine, id) = bounded(vec![
        hung("first"),
        hung("second"),
        then_answers(Vec::new(), "back"),
    ])
    .await;
    h.runtime
        .send_message(id, vec![ContentBlock::text("run the build")], Actor::user())
        .unwrap();
    wait_until("both stops", || (overdue(&h, id).len() == 2).then_some(())).await;
    let agent = turn_count_settles(&h, id, 2).await;
    tokio::time::sleep(BOUND).await;

    let stopped = overdue(&h, id);
    assert_eq!(stopped.len(), 2, "{stopped:?}");
    assert_eq!(stopped[0]["resumed"], true);
    assert_eq!(
        stopped[1]["resumed"], false,
        "a second stop in a row does not wake again"
    );
    assert_eq!(
        engine.prompts.lock().unwrap().len(),
        2,
        "no third turn on its own"
    );
    assert_eq!(
        agent.status.kind(),
        AgentStatusKind::Waiting,
        "the agent stays recoverable"
    );
    assert_eq!(agent.inbox_cursor, h.store.inbox_last_seq(id).unwrap());

    engine_turn(&h, id, "try again").await;
    assert_eq!(
        engine.prompts.lock().unwrap().len(),
        3,
        "a message resumes it"
    );
    h.runtime.shutdown().await;
}

#[tokio::test]
async fn the_fresh_turn_starts_only_once_the_stopped_engine_has_exited() {
    let linger = Duration::from_millis(600);
    let stuck = vec![
        send(session("s")),
        send(tool_use("hung", "Bash")),
        Step::HangThenLinger(linger),
    ];
    let (h, engine, id) = bounded(vec![stuck, then_answers(Vec::new(), "recovered")]).await;
    h.runtime
        .send_message(id, vec![ContentBlock::text("run the build")], Actor::user())
        .unwrap();
    turn_count_settles(&h, id, 2).await;
    assert_eq!(overdue(&h, id).len(), 1);
    let launched = engine.launched.lock().unwrap().clone();
    let closed = engine.closed.lock().unwrap().clone();
    assert_eq!(launched.len(), 2, "{launched:?}");
    assert!(
        closed[0] <= launched[1],
        "the wake's turn launched {:?} before the stopped engine exited",
        closed[0] - launched[1]
    );
    assert!(closed[0] - launched[0] >= BOUND + linger);
    h.runtime.shutdown().await;
}

#[tokio::test]
async fn a_long_call_the_engine_shows_life_for_is_not_cut() {
    let progress = || {
        send(EngineEvent::Assistant {
            content: vec![ContentBlock::text("subagent: still building")],
            usage: None,
            model: None,
        })
    };
    let mut steps = vec![send(session("s")), send(tool_use("long", "Agent"))];
    for _ in 0..12 {
        steps.push(Step::Sleep(BOUND / 4));
        steps.push(progress());
    }
    steps.push(send(result("long")));
    let (h, engine, id) = bounded(vec![then_answers(steps, "built")]).await;
    let started = Instant::now();
    engine_turn(&h, id, "run the build").await;
    assert!(
        started.elapsed() > BOUND * 2,
        "the call ran well past the bound"
    );
    assert!(overdue(&h, id).is_empty(), "{:?}", overdue(&h, id));
    assert!(customs(&h, id, "engine.interrupted").is_empty());
    assert_eq!(engine.prompts.lock().unwrap().len(), 1);
    assert!(
        spoken(h.store.as_ref(), id)
            .iter()
            .any(|(_, text)| text.contains("built"))
    );
    h.runtime.shutdown().await;
}

#[tokio::test]
async fn a_background_job_past_the_bound_is_not_cut() {
    let steps = vec![
        send(session("s")),
        send(tool_use("launch", "Bash")),
        send(job("build", true)),
        send(result("launch")),
        Step::Sleep(BOUND * 2),
        send(job("build", false)),
    ];
    let (h, engine, id) = bounded(vec![then_answers(steps, "built")]).await;
    engine_turn(&h, id, "run the build").await;
    assert!(overdue(&h, id).is_empty(), "{:?}", overdue(&h, id));
    assert_eq!(engine.prompts.lock().unwrap().len(), 1);
    h.runtime.shutdown().await;
}

#[tokio::test]
async fn a_command_the_engine_reports_live_past_the_bound_is_not_cut() {
    // Codex's shape: a command is a live process under its call's own id.
    let steps = vec![
        send(session("s")),
        send(job("cmd", true)),
        send(tool_use("cmd", "Bash")),
        Step::Sleep(BOUND * 2),
        send(result("cmd")),
        send(job("cmd", false)),
    ];
    let (h, engine, id) = bounded(vec![then_answers(steps, "built")]).await;
    engine_turn(&h, id, "run the build").await;
    assert!(overdue(&h, id).is_empty(), "{:?}", overdue(&h, id));
    assert!(customs(&h, id, "engine.interrupted").is_empty());
    assert_eq!(engine.prompts.lock().unwrap().len(), 1);
    h.runtime.shutdown().await;
}
