//! A worker's native turn that answers in prose while its background jobs
//! still run keeps the task open and wakes the agent, at most twice a task.
use super::*;

use aether_core::agent::WaitCondition;
use aether_core::inbox::InboxKind;

/// [`QueuedEngine`], optionally steerable: a steerable run keeps the turn
/// attached after a result it gave while jobs ran, until its stream closes.
struct JobEngine {
    steerable: bool,
    runs: Mutex<std::collections::VecDeque<Vec<EngineEvent>>>,
    prompts: Mutex<Vec<String>>,
}

impl JobEngine {
    fn new(steerable: bool, runs: Vec<Vec<EngineEvent>>) -> Arc<Self> {
        Arc::new(Self {
            steerable,
            runs: Mutex::new(runs.into()),
            prompts: Mutex::new(Vec::new()),
        })
    }

    fn prompts(&self) -> Vec<String> {
        self.prompts.lock().unwrap().clone()
    }
}

#[async_trait::async_trait]
impl Engine for JobEngine {
    fn kind(&self) -> EngineKind {
        EngineKind::CLAUDE_CODE
    }

    fn adapter(&self) -> String {
        "jobs/1".into()
    }

    async fn run(
        &self,
        turn: EngineTurn,
        _cancel: CancellationToken,
    ) -> Result<EngineRun, EngineError> {
        let (tx, rx) = mpsc::channel(64);
        self.prompts.lock().unwrap().push(turn.prompt);
        let events = self.runs.lock().unwrap().pop_front().unwrap_or_default();
        let (input, input_rx) = mpsc::channel(8);
        tokio::spawn(async move {
            let _input_rx = input_rx;
            for event in events {
                if tx.send(event).await.is_err() {
                    return;
                }
            }
        });
        Ok(EngineRun {
            events: rx,
            input: self.steerable.then_some(input),
            decisions: None,
            stop: None,
        })
    }
}

fn job(id: &str, running: bool) -> EngineEvent {
    EngineEvent::BackgroundTask {
        id: id.into(),
        running,
    }
}

/// A run that starts job `ci` and answers while it still runs.
fn answered_over_job(text: &str) -> Vec<EngineEvent> {
    let mut run = vec![job("ci", true)];
    run.extend(answered(text));
    run
}

async fn job_harness(engine: Arc<JobEngine>) -> Harness {
    harness_with_engine_config(engine, |config| {
        config.native_job_wake_after = Duration::from_millis(300);
    })
    .await
}

fn job_notices(h: &Harness, agent: AgentId) -> Vec<String> {
    h.store
        .inbox(agent, 0)
        .unwrap()
        .into_iter()
        .filter_map(|item| match item.kind {
            InboxKind::Wake { reason } if reason.contains("native background jobs") => Some(reason),
            _ => None,
        })
        .collect()
}

fn deadline_waits(h: &Harness, agent: AgentId) -> usize {
    h.store
        .events(0, 10_000, &EventFilter::own_events(agent))
        .unwrap()
        .into_iter()
        .filter(|event| {
            matches!(&event.kind, EventKind::AgentWaiting { condition }
                if matches!(condition, WaitCondition::Deadline { .. }))
        })
        .count()
}

fn task_summary(h: &Harness, task: aether_core::id::TaskId) -> Option<String> {
    let task = h.store.task(task).unwrap().unwrap();
    task.result().map(|result| result.summary.clone())
}

#[tokio::test]
async fn an_answer_while_a_job_runs_keeps_the_task_open_until_the_bounded_wake() {
    let engine = JobEngine::new(
        false,
        vec![
            answered_over_job("`make ci` is running; I'll report when it finishes."),
            answered("CI passed."),
        ],
    );
    let h = job_harness(engine.clone()).await;
    let (agent, task) = engine_worker(&h, "run the checks");
    let waiting = wait_for(h.store.as_ref(), agent, |a| {
        a.stats.turns == 1 && a.status.kind() == AgentStatusKind::Waiting
    })
    .await;
    assert!(
        matches!(
            waiting.status,
            AgentStatus::Waiting {
                condition: WaitCondition::Deadline { .. }
            }
        ),
        "{:?}",
        waiting.status
    );
    assert_eq!(
        task_summary(&h, task),
        None,
        "the prose must not close the task"
    );
    let done = wait_for(h.store.as_ref(), agent, |a| a.status.is_terminal()).await;
    assert_eq!(
        done.status,
        AgentStatus::Completed {
            summary: Some("CI passed.".into())
        }
    );
    assert_eq!(task_summary(&h, task).as_deref(), Some("CI passed."));
    let notices = job_notices(&h, agent);
    assert_eq!(notices.len(), 1, "{notices:?}");
    assert!(notices[0].contains(&task.to_string()) && notices[0].contains("reminder 1 of 2"));
    assert_eq!(deadline_waits(&h, agent), 1);
    let prompts = engine.prompts();
    assert_eq!(prompts.len(), 2, "{prompts:?}");
    assert!(prompts[1].contains("reminder 1 of 2"), "{}", prompts[1]);
    h.runtime.shutdown().await;
}

#[tokio::test]
async fn a_job_that_finished_unanswered_wakes_the_agent_at_once() {
    let mut first = answered_over_job("CI started; I'll report.");
    first.push(job("ci", false));
    let engine = JobEngine::new(true, vec![first, answered("CI passed.")]);
    let h = job_harness(engine.clone()).await;
    let (agent, task) = engine_worker(&h, "run the checks");
    let done = wait_for(h.store.as_ref(), agent, |a| a.status.is_terminal()).await;
    assert_eq!(
        done.status,
        AgentStatus::Completed {
            summary: Some("CI passed.".into())
        }
    );
    assert_eq!(task_summary(&h, task).as_deref(), Some("CI passed."));
    assert_eq!(job_notices(&h, agent).len(), 1);
    assert_eq!(
        deadline_waits(&h, agent),
        0,
        "a finished job needs no bound"
    );
    assert_eq!(engine.prompts().len(), 2);
    h.runtime.shutdown().await;
}

#[tokio::test]
async fn the_third_answer_over_running_jobs_completes_the_task() {
    let engine = JobEngine::new(
        false,
        vec![
            answered_over_job("first: still running"),
            answered_over_job("second: still running"),
            answered_over_job("third: still running"),
        ],
    );
    let h = job_harness(engine.clone()).await;
    let (agent, task) = engine_worker(&h, "run the checks");
    let done = wait_for(h.store.as_ref(), agent, |a| a.status.is_terminal()).await;
    assert_eq!(
        done.status,
        AgentStatus::Completed {
            summary: Some("third: still running".into())
        }
    );
    assert_eq!(
        task_summary(&h, task).as_deref(),
        Some("third: still running")
    );
    let notices = job_notices(&h, agent);
    assert_eq!(notices.len(), 2, "{notices:?}");
    assert!(notices[0].contains("reminder 1 of 2") && notices[1].contains("reminder 2 of 2"));
    assert_eq!(engine.prompts().len(), 3);
    h.runtime.shutdown().await;
}

#[tokio::test]
async fn an_answer_after_its_jobs_finished_completes_as_before() {
    let mut run = vec![job("ci", true), job("ci", false)];
    run.extend(answered("CI passed."));
    for steerable in [false, true] {
        let engine = JobEngine::new(steerable, vec![run.clone()]);
        let h = job_harness(engine.clone()).await;
        let (agent, task) = engine_worker(&h, "run the checks");
        let done = wait_for(h.store.as_ref(), agent, |a| a.status.is_terminal()).await;
        assert_eq!(done.stats.turns, 1, "steerable {steerable}");
        assert_eq!(task_summary(&h, task).as_deref(), Some("CI passed."));
        assert!(job_notices(&h, agent).is_empty());
        assert_eq!(engine.prompts().len(), 1);
        h.runtime.shutdown().await;
    }
}

#[tokio::test]
async fn a_long_lived_agent_answering_over_a_job_waits_on_its_inbox_as_before() {
    let engine = JobEngine::new(false, vec![answered_over_job("CI is running.")]);
    let h = job_harness(engine.clone()).await;
    let (agent, task) = h
        .runtime
        .create_agent(
            CreateAgentSpec {
                role: "assistant".into(),
                objective: Some("run the checks".into()),
                model: Some(ModelRef::new("claude-code", "haiku")),
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
        .expect("create");
    let waiting = wait_for(h.store.as_ref(), agent.id, |a| {
        a.stats.turns == 1 && a.status.kind() == AgentStatusKind::Waiting
    })
    .await;
    assert_eq!(
        waiting.status,
        AgentStatus::Waiting {
            condition: WaitCondition::Inbox
        }
    );
    assert_eq!(task_summary(&h, task.expect("task").id), None);
    assert!(job_notices(&h, agent.id).is_empty());
    assert_eq!(engine.prompts().len(), 1);
    h.runtime.shutdown().await;
}
