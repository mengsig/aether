//! Failed streamed reasoning must survive a reload from durable history.
use super::*;
use aether_core::model::{ContentBlock, ModelErrorClass};
use aether_protocol::Delta;
use aether_store::{Commit, SqliteStore, Store};

const REASONING: &str = "**Checking** the interrupted response";

fn stream(app: &mut App, id: AgentId, invocation: InvocationId, delta: Delta) {
    app.reduce(AppEvent::Frame(SubscriptionFrame::Delta {
        agent: id,
        invocation,
        delta,
    }));
}

fn reload(store: &SqliteStore, row: Agent) -> App {
    let id = row.id;
    let mut app = app_with_status(TuiOptions {
        focus: Some(id),
        ..Default::default()
    });
    app.reduce(AppEvent::Connected);
    let effects = app.reduce(AppEvent::Reply {
        then: Then::Tree,
        result: Ok(Reply::Tree(tree(row))),
    });
    let (query, limit) = effects
        .into_iter()
        .find_map(|effect| match effect {
            Effect::Request {
                command: Command::GetEvents { query, limit, .. },
                then: Then::Backfill { agent, .. },
            } if agent == id => Some((query, limit)),
            _ => None,
        })
        .expect("selecting the agent requests its history");
    assert_eq!(query.agent, Some(id));
    let before = query.before.expect("history cursor");
    let events = store
        .events_before(before, limit, &query.into())
        .expect("stored history");
    assert_eq!(events.len(), 3);
    app.reduce(AppEvent::Reply {
        then: crate::testing::backfill_then(&app, id),
        result: Ok(Reply::Events(events)),
    });
    assert!(app.transcripts[&id].loaded);
    app
}

fn failed_events(
    store: &SqliteStore,
    id: AgentId,
    invocation: InvocationId,
    text: &str,
) -> Vec<Event> {
    let mut partial = vec![ContentBlock::Thinking {
        text: REASONING.into(),
        signature: None,
        redacted: false,
    }];
    if !text.is_empty() {
        partial.push(ContentBlock::text(text));
    }
    let events = store
        .commit(Commit {
            events: [
                EventKind::AgentStarted { turn: 1 },
                EventKind::InvocationFailed {
                    invocation,
                    snapshot: aether_core::id::SnapshotId::new(),
                    error: "response refused mid-stream".into(),
                    class: Some(ModelErrorClass::InvalidRequest),
                    retryable: false,
                    cancelled: false,
                    partial,
                },
                EventKind::AgentFailed {
                    error: "model error: response refused mid-stream".into(),
                },
            ]
            .into_iter()
            .map(|kind| EventDraft::new(Actor::System, kind).agent(id))
            .collect(),
            mutations: Vec::new(),
        })
        .expect("commit failed turn");
    assert_eq!(events.len(), 3);
    assert!(events.windows(2).all(|pair| pair[0].seq < pair[1].seq));
    events
}

fn failed_stream(text: &str) -> (App, App) {
    let (mut live, id) = app_with_agent();
    let invocation = InvocationId::new();
    let store = SqliteStore::open_in_memory().expect("store");
    let events = failed_events(&store, id, invocation, text);
    live.reduce(AppEvent::Frame(SubscriptionFrame::Event(Box::new(
        events[0].clone(),
    ))));
    stream(&mut live, id, invocation, Delta::TurnStarted { turn: 1 });
    stream(
        &mut live,
        id,
        invocation,
        Delta::Thinking {
            text: REASONING.into(),
        },
    );
    stream(&mut live, id, invocation, Delta::Text { text: text.into() });
    assert_eq!(live.transcripts[&id].stream.reasoning, REASONING);
    assert_eq!(live.transcripts[&id].stream.text, text);
    stream(&mut live, id, invocation, Delta::TurnFinished);
    for event in &events[1..] {
        live.reduce(AppEvent::Frame(SubscriptionFrame::Event(Box::new(
            event.clone(),
        ))));
    }
    let mut row = agent(id, "root");
    row.status = AgentStatus::Failed {
        error: "model error: response refused mid-stream".into(),
    };
    live.reduce(AppEvent::Reply {
        then: Then::Tree,
        result: Ok(Reply::Tree(tree(row.clone()))),
    });
    (live, reload(&store, row))
}

#[test]
fn failed_reasoning_only_stream_matches_a_fresh_history_reload() {
    let (mut live, mut reloaded) = failed_stream("");
    for app in [&mut live, &mut reloaded] {
        app.reduce(ctrl_key(KeyCode::Char('t')));
        assert!(app.details);
        let frame = crate::testing::render_to(app, 120, 40);
        assert_eq!(
            frame.matches("the interrupted response").count(),
            1,
            "{frame}"
        );
    }
    assert_eq!(models(&live), models(&reloaded));
    assert!(
        models(&live)
            .iter()
            .any(|model| matches!(model, CellModel::Reasoning { text, .. } if text == REASONING))
    );
    let id = live.selected.expect("viewed agent");
    assert!(live.transcripts[&id].stream.is_empty());
    assert_eq!(
        live.tree.nodes[&id].agent.status,
        reloaded.tree.nodes[&id].agent.status
    );
}

#[test]
fn failed_reasoning_and_text_are_both_kept_once_after_reload() {
    let (live, reloaded) = failed_stream("A partial answer");
    assert_eq!(models(&live), models(&reloaded));
    let kept: Vec<_> = models(&live)
        .into_iter()
        .filter(|model| {
            matches!(
                model,
                CellModel::Reasoning { .. } | CellModel::Assistant { .. }
            )
        })
        .collect();
    assert_eq!(
        kept,
        vec![
            CellModel::Reasoning {
                title: Some("Checking".into()),
                text: REASONING.into(),
                duration_ms: None
            },
            CellModel::Assistant {
                text: "A partial answer".into()
            },
        ]
    );
}
