use super::*;
use aether_core::schedule::{CreateScheduleSpec, ScheduleTarget, SpawnSpec};

/// What `delegate` checks before it asks the runtime for a child: the caller
/// can delegate, and the template's model can run. `Some` is the refusal.
async fn spawn_refusal(
    runtime: &Arc<Runtime>,
    ctx: &TurnCtx,
    template: &SpawnSpec,
) -> Result<Option<ToolOutcome>> {
    let parent = match delegation::load_delegating_parent(runtime, ctx)? {
        Ok(parent) => parent,
        Err(refusal) => return Ok(Some(refusal)),
    };
    // A template without a model is refused by the runtime, with the reason.
    let Some(model) = &template.model else {
        return Ok(None);
    };
    let network = template
        .scope
        .as_ref()
        .map_or(parent.scope.network, |scope| scope.network);
    let checked = delegation::check_delegation_target(runtime, model, network, &parent).await?;
    Ok(checked.err())
}

async fn create_schedule(
    runtime: &Arc<Runtime>,
    ctx: &TurnCtx,
    spec: CreateScheduleSpec,
) -> Result<ToolOutcome> {
    if let ScheduleTarget::Spawn(template) = &spec.target
        && let Some(refusal) = spawn_refusal(runtime, ctx, template).await?
    {
        return Ok(refusal);
    }
    match runtime.create_schedule(spec, ctx.base_actor.clone()) {
        Ok(job) => Ok(ToolOutcome::ok(
            serde_json::json!({"schedule":job}).to_string(),
        )),
        Err(error) => Ok(ToolOutcome::error(format!(
            "Cannot create schedule: {error}"
        ))),
    }
}

fn list_schedules(
    runtime: &Arc<Runtime>,
    ctx: &TurnCtx,
    id: Option<aether_core::id::ScheduleId>,
) -> Result<ToolOutcome> {
    let visible = |job: &aether_core::schedule::Schedule| {
        job.owner == ctx.base_actor || job.target() == Some(ctx.agent.id)
    };
    if let Some(id) = id {
        let Some(job) = runtime.store.schedule(id)?.filter(visible) else {
            return Ok(ToolOutcome::error(
                "Schedule not found among timers you own or that target you.",
            ));
        };
        let history = runtime.store.schedule_history(id, 100)?;
        Ok(ToolOutcome::ok(
            serde_json::json!({"schedule":job,"history":history}).to_string(),
        ))
    } else {
        let jobs = runtime
            .store
            .schedules(None)?
            .into_iter()
            .filter(visible)
            .collect::<Vec<_>>();
        Ok(ToolOutcome::ok(
            serde_json::json!({"schedules":jobs}).to_string(),
        ))
    }
}

fn update_schedule(
    runtime: &Arc<Runtime>,
    ctx: &TurnCtx,
    id: aether_core::id::ScheduleId,
    action: aether_core::schedule::ScheduleAction,
) -> Result<ToolOutcome> {
    match runtime.update_schedule(id, action, ctx.base_actor.clone()) {
        Ok(job) => Ok(ToolOutcome::ok(
            serde_json::json!({"schedule":job}).to_string(),
        )),
        Err(error) => Ok(ToolOutcome::error(format!(
            "Cannot update schedule: {error}"
        ))),
    }
}

pub(super) async fn apply(
    runtime: &Arc<Runtime>,
    ctx: &TurnCtx,
    action: HarnessAction,
) -> Result<ToolOutcome> {
    match action {
        HarnessAction::CreateSchedule { spec } => create_schedule(runtime, ctx, spec).await,
        HarnessAction::ListSchedules { id } => list_schedules(runtime, ctx, id),
        HarnessAction::UpdateSchedule { id, action } => update_schedule(runtime, ctx, id, action),
        other => unreachable!("not a schedule action: {other:?}"),
    }
}
