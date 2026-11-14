use super::*;
use crate::id::{AgentId, EventId, InboxItemId, InvocationId, MessageId, RunId};
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

/// The entries of a message the agent read: none at its delivery, then its
/// own at the `inbox.consumed` that read it.
fn read(from: Actor, text: &str) -> Vec<Entry> {
    let EventKind::InboxDelivered { item } = delivered(InboxKind::Message {
        message_id: MessageId::new(),
        from: from.clone(),
        content: vec![ContentBlock::text(text)],
    }) else {
        unreachable!("delivered builds an inbox.delivered event");
    };
    let arrival = EventKind::InboxDelivered { item: item.clone() };
    assert!(entries(&event(from, arrival)).is_empty(), "unread yet");
    let consumed = EventKind::InboxConsumed {
        up_to: item.seq,
        items: vec![item.id],
    };
    let entries = consumed_entries(&event(Actor::System, consumed), &[item]);
    assert!(entries.iter().all(|entry| entry.seq == 7), "{entries:?}");
    entries
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
    triples(entries(event))
}

fn triples(entries: Vec<Entry>) -> Vec<(EntryKind, bool, String)> {
    entries
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
        (read(Actor::user(), "ship it"), Operator, true),
        (
            entries(&event(Actor::user(), task("build the thing"))),
            Operator,
            true,
        ),
        (read(agent.clone(), "done"), Message, false),
        (
            entries(&event(agent.clone(), task("review"))),
            Message,
            false,
        ),
        (entries(&event(Actor::System, trigger)), Trigger, false),
        (
            entries(&event(
                agent.clone(),
                EventKind::QuestionAsked {
                    question: "merge?".into(),
                },
            )),
            Question,
            true,
        ),
        (
            entries(&event(Actor::user(), post("item shipped", true))),
            Post,
            true,
        ),
        (
            entries(&event(Actor::user(), post("cache pruned", false))),
            Post,
            false,
        ),
    ];
    for (said, kind, important) in cases {
        let got = triples(said);
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
    assert!(read(Actor::user(), "  \n").is_empty());
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
    let got = read(Actor::user(), "hi");
    let wire = serde_json::to_value(&got[0]).unwrap();
    assert!(wire.get("links").is_none(), "{wire}");
    assert_eq!(wire["kind"], "operator");
    assert_eq!(wire["important"], true);
}

fn failed(cancelled: bool, retryable: bool, class: Option<ModelErrorClass>) -> EventKind {
    EventKind::InvocationFailed {
        invocation: InvocationId::new(),
        snapshot: crate::id::SnapshotId::new(),
        error: "stopped".into(),
        class,
        retryable,
        cancelled,
        partial: vec![
            ContentBlock::Thinking {
                text: "weighing it".into(),
                signature: None,
                redacted: false,
            },
            ContentBlock::text("half an answer"),
            ContentBlock::ToolUse {
                id: "t1".into(),
                name: "bash".into(),
                input: json!({"cmd": "ls"}),
            },
        ],
    }
}

/// A stopped or unretried turn's partial reads as said, without its unfinished
/// calls; a turn that is retried says it again, so its partial adds nothing.
#[test]
fn a_failed_turn_keeps_its_partial_text_unless_it_is_retried() {
    use EntryKind::*;
    let said = vec![
        (Thinking, false, "weighing it".to_owned()),
        (Assistant, true, "half an answer".to_owned()),
    ];
    let stopped = event(Actor::System, failed(true, false, None));
    assert_eq!(kinds(&stopped), said);
    assert!(cancelled_invocation(&stopped).is_some());
    let broken = event(Actor::System, failed(false, false, None));
    assert_eq!(kinds(&broken), said);
    assert_eq!(cancelled_invocation(&broken), None);
    let limited = failed(false, true, Some(ModelErrorClass::UsageLimit));
    assert_eq!(kinds(&event(Actor::System, limited)), said);
    let retried = failed(false, true, Some(ModelErrorClass::Overloaded));
    assert!(entries(&event(Actor::System, retried)).is_empty());
}
