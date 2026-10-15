use super::*;
use crate::agent::AgentStatusKind;
use crate::event::{Actor, EventDraft};
use crate::id::EventId;
use crate::tool::{ToolCall, ToolOutcome};

struct Log {
    agent: AgentId,
    seq: u64,
    at: Timestamp,
    events: Vec<Event>,
}

impl Log {
    fn new() -> Self {
        Self {
            agent: AgentId::new(),
            seq: 0,
            at: "2026-01-01T00:00:00Z".parse().expect("fixture timestamp"),
            events: Vec::new(),
        }
    }

    fn push(&mut self, kind: EventKind) -> &mut Self {
        self.seq += 1;
        self.at += chrono::Duration::seconds(1);
        let draft = EventDraft::new(Actor::System, kind).agent(self.agent);
        self.events
            .push(Event::from_draft(draft, EventId::new(), self.seq, self.at));
        self
    }

    fn frames(&self) -> Vec<ReplayFrame> {
        build(self.agent, &self.events)
    }
}

fn message(role: MessageRole, text: &str) -> EventKind {
    EventKind::AgentMessage {
        role,
        content: vec![ContentBlock::Text {
            text: text.to_owned(),
        }],
        invocation: None,
    }
}

fn completed(usage: Usage) -> EventKind {
    EventKind::InvocationCompleted {
        invocation: InvocationId::new(),
        snapshot: SnapshotId::new(),
        model: ModelRef::new("mock", "m"),
        usage,
        stop_reason: Some(StopReason::EndTurn),
        duration_ms: 900,
    }
}

#[test]
fn an_agent_that_has_taken_no_turn_replays_to_nothing() {
    let mut log = Log::new();
    log.push(EventKind::AgentReady);
    assert!(log.frames().is_empty());
}

#[test]
fn what_it_saw_before_its_first_turn_is_that_turns_prologue() {
    let mut log = Log::new();
    log.push(message(MessageRole::User, "do the thing"));
    log.push(EventKind::AgentStarted { turn: 1 });
    log.push(message(MessageRole::Assistant, "on it"));
    let frames = log.frames();
    assert_eq!(frames.len(), 1);
    assert_eq!(frames[0].messages.len(), 2);
    assert_eq!(frames[0].messages[0].role, MessageRole::User);
}

#[test]
fn billing_corrections_replace_each_turn_without_recounting_prior_turns() {
    let mut log = Log::new();
    for turn in 1..=2 {
        log.push(EventKind::AgentStarted { turn });
        for output in [100, 105] {
            log.push(EventKind::AgentUsageUpdated {
                total: Usage {
                    output_tokens: u64::from(output + (turn - 1) * 105),
                    ..Default::default()
                },
                turn_usage: Usage {
                    output_tokens: u64::from(output),
                    ..Default::default()
                },
            });
        }
    }
    let frames = log.frames();
    assert_eq!(frames.len(), 2);
    assert_eq!(frames[0].usage.output_tokens, 105);
    assert_eq!(frames[1].usage.output_tokens, 105);
}

#[test]
fn each_turn_carries_what_it_saw_did_and_cost() {
    let mut log = Log::new();
    let call = ToolCallId::new();
    log.push(EventKind::AgentStarted { turn: 1 });
    let at = log.at;
    log.push(EventKind::SnapshotCreated {
        snapshot: SnapshotId::new(),
        turn: 1,
        model: ModelRef::new("mock", "m"),
        token_estimate: 1234,
        layers: vec![ContextLayer {
            kind: crate::snapshot::LayerKind::RoleTask,
            text: Some("be useful".into()),
            tokens: 12,
            items: 1,
            placement: Default::default(),
            memories: Vec::new(),
        }],
    });
    log.push(EventKind::ToolRequested {
        call: ToolCall {
            id: call,
            provider_call_id: "p".into(),
            name: "bash".into(),
            input: serde_json::json!({"command": "ls"}),
            agent_id: log.agent,
            task_id: None,
            invocation_id: None,
            resources: Vec::new(),
            idempotency_key: None,
            requested_at: at,
        },
        side_effect: SideEffectClass::ReadOnly,
    });
    log.push(EventKind::ToolSucceeded {
        call,
        outcome: ToolOutcome {
            duration_ms: 30,
            ..ToolOutcome::ok("two files")
        },
    });
    log.push(completed(Usage {
        input_tokens: 1234,
        output_tokens: 40,
        cost_usd: 0.01,
        ..Default::default()
    }));
    log.push(EventKind::AgentTurnCompleted {
        turn: 1,
        invocation: None,
        next: AgentStatusKind::Waiting,
    });

    let frames = log.frames();
    assert_eq!(frames.len(), 1);
    let f = &frames[0];
    assert_eq!(f.context.as_ref().expect("context").token_estimate, 1234);
    assert_eq!(f.context.as_ref().expect("context").layers.len(), 1);
    assert_eq!(f.tools.len(), 1);
    assert_eq!(f.tools[0].name, "bash");
    assert!(matches!(
        f.tools[0].outcome,
        ToolResult::Succeeded {
            duration_ms: 30,
            ..
        }
    ));
    assert_eq!(f.usage.cost_usd, 0.01);
    assert_eq!(f.duration_ms, 900);
    assert!(matches!(f.outcome, TurnOutcome::Completed { .. }));
    assert!(f.ended_at.is_some());
    assert!(f.to_seq > f.from_seq);
}

/// A failed tool call carries its real elapsed time into replay too -
/// durations must be real, not just on the success arm.
#[test]
fn a_failed_tool_call_replays_with_its_real_duration() {
    let mut log = Log::new();
    let call = ToolCallId::new();
    log.push(EventKind::AgentStarted { turn: 1 });
    let at = log.at;
    log.push(EventKind::ToolRequested {
        call: ToolCall {
            id: call,
            provider_call_id: "p".into(),
            name: "bash".into(),
            input: serde_json::json!({"command": "false"}),
            agent_id: log.agent,
            task_id: None,
            invocation_id: None,
            resources: Vec::new(),
            idempotency_key: None,
            requested_at: at,
        },
        side_effect: SideEffectClass::ReadOnly,
    });
    log.push(EventKind::ToolFailed {
        call,
        error: "exit 1".into(),
        retryable: false,
        duration_ms: Some(456),
    });
    let frames = log.frames();
    assert_eq!(frames.len(), 1);
    let f = &frames[0];
    assert_eq!(f.tools.len(), 1);
    assert_eq!(f.tools[0].name, "bash");
    assert!(
        matches!(
            f.tools[0].outcome,
            ToolResult::Failed {
                duration_ms: Some(456),
                ..
            }
        ),
        "{:?}",
        f.tools[0].outcome
    );
}

/// The generated clients declare `duration_ms` as `number | undefined` /
/// `Optional[int]`, never a literal `null` - `None` must serialise with
/// the key absent, `Some` with it present.
#[test]
fn tool_result_failed_duration_ms_is_absent_on_the_wire_for_none_and_present_for_some() {
    let none_json = serde_json::to_value(ToolResult::Failed {
        error: "boom".into(),
        retryable: false,
        duration_ms: None,
    })
    .unwrap();
    assert!(
        !none_json.as_object().unwrap().contains_key("duration_ms"),
        "{none_json}"
    );

    let some_json = serde_json::to_value(ToolResult::Failed {
        error: "boom".into(),
        retryable: false,
        duration_ms: Some(456),
    })
    .unwrap();
    assert_eq!(some_json["duration_ms"], serde_json::json!(456));
}

/// An old, already-recorded event with no `duration_ms` field must still
/// decode (`#[serde(default)]`), replaying as an unknown duration.
#[test]
fn a_tool_failed_event_without_duration_ms_still_decodes() {
    let kind: EventKind = serde_json::from_value(serde_json::json!({
        "type": "tool.failed",
        "call": ToolCallId::new(),
        "error": "boom",
        "retryable": false,
    }))
    .expect("old event without duration_ms decodes");
    assert!(matches!(
        kind,
        EventKind::ToolFailed {
            duration_ms: None,
            ..
        }
    ));
}

#[test]
fn turns_are_separate_frames_and_the_last_one_may_still_be_running() {
    let mut log = Log::new();
    log.push(EventKind::AgentStarted { turn: 1 });
    log.push(completed(Usage::default()));
    log.push(EventKind::AgentStarted { turn: 2 });
    log.push(message(MessageRole::Assistant, "still thinking"));
    let frames = log.frames();
    assert_eq!(frames.len(), 2);
    assert_eq!(frames[0].turn, 1);
    assert!(matches!(frames[0].outcome, TurnOutcome::Completed { .. }));
    assert_eq!(frames[1].turn, 2);
    assert_eq!(frames[1].outcome, TurnOutcome::Running);
}

#[test]
fn an_interrupted_turn_says_so_rather_than_failing() {
    let mut log = Log::new();
    log.push(EventKind::AgentStarted { turn: 1 });
    log.push(EventKind::InvocationFailed {
        class: None,
        invocation: InvocationId::new(),
        snapshot: SnapshotId::new(),
        error: "cancelled".into(),
        retryable: false,
        cancelled: true,
        partial: Vec::new(),
    });
    assert_eq!(log.frames()[0].outcome, TurnOutcome::Interrupted);
}

#[test]
fn an_approval_is_shown_with_who_answered_it() {
    let mut log = Log::new();
    let approval = ApprovalId::new();
    log.push(EventKind::AgentStarted { turn: 1 });
    log.push(EventKind::ApprovalRequested {
        approval,
        call: ToolCallId::new(),
        tool: "bash".into(),
        description: "rm -rf build".into(),
        argument: None,
        side_effect: SideEffectClass::Mutation,
    });
    log.push(EventKind::ApprovalResolved {
        approval,
        decision: ApprovalDecision::Approved,
        by: Actor::user(),
        note: None,
    });
    let frames = log.frames();
    assert_eq!(frames[0].approvals.len(), 1);
    assert_eq!(
        frames[0].approvals[0].decision,
        Some(ApprovalDecision::Approved)
    );
    assert_eq!(
        frames[0].approvals[0].decided_by,
        Some(Actor::user()),
        "the actor itself, not a label an import cannot remap"
    );
}

/// Today every outcome lands inside its own turn, so this is latent -
/// but the failure mode is silent loss, not a visible gap, so it is
/// pinned rather than left to be found by whoever hits it first.
#[test]
fn an_outcome_that_lands_after_the_next_turn_opens_is_not_dropped() {
    let mut log = Log::new();
    let (call, approval) = (ToolCallId::new(), ApprovalId::new());
    let at = log.at;
    log.push(EventKind::AgentStarted { turn: 1 });
    log.push(EventKind::ToolRequested {
        call: ToolCall {
            id: call,
            provider_call_id: "p".into(),
            name: "bash".into(),
            input: serde_json::json!({"command": "sleep 30"}),
            agent_id: log.agent,
            task_id: None,
            invocation_id: None,
            resources: Vec::new(),
            idempotency_key: None,
            requested_at: at,
        },
        side_effect: SideEffectClass::ReadOnly,
    });
    log.push(EventKind::ApprovalRequested {
        approval,
        call,
        tool: "bash".into(),
        description: "sleep 30".into(),
        argument: None,
        side_effect: SideEffectClass::ReadOnly,
    });
    // The next turn opens before either is answered.
    log.push(EventKind::AgentStarted { turn: 2 });
    log.push(EventKind::ApprovalResolved {
        approval,
        decision: ApprovalDecision::Approved,
        by: Actor::user(),
        note: None,
    });
    log.push(EventKind::ToolSucceeded {
        call,
        outcome: ToolOutcome {
            duration_ms: 30_000,
            ..ToolOutcome::ok("done at last")
        },
    });

    let frames = log.frames();
    assert_eq!(frames.len(), 2);
    assert_eq!(frames[1].tools.len(), 0, "turn 2 asked for nothing");
    assert_eq!(frames[1].approvals.len(), 0);
    assert!(
        matches!(
            frames[0].tools.first().map(|t| &t.outcome),
            Some(ToolResult::Succeeded { .. })
        ),
        "the outcome was dropped: {:?}",
        frames[0].tools
    );
    assert_eq!(
        frames[0].approvals.first().and_then(|a| a.decision),
        Some(ApprovalDecision::Approved),
        "the answer was dropped: {:?}",
        frames[0].approvals
    );
}

#[test]
fn another_agents_events_never_reach_the_replay() {
    let mut log = Log::new();
    log.push(EventKind::AgentStarted { turn: 1 });
    let other =
        EventDraft::new(Actor::System, EventKind::AgentStarted { turn: 9 }).agent(AgentId::new());
    log.events
        .push(Event::from_draft(other, EventId::new(), 99, log.at));
    assert_eq!(log.frames().len(), 1);
}
