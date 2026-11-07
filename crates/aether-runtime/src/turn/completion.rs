//! Task completion, idle replies, and bounded unanswered-turn recovery.

use std::collections::BTreeSet;

use aether_core::agent::WaitCondition;
use aether_core::event::{Actor, EventKind};
use aether_core::id::AgentId;
use aether_core::inbox::InboxKind;
use aether_core::model::ModelResponse;
use aether_core::task::{Task, TaskResult, TaskStatus, TaskStatusKind};
use aether_core::time::{Timestamp, now};
use aether_store::{Commit, Mutation};

use super::{Next, TurnCtx};
use crate::{Result, Runtime, RuntimeError};

#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum CompletionSource {
    ApiResponse,
    NativeTurn(AnswerJobs),
}

/// The native background jobs of the turn whose final answer this is.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) enum AnswerJobs {
    /// No job was running when the engine gave its answer.
    Idle,
    /// The answer came while jobs ran; they finished, and the engine never
    /// answered again.
    Finished,
    /// Jobs were still running when the turn ended.
    Running,
}

/// How many times a task's agent is woken for an answer it gave while its
/// native jobs ran, before such an answer completes the task as any other.
const MAX_JOB_WAKES: usize = 2;

/// A response without tool calls: answer delivered, or a worker that forgot
/// to close its task. `ran_tools`: the turn called tools before this final reply.
pub(crate) fn prose_only_next(
    runtime: &Runtime,
    ctx: &TurnCtx,
    response: &ModelResponse,
    ran_tools: bool,
    source: CompletionSource,
) -> Result<Next> {
    // Woken early from a wait and merely replied: go back to waiting.
    if let Some(interrupted) = ctx.agent.resume_wait.clone() {
        let oracle = crate::wait::RuntimeOracle {
            runtime,
            agent: ctx.agent.id,
            inbox_cursor: ctx.agent.inbox_cursor,
        };
        if !interrupted.is_satisfied(&oracle) {
            return Ok(Next::Waiting(interrupted));
        }
    }
    let Some(task) = ctx.task.as_ref().filter(|t| !t.status.is_terminal()) else {
        return Ok(Next::Waiting(WaitCondition::Inbox));
    };
    if ctx.agent.config.long_lived {
        return Ok(Next::Waiting(WaitCondition::Inbox));
    }
    let summary = response.text();
    // Nothing said is no answer: never a result, and never the nudged reply.
    if summary.trim().is_empty() {
        return silent_turn(runtime, ctx, task, ran_tools);
    }
    // Native engines already own the tool loop and have ended it. Asking
    // them to close the same task again adds an otherwise unnecessary turn.
    if let CompletionSource::NativeTurn(jobs) = source {
        if jobs != AnswerJobs::Idle
            && let Some(next) = wake_for_jobs(runtime, ctx, task, jobs)?
        {
            return Ok(next);
        }
        return complete_from_prose(runtime, ctx, task, summary);
    }
    let nudge = completion_nudge(task);
    if !wake_reasons(runtime, ctx.agent.id)?.contains(&nudge) {
        push_wake(runtime, ctx, nudge)?;
        return Ok(Next::Ready);
    }
    // Second prose-only answer: treat it as the result rather than looping.
    complete_from_prose(runtime, ctx, task, summary)
}

/// Names its task, so a nudge sent while working on an earlier one never
/// counts as this task's.
fn completion_nudge(task: &Task) -> String {
    format!(
        "Your task {} is still active. Call `complete_task` with a summary if it is done, `fail_task` if it cannot be done, or keep working.",
        task.id
    )
}

/// Names its task and how many such wakes it has had, so the bound counts
/// this task's wakes only.
fn job_wake_notice(task: &Task, wake: usize) -> String {
    format!(
        "Your last turn on task {} ended while native background jobs you started were still \
         running, before you reported their result (reminder {wake} of {MAX_JOB_WAKES}), so the \
         task stays open. Wait for each job that is still running, or rerun it in the foreground \
         if it is gone, then report your result with `complete_task` or `fail_task`. Do not end \
         a turn while a check you depend on still runs.",
        task.id
    )
}

/// A native turn answered in prose while its background jobs ran: keep the
/// task open and wake the agent: at once when the jobs are over, and after the
/// configured bound when they still ran, as the ended turn tracks them no
/// more. `None` once the task had [`MAX_JOB_WAKES`]: the answer completes it.
fn wake_for_jobs(
    runtime: &Runtime,
    ctx: &TurnCtx,
    task: &Task,
    jobs: AnswerJobs,
) -> Result<Option<Next>> {
    debug_assert!(jobs != AnswerJobs::Idle, "an idle answer needs no wake");
    debug_assert!(
        !ctx.agent.config.long_lived,
        "a long-lived agent keeps waiting"
    );
    // Settled while the turn ran (a cancel): nothing is left to wake for, and
    // `complete_from_prose` leaves such a task as it is.
    let stored = runtime
        .store
        .task(task.id)?
        .ok_or_else(|| RuntimeError::NotFound(task.id.to_string()))?;
    if stored.status.is_terminal() {
        return Ok(None);
    }
    let reasons = wake_reasons(runtime, ctx.agent.id)?;
    let sent = (1..=MAX_JOB_WAKES)
        .take_while(|wake| reasons.contains(&job_wake_notice(task, *wake)))
        .count();
    if sent >= MAX_JOB_WAKES {
        tracing::info!(task = %task.id, "native jobs outlived the answer again; completing from it");
        return Ok(None);
    }
    push_wake(runtime, ctx, job_wake_notice(task, sent + 1))?;
    Ok(Some(match jobs {
        AnswerJobs::Running => Next::Waiting(WaitCondition::Deadline {
            at: super::state::deadline_after(runtime.config.native_job_wake_after),
        }),
        AnswerJobs::Finished | AnswerJobs::Idle => Next::Ready,
    }))
}

/// Names the turn as well, so the run of silent turns before this one can
/// be counted back one turn at a time.
fn silent_turn_notice(task: &Task, turn: u32) -> String {
    format!(
        "Your turn {turn} on task {} ended without an answer. Call `complete_task` with a summary if it is done, `fail_task` if it cannot be done, or keep working.",
        task.id
    )
}

/// A worker's turn ended without a word. Wake it again with a notice, and
/// fail the task once as many turns in a row as the failure limit said nothing.
/// A turn that ran tools did work, so it starts a new run rather than extending one.
fn silent_turn(runtime: &Runtime, ctx: &TurnCtx, task: &Task, ran_tools: bool) -> Result<Next> {
    debug_assert!(!task.status.is_terminal(), "a closed task has no turns");
    let limit = runtime.config.max_consecutive_failures;
    let earlier = if ran_tools {
        0
    } else {
        let reasons = wake_reasons(runtime, ctx.agent.id)?;
        (1..limit)
            .map_while(|back| ctx.turn.checked_sub(back))
            .take_while(|turn| reasons.contains(&silent_turn_notice(task, *turn)))
            .count()
    };
    let streak = u32::try_from(earlier).unwrap_or(u32::MAX).saturating_add(1);
    if streak >= limit {
        return Ok(Next::Failed(format!(
            "{streak} turns in a row on task {} ended without an answer",
            task.id
        )));
    }
    push_wake(runtime, ctx, silent_turn_notice(task, ctx.turn))?;
    Ok(Next::Ready)
}

/// Every wake reason this agent has been sent.
fn wake_reasons(runtime: &Runtime, agent: AgentId) -> Result<BTreeSet<String>> {
    Ok(runtime
        .store
        .inbox(agent, 0)?
        .into_iter()
        .filter_map(|item| match item.kind {
            InboxKind::Wake { reason } => Some(reason),
            _ => None,
        })
        .collect())
}

fn push_wake(runtime: &Runtime, ctx: &TurnCtx, reason: String) -> Result<()> {
    let hold = runtime.hold();
    let mut commit = Commit::new();
    runtime.push_inbox(
        &hold,
        &mut commit,
        &ctx.agent,
        InboxKind::Wake { reason },
        Actor::System,
    )?;
    runtime.apply_held(hold, commit)?;
    Ok(())
}

/// Completes the stored copy of the turn's task. Still Assigned or Blocked
/// there, it is the task this turn works, so it is activated first.
pub(super) fn mark_completed(task: &mut Task, result: TaskResult, at: Timestamp) -> Result<()> {
    if matches!(
        task.status.kind(),
        TaskStatusKind::Assigned | TaskStatusKind::Blocked
    ) {
        task.transition(TaskStatus::Active, at)?;
    }
    task.transition(TaskStatus::Completed { result }, at)?;
    Ok(())
}

fn complete_from_prose(
    runtime: &Runtime,
    ctx: &TurnCtx,
    task: &Task,
    summary: String,
) -> Result<Next> {
    debug_assert!(
        !summary.trim().is_empty(),
        "a blank reply is never a result"
    );
    let hold = runtime.hold();
    let mut task = runtime
        .store
        .task(task.id)?
        .ok_or_else(|| RuntimeError::NotFound(task.id.to_string()))?;
    if task.status.is_terminal() {
        tracing::info!(task = %task.id, status = %task.status.kind(), "ignoring prose completion of a terminal task");
        return Ok(Next::Waiting(WaitCondition::Inbox));
    }
    let mut commit = Commit::new();
    let result = super::prose_result::prose_result(summary);
    if result.structured.is_some() {
        tracing::info!(task = %task.id, "the prose answer is a JSON object; kept as the structured result");
    }
    mark_completed(&mut task, result.clone(), now())?;
    commit.push_event(ctx.draft(EventKind::TaskCompleted {
        result: result.clone(),
    }));
    commit.push_mutation(Mutation::UpsertTask(task.clone()));
    runtime.notify_task_creator(
        &hold,
        &mut commit,
        &task,
        InboxKind::TaskCompleted {
            task: task.id,
            result: result.clone(),
        },
        ctx.base_actor.clone(),
    )?;
    runtime.apply_held(hold, commit)?;
    Ok(Next::Completed(Some(result.summary)))
}

#[cfg(test)]
mod tests {
    use super::*;
    use aether_core::id::RunId;
    #[test]
    fn completion_activates_stored_assigned_and_blocked_tasks() {
        for status in [
            TaskStatus::Assigned,
            TaskStatus::Blocked {
                reason: "waiting for turn".into(),
                dependency: None,
            },
        ] {
            let at = now();
            let mut task = Task::new("stored task", Actor::user(), RunId::new(), at);
            task.status = status;
            mark_completed(&mut task, TaskResult::summary("finished"), at).unwrap();
            assert_eq!(
                task.status,
                TaskStatus::Completed {
                    result: TaskResult::summary("finished")
                }
            );
            assert_eq!(task.updated_at, at);
        }
    }
    #[test]
    fn completion_still_refuses_queued_and_terminal_tasks() {
        for status in [
            TaskStatus::Queued,
            TaskStatus::Completed {
                result: TaskResult::summary("original"),
            },
        ] {
            let at = now();
            let mut task = Task::new("stored task", Actor::user(), RunId::new(), at);
            task.status = status.clone();
            assert!(mark_completed(&mut task, TaskResult::summary("replacement"), at).is_err());
            assert_eq!(task.status, status);
        }
    }
}
