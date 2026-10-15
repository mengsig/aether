//! Reuse the partial-answer bus/SQLite harness with an actual outage boundary.
use super::stream_cancel::{TEXT, THINKING, emit, stream};
use super::tui_projection::{models, projection};
use super::*;
use aether_client::SubscriptionFrame;
use aether_tui::app::transcript::CellModel;
use aether_tui::app::{Effect, Then};
use aether_tui::{App, AppEvent};

#[derive(Clone, Copy)]
enum Ending {
    Finished,
    Failed,
    Cancelled,
}

async fn finish(h: &Harness, turn: &TurnStarted, agent: AgentId, ending: Ending) {
    match ending {
        Ending::Finished => {
            emit(
                turn,
                EngineEvent::Assistant {
                    content: vec![
                        ContentBlock::Thinking {
                            text: THINKING.into(),
                            signature: None,
                            redacted: false,
                        },
                        ContentBlock::text(TEXT),
                    ],
                    usage: None,
                    model: None,
                },
            )
            .await;
            emit(turn, EngineEvent::Done(EngineOutcome::default())).await;
        }
        Ending::Failed => {
            emit(
                turn,
                EngineEvent::Done(EngineOutcome {
                    is_error: true,
                    error: Some("offline failure".into()),
                    ..Default::default()
                }),
            )
            .await
        }
        Ending::Cancelled => h
            .runtime
            .cancel_agent(agent, Some("offline cancel".into()), Actor::user())
            .unwrap(),
    }
    let row = wait_until(h.store.as_ref(), agent, |row| {
        row.status.kind() != AgentStatusKind::Running
    })
    .await;
    assert!(row.status.is_terminal() || row.status.kind() == AgentStatusKind::Waiting);
    assert!(
        events(h.store.as_ref())
            .iter()
            .any(|e| e.agent_id == Some(agent))
    );
}

fn replay(app: &mut App, fresh: &App, history: &[Event], after: u64) {
    let effects = app.reduce(AppEvent::Connected);
    assert!(
        effects
            .iter()
            .any(|e| matches!(e, Effect::Subscribe { after: Some(seq) } if *seq == after))
    );
    app.reduce(AppEvent::Reply {
        then: Then::Tree,
        result: Ok(aether_protocol::Reply::Tree(fresh.tree.clone())),
    });
    for event in history.iter().filter(|e| e.seq > after) {
        app.reduce(AppEvent::Frame(SubscriptionFrame::Event(Box::new(
            event.clone(),
        ))));
    }
    assert_eq!(app.last_seq, history.last().map(|e| e.seq));
}

async fn disconnected_stream(ending: Ending) {
    let mut h = harness(Duration::from_secs(5)).await;
    let mut bus = h.runtime.subscribe();
    let agent = start_coordinator(&h);
    let turn = turn_of(&mut h, agent).await;
    let mut live = projection(h.store.agent(agent).unwrap().unwrap(), vec![]);
    stream(&turn, &mut bus, &mut live, true, false).await;
    let cursor = live.last_seq.expect("pre-outage cursor");
    assert!(!live.transcripts[&agent].stream.is_empty());
    live.reduce(AppEvent::Disconnected { because: None });
    assert!(live.transcripts[&agent].stream.is_empty());
    finish(&h, &turn, agent, ending).await;
    let history: Vec<_> = events(h.store.as_ref())
        .into_iter()
        .filter(|e| e.agent_id == Some(agent))
        .collect();
    let fresh = projection(h.store.agent(agent).unwrap().unwrap(), history.clone());
    replay(&mut live, &fresh, &history, cursor);
    h.runtime.shutdown().await;
    assert_eq!(models(&live, agent), models(&fresh, agent));
    assert!(live.transcripts[&agent].stream.is_empty());
    assert_eq!(
        models(&live, agent)
            .iter()
            .filter(|m| matches!(m, CellModel::Assistant { text } if text == TEXT))
            .count(),
        1
    );
}

#[tokio::test]
async fn reconnect_reply_finished_while_disconnected_matches_fresh_without_duplicate_text() {
    disconnected_stream(Ending::Finished).await;
}

#[tokio::test]
async fn reconnect_reply_failed_while_disconnected_matches_fresh_without_stale_stream() {
    disconnected_stream(Ending::Failed).await;
}

#[tokio::test]
async fn reconnect_reply_cancelled_while_disconnected_matches_fresh_without_stale_stream() {
    disconnected_stream(Ending::Cancelled).await;
}
