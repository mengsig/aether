//! Blank, unanswered and orphaned native-turn recovery.
use super::*;

fn blank_claude_receipt(dollars: Option<f64>) -> Vec<EngineEvent> {
    let mut receipt = serde_json::json!({"type":"result", "subtype":"success", "result":"",
        "session_id":"s1", "total_cost_usd":dollars, "modelUsage":{},
        "usage":{"input_tokens":0,"output_tokens":0}});
    if dollars.is_none() {
        receipt.as_object_mut().unwrap().remove("total_cost_usd");
    }
    let events = aether_engine::claude_code::parse_line(
        &receipt.to_string(),
        &mut aether_engine::claude_code::ParseState::default(),
    );
    assert!(matches!(events.last(), Some(EngineEvent::Done(outcome)) if !outcome.is_error));
    assert!(
        !events
            .iter()
            .any(|event| matches!(event, EngineEvent::Assistant { .. }))
    );
    events
}

#[tokio::test]
async fn a_blank_claude_receipt_records_an_unanswered_prompt() {
    for dollars in [Some(0.0), None] {
        let h = harness_with_engine(
            QueuedEngine::new(vec![blank_claude_receipt(dollars), answered("Built.")]),
            Arc::default(),
            Arc::default(),
        )
        .await;
        let (agent, _) = engine_worker(&h, "watch the build");
        let agent = wait_for(h.store.as_ref(), agent, |a| a.status.is_terminal()).await;
        assert!(matches!(agent.status, AgentStatus::Completed { .. }));
        let failures = failed_invocation_errors(h.store.as_ref(), agent.id);
        assert_eq!(failures.len(), 1, "{failures:?}");
        assert!(failures[0].contains("without running the model"));
        h.runtime.shutdown().await;
    }
}

#[tokio::test]
async fn the_retry_after_a_blank_claude_receipt_resends_the_task() {
    for dollars in [Some(0.0), None] {
        let mut first = vec![EngineEvent::Session {
            id: "s1".into(),
            model: None,
            tools: Vec::new(),
        }];
        first.extend(worked_then_fell_silent());
        let engine = QueuedEngine::new(vec![
            first,
            blank_claude_receipt(dollars),
            answered("Built."),
        ]);
        let h = harness_with_engine(engine.clone(), Arc::default(), Arc::default()).await;
        let (agent, _) = engine_worker(&h, "watch the build");
        wait_for(h.store.as_ref(), agent, |a| a.status.is_terminal()).await;
        let prompts = engine.prompts.lock().unwrap().clone();
        assert_eq!(prompts.len(), 3, "{prompts:?}");
        assert_eq!(prompts[0].matches("watch the build").count(), 1);
        assert!(!prompts[1].contains("watch the build"));
        assert_eq!(prompts[2].matches("watch the build").count(), 1);
        h.runtime.shutdown().await;
    }
}

#[tokio::test]
async fn blank_claude_receipts_count_toward_the_unanswered_limit() {
    let engine = QueuedEngine::new(vec![blank_claude_receipt(None); 3]);
    let h = harness_with_engine(engine.clone(), Arc::default(), Arc::default()).await;
    let (agent, _) = engine_worker(&h, "watch the build");
    let agent = wait_for(h.store.as_ref(), agent, |a| a.status.is_terminal()).await;
    assert!(
        matches!(agent.status, AgentStatus::Failed { ref error } if error.contains("3 turns in a row"))
    );
    assert_eq!(engine.prompts.lock().unwrap().len(), 3);
    assert_eq!(
        failed_invocation_errors(h.store.as_ref(), agent.id).len(),
        3
    );
    h.runtime.shutdown().await;
}

/// A worker's engine turn never ran the model: the task closes only on the
/// answer returned by the retry, never on an empty native result.
#[tokio::test]
async fn an_engine_turn_that_never_ran_the_model_does_not_complete_the_task() {
    let h = harness_with_engine(
        QueuedEngine::new(vec![never_invoked(), answered("The build passed.")]),
        Arc::new(Mutex::new(Vec::new())),
        Arc::new(Mutex::new(Vec::new())),
    )
    .await;
    let (agent, task) = engine_worker(&h, "watch the build");
    let agent = wait_for(h.store.as_ref(), agent, |a| a.status.is_terminal()).await;
    assert_eq!(
        agent.status,
        AgentStatus::Completed {
            summary: Some("The build passed.".into())
        }
    );
    let task = h.store.task(task).expect("task").expect("exists");
    assert_eq!(
        task.result().map(|r| r.summary.as_str()),
        Some("The build passed.")
    );
    let failures = failed_invocation_errors(h.store.as_ref(), agent.id);
    assert_eq!(failures.len(), 1, "{failures:?}");
    assert!(
        failures[0].contains("without running the model"),
        "{failures:?}"
    );
}

/// An engine that keeps ending turns without running the model is given up on
/// after the failure limit: the task fails, and is never completed.
#[tokio::test]
async fn engine_turns_that_never_run_the_model_fail_the_task_after_the_limit() {
    let h = harness_with_engine(
        QueuedEngine::new(vec![never_invoked(), never_invoked(), never_invoked()]),
        Arc::new(Mutex::new(Vec::new())),
        Arc::new(Mutex::new(Vec::new())),
    )
    .await;
    let (agent, task) = engine_worker(&h, "watch the build");
    let agent = wait_for(h.store.as_ref(), agent, |a| a.status.is_terminal()).await;
    let AgentStatus::Failed { error } = &agent.status else {
        panic!("expected a failure, got {:?}", agent.status);
    };
    assert!(error.contains("3 turns in a row"), "{error}");
    let task = h.store.task(task).expect("task").expect("exists");
    assert_eq!(
        task.status.kind(),
        aether_core::task::TaskStatusKind::Failed
    );
    let failures = failed_invocation_errors(h.store.as_ref(), agent.id);
    assert_eq!(failures.len(), 3, "{failures:?}");
    assert!(
        failures
            .iter()
            .all(|f| f.contains("without running the model")),
        "{failures:?}"
    );
}

/// The session never read the unanswered prompt, so the retry resends the
/// task it had withheld as already sent.
#[tokio::test]
async fn the_retry_after_an_unanswered_prompt_resends_the_task() {
    let mut first = vec![EngineEvent::Session {
        id: "s1".into(),
        model: None,
        tools: Vec::new(),
    }];
    first.extend(worked_then_fell_silent());
    let engine = QueuedEngine::new(vec![first, never_invoked(), answered("The build passed.")]);
    let h = harness_with_engine(
        engine.clone(),
        Arc::new(Mutex::new(Vec::new())),
        Arc::new(Mutex::new(Vec::new())),
    )
    .await;
    let (agent, _) = engine_worker(&h, "watch the build");
    wait_for(h.store.as_ref(), agent, |a| a.status.is_terminal()).await;
    let prompts = engine.prompts.lock().unwrap().clone();
    assert_eq!(prompts.len(), 3, "{prompts:?}");
    let objective = "watch the build";
    assert_eq!(prompts[0].matches(objective).count(), 1, "{}", prompts[0]);
    assert!(
        !prompts[1].contains(objective),
        "the retained session withheld the task: {}",
        prompts[1]
    );
    assert_eq!(prompts[2].matches(objective).count(), 1, "{}", prompts[2]);
}

/// A chat agent's blank turn is no failure when the model was billed (Codex
/// drops a blank message), and waits for its next message either way.
#[tokio::test]
async fn a_chat_agent_waits_after_a_blank_turn() {
    let billed_blank = vec![EngineEvent::Done(EngineOutcome {
        usage: aether_core::model::Usage {
            input_tokens: 10,
            output_tokens: 2,
            ..Default::default()
        },
        text: Some(String::new()),
        ..Default::default()
    })];
    let h = harness_with_engine(
        QueuedEngine::new(vec![billed_blank, never_invoked()]),
        Arc::new(Mutex::new(Vec::new())),
        Arc::new(Mutex::new(Vec::new())),
    )
    .await;
    let id = chat_agent(&h);
    for (turn, failures) in [(1, 0), (2, 1)] {
        h.runtime
            .send_message(id, vec![ContentBlock::text("hello")], Actor::user())
            .expect("send");
        let agent = wait_for(h.store.as_ref(), id, |a| {
            a.stats.turns == turn && a.status.kind() == AgentStatusKind::Waiting
        })
        .await;
        assert_eq!(
            agent.status,
            AgentStatus::Waiting {
                condition: aether_core::agent::WaitCondition::Inbox
            }
        );
        assert_eq!(
            failed_invocation_errors(h.store.as_ref(), id).len(),
            failures,
            "turn {turn}"
        );
    }
    h.runtime.shutdown().await;
}

/// The wake notices a worker was sent for turns that ended without an answer.
fn silent_notices(store: &dyn Store, id: AgentId) -> Vec<String> {
    store
        .inbox(id, 0)
        .expect("inbox")
        .into_iter()
        .filter_map(|item| match item.kind {
            aether_core::inbox::InboxKind::Wake { reason } => Some(reason),
            _ => None,
        })
        .filter(|reason| reason.contains("ended without an answer"))
        .collect()
}

/// A run in which the model called a tool and then ended with a blank message.
fn worked_then_fell_silent() -> Vec<EngineEvent> {
    vec![
        EngineEvent::Assistant {
            content: vec![ContentBlock::ToolUse {
                id: "call-1".into(),
                name: "Bash".into(),
                input: serde_json::json!({"command": "cargo build"}),
            }],
            usage: None,
            model: None,
        },
        EngineEvent::ToolResult {
            tool_use_id: "call-1".into(),
            content: "Finished".into(),
            is_error: false,
        },
        EngineEvent::Done(EngineOutcome {
            text: Some(String::new()),
            ..Default::default()
        }),
    ]
}

/// A blank turn the model ran (it answered, or it was billed) is silent, never
/// an unanswered prompt: no failed invocation, and the full prompt is not resent.
#[tokio::test]
async fn a_blank_turn_the_model_ran_is_silent_not_unanswered() {
    let spoke_blank = vec![
        EngineEvent::Assistant {
            content: Vec::new(),
            usage: None,
            model: None,
        },
        EngineEvent::Done(EngineOutcome {
            text: Some(String::new()),
            ..Default::default()
        }),
    ];
    let billed_blank = vec![
        EngineEvent::UsageDelta {
            usage: aether_core::model::Usage {
                input_tokens: 10,
                output_tokens: 1,
                ..Default::default()
            },
        },
        EngineEvent::Done(EngineOutcome {
            text: Some(String::new()),
            ..Default::default()
        }),
    ];
    let h = harness_with_engine(
        QueuedEngine::new(vec![
            spoke_blank,
            billed_blank,
            answered("Built."),
            answered("Built."),
        ]),
        Arc::new(Mutex::new(Vec::new())),
        Arc::new(Mutex::new(Vec::new())),
    )
    .await;
    let (agent, _) = engine_worker(&h, "build it");
    let agent = wait_for(h.store.as_ref(), agent, |a| a.status.is_terminal()).await;
    assert_eq!(
        agent.status,
        AgentStatus::Completed {
            summary: Some("Built.".into())
        }
    );
    let failures = failed_invocation_errors(h.store.as_ref(), agent.id);
    assert!(failures.is_empty(), "{failures:?}");
    assert_eq!(silent_notices(h.store.as_ref(), agent.id).len(), 2);
    h.runtime.shutdown().await;
}

/// A Claude Code turn runs its tools inside the turn, so one that worked but
/// ended blank is no idle turn: a run of them never fails the task as silent.
#[tokio::test]
async fn engine_turns_that_ran_tools_do_not_count_as_silent_in_a_row() {
    let h = harness_with_engine(
        QueuedEngine::new(vec![
            worked_then_fell_silent(),
            worked_then_fell_silent(),
            worked_then_fell_silent(),
            answered("Built."),
            answered("Built."),
        ]),
        Arc::new(Mutex::new(Vec::new())),
        Arc::new(Mutex::new(Vec::new())),
    )
    .await;
    let (agent, task) = engine_worker(&h, "build it");
    let agent = wait_for(h.store.as_ref(), agent, |a| a.status.is_terminal()).await;
    assert_eq!(
        agent.status,
        AgentStatus::Completed {
            summary: Some("Built.".into())
        }
    );
    let task = h.store.task(task).expect("task").expect("exists");
    assert_eq!(task.result().map(|r| r.summary.as_str()), Some("Built."));
    assert_eq!(silent_notices(h.store.as_ref(), agent.id).len(), 3);
    h.runtime.shutdown().await;
}

/// Stands for the prompt's own echo in a `ResumeReplayEngine` script.
const PROMPT_ECHO: &str = "<prompt echo>";

/// Each run replays the next list of Claude Code stream-json lines through
/// the real adapter parser, awaiting the run's prompt as the CLI adapter does.
struct ResumeReplayEngine {
    runs: Mutex<std::collections::VecDeque<Vec<String>>>,
}

#[async_trait::async_trait]
impl Engine for ResumeReplayEngine {
    fn kind(&self) -> EngineKind {
        EngineKind::CLAUDE_CODE
    }

    fn adapter(&self) -> String {
        "resume-replay/1".into()
    }

    async fn run(
        &self,
        turn: EngineTurn,
        _cancel: CancellationToken,
    ) -> Result<EngineRun, EngineError> {
        let (tx, rx) = mpsc::channel(64);
        let echo = serde_json::json!({"type":"user","isReplay":true,
            "message":{"role":"user","content":[{"type":"text","text":turn.prompt}]}})
        .to_string();
        let lines: Vec<String> = self
            .runs
            .lock()
            .unwrap()
            .pop_front()
            .unwrap_or_default()
            .into_iter()
            .map(|line| {
                if line == PROMPT_ECHO {
                    echo.clone()
                } else {
                    line
                }
            })
            .collect();
        tokio::spawn(async move {
            let mut state = aether_engine::claude_code::ParseState::awaiting_prompt(&turn.prompt);
            for line in lines {
                for event in aether_engine::claude_code::parse_line(&line, &mut state) {
                    if tx.send(event).await.is_err() {
                        return;
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

/// One Claude Code run in which the model answered the prompt with `text`.
fn replayed_answer(text: &str) -> Vec<String> {
    vec![
        r#"{"type":"system","subtype":"init","session_id":"s1","model":"claude-haiku-5-5"}"#.into(),
        PROMPT_ECHO.into(),
        serde_json::json!({"type":"assistant","message":{"id":format!("msg-{text}"),
            "model":"claude-haiku-5-5","content":[{"type":"text","text":text}]}})
        .to_string(),
        serde_json::json!({"type":"result","subtype":"success","is_error":false,
            "num_turns":1,"result":text,"session_id":"s1"})
        .to_string(),
    ]
}

/// A resumed CLI first closes the orphaned shell's notice with an empty result
/// from no model call; the turn waits past it for the prompt's own answer.
#[tokio::test]
async fn a_resumed_turn_answers_its_prompt_past_an_orphaned_shell_notice() {
    let mut resumed = vec![
        r#"{"type":"system","subtype":"task_notification","task_id":"bigoplw09","status":"stopped","summary":"Background shell command didn't finish before the previous session ended","session_id":"s1"}"#.to_owned(),
        r#"{"type":"system","subtype":"init","session_id":"s1","model":"claude-haiku-5-5"}"#.to_owned(),
        r#"{"type":"result","subtype":"success","is_error":false,"duration_api_ms":0,"num_turns":0,"result":"","session_id":"s1","total_cost_usd":0,"origin":{"kind":"task-notification"}}"#.to_owned(),
    ];
    resumed.extend(replayed_answer("The build passed."));
    let h = harness_with_engine(
        Arc::new(ResumeReplayEngine {
            runs: Mutex::new(vec![replayed_answer(""), resumed].into()),
        }),
        Arc::new(Mutex::new(Vec::new())),
        Arc::new(Mutex::new(Vec::new())),
    )
    .await;
    let (agent, task) = engine_worker(&h, "watch the build");
    let agent = wait_for(h.store.as_ref(), agent, |a| a.status.is_terminal()).await;
    assert_eq!(
        agent.status,
        AgentStatus::Completed {
            summary: Some("The build passed.".into())
        }
    );
    let task = h.store.task(task).expect("task").expect("exists");
    assert_eq!(
        task.result().map(|r| r.summary.as_str()),
        Some("The build passed.")
    );
    assert_eq!(
        failed_invocation_errors(h.store.as_ref(), agent.id),
        Vec::<String>::new(),
        "the notice's result never ended the turn"
    );
    assert_eq!(count(h.store.as_ref(), agent.id, "invocation.completed"), 2);
}
