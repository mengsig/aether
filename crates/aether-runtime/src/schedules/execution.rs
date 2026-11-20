use super::{about, check_calendar_range, child, record_schedule};
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
        subsystem: None,
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
        delegation_reason: None,
        references: Vec::new(),
        inherited_references: Vec::new(),
        reference_scope: None,
    }
}

/// A spawn occurrence's task: one line after the template's objective names
/// the schedule, the due time and any payload.
fn occurrence_objective(job: &Schedule, template: &SpawnSpec, due: Timestamp) -> String {
    let mut objective = format!(
        "{}\n\nScheduled run of `{}` ({}) due {}.",
        template.objective.trim(),
        job.name,
        job.id,
        due.to_rfc3339_opts(chrono::SecondsFormat::Secs, true)
    );
    if !job.payload.is_null() {
        // Compact JSON keeps it on one line; supplied by the owner, shown as is.
        objective.push_str(&format!(" Payload: {}", job.payload));
    }
    objective
}

/// What a spawn occurrence has ready before the hold: an operator's root run,
/// or an agent's child with the creator it was decided against.
// Built once per occurrence and consumed; boxing would buy only an allocation.
#[allow(clippy::large_enum_variant)]
pub(super) enum StagedSpawn {
    Root(Result<PreparedAgent>),
    Child(Result<Option<child::Staged>>),
}

/// Build the run a spawn occurrence due at `due` starts. Outside the hold,
/// since preparing an agent may run `git`.
pub(super) fn stage_spawn(runtime: &Runtime, job: &Schedule, due: Timestamp) -> StagedSpawn {
    let ScheduleMode::Spawn {
        template,
        last_spawned,
    } = &job.mode
    else {
        return StagedSpawn::Root(Err(RuntimeError::Internal(format!(
            "schedule {} does not spawn runs",
            job.id
        ))));
    };
    let objective = occurrence_objective(job, template, due);
    match job.spawns_children_of() {
        Some(creator) => StagedSpawn::Child(child::stage(
            runtime,
            template,
            objective,
            creator,
            *last_spawned,
        )),
        None => StagedSpawn::Root(runtime.prepare_agent(
            spawn_request(template, objective),
            &Actor::Scheduler,
            Preflight::default(),
        )),
    }
}

/// One bounded pass. Storage failures retain the original due time, so the
/// next tick retries the same transaction without duplicating a delivery.
pub(crate) fn tick(runtime: &Runtime, at: Timestamp) -> Result<()> {
    for queued in runtime.store.due_schedules(at, 128)? {
        // Built outside the hold; used only if the job is unchanged under it.
        let spawn = match (&queued.mode, queued.status.next_due()) {
            (ScheduleMode::Spawn { .. }, Some(due)) => Some(stage_spawn(runtime, &queued, due)),
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
            Ok(Some(fired)) => fired,
            // Staged against a creator that has since changed: nothing is
            // written, and the next pass stages the same due time again.
            Ok(None) => continue,
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
pub(super) type Fired = (ScheduleOutcome, Option<Agent>);

pub(super) fn skipped(reason: impl Into<String>) -> Fired {
    (
        ScheduleOutcome::Skipped {
            reason: reason.into(),
        },
        None,
    )
}

/// Decide one due occurrence under the hold: advance the schedule, then fire
/// it unless a limit or the missed-run policy says not to. `None`: staged
/// against rows that have since moved, so nothing may be written.
pub(super) fn prepare_occurrence(
    runtime: &Runtime,
    hold: &crate::CommitHold<'_>,
    commit: &mut Commit,
    job: &mut Schedule,
    due: Timestamp,
    at: Timestamp,
    spawn: Option<StagedSpawn>,
) -> Result<Option<Fired>> {
    if job
        .max_deliveries
        .is_some_and(|limit| job.deliveries >= limit.get())
    {
        job.status = ScheduleStatus::Exhausted { at };
        return Ok(Some(skipped("delivery limit reached")));
    }
    // Checked before the missed-run policy, so a gone target or an ended
    // creator fails the job even on an occurrence that would be skipped.
    let target = match job.target() {
        Some(id) => Some(runtime.schedule_target(id, &job.owner)?),
        None => None,
    };
    if let Some(creator) = job.spawns_children_of() {
        child::check_creator_live(runtime, creator)?;
    }
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
        return Ok(Some(skipped("missed occurrences skipped by policy")));
    }
    // Before any mode field changes, so a refusal leaves nothing half-applied.
    let deliveries = job
        .deliveries
        .checked_add(1)
        .ok_or_else(|| RuntimeError::Invalid("schedule delivery counter exhausted".into()))?;
    let Some(fired) = fire(runtime, hold, commit, job, target, spawn)? else {
        return Ok(None);
    };
    // Only a delivered trigger or a started run counts against the limit.
    if matches!(fired.0, ScheduleOutcome::Skipped { .. }) {
        return Ok(Some(fired));
    }
    job.deliveries = deliveries;
    if job
        .max_deliveries
        .is_some_and(|limit| deliveries >= limit.get())
    {
        job.status = ScheduleStatus::Exhausted { at };
    }
    Ok(Some(fired))
}

/// Deliver the trigger or start the run this occurrence stands for.
fn fire(
    runtime: &Runtime,
    hold: &crate::CommitHold<'_>,
    commit: &mut Commit,
    job: &mut Schedule,
    target: Option<Agent>,
    spawn: Option<StagedSpawn>,
) -> Result<Option<Fired>> {
    let creator = job.spawns_children_of();
    let no_target = || RuntimeError::Internal(format!("schedule {} has no target to fire", job.id));
    match (&mut job.mode, target, spawn) {
        (ScheduleMode::Deliver { last_inbox_seq, .. }, Some(target), None) => {
            if let Some(reason) = busy_reason(&target, *last_inbox_seq, job.busy) {
                return Ok(Some(skipped(reason)));
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
            Ok(Some((ScheduleOutcome::Delivered { inbox: item.id }, None)))
        }
        (ScheduleMode::Spawn { last_spawned, .. }, None, Some(StagedSpawn::Root(prepared)))
            if creator.is_none() =>
        {
            spawn_root(runtime, commit, last_spawned, prepared).map(Some)
        }
        (
            ScheduleMode::Spawn {
                template,
                last_spawned,
            },
            None,
            Some(StagedSpawn::Child(staged)),
        ) => {
            let creator = creator.ok_or_else(no_target)?;
            child::fire(
                runtime,
                hold,
                commit,
                creator,
                template,
                last_spawned,
                staged,
            )
        }
        _ => Err(no_target()),
    }
}

/// Start an operator's root run, or skip while its previous run is live.
fn spawn_root(
    runtime: &Runtime,
    commit: &mut Commit,
    last_spawned: &mut Option<AgentId>,
    prepared: Result<PreparedAgent>,
) -> Result<Fired> {
    if let Some(reason) = previous_run_live(runtime, *last_spawned)? {
        return Ok(skipped(reason));
    }
    let PreparedAgent {
        agent,
        task,
        commit: created,
        ..
    } = prepared?;
    if runtime.workspace_gc_reserves(&agent.scope) {
        return Ok(skipped("workspace maintenance in progress"));
    }
    runtime.recheck_workspace_gc_admission(&agent.scope)?;
    let task =
        task.ok_or_else(|| RuntimeError::Internal("a spawned run must carry its task".into()))?;
    debug_assert!(agent.parent.is_none(), "an operator's run is a root");
    commit.events.extend(created.events);
    commit.mutations.extend(created.mutations);
    commit.push_mutation(Mutation::UpsertAgent(agent.clone()));
    *last_spawned = Some(agent.id);
    let outcome = ScheduleOutcome::Spawned {
        agent: agent.id,
        task: task.id,
        run: agent.run,
    };
    Ok((outcome, Some(agent)))
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
