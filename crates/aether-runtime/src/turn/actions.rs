use super::*;

mod delegation;
mod inspection;
mod lifecycle;
mod memory;
mod notify;
mod schedules;

use crate::workspace::review::path_text as workspace_path_for_reply;

/// Apply a harness action: validate, transition, record. Returns what the
/// model is told, masked like any tool's outcome: a `read_artifact` page or a
/// task result carries whatever text produced it.
pub(crate) async fn apply_action(
    runtime: &Arc<Runtime>,
    ctx: &mut TurnCtx,
    action: HarnessAction,
    effects: &mut ToolEffects,
    mutation_approved: bool,
) -> Result<ToolOutcome> {
    let mut outcome = dispatch_action(runtime, ctx, action, effects, mutation_approved).await?;
    aether_tools::redact_outcome(runtime.tool_runtime.redactor(), &mut outcome);
    Ok(outcome)
}

async fn dispatch_action(
    runtime: &Arc<Runtime>,
    ctx: &mut TurnCtx,
    action: HarnessAction,
    effects: &mut ToolEffects,
    mutation_approved: bool,
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
            schedules::apply(runtime, ctx, action, mutation_approved).await
        }
        action @ (A::CompleteTask { .. }
        | A::FailTask { .. }
        | A::WaitFor { .. }
        | A::RequestCheckpoint { .. }
        | A::AskUser { .. }) => lifecycle::apply(runtime, ctx, effects, action),
        A::Remember { candidate } => memory::remember(runtime, ctx, candidate),
        A::Notify { post } => notify::post(runtime, ctx, post),
        A::ResolveNotification { key } => notify::resolve(runtime, ctx, &key),
    }
}
