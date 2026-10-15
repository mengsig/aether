//! A native turn that fails mid-message keeps what it streamed on the record,
//! under the id its deltas carried, so a reload shows what the live view did.
use super::*;
use aether_core::id::InvocationId;
use aether_core::model::StreamEvent;
use aether_runtime::{BusMessage, DeltaKind};
use tokio::sync::broadcast;

const REASONING: &str = "**Weighing** the two halves";
const STREAMED: &str = "Half of an answer the user already read";

fn text_delta(text: &str) -> EngineEvent {
    EngineEvent::Delta(StreamEvent::TextDelta {
        index: 0,
        text: text.into(),
    })
}

async fn send(turn: &TurnStarted, event: EngineEvent) {
    let events = turn.events.upgrade().expect("the turn is still running");
    events.send(event).await.expect("engine stream open");
}

async fn stream_text(turn: &TurnStarted, text: &str) {
    for part in text.split_inclusive(' ') {
        send(turn, text_delta(part)).await;
    }
}

async fn fail(h: &Harness, turn: &TurnStarted, agent: AgentId) {
    let failure = EngineOutcome {
        is_error: true,
        error: Some("the model refused mid-answer".into()),
        ..Default::default()
    };
    send(turn, EngineEvent::Done(failure)).await;
    let failed = wait_until(h.store.as_ref(), agent, |a| a.status.is_terminal()).await;
    assert!(failed_with(&failed).contains("refused mid-answer"));
}

/// Every id this agent's text deltas streamed under, in order.
fn text_invocations(
    bus: &mut broadcast::Receiver<BusMessage>,
    agent: AgentId,
) -> Vec<InvocationId> {
    let mut ids: Vec<InvocationId> = Vec::new();
    while let Ok(message) = bus.try_recv() {
        if let BusMessage::Delta {
            agent: from,
            invocation,
            delta: DeltaKind::Text { .. },
        } = message
            && from == agent
            && ids.last() != Some(&invocation)
        {
            ids.push(invocation);
        }
    }
    ids
}

/// The agent's one terminal failure record: its id and stored partial.
fn terminal_failure(h: &Harness, agent: AgentId) -> (InvocationId, Vec<ContentBlock>) {
    let failures: Vec<_> = events(h.store.as_ref())
        .into_iter()
        .filter(|e| e.agent_id == Some(agent))
        .filter_map(|e| match e.kind {
            EventKind::InvocationFailed {
                invocation,
                partial,
                retryable: false,
                cancelled: false,
                ..
            } => Some((invocation, partial)),
            _ => None,
        })
        .collect();
    assert_eq!(failures.len(), 1, "{failures:?}");
    failures.into_iter().next().expect("one failure")
}

fn assistant_messages(h: &Harness, agent: AgentId) -> Vec<String> {
    h.store
        .transcript(agent, 0)
        .expect("transcript")
        .into_iter()
        .filter(|entry| entry.message.role == aether_core::model::MessageRole::Assistant)
        .map(|entry| entry.message.text())
        .collect()
}

#[tokio::test]
async fn a_failed_native_stream_keeps_its_reasoning_and_text_on_the_record() {
    let mut h = harness(Duration::from_secs(5)).await;
    let mut bus = h.runtime.subscribe();
    let agent = start_coordinator(&h);
    let turn = turn_of(&mut h, agent).await;
    let thinking = EngineEvent::Delta(StreamEvent::ThinkingDelta {
        index: 0,
        text: REASONING.into(),
    });
    send(&turn, thinking).await;
    stream_text(&turn, STREAMED).await;
    fail(&h, &turn, agent).await;

    let streamed_under = text_invocations(&mut bus, agent);
    assert_eq!(streamed_under.len(), 1, "one uncommitted message streamed");
    let (invocation, partial) = terminal_failure(&h, agent);
    assert_eq!(
        invocation, streamed_under[0],
        "the record must retire the live stream it replaces, or a client shows it twice"
    );
    let expected = vec![
        ContentBlock::Thinking {
            text: REASONING.into(),
            signature: None,
            redacted: false,
        },
        ContentBlock::text(STREAMED),
    ];
    assert_eq!(partial, expected);
    let stored = h.store.invocation(invocation).expect("read").expect("row");
    assert_eq!(stored.content, expected);
    assert_eq!(stored.usage, Usage::default(), "a partial bills nothing");
    assert!(
        assistant_messages(&h, agent).is_empty(),
        "a partial is not an assistant message a resumed session is sent"
    );
    h.runtime.shutdown().await;
}

#[tokio::test]
async fn a_failed_native_stream_keeps_only_what_followed_its_last_message() {
    let mut h = harness(Duration::from_secs(5)).await;
    let mut bus = h.runtime.subscribe();
    let agent = start_coordinator(&h);
    let turn = turn_of(&mut h, agent).await;
    stream_text(&turn, "First answer").await;
    let committed = EngineEvent::Assistant {
        content: vec![ContentBlock::text("First answer")],
        usage: None,
        model: None,
    };
    send(&turn, committed).await;
    stream_text(&turn, STREAMED).await;
    fail(&h, &turn, agent).await;

    let streamed_under = text_invocations(&mut bus, agent);
    assert_eq!(streamed_under.len(), 2, "{streamed_under:?}");
    let (invocation, partial) = terminal_failure(&h, agent);
    assert_eq!(invocation, streamed_under[1]);
    assert_eq!(partial, vec![ContentBlock::text(STREAMED)]);
    assert_eq!(assistant_messages(&h, agent), ["First answer"]);
    h.runtime.shutdown().await;
}
