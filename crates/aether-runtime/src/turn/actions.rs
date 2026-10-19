use super::*;

mod delegation;
mod inspection;
mod lifecycle;
mod memory;
mod schedules;

use crate::workspace::review::path_text as workspace_path_for_reply;

/// Apply a harness action: validate, transition, record. Returns what the
/// model is told.
pub(crate) async fn apply_action(
    runtime: &Arc<Runtime>,
    ctx: &mut TurnCtx,
    action: HarnessAction,
    effects: &mut ToolEffects,
) -> Result<ToolOutcome> {
    use HarnessAction as A;
    match action {
        action @ (A::ListAgents { .. }
        | A::ListPeers { .. }
        | A::ListTasks { .. }
        | A::ReadArtifact { .. }
        | A::ReadTask { .. }
        | A::RefreshProviderUsage { .. }
        | A::ProviderUsage
        | A::InspectState
        | A::ListModels { .. }) => inspection::apply(runtime, ctx, action).await,
        action @ (A::Delegate { .. }
        | A::IntegrateWorkspace { .. }
        | A::CancelAgent { .. }
        | A::SendMessage { .. }) => delegation::apply(runtime, ctx, effects, action).await,
        action
        @ (A::CreateSchedule { .. } | A::ListSchedules { .. } | A::UpdateSchedule { .. }) => {
            schedules::apply(runtime, ctx, action)
        }
        action @ (A::CompleteTask { .. }
        | A::FailTask { .. }
        | A::WaitFor { .. }
        | A::RequestCheckpoint { .. }
        | A::AskUser { .. }) => lifecycle::apply(runtime, ctx, effects, action),
        A::Remember { candidate } => memory::remember(runtime, ctx, candidate),
    }
}
