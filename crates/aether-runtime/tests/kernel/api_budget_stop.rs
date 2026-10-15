//! An API reply cut by a budget sweep: the sweep ends the agent before the
//! turn writes the message it keeps, so the live view must not draw it twice.
use super::tui_projection::{drain_through, models, projection, reduce};
use super::*;
use aether_core::event::Event;
use aether_runtime::{BusMessage, DeltaKind};
use aether_tui::App;
use aether_tui::app::transcript::CellModel;
use tokio::sync::broadcast::Receiver;

const THINKING: &str = "Weighing the cut";

/// Reasoning, then text that streams slowly enough for the sweep to land.
fn slow_reply() -> Vec<StreamEvent> {
    let mut events = vec![
        StreamEvent::MessageStart {
            provider_message_id: Some("scripted".into()),
            model: Some("scripted".into()),
        },
        StreamEvent::BlockStart {
            index: 0,
            block: BlockStart::Thinking {
                text: String::new(),
                redacted: false,
                signature: None,
            },
        },
        StreamEvent::ThinkingDelta {
            index: 0,
            text: THINKING.into(),
        },
        StreamEvent::BlockStop { index: 0 },
        StreamEvent::BlockStart {
            index: 1,
            block: BlockStart::Text {
                text: String::new(),
            },
        },
    ];
    events.extend((0..200).map(|i| StreamEvent::TextDelta {
        index: 1,
        text: format!("word{i} "),
    }));
    events.push(StreamEvent::BlockStop { index: 1 });
    events.push(StreamEvent::MessageStop);
    events
}

/// The agent's assistant text cells, and how many reasoning cells it has.
fn reply_cells(app: &App, agent: AgentId) -> (Vec<String>, usize) {
    let cells = models(app, agent);
    let texts = cells.iter().filter_map(|cell| match cell {
        CellModel::Assistant { text } => Some(text.clone()),
        _ => None,
    });
    let reasoning = cells
        .iter()
        .filter(|cell| matches!(cell, CellModel::Reasoning { text, .. } if text == THINKING));
    (texts.collect(), reasoning.count())
}

#[tokio::test]
async fn an_api_reply_cut_by_a_budget_sweep_is_drawn_once() {
    let h = harness(vec![Ok(slow_reply())]).await;
    h.client.set_delay(Duration::from_millis(20));
    let mut bus = h.runtime.subscribe();
    let mut s = spec("answer slowly");
    s.config.as_mut().expect("config").token_budget = Some(40_000);
    let (agent, _) = h.runtime.create_agent(s, Actor::user()).expect("create");
    let mut child = spec("idle");
    child.parent = Some(agent.id);
    child.start = false;
    let (child, _) = h.runtime.create_agent(child, Actor::user()).expect("child");
    let mut live = projection(agent.clone(), vec![]);
    await_text(&mut bus, &mut live, agent.id).await;
    let mut row = h.store.agent(child.id).expect("read").expect("child");
    row.stats.usage.input_tokens = 41_000;
    h.store
        .commit(Commit::new().mutate(Mutation::UpsertAgent(row)))
        .expect("spend");
    let status = h.wait_until(agent.id, |s| s.is_terminal()).await;
    assert!(matches!(&status, AgentStatus::Failed { error } if error.contains("token budget")));
    let history = settled(&h, &mut bus, &mut live, agent.id).await;
    let reloaded = projection(h.store.agent(agent.id).unwrap().unwrap(), history);
    let (texts, reasoning) = reply_cells(&live, agent.id);
    assert_eq!(reasoning, 1, "{:#?}", models(&live, agent.id));
    assert!(live.transcripts[&agent.id].stream.is_empty());
    let (stored, _) = reply_cells(&reloaded, agent.id);
    assert_eq!(stored.len(), 1, "{stored:?}");
    assert!(stored[0].starts_with("word0 "), "{stored:?}");
    // The stop draws what had streamed; deltas already in flight follow it.
    // Together they are the kept text, shown once.
    assert!((1..=2).contains(&texts.len()), "{texts:?}");
    assert_eq!(
        texts.join(" "),
        stored[0],
        "the live view repeated or lost text"
    );
}

/// Reduce live frames until the reply's text has started streaming.
async fn await_text(bus: &mut Receiver<BusMessage>, live: &mut App, agent: AgentId) {
    loop {
        let message = tokio::time::timeout(Duration::from_secs(10), bus.recv())
            .await
            .expect("the reply never streamed")
            .expect("lost live events");
        let text = matches!(&message, BusMessage::Delta { agent: from, delta: DeltaKind::Text { .. }, .. }
            if *from == agent);
        reduce(live, message);
        if text {
            return;
        }
    }
}

/// The agent's stored history once its failure record is in, with the live
/// view brought up to the end of it.
async fn settled(
    h: &Harness,
    bus: &mut Receiver<BusMessage>,
    live: &mut App,
    agent: AgentId,
) -> Vec<Event> {
    let filter = EventFilter::for_agent(agent);
    let recorded = |events: &[Event]| {
        events
            .iter()
            .any(|event| matches!(event.kind, EventKind::InvocationFailed { .. }))
    };
    h.wait_for("the cut reply's record", || {
        recorded(&h.store.events(0, 10_000, &filter).expect("events"))
    })
    .await;
    let mut history = h.store.events(0, 10_000, &filter).expect("events");
    history.retain(|event| event.agent_id == Some(agent));
    let last = history.last().expect("the agent has a history").seq;
    drain_through(bus, live, agent, last).await;
    history
}
