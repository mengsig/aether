//! Native cancellation keeps an operator record without adding model history.
use super::tui_projection::{drain_through, models, projection, reduce};
use super::*;
use aether_core::id::InvocationId;
use aether_core::model::{MessageRole, ModelErrorClass, StreamEvent};
use aether_runtime::{BusMessage, DeltaKind};
use aether_tui::App;
use aether_tui::app::transcript::CellModel;
use tokio::sync::broadcast;

pub(super) const THINKING: &str = "Checking the unfinished answer";
pub(super) const TEXT: &str = "An unfinished answer";

pub(super) async fn emit(turn: &TurnStarted, event: EngineEvent) {
    turn.events
        .upgrade()
        .expect("running engine")
        .send(event)
        .await
        .expect("open stream");
}

/// The agent's own history, with the live view brought up to its end.
pub(super) async fn settle(
    h: &Harness,
    bus: &mut broadcast::Receiver<BusMessage>,
    live: &mut App,
    agent: AgentId,
) -> Vec<Event> {
    let history: Vec<Event> = events(h.store.as_ref())
        .into_iter()
        .filter(|event| event.agent_id == Some(agent))
        .collect();
    let last = history.last().expect("the agent has a history").seq;
    drain_through(bus, live, agent, last).await;
    history
}

pub(super) async fn stream(
    turn: &TurnStarted,
    bus: &mut broadcast::Receiver<BusMessage>,
    live: &mut App,
    text: bool,
    committed: bool,
) -> InvocationId {
    if committed {
        emit(
            turn,
            EngineEvent::Assistant {
                content: vec![ContentBlock::text("Earlier committed answer")],
                usage: None,
                model: None,
            },
        )
        .await;
    }
    emit(
        turn,
        EngineEvent::Delta(StreamEvent::ThinkingDelta {
            index: 0,
            text: THINKING.into(),
        }),
    )
    .await;
    if text {
        emit(
            turn,
            EngineEvent::Delta(StreamEvent::TextDelta {
                index: 0,
                text: TEXT.into(),
            }),
        )
        .await;
    }
    await_stream(bus, live, text).await
}

pub(super) async fn await_stream(
    bus: &mut broadcast::Receiver<BusMessage>,
    live: &mut App,
    text: bool,
) -> InvocationId {
    loop {
        let message = tokio::time::timeout(Duration::from_secs(5), bus.recv())
            .await
            .expect("stream deadline")
            .expect("live event");
        let last = match &message {
            BusMessage::Delta {
                invocation, delta, ..
            } if matches!(delta, DeltaKind::Text { .. })
                || (!text && matches!(delta, DeltaKind::Thinking { .. })) =>
            {
                Some(*invocation)
            }
            _ => None,
        };
        reduce(live, message);
        if let Some(id) = last {
            let agent = live.selected.expect("selected");
            assert_eq!(live.transcripts[&agent].stream.reasoning, THINKING);
            assert_eq!(
                live.transcripts[&agent].stream.text,
                if text { TEXT } else { "" }
            );
            return id;
        }
    }
}

fn assert_record(
    h: &Harness,
    agent: AgentId,
    invocation: InvocationId,
    text: bool,
    committed: bool,
) {
    let row = h
        .store
        .invocation(invocation)
        .expect("read invocation")
        .expect("cancelled partial is durable");
    assert_eq!(row.usage, Usage::default(), "partial must not bill again");
    assert_eq!(row.content.len(), if text { 2 } else { 1 });
    assert_eq!(row.error.as_deref(), Some("cancelled"));
    let failures: Vec<_> = events(h.store.as_ref()).into_iter().filter(|event| {
        matches!(&event.kind, EventKind::InvocationFailed { invocation: id, cancelled: true, retryable: false, partial, class, .. }
            if *id == invocation && *partial == row.content && *class == Some(ModelErrorClass::Cancelled))
    }).collect();
    assert_eq!(failures.len(), 1);
    let messages: Vec<_> = h
        .store
        .transcript(agent, 0)
        .expect("model transcript")
        .into_iter()
        .filter(|entry| entry.message.role == MessageRole::Assistant)
        .collect();
    assert_eq!(messages.len(), usize::from(committed));
    assert!(
        messages
            .iter()
            .all(|entry| entry.message.text() == "Earlier committed answer")
    );
}

async fn cancelled_case(interrupt: bool, text: bool, committed: bool) {
    let mut h = harness(Duration::from_secs(5)).await;
    let mut bus = h.runtime.subscribe();
    let agent = start_coordinator(&h);
    let turn = turn_of(&mut h, agent).await;
    let row = h.store.agent(agent).unwrap().unwrap();
    let mut live = projection(row, vec![]);
    let invocation = stream(&turn, &mut bus, &mut live, text, committed).await;
    if interrupt {
        assert!(
            h.runtime
                .interrupt_agent(agent, Actor::user())
                .expect("interrupt")
        );
    } else {
        h.runtime
            .cancel_agent(agent, Some("operator stop".into()), Actor::user())
            .expect("cancel");
    }
    let ended = wait_until(h.store.as_ref(), agent, |row| {
        row.status.kind()
            == if interrupt {
                AgentStatusKind::Waiting
            } else {
                AgentStatusKind::Cancelled
            }
    })
    .await;
    let history = settle(&h, &mut bus, &mut live, agent).await;
    let session = ended.engine_session.clone();
    let reloaded = projection(ended, history);
    assert_projection(&live, &reloaded, agent, text);
    assert_record(&h, agent, invocation, text, committed);
    if interrupt {
        assert_resumed_prompt(&mut h, agent, session).await;
    }
    h.runtime.shutdown().await;
}

pub(super) async fn assert_resumed_prompt(
    h: &mut Harness,
    agent: AgentId,
    session: Option<String>,
) {
    assert!(session.is_some());
    h.runtime
        .send_message(
            agent,
            vec![ContentBlock::text("Continue please")],
            Actor::user(),
        )
        .expect("wake");
    let resumed = turn_of(h, agent).await;
    assert_eq!(resumed.resumed, session);
    assert!(!resumed.prompt.contains(THINKING));
    assert!(!resumed.prompt.contains(TEXT));
}

fn assert_projection(live: &App, reloaded: &App, agent: AgentId, text: bool) {
    assert_eq!(models(live, agent), models(reloaded, agent));
    assert!(live.transcripts[&agent].stream.is_empty());
    assert!(reloaded.transcripts[&agent].stream.is_empty());
    let cells = models(reloaded, agent);
    assert_eq!(
        cells
            .iter()
            .filter(|cell| matches!(cell, CellModel::Reasoning { text, .. } if text == THINKING))
            .count(),
        1
    );
    assert_eq!(
        cells
            .iter()
            .filter(|cell| matches!(cell, CellModel::Assistant { text } if text == TEXT))
            .count(),
        usize::from(text)
    );
    assert!(
        !cells
            .iter()
            .any(|cell| matches!(cell, CellModel::Error { .. }))
    );
}

#[tokio::test]
async fn caller_cancelled_native_partials_match_reload_without_entering_model_history() {
    for text in [false, true] {
        for committed in [false, true] {
            cancelled_case(false, text, committed).await;
        }
    }
}

#[tokio::test]
async fn operator_interrupted_native_partials_match_reload_without_entering_model_history() {
    for text in [false, true] {
        for committed in [false, true] {
            cancelled_case(true, text, committed).await;
        }
    }
}

/// A budget stop ends the agent before its turn ends, so the live view draws
/// the cut stream first. The partial the turn records after must not repeat it.
#[tokio::test]
async fn a_budget_stopped_native_partial_is_drawn_once_live_and_on_reload() {
    let mut h = harness(Duration::from_secs(5)).await;
    let mut bus = h.runtime.subscribe();
    let agent = start_agent(&h, budgeted_spec(40_000));
    let child = idle_child(&h, agent);
    let turn = turn_of(&mut h, agent).await;
    let row = h.store.agent(agent).unwrap().unwrap();
    let mut live = projection(row, vec![]);
    let invocation = stream(&turn, &mut bus, &mut live, true, false).await;
    spend_over_budget(&h, child, 41_000);
    let stopped = wait_until(h.store.as_ref(), agent, |row| !row.is_live()).await;
    assert!(failed_with(&stopped).contains("token budget of 40000"));
    wait_for_event(h.store.as_ref(), |event| {
        matches!(&event.kind, EventKind::InvocationFailed { invocation: id, .. } if *id == invocation)
    })
    .await;
    let history = settle(&h, &mut bus, &mut live, agent).await;
    let reloaded = projection(stopped, history);
    for app in [&live, &reloaded] {
        let cells = models(app, agent);
        let thinking = cells
            .iter()
            .filter(|cell| matches!(cell, CellModel::Reasoning { text, .. } if text == THINKING));
        let text = cells
            .iter()
            .filter(|cell| matches!(cell, CellModel::Assistant { text } if text == TEXT));
        assert_eq!((thinking.count(), text.count()), (1, 1), "{cells:#?}");
        assert!(app.transcripts[&agent].stream.is_empty());
    }
    assert_record(&h, agent, invocation, true, false);
    h.runtime.shutdown().await;
}

#[tokio::test]
async fn cancellation_without_an_uncommitted_message_adds_no_invocation() {
    for committed in [false, true] {
        let mut h = harness(Duration::from_secs(5)).await;
        let agent = start_coordinator(&h);
        let turn = turn_of(&mut h, agent).await;
        if committed {
            emit(
                &turn,
                EngineEvent::Assistant {
                    content: vec![ContentBlock::text("Complete answer")],
                    usage: None,
                    model: None,
                },
            )
            .await;
        }
        h.runtime
            .cancel_agent(agent, None, Actor::user())
            .expect("cancel");
        wait_until(h.store.as_ref(), agent, |row| row.status.is_terminal()).await;
        let history = events(h.store.as_ref());
        assert!(
            !history
                .iter()
                .any(|event| matches!(event.kind, EventKind::InvocationFailed { .. }))
        );
        let messages = h.store.transcript(agent, 0).expect("transcript");
        assert_eq!(
            messages
                .iter()
                .filter(|entry| entry.message.role == MessageRole::Assistant)
                .count(),
            usize::from(committed)
        );
        h.runtime.shutdown().await;
    }
}
