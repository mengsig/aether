//! A child asks its parent and gets the answer; a root asks before it builds.
use super::*;

const QUESTIONS: usize = 25;
const OPENER: &str = "i want to start a cool new fun zig project, i was thinking we could \
                      build something together. Ok?";

fn sender_of(item: &InboxItem) -> Option<AgentId> {
    match &item.kind {
        aether_core::inbox::InboxKind::Message {
            from: Actor::Agent { id },
            ..
        } => Some(*id),
        _ => None,
    }
}

async fn first_child(h: &Harness, parent: AgentId) -> Agent {
    let children = || {
        h.store
            .agents(&AgentFilter {
                parent: Some(parent),
                ..Default::default()
            })
            .expect("agents")
    };
    h.wait_for("the delegated child", || !children().is_empty())
        .await;
    let mut found = children();
    assert_eq!(found.len(), 1, "one child: {found:?}");
    found.pop().expect("child")
}

fn messages_from(store: &dyn Store, to: AgentId, from: AgentId) -> usize {
    store
        .inbox(to, 0)
        .expect("inbox")
        .iter()
        .filter(|item| sender_of(item) == Some(from))
        .count()
}

/// The parent waits on its child's task; the child sends more questions than
/// any message cap would allow, then waits for the answer. Before questions
/// were urgent, both waited on each other forever.
#[tokio::test]
async fn a_childs_question_wakes_its_waiting_parent_and_the_answer_comes_back() {
    let h = harness(vec![ScriptedClient::tool_call(
        "delegate",
        json!({"model": "anthropic/claude-sonnet-5-5", "effort": "low", "role": "worker",
               "objective": "build the store layer", "wait": true}),
    )])
    .await;
    let parent_turn = h.client.hold_next_response();
    let (parent, _) = h
        .runtime
        .create_agent(spec("build the app"), Actor::user())
        .expect("create");
    let mut asks: Vec<(&str, serde_json::Value)> = (1..=QUESTIONS)
        .map(|n| {
            let content = format!("Question {n}: sqlite or postgres?");
            ("send_message", json!({"to": parent.id, "content": content}))
        })
        .collect();
    asks.push(("wait_for", json!({"message_from": parent.id})));
    h.client.push(ScriptedClient::tool_calls(&asks, None));
    h.wait_for("the parent's first call", || {
        !h.client.requests().is_empty()
    })
    .await;
    let child_turn = h.client.hold_response_at(2);
    parent_turn.cancel();

    let child = first_child(&h, parent.id).await;
    let child_task = child.current_task.expect("delegated task");
    h.client.push(ScriptedClient::tool_calls(
        &[
            ("send_message", json!({"to": child.id, "content": "sqlite"})),
            ("wait_for", json!({"tasks": [child_task]})),
        ],
        None,
    ));
    h.client.push(ScriptedClient::tool_call(
        "complete_task",
        json!({"summary": "stored in sqlite"}),
    ));
    h.client.push(ScriptedClient::tool_call(
        "complete_task",
        json!({"summary": "app built"}),
    ));
    child_turn.cancel();

    let status = h.wait_until(parent.id, |s| s.is_terminal()).await;
    assert_eq!(status.kind(), AgentStatusKind::Completed);
    assert_eq!(
        messages_from(h.store.as_ref(), parent.id, child.id),
        QUESTIONS,
        "every question reached the parent; none was capped"
    );
    let requests = h.client.requests();
    let parent_saw = format!("{:?}", requests[2]);
    assert!(
        parent_saw.contains(&format!("Question {QUESTIONS}:")),
        "{parent_saw}"
    );
    let child_saw = format!("{:?}", requests[3]);
    assert!(
        child_saw.contains("sqlite"),
        "the child got the answer: {child_saw}"
    );
    let reasons = h
        .store
        .events(0, 10_000, &EventFilter::for_agent(parent.id))
        .expect("events")
        .into_iter()
        .filter_map(|e| match e.kind {
            EventKind::AgentResumed { reason, .. } => Some(reason),
            _ => None,
        })
        .collect::<Vec<_>>();
    assert!(
        reasons.iter().any(|r| r.starts_with("question from")),
        "the question woke the parent: {reasons:?}"
    );
}

/// A report from a child that is not waiting on its parent keeps the old
/// rule: it waits until the parent's own condition is satisfied.
#[tokio::test]
async fn a_childs_report_still_waits_for_the_parents_condition() {
    let h = harness(vec![ScriptedClient::tool_call(
        "delegate",
        json!({"model": "anthropic/claude-sonnet-5-5", "effort": "low", "role": "worker",
               "objective": "build the store layer", "wait": true}),
    )])
    .await;
    let parent_turn = h.client.hold_next_response();
    let (parent, _) = h
        .runtime
        .create_agent(spec("build the app"), Actor::user())
        .expect("create");
    h.client.push(ScriptedClient::tool_calls(
        &[
            (
                "send_message",
                json!({"to": parent.id, "content": "halfway"}),
            ),
            ("ask_user", json!({"question": "which colour?"})),
        ],
        None,
    ));
    parent_turn.cancel();
    let child = first_child(&h, parent.id).await;
    h.wait_until(child.id, |s| matches!(s, AgentStatus::Waiting { .. }))
        .await;
    let calls = h.client.requests().len();
    tokio::time::sleep(Duration::from_millis(600)).await;
    assert_eq!(h.client.requests().len(), calls, "the report woke nobody");
    assert!(matches!(
        h.store
            .agent(parent.id)
            .expect("agent")
            .expect("exists")
            .status,
        AgentStatus::Waiting { .. }
    ));
}

fn system_text(request: &aether_core::model::ModelRequest) -> String {
    request
        .system
        .iter()
        .map(|b| b.text.as_str())
        .collect::<Vec<_>>()
        .join("\n")
}

fn tool_events(store: &dyn Store, id: AgentId) -> usize {
    types(store, id)
        .iter()
        .filter(|t| t.starts_with("tool."))
        .count()
}

/// The inbox 533 opener: the root is told to answer with options and one
/// question, and a reply that does so runs no tool and writes nothing.
#[tokio::test]
async fn an_open_opener_gets_options_and_a_question_and_no_writes() {
    let reply = "Fun! A few directions:\n1. A terminal Tetris-like game\n2. A tiny HTTP \
                 server\n3. A JSON pretty-printer\nWhich one appeals to you?";
    let h = harness(vec![ScriptedClient::text(reply)]).await;
    let files_before = std::fs::read_dir(&h.workspace_root)
        .expect("workspace")
        .count();
    let (agent, _) = h
        .runtime
        .create_agent(idle_spec(), Actor::user())
        .expect("create");
    h.runtime
        .send_message(agent.id, vec![ContentBlock::text(OPENER)], Actor::user())
        .expect("send");
    h.wait_for("the opener's reply", || h.client.remaining() == 0)
        .await;
    h.wait_until(agent.id, |s| matches!(s, AgentStatus::Waiting { .. }))
        .await;

    let requests = h.client.requests();
    assert_eq!(requests.len(), 1, "one reply, no tool loop");
    let system = system_text(&requests[0]);
    assert!(
        system.contains("2-4 concrete options and one question"),
        "{system}"
    );
    assert!(
        system.contains("Write no files and start no builds"),
        "{system}"
    );
    assert!(format!("{:?}", requests[0].messages).contains("Ok?"));
    assert_eq!(tool_events(h.store.as_ref(), agent.id), 0);
    let files_after = std::fs::read_dir(&h.workspace_root)
        .expect("workspace")
        .count();
    assert_eq!(
        files_after, files_before,
        "nothing was written to the workspace"
    );
}

/// A fully specified request still acts at once: the same wiring runs the
/// tool call it answers with.
#[tokio::test]
async fn a_fully_specified_request_still_acts() {
    let h = harness(vec![
        ScriptedClient::tool_call(
            "write_file",
            json!({"path": "hello.zig", "content": "pub fn main() void {}\n"}),
        ),
        ScriptedClient::text("Created hello.zig."),
    ])
    .await;
    let (agent, _) = h
        .runtime
        .create_agent(idle_spec(), Actor::user())
        .expect("create");
    let request = "Create hello.zig in the workspace root with an empty main function.";
    h.runtime
        .send_message(agent.id, vec![ContentBlock::text(request)], Actor::user())
        .expect("send");
    h.wait_for("the file", || h.workspace_root.join("hello.zig").exists())
        .await;
    h.wait_until(agent.id, |s| matches!(s, AgentStatus::Waiting { .. }))
        .await;
    let system = system_text(&h.client.requests()[0]);
    assert!(
        system.contains("A fully specified request starts at once"),
        "{system}"
    );
    assert!(tool_events(h.store.as_ref(), agent.id) > 0);
}

/// A delegated child gets the how-to-ask guidance, naming its parent, and
/// not the person-facing opener rule.
#[tokio::test]
async fn a_delegated_child_is_told_how_to_ask_its_parent() {
    let h = harness(vec![]).await;
    let (root, _) = h
        .runtime
        .create_agent(idle_spec(), Actor::user())
        .expect("root");
    let mut child_spec = idle_spec();
    child_spec.parent = Some(root.id);
    let (child, _) = h
        .runtime
        .create_agent(child_spec, Actor::user())
        .expect("child");
    let prompt = aether_context::render::role_task(&child, None, Some(&root));
    assert!(
        prompt.contains(&format!("send_message to {}", root.id)),
        "{prompt}"
    );
    assert!(
        prompt.contains(&format!("wait_for with message_from {}", root.id)),
        "{prompt}"
    );
    assert!(prompt.contains("deviation from an approved requirement always asks"));
    assert!(!prompt.contains("2-4 concrete options"), "{prompt}");
}
