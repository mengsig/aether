//! Tasks: durable units of work with their own lifecycle.

use std::fmt;

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::error::TransitionError;
use crate::event::Actor;
use crate::id::{AgentId, ArtifactId, RunId, TaskId};
use crate::time::Timestamp;

#[derive(
    Debug,
    Clone,
    Copy,
    PartialEq,
    Eq,
    PartialOrd,
    Ord,
    Hash,
    Default,
    Serialize,
    Deserialize,
    schemars::JsonSchema,
)]
#[serde(rename_all = "snake_case")]
pub enum Priority {
    Low,
    #[default]
    Normal,
    High,
    Critical,
}

impl Priority {
    pub fn as_str(self) -> &'static str {
        match self {
            Priority::Low => "low",
            Priority::Normal => "normal",
            Priority::High => "high",
            Priority::Critical => "critical",
        }
    }
}

impl fmt::Display for Priority {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

impl std::str::FromStr for Priority {
    type Err = ();

    fn from_str(s: &str) -> Result<Self, ()> {
        match s.trim().to_ascii_lowercase().as_str() {
            "low" => Ok(Priority::Low),
            "normal" | "default" => Ok(Priority::Normal),
            "high" => Ok(Priority::High),
            "critical" => Ok(Priority::Critical),
            _ => Err(()),
        }
    }
}

/// The durable outcome of a task. Cheap to carry in context forever.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, schemars::JsonSchema)]
pub struct TaskResult {
    pub summary: String,
    #[serde(default)]
    pub artifacts: Vec<ArtifactId>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub structured: Option<Value>,
}

impl TaskResult {
    pub fn summary(text: impl Into<String>) -> Self {
        Self {
            summary: text.into(),
            artifacts: Vec::new(),
            structured: None,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(tag = "state", rename_all = "snake_case")]
pub enum TaskStatus {
    Queued,
    Assigned,
    Active,
    Blocked {
        reason: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        dependency: Option<TaskId>,
    },
    Completed {
        result: TaskResult,
    },
    Failed {
        error: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        structured: Option<Value>,
    },
    Cancelled {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        reason: Option<String>,
    },
}

#[derive(
    Debug,
    Clone,
    Copy,
    PartialEq,
    Eq,
    Hash,
    PartialOrd,
    Ord,
    Serialize,
    Deserialize,
    schemars::JsonSchema,
)]
#[serde(rename_all = "snake_case")]
pub enum TaskStatusKind {
    Queued,
    Assigned,
    Active,
    Blocked,
    Completed,
    Failed,
    Cancelled,
}

impl TaskStatusKind {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Queued => "queued",
            Self::Assigned => "assigned",
            Self::Active => "active",
            Self::Blocked => "blocked",
            Self::Completed => "completed",
            Self::Failed => "failed",
            Self::Cancelled => "cancelled",
        }
    }

    pub fn is_terminal(self) -> bool {
        matches!(self, Self::Completed | Self::Failed | Self::Cancelled)
    }

    pub fn can_transition_to(self, to: TaskStatusKind) -> bool {
        use TaskStatusKind::*;
        match self {
            Queued => matches!(to, Assigned | Blocked | Cancelled | Failed),
            Assigned => matches!(to, Active | Queued | Blocked | Cancelled | Failed),
            // Active -> Queued models a lost lease: work becomes recoverable.
            Active => matches!(to, Completed | Failed | Blocked | Cancelled | Queued),
            Blocked => matches!(to, Queued | Assigned | Active | Cancelled | Failed),
            Completed | Failed | Cancelled => false,
        }
    }
}

impl fmt::Display for TaskStatusKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

impl TaskStatus {
    pub fn kind(&self) -> TaskStatusKind {
        match self {
            Self::Queued => TaskStatusKind::Queued,
            Self::Assigned => TaskStatusKind::Assigned,
            Self::Active => TaskStatusKind::Active,
            Self::Blocked { .. } => TaskStatusKind::Blocked,
            Self::Completed { .. } => TaskStatusKind::Completed,
            Self::Failed { .. } => TaskStatusKind::Failed,
            Self::Cancelled { .. } => TaskStatusKind::Cancelled,
        }
    }

    pub fn is_terminal(&self) -> bool {
        self.kind().is_terminal()
    }
}

/// A durable task record.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, schemars::JsonSchema)]
pub struct Task {
    pub id: TaskId,
    pub objective: String,
    /// Agent executing the task, once assigned.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub owner: Option<AgentId>,
    pub creator: Actor,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub parent_task: Option<TaskId>,
    pub status: TaskStatus,
    /// Tasks that must be terminal before this one can start.
    #[serde(default)]
    pub dependencies: Vec<TaskId>,
    #[serde(default)]
    pub artifacts: Vec<ArtifactId>,
    #[serde(default)]
    pub priority: Priority,
    pub run: RunId,
    pub created_at: Timestamp,
    pub updated_at: Timestamp,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub started_at: Option<Timestamp>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub finished_at: Option<Timestamp>,
    /// Why the delegating parent put this task on its own frontier model,
    /// as it said when it delegated.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub delegation_reason: Option<String>,
    /// Approved images and files the owner must see, re-attached every turn.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub references: Vec<crate::reference::TaskReference>,
}

impl Task {
    pub fn new(objective: impl Into<String>, creator: Actor, run: RunId, at: Timestamp) -> Self {
        Self {
            id: TaskId::new(),
            objective: objective.into(),
            owner: None,
            creator,
            parent_task: None,
            status: TaskStatus::Queued,
            dependencies: Vec::new(),
            artifacts: Vec::new(),
            priority: Priority::Normal,
            run,
            created_at: at,
            updated_at: at,
            started_at: None,
            finished_at: None,
            delegation_reason: None,
            references: Vec::new(),
        }
    }

    pub fn transition(&mut self, next: TaskStatus, at: Timestamp) -> Result<(), TransitionError> {
        let from = self.status.kind();
        let to = next.kind();
        if !from.can_transition_to(to) {
            return Err(TransitionError {
                entity: "task",
                id: self.id.to_string(),
                from: from.to_string(),
                to: to.to_string(),
            });
        }
        if to == TaskStatusKind::Active && self.started_at.is_none() {
            self.started_at = Some(at);
        }
        if to.is_terminal() {
            self.finished_at = Some(at);
        }
        if let TaskStatus::Completed { result } = &next {
            for artifact in &result.artifacts {
                if !self.artifacts.contains(artifact) {
                    self.artifacts.push(*artifact);
                }
            }
        }
        self.status = next;
        self.updated_at = at;
        Ok(())
    }

    pub fn result(&self) -> Option<&TaskResult> {
        match &self.status {
            TaskStatus::Completed { result } => Some(result),
            _ => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::time::now;

    #[test]
    fn full_lifecycle() {
        let mut t = Task::new("do it", Actor::user(), RunId::new(), now());
        let at = now();
        t.transition(TaskStatus::Assigned, at).expect("assign");
        t.transition(TaskStatus::Active, at).expect("start");
        assert!(t.started_at.is_some());
        let art = ArtifactId::new();
        t.transition(
            TaskStatus::Completed {
                result: TaskResult {
                    summary: "ok".into(),
                    artifacts: vec![art],
                    structured: None,
                },
            },
            at,
        )
        .expect("complete");
        assert!(t.finished_at.is_some());
        assert_eq!(t.artifacts, vec![art]);
        assert!(t.transition(TaskStatus::Queued, at).is_err());
    }

    #[test]
    fn lost_lease_requeues_active_task() {
        let mut t = Task::new("x", Actor::System, RunId::new(), now());
        t.transition(TaskStatus::Assigned, now()).expect("assign");
        t.transition(TaskStatus::Active, now()).expect("start");
        t.transition(TaskStatus::Queued, now()).expect("requeue");
        assert_eq!(t.status.kind(), TaskStatusKind::Queued);
    }

    #[test]
    fn queued_cannot_complete_directly() {
        let mut t = Task::new("x", Actor::System, RunId::new(), now());
        assert!(
            t.transition(
                TaskStatus::Completed {
                    result: TaskResult::summary("no")
                },
                now()
            )
            .is_err()
        );
    }
}
