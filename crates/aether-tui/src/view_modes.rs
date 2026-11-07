//! Transcript presentation rules that depend on TUI cell models.
pub use aether_core::view_modes::{
    Catalog, Category, Definition, Density, Diagnostic, Mode, Scope, discover, valid_name,
};

use crate::app::transcript::{
    CallStatus, CellModel, ChildOutcome, CompactionState, NoticeKind, UserFrom,
};

pub fn category(model: &CellModel) -> Category {
    use Category::*;
    match model {
        CellModel::Header { .. } => Status,
        CellModel::User { from, .. } => match from {
            UserFrom::Operator => UserText,
            UserFrom::Task { .. } => Delegation,
            UserFrom::Message { .. } => AgentMessage,
        },
        CellModel::Assistant { .. } => AssistantText,
        CellModel::Reasoning { .. } => Reasoning,
        CellModel::Exec { .. }
        | CellModel::Explored { .. }
        | CellModel::Edited { .. }
        | CellModel::Called { .. } => Tool,
        CellModel::Memory(_) => Memory,
        CellModel::Spawned { .. } => Delegation,
        CellModel::Waiting { .. } => Status,
        CellModel::ChildResult { .. } => Completion,
        CellModel::Sent { .. } => AgentMessage,
        CellModel::ApprovalRecord { .. } => Approval,
        CellModel::Question { .. } => Question,
        CellModel::Error { .. } => Error,
        CellModel::Notice { kind, .. } => match kind {
            NoticeKind::ClientError
            | NoticeKind::Backoff { .. }
            | NoticeKind::Fallback
            | NoticeKind::McpFailed
            | NoticeKind::BudgetWarning
            | NoticeKind::Engine
            | NoticeKind::Store => Error,
            NoticeKind::Recovered { .. }
            | NoticeKind::Reconnected
            | NoticeKind::Interrupted
            | NoticeKind::ModelChange
            | NoticeKind::Handoff
            | NoticeKind::Cancelled
            | NoticeKind::Echo
            | NoticeKind::Trigger { .. }
            | NoticeKind::Note(_) => Status,
        },
        CellModel::Compaction(_) => Compaction,
        CellModel::Separator { .. } => Usage,
    }
}

pub fn mandatory(model: &CellModel) -> bool {
    matches!(
        category(model),
        Category::Question | Category::Approval | Category::Error
    ) || match model {
        CellModel::Header { .. } => true,
        CellModel::Exec { status, .. }
        | CellModel::Called { status, .. }
        | CellModel::Edited { status, .. } => matches!(
            status,
            CallStatus::Awaiting | CallStatus::Failed | CallStatus::Denied | CallStatus::Cancelled
        ),
        CellModel::ChildResult { outcome, .. } => !matches!(outcome, ChildOutcome::Finished),
        CellModel::Compaction(CompactionState::Failed { .. }) => true,
        _ => false,
    }
}

/// A failed call that carries its own error text.
fn failed_call(model: &CellModel) -> bool {
    matches!(
        model,
        CellModel::Exec {
            status: CallStatus::Failed,
            ..
        } | CellModel::Called {
            status: CallStatus::Failed,
            ..
        }
    )
}

#[derive(Debug, Clone, Copy, Default)]
pub struct Policy<'a> {
    pub definition: Option<&'a Definition>,
}
impl Policy<'_> {
    pub fn shows(&self, model: &CellModel) -> bool {
        let Some(definition) = self.definition else {
            return true;
        };
        if definition.show.contains(&category(model)) {
            return true;
        }
        // A failed edit's line is its named-only call cell; the edit itself is tool detail.
        let failed_edit = matches!(
            model,
            CellModel::Edited {
                status: CallStatus::Failed,
                ..
            }
        );
        mandatory(model) && !failed_edit
    }
    pub fn shows_category(&self, kind: Category) -> bool {
        self.definition.is_none_or(|d| d.show.contains(&kind))
    }
    /// A failed call in a mode without tools: one line, never its command or output.
    pub fn failed_tool_line(&self, model: &CellModel) -> bool {
        failed_call(model)
            && self
                .definition
                .is_some_and(|d| !d.show.contains(&Category::Tool))
    }
    pub fn compact(&self, model: &CellModel) -> bool {
        self.failed_tool_line(model)
            || self
                .definition
                .is_some_and(|d| d.density == Density::Compact)
                && !mandatory(model)
                && matches!(
                    category(model),
                    Category::Delegation
                        | Category::AgentMessage
                        | Category::Completion
                        | Category::Status
                )
    }
    /// A decision or failure a named mode draws whole, as details would.
    pub fn expands(&self, model: &CellModel) -> bool {
        self.definition.is_some() && mandatory(model) && !self.failed_tool_line(model)
    }
}
