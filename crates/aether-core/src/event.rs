//! The event log vocabulary. Events answer "what happened".

use crate::{
    id::{AgentId, EventId, RunId, TaskId},
    time::Timestamp,
};
use serde::{Deserialize, Serialize};

mod actor;
mod kind;
mod triggers;
pub use actor::{Actor, OperatorOrigin};
pub use kind::EventKind;
pub use triggers::{CompactionTrigger, RecoveryClass};

/// Acknowledging a request does not imply the asynchronous compaction succeeded.
pub fn compaction_request_notice(follow_up: &str) -> String {
    format!(
        "Compaction requested: older turns are summarized as soon as the agent is free, \
         usually within a minute or two. Watch the transcript or run `{follow_up}`."
    )
}

/// An event before it has been committed. The store assigns id, sequence and
/// timestamp, so an uncommitted event can never be mistaken for history.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, schemars::JsonSchema)]
pub struct EventDraft {
    pub actor: Actor,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub agent_id: Option<AgentId>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub task_id: Option<TaskId>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub parent_agent_id: Option<AgentId>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub run: Option<RunId>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub causation: Option<EventId>,
    pub kind: EventKind,
}

impl EventDraft {
    pub fn new(actor: Actor, kind: EventKind) -> Self {
        Self {
            actor,
            agent_id: None,
            task_id: None,
            parent_agent_id: None,
            run: None,
            causation: None,
            kind,
        }
    }

    pub fn agent(mut self, id: AgentId) -> Self {
        self.agent_id = Some(id);
        self
    }

    pub fn task(mut self, id: TaskId) -> Self {
        self.task_id = Some(id);
        self
    }

    pub fn parent(mut self, id: Option<AgentId>) -> Self {
        self.parent_agent_id = id;
        self
    }

    pub fn run(mut self, id: RunId) -> Self {
        self.run = Some(id);
        self
    }

    pub fn caused_by(mut self, id: EventId) -> Self {
        self.causation = Some(id);
        self
    }
}

/// A committed event.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, schemars::JsonSchema)]
pub struct Event {
    pub id: EventId,
    /// Global, gapless, strictly increasing sequence assigned by the store.
    pub seq: u64,
    pub at: Timestamp,
    pub actor: Actor,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub agent_id: Option<AgentId>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub task_id: Option<TaskId>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub parent_agent_id: Option<AgentId>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub run: Option<RunId>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub causation: Option<EventId>,
    pub kind: EventKind,
}

impl Event {
    pub fn from_draft(draft: EventDraft, id: EventId, seq: u64, at: Timestamp) -> Self {
        Self {
            id,
            seq,
            at,
            actor: draft.actor,
            agent_id: draft.agent_id,
            task_id: draft.task_id,
            parent_agent_id: draft.parent_agent_id,
            run: draft.run,
            causation: draft.causation,
            kind: draft.kind,
        }
    }

    pub fn type_name(&self) -> String {
        self.kind.type_name()
    }
}

#[cfg(test)]
mod tests;
