use aether_core::event::EventDraft;
use aether_core::id::{EventId, TaskId, ToolCallId};
use aether_core::task::TaskResult;
use aether_core::tool::{ApprovalDecision, SideEffectClass};

use super::*;
use crate::push::testing::StaticFleet;

const INSTANCE: &str = "inst_test";
const SECRET: &str = "rm -rf /srv/secret-project";

fn event(seq: u64, draft: EventDraft) -> Event {
    Event::from_draft(draft, EventId::new(), seq, aether_core::time::now())
}

fn by_agent(agent: AgentId, kind: EventKind) -> EventDraft {
    EventDraft::new(Actor::agent(agent), kind).agent(agent)
}

fn facts(name: &str, root: bool, block: Option<OperatorBlock>) -> AgentFacts {
    AgentFacts {
        name: name.into(),
        root,
        block,
    }
}

fn approval_requested(approval: ApprovalId) -> EventKind {
    EventKind::ApprovalRequested {
        approval,
        call: ToolCallId::new(),
        tool: "shell".into(),
        description: SECRET.into(),
        argument: Some(SECRET.into()),
        side_effect: SideEffectClass::Mutation,
    }
}

fn composed(event: &Event, fleet: &StaticFleet, previews: bool) -> Option<Notice> {
    compose(event, fleet, INSTANCE, previews).expect("the fleet answers")
}

#[test]
fn a_pending_approval_notifies_with_its_id_and_route() {
    let fleet = StaticFleet::default();
    let agent = AgentId::new();
    let approval = ApprovalId::new();
    fleet.set_agent(agent, facts("builder", false, None));
    fleet.set_pending(approval);
    let event = event(41, by_agent(agent, approval_requested(approval)));

    assert_eq!(classify(&event), Signal::Candidate(AttentionKind::Approval));
    let notice = composed(&event, &fleet, false).expect("a notification");
    let payload = &notice.payload;
    assert_eq!(payload.v, 1);
    assert_eq!(payload.kind, AttentionKind::Approval);
    assert_eq!(payload.instance_id, INSTANCE);
    assert_eq!(payload.seq, 41);
    assert_eq!(payload.at, event.at);
    assert_eq!(payload.title, "Approval needed");
    assert_eq!(payload.url, format!("#attention?item={approval}"));
    assert_eq!(payload.agent, Some(agent));
    assert_eq!(payload.agent_name.as_deref(), Some("builder"));
    assert_eq!(payload.approval, Some(approval));
    assert_eq!(payload.question_seq, None);
    assert_eq!(payload.preview, None);
    assert_eq!(notice.urgency, Urgency::High);
    assert_eq!(notice.origin, None);
}

#[test]
fn an_approval_a_rule_already_answered_notifies_nobody() {
    let fleet = StaticFleet::default();
    let agent = AgentId::new();
    let event = event(7, by_agent(agent, approval_requested(ApprovalId::new())));
    assert_eq!(composed(&event, &fleet, true), None);
}

#[test]
fn a_question_carries_the_sequence_a_client_answers_with() {
    let fleet = StaticFleet::default();
    let agent = AgentId::new();
    fleet.set_agent(agent, facts("planner", true, None));
    let asked = EventKind::QuestionAsked {
        question: "Ship it?".into(),
    };
    let event = event(99, by_agent(agent, asked));

    let notice = composed(&event, &fleet, true).expect("a notification");
    assert_eq!(notice.payload.kind, AttentionKind::Question);
    assert_eq!(notice.payload.question_seq, Some(99));
    assert_eq!(notice.payload.url, "#attention?item=question-99");
    assert_eq!(notice.payload.preview.as_deref(), Some("Ship it?"));
    assert_eq!(notice.urgency, Urgency::High);
}

#[test]
fn any_agent_failing_notifies_and_lands_on_the_agent() {
    let fleet = StaticFleet::default();
    let agent = AgentId::new();
    fleet.set_agent(agent, facts("worker", false, None));
    let event = event(
        5,
        by_agent(
            agent,
            EventKind::AgentFailed {
                error: "boom".into(),
            },
        ),
    );

    let notice = composed(&event, &fleet, false).expect("a notification");
    assert_eq!(notice.payload.kind, AttentionKind::AgentFailed);
    assert_eq!(notice.payload.url, format!("#agent/{agent}"));
    assert_eq!(notice.urgency, Urgency::Normal);
}

#[test]
fn only_a_block_the_operator_clears_notifies() {
    let agent = AgentId::new();
    let blocked = |reason: &str| EventKind::AgentBlocked {
        reason: reason.into(),
        dependency: None,
    };
    let event = event(12, by_agent(agent, blocked("waiting")));
    let exhausted = |first| OperatorBlock::Exhausted {
        provider: "anthropic".into(),
        first,
    };
    let cases = [
        (None, false),
        (Some(OperatorBlock::Inspection), true),
        (Some(exhausted(true)), true),
        (Some(exhausted(false)), false),
    ];
    for (block, notifies) in cases {
        let fleet = StaticFleet::default();
        fleet.set_agent(agent, facts("worker", false, block.clone()));
        let notice = composed(&event, &fleet, false);
        assert_eq!(notice.is_some(), notifies, "{block:?}");
    }
    // An agent the fleet does not know is not known to wait on the operator.
    assert_eq!(composed(&event, &StaticFleet::default(), false), None);
}

#[test]
fn a_task_ending_notifies_only_for_a_root_agent() {
    let task = TaskId::new();
    let done = EventKind::TaskCompleted {
        result: TaskResult::summary(SECRET),
    };
    let failed = EventKind::TaskFailed {
        error: SECRET.into(),
        structured: None,
    };
    for (kind, expected) in [
        (done, AttentionKind::TaskCompleted),
        (failed, AttentionKind::TaskFailed),
    ] {
        let agent = AgentId::new();
        let event = event(3, by_agent(agent, kind).task(task));
        let fleet = StaticFleet::default();
        fleet.set_agent(agent, facts("child", false, None));
        assert_eq!(composed(&event, &fleet, true), None, "a child's task");

        fleet.set_agent(agent, facts("root", true, None));
        let notice = composed(&event, &fleet, false).expect("a root's task");
        assert_eq!(notice.payload.kind, expected);
        assert_eq!(notice.payload.task, Some(task));
        assert_eq!(notice.payload.url, format!("#agent/{agent}"));
        assert_eq!(notice.urgency, Urgency::Normal);
    }
    let ownerless = event(
        4,
        EventDraft::new(
            Actor::Scheduler,
            EventKind::TaskFailed {
                error: "x".into(),
                structured: None,
            },
        ),
    );
    assert_eq!(composed(&ownerless, &StaticFleet::default(), false), None);
}

#[test]
fn other_events_and_imported_history_are_nothing() {
    let agent = AgentId::new();
    let quiet = [
        EventKind::LeaseReleased,
        EventKind::AgentCompleted { summary: None },
        EventKind::TaskCancelled { reason: None },
        EventKind::Custom {
            name: "remote.configured".into(),
            payload: serde_json::json!({}),
        },
    ];
    for kind in quiet {
        let event = event(1, by_agent(agent, kind));
        assert_eq!(classify(&event), Signal::Nothing, "{:?}", event.kind);
        assert_eq!(composed(&event, &StaticFleet::default(), true), None);
    }
    let imported = Actor::Imported {
        was: Box::new(Actor::agent(agent)),
    };
    let failed = EventKind::AgentFailed {
        error: "elsewhere".into(),
    };
    let event = event(2, EventDraft::new(imported, failed).agent(agent));
    assert_eq!(classify(&event), Signal::Nothing);
}

#[test]
fn a_resolution_retracts_and_a_revocation_is_recognised() {
    let approval = ApprovalId::new();
    let resolved = EventKind::ApprovalResolved {
        approval,
        decision: ApprovalDecision::Approved,
        by: Actor::user(),
        note: None,
    };
    let event_resolved = event(8, EventDraft::new(Actor::user(), resolved));
    assert_eq!(classify(&event_resolved), Signal::Retract(approval));

    let device = DeviceId::new();
    let revoked = |payload| {
        let kind = EventKind::Custom {
            name: DEVICE_REVOKED_EVENT.into(),
            payload,
        };
        classify(&event(9, EventDraft::new(Actor::user(), kind)))
    };
    let named =
        serde_json::json!({"device": device.to_string(), "name": "phone", "reason": "lost"});
    assert_eq!(revoked(named), Signal::Revoked(device));
    assert_eq!(
        revoked(serde_json::json!({"device": "not an id"})),
        Signal::Nothing
    );
    assert_eq!(revoked(serde_json::json!({})), Signal::Nothing);

    let forged = EventKind::Custom {
        name: DEVICE_REVOKED_EVENT.into(),
        payload: serde_json::json!({"device": device.to_string()}),
    };
    let by_an_agent = event(10, by_agent(AgentId::new(), forged));
    assert_eq!(classify(&by_an_agent), Signal::Nothing);
}

#[test]
fn content_reaches_a_payload_only_as_an_allowed_preview() {
    let fleet = StaticFleet::default();
    let agent = AgentId::new();
    let approval = ApprovalId::new();
    fleet.set_agent(
        agent,
        facts("builder", true, Some(OperatorBlock::Inspection)),
    );
    fleet.set_pending(approval);
    let kinds = [
        approval_requested(approval),
        EventKind::QuestionAsked {
            question: SECRET.into(),
        },
        EventKind::AgentFailed {
            error: SECRET.into(),
        },
        EventKind::AgentBlocked {
            reason: SECRET.into(),
            dependency: None,
        },
        EventKind::TaskFailed {
            error: SECRET.into(),
            structured: None,
        },
        EventKind::TaskCompleted {
            result: TaskResult::summary(SECRET),
        },
    ];
    for kind in kinds {
        let event = event(20, by_agent(agent, kind));
        let redacted = composed(&event, &fleet, false).expect("a notification");
        let wire = serde_json::to_string(&redacted.payload).expect("json");
        assert!(!wire.contains("secret"), "{wire}");
        assert!(!wire.contains("preview"), "{wire}");

        let previewed = composed(&event, &fleet, true).expect("a notification");
        let completed = previewed.payload.kind == AttentionKind::TaskCompleted;
        let expected = (!completed).then(|| SECRET.to_owned());
        assert_eq!(
            previewed.payload.preview, expected,
            "{:?}",
            previewed.payload.kind
        );
    }
}

#[test]
fn a_preview_is_one_line_of_at_most_two_hundred_characters() {
    assert_eq!(
        preview("  two\n\tlines\u{7} here  ").as_deref(),
        Some("two lines here")
    );
    assert_eq!(preview(" \n "), None);
    assert_eq!(
        preview(&"é".repeat(200)).expect("fits").chars().count(),
        200
    );

    let long = preview(&"é".repeat(201)).expect("clipped");
    assert_eq!(long.chars().count(), MAX_PREVIEW_CHARS);
    assert!(long.ends_with('…'));
    let words = preview(&"word ".repeat(100)).expect("clipped");
    assert!(words.chars().count() <= MAX_PREVIEW_CHARS);
    assert!(words.ends_with('…'));

    let fleet = StaticFleet::default();
    let agent = AgentId::new();
    fleet.set_agent(agent, facts(&"n".repeat(500), false, None));
    let event = event(
        1,
        by_agent(
            agent,
            EventKind::AgentFailed {
                error: "x".repeat(5000),
            },
        ),
    );
    let notice = composed(&event, &fleet, true).expect("a notification");
    let name = notice.payload.agent_name.as_deref().expect("a name");
    assert_eq!(name.chars().count(), MAX_AGENT_NAME_CHARS);
    let wire = serde_json::to_vec(&notice.payload).expect("json");
    assert!(
        wire.len() < crate::push::ece::MAX_PLAINTEXT_BYTES,
        "{}",
        wire.len()
    );
}

#[test]
fn topics_coalesce_per_agent_and_keep_asks_apart_from_status() {
    let fleet = StaticFleet::default();
    let (first, second) = (AgentId::new(), AgentId::new());
    let approval = ApprovalId::new();
    fleet.set_pending(approval);
    for agent in [first, second] {
        fleet.set_agent(agent, facts("worker", true, None));
    }
    let topic_of = |agent, kind| {
        let notice = composed(&event(1, by_agent(agent, kind)), &fleet, false).expect("a notice");
        notice.topic
    };
    let failed = || EventKind::AgentFailed { error: "x".into() };
    let task_failed = || EventKind::TaskFailed {
        error: "x".into(),
        structured: None,
    };
    let asked = || EventKind::QuestionAsked {
        question: "?".into(),
    };

    let status = topic_of(first, failed());
    assert_eq!(
        status,
        topic_of(first, task_failed()),
        "one agent's status is one topic"
    );
    let ask = topic_of(first, asked());
    assert_eq!(ask, topic_of(first, approval_requested(approval)));
    assert_ne!(
        status, ask,
        "a status change never displaces a waiting question"
    );
    assert_ne!(
        status,
        topic_of(second, failed()),
        "agents do not share a topic"
    );
    for topic in [&status, &ask] {
        assert_eq!(topic.len(), 32);
        assert!(
            topic
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
        );
        assert!(!topic.contains(&first.to_string()));
    }
    assert_ne!(
        topic(INSTANCE, "x"),
        topic("inst_other", "x"),
        "topics are per daemon"
    );
}

#[test]
fn agents_exhausting_one_provider_share_a_topic() {
    let fleet = StaticFleet::default();
    let blocked = || EventKind::AgentBlocked {
        reason: "credits".into(),
        dependency: None,
    };
    let exhausted = |provider: &str| OperatorBlock::Exhausted {
        provider: provider.into(),
        first: true,
    };
    let (a, b, c) = (AgentId::new(), AgentId::new(), AgentId::new());
    fleet.set_agent(a, facts("a", false, Some(exhausted("anthropic"))));
    fleet.set_agent(b, facts("b", false, Some(exhausted("anthropic"))));
    fleet.set_agent(c, facts("c", false, Some(exhausted("openai"))));
    let topic_of = |agent| {
        composed(&event(1, by_agent(agent, blocked())), &fleet, false)
            .expect("a notice")
            .topic
    };
    assert_eq!(topic_of(a), topic_of(b));
    assert_ne!(topic_of(a), topic_of(c));
}

#[test]
fn the_acting_device_is_remembered_so_it_is_not_told() {
    let fleet = StaticFleet::default();
    let agent = AgentId::new();
    let device = DeviceId::new();
    fleet.set_agent(agent, facts("root", true, None));
    let origin = OperatorOrigin::Device {
        device,
        name: "phone".into(),
        connection: "c1".into(),
    };
    let done = EventKind::TaskCompleted {
        result: TaskResult::summary("done"),
    };
    let by_device = event(
        6,
        EventDraft::new(Actor::connected_user(origin), done.clone()).agent(agent),
    );
    assert_eq!(
        composed(&by_device, &fleet, false)
            .expect("a notice")
            .origin,
        Some(device)
    );

    let by_host = event(7, EventDraft::new(Actor::user(), done).agent(agent));
    assert_eq!(
        composed(&by_host, &fleet, false).expect("a notice").origin,
        None
    );
}

#[test]
fn the_test_notice_names_no_event() {
    let notice = test_notice(INSTANCE, aether_core::time::now());
    assert_eq!(notice.payload.kind, AttentionKind::Test);
    assert_eq!(notice.payload.seq, 0);
    assert_eq!(notice.payload.title, "Aether notifications are working");
    assert_eq!(notice.payload.url, "#activity");
    assert_eq!(notice.urgency, Urgency::Normal);
    let wire = serde_json::to_value(&notice.payload).expect("json");
    let fields: Vec<&str> = wire
        .as_object()
        .expect("an object")
        .keys()
        .map(String::as_str)
        .collect();
    assert_eq!(
        fields,
        ["at", "instance_id", "kind", "seq", "title", "url", "v"]
    );
}

#[test]
fn titles_are_fixed_text() {
    for kind in [
        AttentionKind::Approval,
        AttentionKind::Question,
        AttentionKind::AgentFailed,
        AttentionKind::AgentBlocked,
        AttentionKind::TaskCompleted,
        AttentionKind::TaskFailed,
        AttentionKind::Test,
    ] {
        assert!(!title(kind).is_empty());
    }
}
