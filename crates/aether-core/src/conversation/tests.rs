use super::*;
use crate::id::{AgentId, EventId, InboxItemId, MessageId, RunId};
use crate::inbox::InboxItem;
use crate::note::{NoteContext, NoteLevel};
use crate::task::Priority;
use serde_json::json;

fn event(actor: Actor, kind: EventKind) -> Event {
    Event {
        id: EventId::new(),
        seq: 7,
        at: crate::time::now(),
        actor,
        agent_id: None,
        task_id: None,
        parent_agent_id: None,
        run: None,
        causation: None,
        kind,
    }
}

fn delivered(kind: InboxKind) -> EventKind {
    EventKind::InboxDelivered {
        item: InboxItem {
            id: InboxItemId::new(),
            agent_id: AgentId::new(),
            seq: 1,
            kind,
            delivered_at: crate::time::now(),
        },
    }
}

fn message(from: Actor, text: &str) -> EventKind {
    delivered(InboxKind::Message {
        message_id: MessageId::new(),
        from,
        content: vec![ContentBlock::text(text)],
    })
}

fn post(text: &str, important: bool) -> EventKind {
    let note = Note {
        text: text.into(),
        links: vec!["https://example.com/pr/1".into()],
        context: NoteContext::None,
        level: NoteLevel::Info,
        important,
    };
    note.draft(AgentId::new(), RunId::new(), Actor::user())
        .expect("a valid note")
        .kind
}

fn kinds(event: &Event) -> Vec<(EntryKind, bool, String)> {
    entries(event)
        .into_iter()
        .map(|entry| (entry.kind, entry.important, entry.text))
        .collect()
}

#[test]
fn every_entry_kind_carries_its_importance() {
    use EntryKind::*;
    let agent = Actor::agent(AgentId::new());
    let task = |objective: &str| EventKind::TaskCreated {
        objective: objective.into(),
        parent_task: None,
        dependencies: Vec::new(),
        priority: Priority::Normal,
    };
    let trigger = delivered(InboxKind::ExternalTrigger {
        source: "archon".into(),
        payload: json!({"kind": "gate"}),
        operator: false,
    });
    let cases = [
        (
            event(Actor::user(), message(Actor::user(), "ship it")),
            Operator,
            true,
        ),
        (
            event(Actor::user(), task("build the thing")),
            Operator,
            true,
        ),
        (
            event(agent.clone(), message(agent.clone(), "done")),
            Message,
            false,
        ),
        (event(agent.clone(), task("review")), Message, false),
        (event(Actor::System, trigger), Trigger, false),
        (
            event(
                agent.clone(),
                EventKind::QuestionAsked {
                    question: "merge?".into(),
                },
            ),
            Question,
            true,
        ),
        (event(Actor::user(), post("item shipped", true)), Post, true),
        (
            event(Actor::user(), post("cache pruned", false)),
            Post,
            false,
        ),
    ];
    for (event, kind, important) in cases {
        let got = kinds(&event);
        assert_eq!(got.len(), 1, "{kind:?}: {got:?}");
        assert_eq!((got[0].0, got[0].1), (kind, important), "{got:?}");
    }
}

/// Text beside tool calls is the agent's own text: important, while the
/// reasoning, calls and results around it are not.
#[test]
fn an_assistant_message_splits_into_text_thinking_and_calls() {
    use EntryKind::*;
    let said = EventKind::AgentMessage {
        role: MessageRole::Assistant,
        content: vec![
            ContentBlock::Thinking {
                text: "plan".into(),
                signature: None,
                redacted: false,
            },
            ContentBlock::text("Checking the build."),
            ContentBlock::ToolUse {
                id: "c1".into(),
                name: "shell".into(),
                input: json!({"command": "make ci"}),
            },
            ContentBlock::text(" Then I report."),
        ],
        invocation: None,
    };
    let event = event(Actor::System, said);
    let got = entries(&event);
    let shape: Vec<(EntryKind, bool)> = got.iter().map(|e| (e.kind, e.important)).collect();
    assert_eq!(
        shape,
        [(Thinking, false), (Assistant, true), (ToolCall, false)]
    );
    assert_eq!(got[1].text, "Checking the build. Then I report.");
    assert!(got[2].text.starts_with("shell "), "{}", got[2].text);
    let parts: Vec<u32> = got.iter().map(|e| e.part).collect();
    assert_eq!(parts, [0, 1, 2], "each entry of one event has its own id");
    assert!(got.iter().all(|e| e.seq == event.seq));
}

#[test]
fn a_user_message_keeps_tool_results_and_drops_its_rendered_inbox_text() {
    let shown = EventKind::AgentMessage {
        role: MessageRole::User,
        content: vec![
            ContentBlock::text("[inbox 1] Message from operator: ship it"),
            ContentBlock::tool_result_text("c1", "ok", false),
        ],
        invocation: None,
    };
    let got = kinds(&event(Actor::System, shown));
    assert_eq!(got, [(EntryKind::ToolResult, false, "ok".to_owned())]);
}

#[test]
fn a_post_keeps_its_links_and_bulky_detail_is_bounded() {
    let got = entries(&event(Actor::user(), post("item shipped", true)));
    assert_eq!(got[0].links, ["https://example.com/pr/1"]);
    let huge = EventKind::AgentMessage {
        role: MessageRole::User,
        content: vec![ContentBlock::tool_result_text(
            "c1",
            "é".repeat(4000),
            false,
        )],
        invocation: None,
    };
    let got = entries(&event(Actor::System, huge));
    assert!(got[0].text.len() <= MAX_DETAIL_BYTES + '…'.len_utf8());
    assert!(got[0].text.ends_with('…'));
}

#[test]
fn blank_text_and_unrelated_events_add_no_entries() {
    let blank = event(Actor::user(), message(Actor::user(), "  \n"));
    assert!(entries(&blank).is_empty());
    let started = event(Actor::System, EventKind::AgentStarted { turn: 1 });
    assert!(entries(&started).is_empty());
    let other = EventKind::Custom {
        name: "tool.interrupted".into(),
        payload: json!({}),
    };
    assert!(entries(&event(Actor::System, other)).is_empty());
}

#[test]
fn an_entry_without_links_keeps_them_off_the_wire() {
    let got = entries(&event(Actor::user(), message(Actor::user(), "hi")));
    let wire = serde_json::to_value(&got[0]).unwrap();
    assert!(wire.get("links").is_none(), "{wire}");
    assert_eq!(wire["kind"], "operator");
    assert_eq!(wire["important"], true);
}
