//! The TUI's view of one agent, built live from the runtime's bus and again
//! from its stored history, so a test can hold the two to each other.
use std::time::Duration;

use aether_client::SubscriptionFrame;
use aether_core::agent::Agent;
use aether_core::event::Event;
use aether_core::id::AgentId;
use aether_core::model::Usage;
use aether_protocol::{AgentTree, Delta, Reply, TreeNode};
use aether_runtime::{BusMessage, DeltaKind};
use aether_tui::app::Then;
use aether_tui::app::transcript::CellModel;
use aether_tui::{App, AppEvent, TuiOptions};
use tokio::sync::broadcast;

fn frame(message: BusMessage) -> SubscriptionFrame {
    match message {
        BusMessage::Event(event) => SubscriptionFrame::Event(Box::new((*event).clone())),
        BusMessage::Delta {
            agent,
            invocation,
            delta,
        } => SubscriptionFrame::Delta {
            agent,
            invocation,
            delta: match delta {
                DeltaKind::TurnStarted { turn } => Delta::TurnStarted { turn },
                DeltaKind::Text { text } => Delta::Text { text },
                DeltaKind::Thinking { text } => Delta::Thinking { text },
                DeltaKind::ToolInput { name, partial_json } => {
                    Delta::ToolInput { name, partial_json }
                }
                DeltaKind::Accounting {
                    total,
                    turn_usage,
                    context_tokens,
                    ..
                } => Delta::Accounting {
                    total,
                    turn_usage,
                    context_tokens,
                },
                DeltaKind::TurnFinished => Delta::TurnFinished,
            },
        },
    }
}

/// Reduce one live bus message the way the TUI receives it.
pub(crate) fn reduce(app: &mut App, message: BusMessage) {
    app.reduce(AppEvent::Frame(frame(message)));
}

pub(crate) fn projection(agent: Agent, history: Vec<Event>) -> App {
    let id = agent.id;
    assert!(history.iter().all(|event| event.agent_id == Some(id)));
    assert!(history.windows(2).all(|pair| pair[0].seq < pair[1].seq));
    let mut app = App::new(TuiOptions {
        focus: Some(id),
        ..Default::default()
    });
    let node = TreeNode {
        agent,
        children: vec![],
        task: None,
        waiting_on: vec![],
        lease: None,
        subtree_usage: Usage::default(),
        hidden_usage: None,
    };
    app.reduce(AppEvent::Connected);
    app.reduce(AppEvent::Reply {
        then: Then::Tree,
        result: Ok(Reply::Tree(AgentTree {
            roots: vec![id],
            nodes: [(id, node)].into(),
        })),
    });
    app.reduce(AppEvent::Reply {
        then: aether_tui::testing::backfill_then(&app, id),
        result: Ok(Reply::Events(history)),
    });
    assert!(app.transcripts[&id].loaded);
    app
}

pub(crate) fn models(app: &App, agent: AgentId) -> Vec<CellModel> {
    app.transcripts[&agent]
        .all_cells()
        .map(|cell| cell.model.clone())
        .collect()
}

/// Reduce live frames through the agent's event `last`. The store is written
/// before the bus, so a store read can be ahead of the frames already sent.
pub(crate) async fn drain_through(
    bus: &mut broadcast::Receiver<BusMessage>,
    app: &mut App,
    agent: AgentId,
    last: u64,
) {
    loop {
        let message = tokio::time::timeout(Duration::from_secs(5), bus.recv())
            .await
            .expect("the agent's last event never reached the bus")
            .expect("lost live events");
        let reached = matches!(&message, BusMessage::Event(event)
            if event.agent_id == Some(agent) && event.seq >= last);
        reduce(app, message);
        if reached {
            return;
        }
    }
}
