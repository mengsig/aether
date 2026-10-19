use super::*;

fn create_schedule(
    runtime: &Arc<Runtime>,
    ctx: &TurnCtx,
    spec: aether_core::schedule::CreateScheduleSpec,
) -> Result<ToolOutcome> {
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

pub(super) fn apply(
    runtime: &Arc<Runtime>,
    ctx: &TurnCtx,
    action: HarnessAction,
) -> Result<ToolOutcome> {
    match action {
        HarnessAction::CreateSchedule { spec } => create_schedule(runtime, ctx, spec),
        HarnessAction::ListSchedules { id } => list_schedules(runtime, ctx, id),
        HarnessAction::UpdateSchedule { id, action } => update_schedule(runtime, ctx, id, action),
        other => unreachable!("not a schedule action: {other:?}"),
    }
}
