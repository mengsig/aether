//! A named mode draws a conversation's entries as `get_conversation` flags
//! them, whether its events arrived live or as a fresh launch's history.
use super::*;
use aether_core::conversation::{self, Entry};
use aether_core::event::{Actor, Event, EventDraft, EventKind};
use aether_core::id::{AgentId, EventId, InboxItemId, MessageId};
use aether_core::inbox::{InboxItem, InboxKind};
use aether_core::model::{ContentBlock, MessageRole};
use aether_tui::testing::app_with_history;

/// An operator line, an external trigger, a plain note, an important note
/// and the agent's reply, in that order.
fn seeded(agent: AgentId) -> Vec<EventKind> {
    let delivered = |kind| EventKind::InboxDelivered {
        item: InboxItem {
            id: InboxItemId::new(),
            agent_id: agent,
            seq: 1,
            kind,
            delivered_at: aether_core::time::now(),
        },
    };
    let operator = delivered(InboxKind::Message {
        message_id: MessageId::new(),
        from: Actor::user(),
        content: vec![ContentBlock::text("ship the fix")],
    });
    let EventKind::InboxDelivered { item } = &operator else {
        unreachable!("built as a delivery");
    };
    let consumed = EventKind::InboxConsumed {
        up_to: 1,
        items: vec![item.id],
    };
    let trigger = delivered(InboxKind::ExternalTrigger {
        source: "archon".into(),
        payload: serde_json::json!({"item": "qi_1", "stage": "review"}),
        operator: false,
    });
    vec![
        operator,
        consumed,
        trigger,
        note("**Progress** (qi_1)", false),
        note("**Item shipped**\n\nMerged into next.", true),
        EventKind::AgentMessage {
            role: MessageRole::Assistant,
            content: vec![ContentBlock::text("The fix is in review.")],
            invocation: None,
        },
    ]
}

fn numbered(agent: AgentId, kinds: Vec<EventKind>) -> Vec<Event> {
    let at = aether_core::time::now();
    (1..)
        .zip(kinds)
        .map(|(seq, kind)| {
            let draft = EventDraft::new(Actor::System, kind).agent(agent);
            Event::from_draft(draft, EventId::new(), seq, at)
        })
        .collect()
}

/// `get_conversation(important_only)`'s fold of `events`, by entry text.
fn important_entries(events: &[Event]) -> Vec<String> {
    let delivered: Vec<InboxItem> = events
        .iter()
        .filter_map(|event| match &event.kind {
            EventKind::InboxDelivered { item } => Some(item.clone()),
            _ => None,
        })
        .collect();
    let entries = events.iter().flat_map(|event| match event.kind {
        EventKind::InboxConsumed { .. } => conversation::consumed_entries(event, &delivered),
        _ => conversation::entries(event),
    });
    let texts: Vec<String> = entries
        .filter(|entry: &Entry| entry.important)
        .map(post_text)
        .collect();
    assert!(!texts.is_empty(), "the seed says something important");
    texts
}

/// An entry as a cell reads it: a post lists its links under its text.
fn post_text(entry: Entry) -> String {
    if entry.links.is_empty() {
        return entry.text;
    }
    let links: Vec<String> = entry.links.iter().map(|link| format!("- {link}")).collect();
    format!("{}\n\n{}", entry.text, links.join("\n"))
}

/// The text of each entry the viewed agent's mode draws, in order.
fn drawn_entries(app: &App) -> Vec<String> {
    let agent = app.selected.expect("an agent is viewed");
    let policy = app.policy(Some(agent));
    assert!(policy.definition.is_some(), "a named mode applies");
    app.transcripts[&agent]
        .presentation_cells(true)
        .filter(|cell| policy.shows(&cell.model))
        .filter(|cell| !matches!(cell.model, CellModel::Header { .. }))
        .map(|cell| match &cell.model {
            CellModel::User { text, .. }
            | CellModel::Assistant { text }
            | CellModel::Post { text }
            | CellModel::Question { text, .. } => text.clone(),
            other => format!("{other:?}"),
        })
        .collect()
}

fn live_app() -> App {
    let mut app = app_with_history(|_| Vec::new());
    let agent = app.selected.unwrap();
    for kind in seeded(agent) {
        own_event(&mut app, kind);
    }
    app
}

fn fresh_app() -> (App, Vec<Event>) {
    let mut history = Vec::new();
    let app = app_with_history(|agent| {
        history = numbered(agent, seeded(agent));
        history.clone()
    });
    (app, history)
}

fn assert_parity(enter: fn(&mut App)) {
    let mut live = live_app();
    let (mut fresh, history) = fresh_app();
    enter(&mut live);
    enter(&mut fresh);
    let expected = important_entries(&history);
    assert_eq!(drawn_entries(&live), expected, "live");
    assert_eq!(drawn_entries(&fresh), expected, "fresh launch");
    let (live, fresh) = (render_to(&live, 100, 30), render_to(&fresh, 100, 30));
    assert_eq!(live, fresh);
    for hidden in ["Trigger from archon", "Progress", "qi_1"] {
        assert!(!fresh.contains(hidden), "{hidden}: {fresh}");
    }
}

#[test]
fn a_fresh_launch_in_focus_draws_exactly_the_important_entries() {
    assert_parity(|app| {
        submit(app, "/focus");
        assert_eq!(app.view_mode, "focus");
    });
}

#[test]
fn a_fresh_launch_in_a_mode_showing_status_draws_exactly_the_important_entries() {
    assert_parity(enter_calm);
}

#[test]
fn the_default_view_still_draws_the_trigger_and_the_plain_note() {
    let (fresh, _) = fresh_app();
    let screen = render_to(&fresh, 100, 30);
    for shown in [
        "Trigger from archon",
        "Progress",
        "Item shipped",
        "ship the fix",
    ] {
        assert!(screen.contains(shown), "{shown}: {screen}");
    }
}
