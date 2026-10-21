use super::super::Milestone;
use crate::event::{Event, EventKind};

/// Event types that make the timeline. Everything else is in the tallies:
/// a run's story is its shape, not its every tool call.
pub(super) fn milestone_of(event: &Event) -> Option<Milestone> {
    let detail = match &event.kind {
        EventKind::AgentCreated { name, role, .. } => format!("{name} ({role})"),
        EventKind::AgentCompleted { summary } => summary.clone().unwrap_or_default(),
        EventKind::AgentFailed { error } => error.clone(),
        EventKind::AgentCancelled { reason } => reason.clone().unwrap_or_default(),
        EventKind::AgentBlocked { reason, .. } => reason.clone(),
        EventKind::TaskCreated { objective, .. } => objective.clone(),
        EventKind::TaskCompleted { result } => result.summary.clone(),
        EventKind::TaskFailed { error, .. } => error.clone(),
        EventKind::TaskCancelled { reason } => reason.clone().unwrap_or_default(),
        EventKind::QuestionAsked { question } => question.clone(),
        EventKind::ApprovalRequested {
            tool, description, ..
        } => format!("{tool}: {description}"),
        EventKind::ApprovalResolved { decision, by, .. } => {
            // `role_label`, not `label`: a rule id inside this sentence
            // would not be remapped on import, and the approvals table
            // carries which rule it was anyway.
            format!("{} by {}", decision.as_str(), by.role_label())
        }
        EventKind::BudgetWarning { reason, .. } | EventKind::BudgetExceeded { reason, .. } => {
            reason.clone()
        }
        EventKind::ProviderBackoff {
            provider, reason, ..
        } => format!("{provider}: {reason}"),
        EventKind::CompactionCompleted {
            from_seq, to_seq, ..
        } => format!("turns {from_seq}..{to_seq} compacted"),
        EventKind::CompactionNative {
            provider,
            before_tokens,
            after_tokens,
            ..
        } => match (before_tokens, after_tokens) {
            (Some(before), Some(after)) => {
                format!("{provider} compacted its context: {before} -> {after} tokens")
            }
            _ => format!("{provider} compacted its context"),
        },
        // The lossier sibling of a compaction: what a handoff could not
        // carry across the engine/API boundary belongs beside it, not only
        // in the log and the TUI.
        EventKind::ContextHandoff { from, to, lost, .. } => format!(
            "{from} -> {to}{}",
            if lost.is_empty() {
                String::new()
            } else {
                format!(" (lost: {})", lost.join("; "))
            }
        ),
        EventKind::RecoveryClassified { class, detail } => format!("{class:?}: {detail}"),
        _ => return None,
    };
    Some(Milestone {
        at: event.at,
        kind: event.kind.type_name(),
        agent: event.agent_id,
        detail,
    })
}
