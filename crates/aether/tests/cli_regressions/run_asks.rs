//! `aether run` hears what the root's delegated agents ask, and `--yes`
//! answers them as it answers the root, but no agent outside the root's tree.
use super::{AGENT, AT, RunEnd, Stub, agent, cli, stderr, stdout};
use aether_core::agent::Agent;
use aether_core::event::{Actor, Event, EventDraft, EventKind};
use aether_core::id::{AgentId, ApprovalId, EventId, InvocationId, ToolCallId};
use aether_core::tool::{ApprovalDecision, ApprovalRequest, ApprovalStatus, SideEffectClass};
use aether_protocol::{Delta, ServerFrame};

const CHILD: &str = "agt_01J000000000000000000000C1";
const GRANDCHILD: &str = "agt_01J000000000000000000000C2";
/// In the run, but placed there by another client: not the root's.
const STRANGER: &str = "agt_01J000000000000000000000S1";
const CHILD_APPROVAL: &str = "apr_01J000000000000000000000A1";
const GRANDCHILD_APPROVAL: &str = "apr_01J000000000000000000000A2";
const STRANGER_APPROVAL: &str = "apr_01J000000000000000000000A3";

fn id(text: &str) -> AgentId {
    text.parse().unwrap()
}

/// The stub's other agents of the run, with their parents.
pub(super) fn member(wanted: AgentId) -> Option<Agent> {
    let parent = [
        (CHILD, Some(AGENT)),
        (GRANDCHILD, Some(CHILD)),
        (STRANGER, None),
    ]
    .into_iter()
    .find(|(member, _)| id(member) == wanted)?
    .1;
    let mut found = agent();
    found.id = wanted;
    found.parent = parent.map(id);
    Some(found)
}

fn from(member: &str, kind: EventKind) -> ServerFrame {
    let root = agent();
    let parent = self::member(id(member)).unwrap().parent;
    ServerFrame::Event {
        sub: 1,
        event: Event::from_draft(
            EventDraft::new(Actor::System, kind)
                .agent(id(member))
                .parent(parent)
                .run(root.run),
            EventId::new(),
            1,
            AT.parse().unwrap(),
        ),
    }
}

fn asks(approval: &str, tool: &str) -> EventKind {
    EventKind::ApprovalRequested {
        approval: approval.parse().unwrap(),
        call: ToolCallId::new(),
        tool: tool.into(),
        description: format!("{tool} call"),
        argument: None,
        side_effect: SideEffectClass::External,
    }
}

/// What the other agents do before the root completes: ask, stream, finish.
pub(super) fn family_frames() -> Vec<ServerFrame> {
    vec![
        from(CHILD, asks(CHILD_APPROVAL, "web_fetch")),
        from(GRANDCHILD, asks(GRANDCHILD_APPROVAL, "bash")),
        from(STRANGER, asks(STRANGER_APPROVAL, "deploy")),
        from(
            CHILD,
            EventKind::QuestionAsked {
                question: "Which branch?".into(),
            },
        ),
        ServerFrame::Delta {
            sub: 1,
            agent: id(CHILD),
            invocation: InvocationId::new(),
            delta: Delta::Text {
                text: "CHILD-TEXT".into(),
            },
        },
        from(
            CHILD,
            EventKind::AgentCompleted {
                summary: Some("child finished".into()),
            },
        ),
    ]
}

/// Only the root's descendants may be approved, and only approved.
pub(super) fn check_resolution(approval: ApprovalId, decision: ApprovalDecision) {
    assert_eq!(decision, ApprovalDecision::Approved);
    assert!(
        [CHILD_APPROVAL, GRANDCHILD_APPROVAL].contains(&approval.to_string().as_str()),
        "{approval} is not the root's to approve"
    );
}

pub(super) fn root_approval() -> ApprovalRequest {
    ApprovalRequest {
        id: ApprovalId::new(),
        agent_id: id(AGENT),
        task_id: None,
        call_id: ToolCallId::new(),
        tool: "Bash".into(),
        description: "make deploy".into(),
        argument: None,
        resources: Vec::new(),
        side_effect: SideEffectClass::Mutation,
        requested_at: AT.parse().unwrap(),
        status: ApprovalStatus::Pending,
    }
}

/// The root completes after everything its family asked.
fn family_run() -> Stub {
    let stub = Stub {
        run_family: true,
        ..Default::default()
    };
    assert!(matches!(stub.end, RunEnd::Completed));
    stub
}

#[tokio::test]
async fn yes_approves_the_root_descendants_and_no_other_agent_of_the_run() {
    let out = cli(
        &[
            "run",
            "hello",
            "--model",
            "mock/m1",
            "--network=false",
            "--yes",
        ],
        family_run(),
    )
    .await;
    let text = stderr(&out);
    assert!(out.status.success(), "{text}");
    for member in [CHILD, GRANDCHILD] {
        assert!(text.contains(&format!("for agent {member}")), "{text}");
    }
    assert_eq!(text.matches("approved ").count(), 2, "{text}");
    assert!(
        text.contains(&format!("(agent {STRANGER}, not started by this run)"))
            && text.contains(&format!("approve {STRANGER_APPROVAL}")),
        "{text}"
    );
    assert!(text.contains(&format!("send {CHILD} ")), "{text}");
    // The child's end and text are its own; the command follows the root.
    assert!(!text.contains("child finished"), "{text}");
    assert!(text.contains("done: done"), "{text}");
    assert!(!stdout(&out).contains("CHILD-TEXT"));
}

#[tokio::test]
async fn yes_watches_on_when_a_descendant_approval_already_expired() {
    let out = cli(
        &[
            "run",
            "hello",
            "--model",
            "mock/m1",
            "--network=false",
            "--yes",
        ],
        Stub {
            expired_approvals: true,
            ..family_run()
        },
    )
    .await;
    let text = stderr(&out);
    assert!(out.status.success(), "{text}");
    assert!(
        !text.lines().any(|line| line.starts_with("approved ")),
        "{text}"
    );
    for (member, tool) in [(CHILD, "web_fetch"), (GRANDCHILD, "bash")] {
        let line = format!("already resolved, not approved: {tool} for agent {member}");
        assert!(text.contains(&line), "{text}");
    }
    // The stranger was never answered, so it still gets its command.
    assert!(
        text.contains(&format!("approve {STRANGER_APPROVAL}")),
        "{text}"
    );
    assert!(text.contains("done: done"), "{text}");
}

#[tokio::test]
async fn without_yes_every_descendant_approval_prints_its_command() {
    let out = cli(
        &["run", "hello", "--model", "mock/m1", "--network=false"],
        family_run(),
    )
    .await;
    let text = stderr(&out);
    assert!(out.status.success(), "{text}");
    assert!(!text.contains("approved "), "{text}");
    for (member, approval) in [(CHILD, CHILD_APPROVAL), (GRANDCHILD, GRANDCHILD_APPROVAL)] {
        assert!(text.contains(&format!("(agent {member}): ")), "{text}");
        assert!(text.contains(&format!("approve {approval}")), "{text}");
    }
}

#[tokio::test]
async fn an_agent_mid_turn_on_an_approval_lists_as_waiting_on_it() {
    let out = cli(
        &["agents"],
        Stub {
            awaiting_approval: true,
            ..Default::default()
        },
    )
    .await;
    assert!(out.status.success(), "{}", stderr(&out));
    assert!(
        stdout(&out).contains("waiting: approval"),
        "{}",
        stdout(&out)
    );
    let idle = cli(&["agents"], Stub::default()).await;
    assert!(!stdout(&idle).contains("waiting: approval"));
}
