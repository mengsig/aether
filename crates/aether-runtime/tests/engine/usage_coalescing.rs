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
