use super::{about, check_calendar_range, record_schedule};
use crate::{
    CreateAgentSpec, Result, Runtime, RuntimeError,
    commands::{Preflight, PreparedAgent},
};
use aether_core::{
    agent::{Agent, AgentStatus},
    event::{Actor, EventDraft, EventKind},
    id::AgentId,
    inbox::InboxKind,
    schedule::{
        BusyPolicy, Schedule, ScheduleMode, ScheduleOccurrence, ScheduleOutcome, ScheduleStatus,
        SpawnSpec,
    },
    time::Timestamp,
};
use aether_store::{AgentFilter, Commit, Mutation};

/// The `create_agent` request one spawn occurrence makes.
pub(super) fn spawn_request(template: &SpawnSpec, objective: String) -> CreateAgentSpec {
    CreateAgentSpec {
        role: template.role.clone(),
        name: template.name.clone(),
        objective: Some(objective),
        model: template.model.clone(),
        fallbacks: template.fallbacks.clone(),
        scope: template.scope.clone(),
        repository_root: None,
        parent: None,
        config: template.config.clone(),
        approval: template.approval.clone(),
        run: None,
        start: true,
    }
}

/// The run a spawn occurrence due at `due` starts. One line after the
/// template's objective names the schedule, the due time and any payload.
pub(super) fn prepare_spawn(
    runtime: &Runtime,
    job: &Schedule,
    due: Timestamp,
) -> Result<PreparedAgent> {
    let ScheduleMode::Spawn { template, .. } = &job.mode else {
        return Err(RuntimeError::Internal(format!(
            "schedule {} does not spawn runs",
            job.id
        )));
    };
    let mut objective = format!(
        "{}\n\nScheduled run of `{}` ({}) due {}.",
        template.objective.trim(),
        job.name,
        job.id,
        due.to_rfc3339_opts(chrono::SecondsFormat::Secs, true)
    );
    if !job.payload.is_null() {
        // Compact JSON keeps it on one line; operator-supplied, shown as is.
        objective.push_str(&format!(" Payload: {}", job.payload));
    }
    runtime.prepare_agent(
        spawn_request(template, objective),
        &Actor::Scheduler,
        Preflight::default(),
    )
}

/// One bounded pass. Storage failures retain the original due time, so the
/// next tick retries the same transaction without duplicating a delivery.
pub(crate) fn tick(runtime: &Runtime, at: Timestamp) -> Result<()> {
    for queued in runtime.store.due_schedules(at, 128)? {
        // Built outside the hold, since preparing an agent may run `git`; used
        // only if the job is unchanged under the hold.
        let spawn = match (&queued.mode, queued.status.next_due()) {
            (ScheduleMode::Spawn { .. }, Some(due)) => Some(prepare_spawn(runtime, &queued, due)),
            _ => None,
        };
        let hold = runtime.hold();
        // Pause/cancel can win between the queue read and taking the gate.
        let Some(mut job) = runtime.store.schedule(queued.id)? else {
            continue;
        };
        let Some(due) = job.status.next_due().filter(|due| *due <= at) else {
            continue;
        };
        if spawn.is_some() && job != queued {
            // Prepared for a job that has since changed; the next pass rebuilds.
            continue;
        }
        let mut commit = Commit::new();
        let result = prepare_occurrence(runtime, &hold, &mut commit, &mut job, due, at, spawn);
        let (outcome, spawned) = match result {
            Ok(fired) => fired,
            // Store failures are transient candidates, not a failed job. No
            // state from a partially built commit may reach the database.
            Err(error @ RuntimeError::Store(_)) => return Err(error),
            Err(error) => {
                commit = Commit::new();
                let error = error.to_string();
                job.status = ScheduleStatus::Failed {
                    at,
                    error: error.clone(),
                };
                (ScheduleOutcome::Failed { error }, None)
            }
        };
        let occurrence = ScheduleOccurrence {
            schedule: job.id,
            due,
            recorded_at: at,
            outcome,
        };
        let event = EventDraft::new(
            Actor::Scheduler,
            EventKind::Custom {
                name: "schedule.occurrence".into(),
                payload: serde_json::json!(occurrence),
            },
        );
        // A spawned run's log shows the occurrence that started it.
        let event = match &occurrence.outcome {
            ScheduleOutcome::Spawned { agent, run, .. } => event.agent(*agent).run(*run),
            _ => about(event, &job),
        };
        commit.push_event(event);
        commit.push_mutation(Mutation::RecordScheduleOccurrence(occurrence.clone()));
        job.last = Some(occurrence);
        job.updated_at = at;
        record_schedule(&mut commit, &job, "advanced", Actor::Scheduler);
        runtime.apply_held(hold, commit)?;
        if let Some(agent) = spawned {
            runtime.seed_repository_knowledge(&agent, Actor::Scheduler);
        }
    }
    Ok(())
}

/// What an occurrence did, and the root it spawned, if any.
type Fired = (ScheduleOutcome, Option<Agent>);

fn skipped(reason: impl Into<String>) -> Fired {
    (
        ScheduleOutcome::Skipped {
            reason: reason.into(),
        },
        None,
    )
}

pub(super) fn prepare_occurrence(
    runtime: &Runtime,
    hold: &crate::CommitHold<'_>,
    commit: &mut Commit,
    job: &mut Schedule,
    due: Timestamp,
    at: Timestamp,
    spawn: Option<Result<PreparedAgent>>,
) -> Result<Fired> {
    if job
        .max_deliveries
        .is_some_and(|limit| job.deliveries >= limit.get())
    {
        job.status = ScheduleStatus::Exhausted { at };
        return Ok(skipped("delivery limit reached"));
    }
    // Checked before the missed-run policy, so a gone target fails the job
    // even on an occurrence that would have been skipped.
    let target = match job.target() {
        Some(id) => Some(runtime.schedule_target(id, &job.owner)?),
        None => None,
    };
    let planned = job
        .recurrence
        .plan_due(due, at, job.missed)
        .map_err(|error| RuntimeError::Invalid(error.to_string()))?
        .ok_or_else(|| {
            RuntimeError::Internal("schedule was not due under its commit lock".into())
        })?;
    check_calendar_range(planned.next_due)?;
    job.status = ScheduleStatus::Active {
        next_due: planned.next_due,
    };
    if !planned.deliver {
        return Ok(skipped("missed occurrences skipped by policy"));
    }
    // Before any mode field changes, so a refusal leaves nothing half-applied.
    let deliveries = job
        .deliveries
        .checked_add(1)
        .ok_or_else(|| RuntimeError::Invalid("schedule delivery counter exhausted".into()))?;
    let fired = match (&mut job.mode, target) {
        (ScheduleMode::Deliver { last_inbox_seq, .. }, Some(target)) => {
            if let Some(reason) = busy_reason(&target, *last_inbox_seq, job.busy) {
                return Ok(skipped(reason));
            }
            let item = runtime.push_inbox(
                hold,
                commit,
                &target,
                InboxKind::ExternalTrigger {
                    source: format!("schedule:{}", job.id),
                    payload: job.payload.clone(),
                    operator: false,
                },
                Actor::Scheduler,
            )?;
            *last_inbox_seq = Some(item.seq);
            (ScheduleOutcome::Delivered { inbox: item.id }, None)
        }
        (ScheduleMode::Spawn { last_spawned, .. }, None) => {
            if let Some(reason) = previous_run_live(runtime, *last_spawned)? {
                return Ok(skipped(reason));
            }
            let PreparedAgent {
                agent,
                task,
                commit: created,
                ..
            } = spawn.ok_or_else(|| {
                RuntimeError::Internal("spawn occurrence was not prepared".into())
            })??;
            if runtime.workspace_gc_reserves(&agent.scope) {
                return Ok(skipped("workspace maintenance in progress"));
            }
            runtime.recheck_workspace_gc_admission(&agent.scope)?;
            let task = task.ok_or_else(|| {
                RuntimeError::Internal("a spawned run must carry its task".into())
            })?;
            commit.events.extend(created.events);
            commit.mutations.extend(created.mutations);
            commit.push_mutation(Mutation::UpsertAgent(agent.clone()));
            *last_spawned = Some(agent.id);
            let outcome = ScheduleOutcome::Spawned {
                agent: agent.id,
                task: task.id,
                run: agent.run,
            };
            (outcome, Some(agent))
        }
        _ => {
            return Err(RuntimeError::Internal(format!(
                "schedule {} has no target to fire",
                job.id
            )));
        }
    };
    job.deliveries = deliveries;
    if job
        .max_deliveries
        .is_some_and(|limit| deliveries >= limit.get())
    {
        job.status = ScheduleStatus::Exhausted { at };
    }
    Ok(fired)
}

/// Why the next spawn must wait: any agent of the previous run is still live.
/// The whole run, so a restarted root or a finished root's live children count.
fn previous_run_live(runtime: &Runtime, last_spawned: Option<AgentId>) -> Result<Option<String>> {
    let Some(previous) = last_spawned else {
        return Ok(None);
    };
    let Some(root) = runtime.store.agent(previous)? else {
        return Ok(None);
    };
    let live = runtime.store.agents(&AgentFilter {
        run: Some(root.run),
        live_only: true,
        ..AgentFilter::default()
    })?;
    Ok(live.first().map(|agent| {
        let state = if agent.paused {
            "paused".to_owned()
        } else {
            agent.status.kind().to_string()
        };
        format!(
            "previous run {} is still live: {} is {state}",
            root.run, agent.id
        )
    }))
}

/// Why a delivery must wait. Whatever the policy, at most one trigger from
/// a schedule is pending in the target's inbox.
fn busy_reason(
    target: &Agent,
    last_inbox_seq: Option<u64>,
    busy: BusyPolicy,
) -> Option<&'static str> {
    let pending = last_inbox_seq.is_some_and(|seq| target.inbox_cursor < seq);
    if target.paused {
        Some("target is paused")
    } else if busy == BusyPolicy::Skip && (target.status == AgentStatus::Running || pending) {
        Some("target is running or the previous trigger is still pending")
    } else if pending {
        Some("the previous trigger is still pending")
    } else {
        None
    }
}
