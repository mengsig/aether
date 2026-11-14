//! A named mode draws a conversation's entries as `get_conversation` flags
//! them, whether its events arrived live or as a fresh launch's history.
use super::*;
use aether_core::conversation::{self, Entry};
use aether_core::event::{Actor, Event, EventDraft, EventKind};
use aether_core::id::{AgentId, EventId, InboxItemId, InvocationId, MessageId, SnapshotId};
use aether_core::inbox::{InboxItem, InboxKind};
use aether_core::model::{ContentBlock, MessageRole};
use aether_tui::testing::{app_with_history, app_with_window};

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
    important_entries_of(events, events)
}

/// [`important_entries`] of the `page` of `history`, its consumptions
/// read from the whole inbox and a stop's partial less what the message
/// before it said, as `get_conversation` reads them.
fn important_entries_of(page: &[Event], history: &[Event]) -> Vec<String> {
    let delivered: Vec<InboxItem> = history
        .iter()
        .filter_map(|event| match &event.kind {
            EventKind::InboxDelivered { item } => Some(item.clone()),
            _ => None,
        })
        .collect();
    let entries = page.iter().flat_map(|event| {
        let said = match event.kind {
            EventKind::InboxConsumed { .. } => conversation::consumed_entries(event, &delivered),
            _ => conversation::entries(event),
        };
        let before = said_before(event, history);
        said.into_iter()
            .filter(move |entry| !before.contains(&entry.kind))
    });
    let texts: Vec<String> = entries
        .filter(|entry: &Entry| entry.important)
        .map(post_text)
        .collect();
    assert!(!texts.is_empty(), "the seed says something important");
    texts
}

/// The entry kinds of the message just before a stop of the same turn.
fn said_before(event: &Event, history: &[Event]) -> Vec<conversation::EntryKind> {
    let Some(stopped) = conversation::cancelled_invocation(event) else {
        return Vec::new();
    };
    let message = history
        .iter()
        .rev()
        .filter(|earlier| earlier.seq < event.seq)
        .find(|earlier| matches!(earlier.kind, EventKind::AgentMessage { .. }));
    match message {
        Some(
            message @ Event {
                kind:
                    EventKind::AgentMessage {
                        invocation: Some(said),
                        ..
                    },
                ..
            },
        ) if *said == stopped => conversation::entries(message)
            .into_iter()
            .map(|entry| entry.kind)
            .collect(),
        _ => Vec::new(),
    }
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

fn operator_line(agent: AgentId, seq: u64, text: &str) -> (EventKind, EventKind) {
    let item = InboxItem {
        id: InboxItemId::new(),
        agent_id: agent,
        seq,
        kind: InboxKind::Message {
            message_id: MessageId::new(),
            from: Actor::user(),
            content: vec![ContentBlock::text(text)],
        },
        delivered_at: aether_core::time::now(),
    };
    let consumed = EventKind::InboxConsumed {
        up_to: seq,
        items: vec![item.id],
    };
    (EventKind::InboxDelivered { item }, consumed)
}

/// A fresh launch whose history window holds the last `window` events.
fn focused_window(window: usize, seed: fn(AgentId) -> Vec<EventKind>) -> (App, Vec<Event>) {
    let mut history = Vec::new();
    let mut app = app_with_window(
        |agent| {
            history = numbered(agent, seed(agent));
            history.clone()
        },
        window,
    );
    submit(&mut app, "/focus");
    assert_eq!(app.view_mode, "focus");
    (app, history)
}

#[test]
fn a_fresh_focus_keeps_a_message_delivered_before_its_history_window() {
    // The window starts at the consumption: the delivery lies before it.
    let (app, history) = focused_window(5, seeded);
    let page = &history[1..];
    assert!(matches!(history[0].kind, EventKind::InboxDelivered { .. }));
    let expected = important_entries_of(page, &history);
    assert!(
        expected.contains(&"ship the fix".to_owned()),
        "{expected:?}"
    );
    assert_eq!(drawn_entries(&app), expected);
}

#[test]
fn a_fresh_focus_reads_the_inbox_after_the_consumption_before_its_window() {
    fn seed(agent: AgentId) -> Vec<EventKind> {
        let (first, first_read) = operator_line(agent, 1, "check the logs");
        let (second, second_read) = operator_line(agent, 2, "ship the fix");
        vec![
            first,
            first_read,
            second,
            second_read,
            note("**Item shipped**", true),
        ]
    }
    let (app, history) = focused_window(2, seed);
    let expected = important_entries_of(&history[3..], &history);
    assert_eq!(
        expected.len(),
        2,
        "the second line and the post: {expected:?}"
    );
    assert_eq!(expected[0], "ship the fix");
    assert_eq!(drawn_entries(&app), expected);
}

#[test]
fn a_fresh_focus_keeps_a_stop_drawn_once_when_its_message_is_before_the_window() {
    fn seed(_: AgentId) -> Vec<EventKind> {
        let invocation = InvocationId::new();
        let partial = vec![
            ContentBlock::Thinking {
                text: "pondering".into(),
                signature: None,
                redacted: false,
            },
            ContentBlock::text("half an answer"),
        ];
        vec![
            EventKind::AgentMessage {
                role: MessageRole::Assistant,
                content: vec![ContentBlock::text("half an answer")],
                invocation: Some(invocation),
            },
            EventKind::InvocationFailed {
                invocation,
                snapshot: SnapshotId::new(),
                error: "cancelled".into(),
                class: None,
                retryable: false,
                cancelled: true,
                partial,
            },
            note("**Item shipped**", true),
        ]
    }
    // The window starts at the stop: the message that kept its text lies before it.
    let (app, history) = focused_window(2, seed);
    let expected = important_entries_of(&history[1..], &history);
    assert_eq!(expected.len(), 1, "the stop adds nothing: {expected:?}");
    assert!(expected[0].starts_with("**Item shipped**"));
    assert_eq!(drawn_entries(&app), expected);

    let (whole, history) = focused_window(usize::MAX, seed);
    let expected = important_entries_of(&history, &history);
    assert_eq!(
        expected.len(),
        2,
        "the message, then the post: {expected:?}"
    );
    assert_eq!(expected[0], "half an answer");
    assert_eq!(drawn_entries(&whole), expected);
}
