#![allow(clippy::expect_used, clippy::unwrap_used, clippy::panic)]

use super::*;
use crate::app::transcript::CellModel;
use aether_core::agent::{Agent, AgentStatus, WaitCondition};
use aether_core::event::{Actor, EventDraft, EventKind};
use aether_core::id::{AgentId, ToolCallId};
use aether_core::model::{ContentBlock, MessageRole, Usage};
use aether_core::time::now;
use aether_store::{Commit, Mutation, SqliteStore, Store};

#[path = "reconnect_tests/fixture.rs"]
mod fixture;
#[path = "reconnect_tests/in_flight.rs"]
mod in_flight;
#[path = "reconnect_tests/late_replies.rs"]
mod late_replies;
#[path = "reconnect_tests/state.rs"]
mod state;
use fixture::{Fixture, models};

fn message(text: &str) -> EventKind {
    EventKind::AgentMessage {
        role: MessageRole::Assistant,
        content: vec![ContentBlock::text(text)],
        invocation: None,
    }
}

#[tokio::test]
async fn reconnect_replays_offline_messages_in_order_exactly_once() {
    let mut fixture = Fixture::new().await;
    fixture.commit(vec![message("before outage")]);
    let mut live = fixture.fresh().await;
    let cursor = live.last_seq.unwrap();
    live.reduce(AppEvent::Disconnected { because: None });
    fixture.commit(
        (0..650)
            .map(|n| message(&format!("offline {n:04}")))
            .collect(),
    );
    fixture.reconnect(&mut live).await;
    let fresh = fixture.fresh().await;
    assert!(
        live.last_seq.unwrap() > cursor + 500,
        "crosses a subscription replay page"
    );
    assert_eq!(
        models(&live, fixture.agent.id),
        models(&fresh, fixture.agent.id)
    );
    assert_eq!(
        models(&live, fixture.agent.id)
            .iter()
            .filter(|m| matches!(m, CellModel::Assistant { .. }))
            .count(),
        651
    );
    fixture.stop().await;
}

#[tokio::test]
async fn reconnect_catches_up_more_than_one_recent_history_window() {
    let mut fixture = Fixture::new().await;
    let mut live = fixture.fresh().await;
    live.reduce(AppEvent::Disconnected { because: None });
    fixture.commit(
        (0..5_100)
            .map(|n| message(&format!("offline {n:04}")))
            .collect(),
    );
    fixture.reconnect(&mut live).await;
    let mut fresh = fixture.fresh().await;
    let live_messages = fixture.load_all(&mut live).await;
    let fresh_messages = fixture.load_all(&mut fresh).await;
    assert_eq!(live_messages, fresh_messages);
    assert_eq!(
        live_messages,
        (0..5_100)
            .map(|n| format!("offline {n:04}"))
            .collect::<Vec<_>>()
    );
    fixture.stop().await;
}

#[tokio::test]
async fn reconnect_offline_archive_adopts_the_same_remaining_root() {
    let mut fixture = Fixture::new().await;
    let mut live = fixture.fresh().await;
    live.reduce(AppEvent::Disconnected { because: None });
    let mut successor = fixture.agent.clone();
    successor.id = AgentId::new();
    successor.name = "remaining root".into();
    fixture.put_agent(successor.clone());
    fixture.agent.archived = true;
    fixture.put_agent(fixture.agent.clone());
    fixture.commit(vec![EventKind::AgentArchived { archived: true }]);
    fixture.reconnect(&mut live).await;
    let fresh = fixture.fresh().await;
    assert_eq!(live.selected, Some(successor.id));
    assert_eq!(live.selected, fresh.selected);
    assert_eq!(live.tree, fresh.tree);
    fixture.stop().await;
}

#[tokio::test]
async fn reconnect_offline_restart_matches_fresh_selection_and_model_target() {
    let mut fixture = Fixture::new().await;
    let mut live = fixture.fresh().await;
    live.reduce(AppEvent::Disconnected { because: None });
    let mut successor = fixture.agent.clone();
    successor.id = AgentId::new();
    successor.restarted_from = Some(fixture.agent.id);
    fixture.agent.status = AgentStatus::Failed {
        error: "old turn failed".into(),
    };
    fixture.agent.restarted_as = Some(successor.id);
    fixture.put_agent(fixture.agent.clone());
    fixture.put_agent(successor.clone());
    fixture.commit(vec![EventKind::AgentRestarted {
        from: fixture.agent.id,
        to: successor.id,
        checkpoint: None,
        transcript_tail: 0,
        transcript_dropped: 0,
        inbox_carried: 0,
        dropped_deadline: None,
    }]);
    fixture.reconnect(&mut live).await;
    let fresh = fixture.fresh().await;
    assert_eq!(live.selected, fresh.selected);
    assert_eq!(live.model_target(), Some(successor.id));
    assert_eq!(live.model_target(), fresh.model_target());
    fixture.stop().await;
}

#[tokio::test]
async fn reconnect_same_database_after_daemon_restart_keeps_valid_cursor() {
    let mut fixture = Fixture::new().await;
    fixture.commit(vec![message("before restart")]);
    let mut live = fixture.fresh().await;
    let instance = live.options.instance_id.clone();
    let cursor = live.last_seq;
    live.reduce(AppEvent::Disconnected { because: None });
    fixture.restart().await;
    fixture.commit(vec![message("after restart")]);
    fixture.reconnect(&mut live).await;
    let fresh = fixture.fresh().await;
    assert_eq!(live.options.instance_id, instance);
    assert!(live.last_seq > cursor);
    assert_eq!(
        models(&live, fixture.agent.id),
        models(&fresh, fixture.agent.id)
    );
    fixture.stop().await;
}

#[tokio::test]
async fn reconnect_different_database_discards_the_old_cursor_and_transcript() {
    let mut original = Fixture::new().await;
    original.commit(vec![message("old database")]);
    let mut live = original.fresh().await;
    let old_instance = live.options.instance_id.clone();
    live.input.text = "old draft".into();
    live.reduce(AppEvent::Disconnected { because: None });
    let mut replacement = Fixture::new().await;
    replacement.commit(vec![message("new database")]);
    replacement.reconnect(&mut live).await;
    let fresh = replacement.fresh().await;
    assert_ne!(live.options.instance_id, old_instance);
    assert_eq!(live.selected, fresh.selected);
    assert_eq!(live.tree, fresh.tree);
    assert!(!live.transcripts.contains_key(&original.agent.id));
    assert!(live.input.text.is_empty());
    assert_eq!(
        models(&live, replacement.agent.id),
        models(&fresh, replacement.agent.id)
    );
    original.stop().await;
    replacement.stop().await;
}
