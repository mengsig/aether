//! Schedules: creation, lookup, updates and history.

use crate::Daemon;
use aether_core::event::Actor;
use aether_core::idempotency::Submission;
use aether_core::schedule::ScheduleTarget;
use aether_protocol::{Command, ErrorReply, Reply};

use crate::connection::{map_err, store_err};

pub(super) fn owns(command: &Command) -> bool {
    matches!(
        command,
        Command::CreateSchedule { .. }
            | Command::GetSchedule { .. }
            | Command::ListSchedules { .. }
            | Command::UpdateSchedule { .. }
            | Command::ScheduleHistory { .. }
    )
}

pub(super) async fn dispatch(
    daemon: &Daemon,
    command: Command,
    by: Actor,
    submission: Option<Submission>,
) -> Result<Reply, ErrorReply> {
    let rt = &daemon.runtime;
    match command {
        Command::CreateSchedule { spec, .. } => {
            // A spawn template without a model is frozen to the default now,
            // so resolve it lazily the way `create_agent` does.
            if rt.default_model().is_none()
                && matches!(&spec.target, ScheduleTarget::Spawn(template) if template.model.is_none())
            {
                rt.set_default_model(daemon.models.default_model().await);
            }
            Ok(Reply::Schedule(
                rt.create_schedule_keyed(spec, by, submission)
                    .map_err(map_err)?,
            ))
        }
        Command::GetSchedule { id } => Ok(Reply::Schedule(
            rt.store()
                .schedule(id)
                .map_err(store_err)?
                .ok_or_else(|| map_err(aether_runtime::RuntimeError::NotFound(id.to_string())))?,
        )),
        Command::ListSchedules { target } => Ok(Reply::Schedules(
            rt.store().schedules(target).map_err(store_err)?,
        )),
        Command::UpdateSchedule { id, action, .. } => Ok(Reply::Schedule(
            rt.update_schedule_keyed(id, action, by, submission)
                .map_err(map_err)?,
        )),
        Command::ScheduleHistory { id, limit } => Ok(Reply::ScheduleHistory(
            rt.store()
                .schedule_history(id, limit.min(1000))
                .map_err(store_err)?,
        )),
        _ => unreachable!("routed to the wrong command family"),
    }
}
