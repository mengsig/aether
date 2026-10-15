//! Streamed usage and context readings are coalesced into few commits (#57),
//! yet the turn's final figure is exact and a held reading reaches the store
//! within the flush window.
use super::*;
use aether_core::model::Usage;

const MESSAGES: u64 = 4;
/// What a Claude stream reports per message: start, each block, the delta.
const READINGS: u64 = 20;

fn reading() -> Usage {
    Usage {
        input_tokens: 3,
        output_tokens: 1,
        ..Default::default()
    }
}

fn streamed_total() -> Usage {
    let mut total = Usage::default();
    for _ in 0..MESSAGES * READINGS {
        total += reading();
    }
    total
}

/// Reports usage and context many times per message, as a streaming engine
/// does. With `hold`, stops before its receipt until the test releases it.
struct ChattyUsageEngine {
    hold: Option<Arc<tokio::sync::Notify>>,
}

impl ChattyUsageEngine {
    /// Half of each message's readings arrive after it, as a stream's final
    /// output count does, so the turn ends with readings still held.
    fn script() -> Vec<EngineEvent> {
        let mut events = Vec::new();
        let mut context = 1_000;
        let mut readings = |events: &mut Vec<EngineEvent>| {
            for _ in 0..READINGS / 2 {
                events.push(EngineEvent::UsageDelta { usage: reading() });
                events.push(EngineEvent::ContextUsage { tokens: context });
                context += 1;
            }
        };
        for message in 0..MESSAGES {
            readings(&mut events);
            events.push(EngineEvent::Assistant {
                content: vec![ContentBlock::text(format!("message {message}"))],
                usage: None,
                model: None,
            });
            readings(&mut events);
        }
        events
    }

    fn receipt() -> EngineEvent {
        EngineEvent::Done(EngineOutcome {
            closing_accounting: true,
            text: Some(format!("message {}", MESSAGES - 1)),
            cost_usd: Some(0.25),
            usage: streamed_total(),
            ..Default::default()
        })
    }
}

#[async_trait::async_trait]
impl Engine for ChattyUsageEngine {
    fn kind(&self) -> EngineKind {
        EngineKind::CLAUDE_CODE
    }
    fn adapter(&self) -> String {
        "chatty-usage/1".into()
    }
    async fn run(
        &self,
        _turn: EngineTurn,
        _cancel: CancellationToken,
    ) -> Result<EngineRun, EngineError> {
        let script = Self::script();
        let (tx, rx) = mpsc::channel(script.len() + 1);
        for event in script {
            tx.send(event).await.expect("fixture channel capacity");
        }
        match self.hold.clone() {
            None => tx.send(Self::receipt()).await.expect("fixture capacity"),
            Some(hold) => {
                tokio::spawn(async move {
                    hold.notified().await;
                    let _ = tx.send(Self::receipt()).await;
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

async fn chatty_harness(hold: Option<Arc<tokio::sync::Notify>>) -> Harness {
    harness_with_engine(
        Arc::new(ChattyUsageEngine { hold }),
        Arc::new(Mutex::new(Vec::new())),
        Arc::new(Mutex::new(Vec::new())),
    )
    .await
}

fn usage_totals(store: &dyn Store, id: AgentId) -> Vec<Usage> {
    store
        .events(0, usize::MAX, &EventFilter::for_agent(id))
        .unwrap()
        .into_iter()
        .filter_map(|event| match event.kind {
            EventKind::AgentUsageUpdated { total, .. } => Some(total),
            _ => None,
        })
        .collect()
}

#[tokio::test]
async fn a_chatty_turn_commits_few_usage_updates_and_ends_exact() {
    let h = chatty_harness(None).await;
    let id = chat_agent(&h);
    let commits_before = h.store.commits();
    h.runtime
        .send_message(id, vec![ContentBlock::text("Stream usage")], Actor::user())
        .expect("send");
    let agent = wait_for(h.store.as_ref(), id, |a| {
        a.stats.turns == 1 && a.status.kind() == AgentStatusKind::Waiting
    })
    .await;
    let commits = h.store.commits() - commits_before;
    let totals = usage_totals(h.store.as_ref(), id);
    let contexts = h
        .store
        .events(0, usize::MAX, &EventFilter::for_agent(id))
        .unwrap()
        .into_iter()
        .filter(|e| matches!(&e.kind, EventKind::Custom { name, .. } if name == "context.usage"))
        .count();
    eprintln!(
        "{} readings: {commits} commits, {} usage_updated, {contexts} context.usage",
        MESSAGES * READINGS,
        totals.len()
    );
    // Before coalescing, every reading was two commits of its own (> 160).
    assert!(
        commits < 40,
        "{commits} commits for {} streamed readings",
        MESSAGES * READINGS
    );
    assert!(
        totals.len() as u64 <= MESSAGES + 2,
        "{} updates",
        totals.len()
    );

    // The closing receipt is the figure, on the row and as the last event.
    let expected = streamed_total();
    assert_eq!(agent.stats.usage.input_tokens, expected.input_tokens);
    assert_eq!(agent.stats.usage.output_tokens, expected.output_tokens);
    assert_eq!(agent.stats.usage.cost_usd, 0.25);
    assert_eq!(*totals.last().expect("a usage update"), agent.stats.usage);
    assert_eq!(
        agent.stats.context_tokens,
        Some(1_000 + MESSAGES * READINGS - 1),
        "the last context reading wins"
    );
    h.runtime.shutdown().await;
}

#[tokio::test]
async fn a_held_reading_reaches_the_store_within_the_flush_window() {
    let hold = Arc::new(tokio::sync::Notify::new());
    let h = chatty_harness(Some(hold.clone())).await;
    let id = chat_agent(&h);
    h.runtime
        .send_message(id, vec![ContentBlock::text("Stream usage")], Actor::user())
        .expect("send");
    // The engine has gone quiet before its receipt; only the timer flushes.
    let expected = streamed_total();
    let agent = wait_for(h.store.as_ref(), id, |a| {
        a.stats.usage.output_tokens == expected.output_tokens
    })
    .await;
    let totals = usage_totals(h.store.as_ref(), id);
    let last = *totals.last().expect("a flushed usage update");
    assert_eq!(last.output_tokens, expected.output_tokens);
    assert_eq!(last, agent.stats.usage, "the row and the log agree");
    assert_eq!(agent.status.kind(), AgentStatusKind::Running);

    hold.notify_one();
    let agent = wait_for(h.store.as_ref(), id, |a| {
        a.stats.turns == 1 && a.status.kind() == AgentStatusKind::Waiting
    })
    .await;
    assert_eq!(agent.stats.usage.output_tokens, expected.output_tokens);
    assert_eq!(agent.stats.usage.cost_usd, 0.25);
    h.runtime.shutdown().await;
}

/// Reports usage, then asks something of a person and waits for the answer.
struct AskingEngine {
    ask: EngineEvent,
}

#[async_trait::async_trait]
impl Engine for AskingEngine {
    fn kind(&self) -> EngineKind {
        EngineKind::CLAUDE_CODE
    }
    fn adapter(&self) -> String {
        "asking-usage/1".into()
    }
    async fn run(
        &self,
        _turn: EngineTurn,
        _cancel: CancellationToken,
    ) -> Result<EngineRun, EngineError> {
        let (tx, rx) = mpsc::channel(8);
        let (decisions, mut answers) = mpsc::channel(1);
        for event in [
            EngineEvent::UsageDelta { usage: reading() },
            self.ask.clone(),
        ] {
            tx.send(event).await.expect("fixture channel capacity");
        }
        tokio::spawn(async move {
            if answers.recv().await.is_some() {
                let _ = tx.send(Self::done()).await;
            }
        });
        Ok(EngineRun {
            events: rx,
            input: None,
            decisions: Some(decisions),
            stop: None,
        })
    }
}

impl AskingEngine {
    fn done() -> EngineEvent {
        EngineEvent::Done(EngineOutcome {
            closing_accounting: true,
            text: Some("asked".into()),
            usage: reading(),
            ..Default::default()
        })
    }
}

/// An approval can wait on a person for as long as they take; what the engine
/// spent before it asked must not wait with it.
#[tokio::test]
async fn readings_reach_the_store_before_a_permission_prompt_waits() {
    let h = harness_with_engine(
        Arc::new(AskingEngine {
            ask: EngineEvent::Permission {
                request_id: "ask-1".into(),
                tool: "Bash".into(),
                input: serde_json::json!({"command": "touch example"}),
                side_effect: Some(aether_core::tool::SideEffectClass::Mutation),
            },
        }),
        Arc::new(Mutex::new(Vec::new())),
        Arc::new(Mutex::new(Vec::new())),
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
                    approval: ApprovalPolicy {
                        require_for: [aether_core::tool::SideEffectClass::Mutation]
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
    let id = agent.id;
    h.runtime
        .send_message(id, vec![ContentBlock::text("Ask first")], Actor::user())
        .expect("send");
    let asked = || {
        h.store
            .events(0, usize::MAX, &EventFilter::for_agent(id))
            .unwrap()
            .iter()
            .any(|e| matches!(e.kind, EventKind::ApprovalRequested { .. }))
    };
    for _ in 0..750 {
        if asked() {
            break;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    assert!(asked(), "the prompt reached an operator");
    // The drive loop is awaiting the verdict, so no timer can have flushed this.
    let row = h.store.agent(id).unwrap().expect("row");
    assert_eq!(row.stats.usage.output_tokens, reading().output_tokens);
    assert_eq!(usage_totals(h.store.as_ref(), id).len(), 1);
    h.runtime.shutdown().await;
}

/// A question is committed as the agent speaking; what it spent first is
/// recorded before it, and does not wait for the reply.
#[tokio::test]
async fn readings_precede_a_question_and_do_not_wait_for_its_reply() {
    let h = harness_with_engine(
        Arc::new(AskingEngine {
            ask: EngineEvent::Question {
                request_id: "ask-2".into(),
                prompt: "Which branch?".into(),
            },
        }),
        Arc::new(Mutex::new(Vec::new())),
        Arc::new(Mutex::new(Vec::new())),
    )
    .await;
    let id = chat_agent(&h);
    h.runtime
        .send_message(id, vec![ContentBlock::text("Ask first")], Actor::user())
        .expect("send");
    let question_seq = || {
        h.store
            .events(0, usize::MAX, &EventFilter::for_agent(id))
            .unwrap()
            .into_iter()
            .find(|e| {
                matches!(&e.kind, EventKind::AgentMessage { role: MessageRole::Assistant, content, .. }
                    if content == &vec![ContentBlock::text("Which branch?")])
            })
            .map(|e| e.seq)
    };
    for _ in 0..750 {
        if question_seq().is_some() {
            break;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    let asked = question_seq().expect("the question was recorded");
    let usage_seqs: Vec<u64> = h
        .store
        .events(0, usize::MAX, &EventFilter::for_agent(id))
        .unwrap()
        .into_iter()
        .filter(|e| matches!(e.kind, EventKind::AgentUsageUpdated { .. }))
        .map(|e| e.seq)
        .collect();
    assert_eq!(usage_seqs.len(), 1, "{usage_seqs:?}");
    assert!(
        usage_seqs[0] < asked,
        "usage {usage_seqs:?}, question {asked}"
    );
    h.runtime
        .send_message(id, vec![ContentBlock::text("main")], Actor::user())
        .expect("reply");
    wait_for(h.store.as_ref(), id, |a| {
        a.stats.turns == 1 && a.status.kind() == AgentStatusKind::Waiting
    })
    .await;
    h.runtime.shutdown().await;
}

/// Streams its readings, says so live, then holds its stream open until the
/// turn is cancelled: only the turn's own end can flush what is held.
struct CancelledUsageEngine;

#[async_trait::async_trait]
impl Engine for CancelledUsageEngine {
    fn kind(&self) -> EngineKind {
        EngineKind::CLAUDE_CODE
    }
    fn adapter(&self) -> String {
        "cancelled-usage/1".into()
    }
    async fn run(
        &self,
        _turn: EngineTurn,
        cancel: CancellationToken,
    ) -> Result<EngineRun, EngineError> {
        let mut script = ChattyUsageEngine::script();
        script.push(EngineEvent::Delta(
            aether_core::model::StreamEvent::TextDelta {
                index: 0,
                text: "ready".into(),
            },
        ));
        let (tx, rx) = mpsc::channel(script.len());
        for event in script {
            tx.send(event).await.expect("fixture channel capacity");
        }
        tokio::spawn(async move {
            cancel.cancelled().await;
            drop(tx);
        });
        Ok(EngineRun {
            events: rx,
            input: None,
            decisions: None,
            stop: None,
        })
    }
}

/// A cancel ends the turn well inside the flush window; what was held is on
/// the record as the turn's last usage, equal to the row.
#[tokio::test]
async fn a_cancelled_turn_flushes_held_readings() {
    let h = harness_with_engine(
        Arc::new(CancelledUsageEngine),
        Arc::new(Mutex::new(Vec::new())),
        Arc::new(Mutex::new(Vec::new())),
    )
    .await;
    let id = chat_agent(&h);
    let mut bus = h.runtime.subscribe();
    h.runtime
        .send_message(id, vec![ContentBlock::text("Stream usage")], Actor::user())
        .expect("send");
    tokio::time::timeout(Duration::from_secs(10), async {
        loop {
            if let Ok(aether_runtime::BusMessage::Delta {
                agent,
                delta: aether_runtime::DeltaKind::Text { text },
                ..
            }) = bus.recv().await
                && agent == id
                && text == "ready"
            {
                break;
            }
        }
    })
    .await
    .expect("the engine streamed its readings");
    h.runtime
        .cancel_agent(id, Some("stop".into()), Actor::user())
        .expect("cancel");
    let agent = wait_for(h.store.as_ref(), id, |a| {
        a.status.kind() == AgentStatusKind::Cancelled
    })
    .await;
    let expected = streamed_total();
    assert_eq!(agent.stats.usage.output_tokens, expected.output_tokens);
    let totals = usage_totals(h.store.as_ref(), id);
    assert_eq!(
        *totals.last().expect("a usage update"),
        agent.stats.usage,
        "the held readings reached the log, not only the row"
    );
    assert_eq!(
        agent.stats.context_tokens,
        Some(1_000 + MESSAGES * READINGS - 1)
    );
    h.runtime.shutdown().await;
}

#[tokio::test]
async fn shutdown_flushes_held_usage_before_the_store_reopens() {
    let dir = tempfile::tempdir().expect("tempdir");
    let path = dir.path().join("state.db");
    let h = build_harness(
        Arc::new(CancelledUsageEngine),
        Arc::default(),
        Arc::default(),
        None,
        None,
        Some(Arc::new(SqliteStore::open(&path).expect("store"))),
        |_| {},
    )
    .await;
    let id = chat_agent(&h);
    let mut bus = h.runtime.subscribe();
    h.runtime
        .send_message(id, vec![ContentBlock::text("Stream usage")], Actor::user())
        .expect("send");
    tokio::time::timeout(Duration::from_secs(10), async {
        loop {
            if let Ok(aether_runtime::BusMessage::Delta {
                agent,
                delta: aether_runtime::DeltaKind::Text { text },
                ..
            }) = bus.recv().await
                && agent == id
                && text == "ready"
            {
                break;
            }
        }
    })
    .await
    .expect("readings streamed");
    h.runtime.shutdown().await;
    drop(h);
    let store = SqliteStore::open(path).expect("reopen");
    let row = store.agent(id).expect("read").expect("agent");
    assert_eq!(
        row.stats.usage.output_tokens,
        streamed_total().output_tokens
    );
    assert_eq!(
        row.stats.context_tokens,
        Some(1_000 + MESSAGES * READINGS - 1)
    );
    assert_eq!(
        *usage_totals(&store, id).last().expect("usage"),
        row.stats.usage
    );
}

struct CrashWindowEngine {
    next: Arc<tokio::sync::Notify>,
}

#[async_trait::async_trait]
impl Engine for CrashWindowEngine {
    fn kind(&self) -> EngineKind {
        EngineKind::CLAUDE_CODE
    }
    fn adapter(&self) -> String {
        "crash-window/1".into()
    }
    async fn run(
        &self,
        _turn: EngineTurn,
        cancel: CancellationToken,
    ) -> Result<EngineRun, EngineError> {
        let (tx, events) = mpsc::channel(4);
        tx.send(EngineEvent::UsageDelta { usage: reading() })
            .await
            .expect("first");
        let next = self.next.clone();
        tokio::spawn(async move {
            next.notified().await;
            tx.send(EngineEvent::UsageDelta { usage: reading() })
                .await
                .expect("second");
            tx.send(EngineEvent::Delta(
                aether_core::model::StreamEvent::TextDelta {
                    index: 0,
                    text: "pending".into(),
                },
            ))
            .await
            .expect("marker");
            cancel.cancelled().await;
        });
        Ok(EngineRun {
            events,
            input: None,
            decisions: None,
            stop: None,
        })
    }
}

// Invoked in a separate process so termination cannot run Runtime::shutdown.
#[tokio::test]
async fn crash_window_fixture() {
    let Some(dir) = std::env::var_os("AETHER_USAGE_CRASH_FIXTURE") else {
        return;
    };
    let dir = std::path::PathBuf::from(dir);
    let next = Arc::new(tokio::sync::Notify::new());
    let h = build_harness(
        Arc::new(CrashWindowEngine { next: next.clone() }),
        Arc::default(),
        Arc::default(),
        None,
        None,
        Some(Arc::new(
            SqliteStore::open(dir.join("state.db")).expect("store"),
        )),
        |_| {},
    )
    .await;
    let id = chat_agent(&h);
    let mut bus = h.runtime.subscribe();
    h.runtime
        .send_message(id, vec![ContentBlock::text("Stream")], Actor::user())
        .expect("send");
    wait_for(h.store.as_ref(), id, |a| a.stats.usage.output_tokens == 1).await;
    next.notify_one();
    loop {
        if let Ok(aether_runtime::BusMessage::Delta {
            delta: aether_runtime::DeltaKind::Text { text },
            ..
        }) = bus.recv().await
            && text == "pending"
        {
            break;
        }
    }
    std::fs::write(dir.join("ready"), serde_json::to_vec(&id).expect("id")).expect("ready");
    std::future::pending::<()>().await;
}

#[tokio::test]
async fn process_crash_loses_only_usage_since_the_last_flush() {
    let dir = tempfile::tempdir().expect("tempdir");
    let mut child = std::process::Command::new(std::env::current_exe().expect("test binary"))
        .args([
            "--exact",
            "usage_coalescing::crash_window_fixture",
            "--nocapture",
        ])
        .env("AETHER_USAGE_CRASH_FIXTURE", dir.path())
        .stdout(std::process::Stdio::null())
        .spawn()
        .expect("spawn fixture");
    let ready = tokio::time::timeout(Duration::from_secs(10), async {
        loop {
            if dir.path().join("ready").exists() {
                break;
            }
            if let Some(status) = child.try_wait().expect("poll fixture") {
                panic!("fixture exited before the crash: {status}");
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await;
    child.kill().expect("kill fixture");
    child.wait().expect("reap fixture");
    ready.expect("fixture reaches the unflushed window within 10 seconds");
    let id = serde_json::from_slice(&std::fs::read(dir.path().join("ready")).expect("ready"))
        .expect("id");
    let store = SqliteStore::open(dir.path().join("state.db")).expect("recover WAL");
    let row = store.agent(id).expect("read").expect("agent");
    assert_eq!(
        row.stats.usage.output_tokens, 1,
        "only the second reading was lost"
    );
    assert_eq!(
        usage_totals(&store, id),
        vec![row.stats.usage],
        "one absolute persisted total, no duplicate replay"
    );
}
