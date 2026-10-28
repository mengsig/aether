use super::wake::person_message_pending;
use crate::problems::Subject;
use crate::{CommitHold, Result, Runtime};
use aether_core::{
    agent::WaitCondition,
    id::TaskId,
    inbox::InboxKind,
    task::{TaskStatus, TaskStatusKind},
};
use aether_core::{
    agent::{AgentStatus, BlockReason},
    event::{Actor, EventDraft, EventKind},
    time::now,
};
use aether_store::TaskFilter;
use aether_store::{Commit, Mutation};
use std::sync::Arc;

/// VISION §3.2: a task stays Queued until every dependency is terminal and
/// its owner is not scheduled for it before then. The scheduler owns the
/// gate, so it holds however the task was created and whoever finished the
/// dependency.
pub(super) fn gate_dependencies(
    runtime: &Arc<Runtime>,
    blocked: &mut std::collections::HashSet<aether_core::id::AgentId>,
    errors: &mut Vec<crate::RuntimeError>,
) -> Result<()> {
    let queued = runtime.store.tasks(&TaskFilter {
        statuses: vec![TaskStatusKind::Queued],
        ..Default::default()
    })?;
    let gated: Vec<_> = queued
        .into_iter()
        .filter(|t| t.owner.is_some() && !t.dependencies.is_empty())
        .collect();
    // A task that left the queue no longer withholds its owner.
    let ids: std::collections::HashSet<_> = gated.iter().map(|t| t.id).collect();
    runtime.retain_problems(|subject| match subject {
        Subject::Task(id) => ids.contains(&id),
        Subject::Agent(_) => true,
    });
    for task in gated {
        let key = format!("dependency.{}", task.id);
        match super::record_work(|| gate_task(runtime, &task)) {
            Ok(()) => runtime.clear_problem(&key),
            Err(error) => {
                blocked.extend(task.owner);
                runtime.report_problem_about(Subject::Task(task.id), key, format!("Dependencies for task {} cannot be checked: {error}; its owner is withheld. Run `aether doctor` and `aether diagnose`.", task.id));
                errors.push(error);
            }
        }
    }
    Ok(())
}

fn gate_task(runtime: &Arc<Runtime>, task: &aether_core::task::Task) -> Result<()> {
    // The scan is only a candidate list: cancellation and pause may land
    // before this task's gate decision, so read its rows under the hold.
    let hold = runtime.hold();
    let Some(task) = runtime.store.task(task.id)? else {
        return Ok(());
    };
    if task.status != TaskStatus::Queued || task.owner.is_none() || task.dependencies.is_empty() {
        return Ok(());
    }
    match unmet_dependency(runtime, &task)? {
        Some(dependency) => block_owner(runtime, hold, &task, dependency)?,
        None => assign_and_wake(runtime, hold, task)?,
    }
    Ok(())
}

/// The first dependency that has not finished. A dependency that no longer
/// exists counts as unmet: it can never report a result.
fn unmet_dependency(runtime: &Runtime, task: &aether_core::task::Task) -> Result<Option<TaskId>> {
    for id in &task.dependencies {
        match runtime.store.task(*id)? {
            Some(dep)
                if dep.status.is_terminal() && !crate::wait::task_turn_running(runtime, dep.id) => {
            }
            _ => return Ok(Some(*id)),
        }
    }
    Ok(None)
}

/// Hold the owner in Blocked, naming the dependency it is waiting on. A wait
/// the agent chose is never discarded for the gate (VISION §3.6): only a
/// Ready agent, or one the scheduler itself parked as idle, is held.
fn block_owner(
    runtime: &Runtime,
    hold: CommitHold<'_>,
    task: &aether_core::task::Task,
    dependency: TaskId,
) -> Result<()> {
    let Some(owner) = task
        .owner
        .and_then(|id| runtime.store.agent(id).transpose())
    else {
        return Ok(());
    };
    let mut owner = owner?;
    if matches!(
        &owner.status,
        AgentStatus::Blocked { reason: BlockReason::Dependency { on } } if *on == dependency
    ) {
        return Ok(());
    }
    let holdable = match &owner.status {
        AgentStatus::Ready => true,
        // `park` writes this one; anything else is the agent's own decision,
        // and an approval wait is never bounded or interrupted.
        AgentStatus::Waiting { condition } => *condition == WaitCondition::Inbox,
        _ => false,
    };
    if !holdable {
        return Ok(());
    }
    // A turn in flight finishes first; the status lags the dispatch, so ask
    // the scheduler rather than the store.
    if runtime.sched.lock().running.contains_key(&owner.id) {
        return Ok(());
    }
    // Limit 1 outranks the gate: never re-block an agent a person is waiting on.
    if person_message_pending(runtime, &owner)? {
        return Ok(());
    }
    let reason = BlockReason::Dependency { on: dependency };
    let described = format!("task {} waits for {dependency}", task.id);
    owner.transition(
        AgentStatus::Blocked {
            reason: reason.clone(),
        },
        now(),
    )?;
    runtime.apply_held(
        hold,
        Commit::new()
            .event(
                EventDraft::new(
                    Actor::Scheduler,
                    EventKind::AgentBlocked {
                        reason: described,
                        dependency: reason.dependency(),
                    },
                )
                .agent(owner.id)
                .task(task.id)
                .run(owner.run),
            )
            .mutate(Mutation::UpsertAgent(owner)),
    )?;
    Ok(())
}

/// Every dependency is terminal: assign the task and wake whoever owns it.
fn assign_and_wake(
    runtime: &Runtime,
    hold: CommitHold<'_>,
    mut task: aether_core::task::Task,
) -> Result<()> {
    let Some(owner_id) = task.owner else {
        return Ok(());
    };
    let Some(mut owner) = runtime.store.agent(owner_id)? else {
        return Ok(());
    };
    if owner.status.is_terminal() {
        return Ok(());
    }
    task.transition(TaskStatus::Assigned, now())?;
    let mut commit = Commit::new()
        .event(
            EventDraft::new(Actor::Scheduler, EventKind::TaskUnblocked)
                .agent(owner_id)
                .task(task.id)
                .run(task.run),
        )
        .event(
            EventDraft::new(
                Actor::Scheduler,
                EventKind::TaskAssigned { owner: owner_id },
            )
            .agent(owner_id)
            .task(task.id)
            .run(task.run),
        )
        .mutate(Mutation::UpsertTask(task.clone()));
    let reason = format!(
        "dependencies of task {} finished ({})",
        task.id,
        describe_dependencies(runtime, &task)?
    );
    match &owner.status {
        // Only the block this gate created is ours to clear: an inspection or
        // another gate's hold survives.
        AgentStatus::Blocked {
            reason: BlockReason::Dependency { on },
        } if task.dependencies.contains(on) => {
            owner.transition(AgentStatus::Ready, now())?;
            commit.push_event(
                EventDraft::new(
                    Actor::Scheduler,
                    EventKind::AgentResumed {
                        reason: reason.clone(),
                        via: None,
                    },
                )
                .agent(owner_id)
                .run(owner.run),
            );
            // Ready alone is not work: an agent whose last word was its own
            // answer needs the inbox item to be dispatched at all.
            runtime.push_inbox(
                &hold,
                &mut commit,
                &owner,
                InboxKind::Wake { reason },
                Actor::Scheduler,
            )?;
            commit.push_mutation(Mutation::UpsertAgent(owner));
        }
        // A wait of its own decides whether this is worth waking for.
        AgentStatus::Waiting { .. } => {
            runtime.push_inbox(
                &hold,
                &mut commit,
                &owner,
                InboxKind::Wake { reason },
                Actor::Scheduler,
            )?;
        }
        _ => {}
    }
    runtime.apply_held(hold, commit)?;
    Ok(())
}

/// How each dependency ended, so the owner is not dispatched onto dependent
/// work believing everything it waited for produced a result.
fn describe_dependencies(runtime: &Runtime, task: &aether_core::task::Task) -> Result<String> {
    let mut out = Vec::with_capacity(task.dependencies.len());
    for id in &task.dependencies {
        let status = match runtime.store.task(*id)? {
            Some(dep) => dep.status.kind().to_string(),
            None => "gone".to_owned(),
        };
        out.push(format!("{id} {status}"));
    }
    Ok(out.join(", "))
}
