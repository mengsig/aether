use super::*;
use crate::agent::AgentStatusKind;
use crate::agent::Scope;
use crate::artifact::ArtifactKind;
use crate::event::{Actor, EventDraft};
use crate::event::{Event, EventKind};
use crate::id::ToolCallId;
use crate::id::{EventId, InvocationId};
use crate::task::TaskResult;
use crate::tool::{SideEffectClass, ToolCall, ToolOutcome};

struct Log {
    run: RunId,
    seq: u64,
    at: Timestamp,
    events: Vec<Event>,
}

impl Log {
    fn new() -> Self {
        Self {
            run: RunId::new(),
            seq: 0,
            at: "2026-01-01T00:00:00Z".parse().expect("fixture timestamp"),
            events: Vec::new(),
        }
    }

    fn push(&mut self, draft: EventDraft) -> &mut Self {
        self.seq += 1;
        self.at += chrono::Duration::seconds(1);
        let draft = EventDraft {
            run: Some(self.run),
            ..draft
        };
        self.events
            .push(Event::from_draft(draft, EventId::new(), self.seq, self.at));
        self
    }

    fn report(&self) -> RunReport {
        build(self.run, &self.events, self.at)
    }
}

fn created(id: AgentId, parent: Option<AgentId>, name: &str) -> EventDraft {
    EventDraft::new(
        Actor::System,
        EventKind::AgentCreated {
            role: Role::new("worker"),
            name: name.into(),
            parent,
            model: ModelRef::new("mock", "m"),
            fallbacks: Vec::new(),
            scope: Scope::default(),
        },
    )
    .agent(id)
}

/// A turn the engine reported tokens but no dollar cost for.
fn unpriced(id: AgentId) -> EventDraft {
    EventDraft::new(
        Actor::agent(id),
        EventKind::InvocationCompleted {
            invocation: InvocationId::new(),
            snapshot: crate::id::SnapshotId::new(),
            model: ModelRef::new("mock", "m"),
            usage: Usage {
                input_tokens: 100,
                output_tokens: 10,
                cost_complete: false,
                cost_gap: Some(crate::model::CostGap::Unpriced),
                ..Default::default()
            },
            stop_reason: None,
            duration_ms: 5,
        },
    )
    .agent(id)
}

fn spent(id: AgentId, cost: f64) -> EventDraft {
    EventDraft::new(
        Actor::agent(id),
        EventKind::InvocationCompleted {
            invocation: InvocationId::new(),
            snapshot: crate::id::SnapshotId::new(),
            model: ModelRef::new("mock", "m"),
            usage: Usage {
                input_tokens: 100,
                output_tokens: 10,
                cost_usd: cost,
                ..Default::default()
            },
            stop_reason: None,
            duration_ms: 5,
        },
    )
    .agent(id)
}

mod accounting;
mod artifacts;
mod comparison;
mod outcomes;
