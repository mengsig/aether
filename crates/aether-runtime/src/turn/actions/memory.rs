use super::*;

pub(super) fn remember(
    runtime: &Arc<Runtime>,
    ctx: &TurnCtx,
    mut candidate: aether_core::memory::MemoryCandidate,
) -> Result<ToolOutcome> {
    if let MemoryScope::Role { role } = &candidate.scope
        && role == "unknown"
    {
        candidate.scope = MemoryScope::Role {
            role: ctx.agent.role.as_str().to_owned(),
        };
    }
    candidate.provenance.agent = Some(ctx.agent.id);
    candidate.provenance.task = ctx.task.as_ref().map(|t| t.id);
    let hold = runtime.hold();
    let (admission, mut commit) = runtime.memory.evaluate(
        &hold,
        candidate,
        ctx.base_actor.clone(),
        Some(ctx.agent.id),
        ctx.task.as_ref().map(|t| t.id),
    )?;
    for event in &mut commit.events {
        event.run = Some(ctx.agent.run);
    }
    runtime.apply_held(hold, commit)?;
    Ok(match admission {
        aether_memory::Admission::Accepted { record } => {
            ToolOutcome::ok(format!("Remembered as {}.", record.id))
        }
        aether_memory::Admission::Superseded { record, old } => {
            ToolOutcome::ok(format!("Remembered as {} (supersedes {old}).", record.id))
        }
        aether_memory::Admission::Rejected { reason } => {
            ToolOutcome::ok(format!("Not stored: {reason}."))
        }
    })
}
