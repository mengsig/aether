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

/// A run built for one occurrence, with the template it was built from:
/// for an agent's schedule, bounded by its creator when it was built.
pub(super) struct PreparedSpawn {
    pub run: PreparedAgent,
    pub template: SpawnSpec,
}

/// The run a spawn occurrence due at `due` starts. One line after the
/// template's objective names the schedule, the due time and any payload.
pub(super) fn prepare_spawn(
    runtime: &Runtime,
    job: &Schedule,
    due: Timestamp,
) -> Result<PreparedSpawn> {
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
        // Compact JSON keeps it on one line; shown as its creator sent it.
        objective.push_str(&format!(" Payload: {}", job.payload));
    }
    // An agent's authority may have narrowed, or ended, since it scheduled this.
    let template = match &job.owner {
        Actor::Agent { id } => runtime.bounded_agent_template(template.clone(), *id)?,
        _ => template.clone(),
    };
    let run = runtime.prepare_agent(
        spawn_request(&template, objective),
        &Actor::Scheduler,
        Preflight::default(),
    )?;
    // Creation drops the gates of an engine that cannot ask first; the
    // capability may have changed since the template was bounded.
    if matches!(job.owner, Actor::Agent { .. })
        && !runtime.config.yolo
        && Some(&run.agent.config.approval) != template.approval.as_ref()
    {
        return Err(RuntimeError::Invalid(
            "spawn schedule refused: the run would not keep its creator's approval gates".into(),
        ));
    }
    Ok(PreparedSpawn { run, template })
}

/// The run of a due job, built before the commit hold since preparing an
/// agent may run `git`; none for a job that does not spawn.
pub(super) fn prepare_queued(
    runtime: &Runtime,
    queued: &Schedule,
) -> Option<Result<PreparedSpawn>> {
    match (&queued.mode, queued.status.next_due()) {
        (ScheduleMode::Spawn { .. }, Some(due)) => Some(prepare_spawn(runtime, queued, due)),
        _ => None,
    }
}

/// One bounded pass. Storage failures retain the original due time, so the
/// next tick retries the same transaction without duplicating a delivery.
pub(crate) fn tick(runtime: &Runtime, at: Timestamp) -> Result<()> {
    for queued in runtime.store.due_schedules(at, 128)? {
        let spawn = prepare_queued(runtime, &queued);
        fire_queued(runtime, &queued, spawn, at)?;
    }
    Ok(())
}

/// Fires `queued` under the commit hold with the run built for it, unless
/// the job or its creator's authority changed since; the next pass rebuilds.
pub(super) fn fire_queued(
    runtime: &Runtime,
    queued: &Schedule,
    spawn: Option<Result<PreparedSpawn>>,
    at: Timestamp,
) -> Result<()> {
    let hold = runtime.hold();
    // Pause/cancel can win between the queue read and taking the gate.
    let Some(mut job) = runtime.store.schedule(queued.id)? else {
        return Ok(());
    };
    let Some(due) = job.status.next_due().filter(|due| *due <= at) else {
        return Ok(());
    };
    if spawn.is_some() && job != *queued {
        return Ok(());
    }
    let mut commit = Commit::new();
    let result = prepare_occurrence(runtime, &hold, &mut commit, &mut job, due, at, spawn);
    let (outcome, spawned) = match result {
        Ok(Some(fired)) => fired,
        Ok(None) => return Ok(()),
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
    record_occurrence(&mut commit, &mut job, due, at, outcome);
    runtime.apply_held(hold, commit)?;
    if let Some(agent) = spawned {
        runtime.seed_repository_knowledge(&agent, Actor::Scheduler);
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

/// The occurrence `job` records under `hold`; none when the run was built
/// under authority its creator no longer has, so nothing is committed.
pub(super) fn prepare_occurrence(
    runtime: &Runtime,
    hold: &crate::CommitHold<'_>,
    commit: &mut Commit,
    job: &mut Schedule,
    due: Timestamp,
    at: Timestamp,
    spawn: Option<Result<PreparedSpawn>>,
) -> Result<Option<Fired>> {
    if job
        .max_deliveries
        .is_some_and(|limit| job.deliveries >= limit.get())
    {
        job.status = ScheduleStatus::Exhausted { at };
        return Ok(Some(skipped("delivery limit reached")));
    }
    // Checked before the missed-run policy, so a gone target fails the job
    // even on an occurrence that would have been skipped.
    let target = match job.target() {
        Some(id) => Some(runtime.schedule_target(id, &job.owner)?),
        None => None,
    };
    // Likewise a finished or cancelling creator fails its spawn schedule
    // before any skip.
    let creator = held_creator(runtime, hold, job)?;
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
    if let Some(reason) = spawn_wait(runtime, job, creator.as_ref())? {
        return Ok(Some(skipped(reason)));
    }
    if let Some(creator) = &creator
        && !built_under_current_authority(runtime, job, creator, spawn.as_ref())?
    {
        return Ok(None);
    }
    // Before any mode field changes, so a refusal leaves nothing half-applied.
    let deliveries = job
        .deliveries
        .checked_add(1)
        .ok_or_else(|| RuntimeError::Invalid("schedule delivery counter exhausted".into()))?;
    let fired = match (&mut job.mode, target) {
        (ScheduleMode::Deliver { last_inbox_seq, .. }, Some(target)) => {
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
            (ScheduleOutcome::Delivered { inbox: item.id }, None)
        }
        (ScheduleMode::Spawn { last_spawned, .. }, None) => {
            let prepared = spawn.ok_or_else(|| {
                RuntimeError::Internal("spawn occurrence was not prepared".into())
            })??;
            match fire_spawn(runtime, commit, prepared.run)? {
                Some((outcome, agent)) => {
                    *last_spawned = Some(agent.id);
                    (outcome, Some(agent))
                }
                None => return Ok(Some(skipped("workspace maintenance in progress"))),
            }
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
    Ok(Some(fired))
}

/// Adds a prepared run to `commit`; none while workspace maintenance holds
/// its workspace.
fn fire_spawn(
    runtime: &Runtime,
    commit: &mut Commit,
    run: PreparedAgent,
) -> Result<Option<(ScheduleOutcome, Agent)>> {
    let PreparedAgent {
        agent,
        task,
        commit: created,
        ..
    } = run;
    if runtime.workspace_gc_reserves(&agent.scope) {
        return Ok(None);
    }
    runtime.recheck_workspace_gc_admission(&agent.scope)?;
    let task =
        task.ok_or_else(|| RuntimeError::Internal("a spawned run must carry its task".into()))?;
    commit.events.extend(created.events);
    commit.mutations.extend(created.mutations);
    commit.push_mutation(Mutation::UpsertAgent(agent.clone()));
    let outcome = ScheduleOutcome::Spawned {
        agent: agent.id,
        task: task.id,
        run: agent.run,
    };
    Ok(Some((outcome, agent)))
}

/// The live creator of an agent's spawn job, outside any cancellation.
fn held_creator(
    runtime: &Runtime,
    hold: &crate::CommitHold<'_>,
    job: &Schedule,
) -> Result<Option<Agent>> {
    let (Actor::Agent { id }, ScheduleMode::Spawn { .. }) = (&job.owner, &job.mode) else {
        return Ok(None);
    };
    let creator = runtime.schedule_creator(*id)?;
    crate::cancellation::check_parent(runtime, hold, creator.id)?;
    Ok(Some(creator))
}

/// Why a spawn occurrence must wait: its creator is paused or the previous
/// run is still live.
fn spawn_wait(
    runtime: &Runtime,
    job: &Schedule,
    creator: Option<&Agent>,
) -> Result<Option<String>> {
    let ScheduleMode::Spawn { last_spawned, .. } = &job.mode else {
        return Ok(None);
    };
    if let Some(creator) = creator.filter(|creator| creator.paused) {
        return Ok(Some(format!("its creator {} is paused", creator.id)));
    }
    previous_run_live(runtime, *last_spawned)
}

/// Records `outcome` on `job` and in `commit`, with the event naming it.
fn record_occurrence(
    commit: &mut Commit,
    job: &mut Schedule,
    due: Timestamp,
    at: Timestamp,
    outcome: ScheduleOutcome,
) {
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
        _ => about(event, job),
    };
    commit.push_event(event);
    commit.push_mutation(Mutation::RecordScheduleOccurrence(occurrence.clone()));
    job.last = Some(occurrence);
    job.updated_at = at;
    record_schedule(commit, job, "advanced", Actor::Scheduler);
}

/// Whether an agent's run was built from the template its creator's current
/// row bounds. A refusal under the hold fails the schedule.
fn built_under_current_authority(
    runtime: &Runtime,
    job: &Schedule,
    creator: &Agent,
    spawn: Option<&Result<PreparedSpawn>>,
) -> Result<bool> {
    let ScheduleMode::Spawn { template, .. } = &job.mode else {
        return Err(RuntimeError::Internal("an agent's job must spawn".into()));
    };
    debug_assert_eq!(job.owner, Actor::agent(creator.id), "the job's own creator");
    let current = runtime.bounded_agent_template(template.clone(), creator.id)?;
    Ok(match spawn {
        Some(Ok(prepared)) => prepared.template == current,
        // A failed or missing build fails the job once the authority holds.
        Some(Err(_)) | None => true,
    })
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
