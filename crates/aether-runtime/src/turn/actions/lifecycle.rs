use super::*;

struct SettleVerb {
    imperative: &'static str,
    past: &'static str,
    no_task: &'static str,
}

/// The caller's task, reloaded under `hold`, if it can still be settled.
fn open_task(
    runtime: &Arc<Runtime>,
    ctx: &TurnCtx,
    effects: &ToolEffects,
    _hold: &CommitHold,
    verb: SettleVerb,
) -> Result<std::result::Result<Task, ToolOutcome>> {
    let task = match ctx.task.as_ref() {
        // `structured.task_less` lets a client mark this
        // informational row distinctly instead of a bare green
        // "succeeded" (the outcome is genuinely not an error).
        None => {
            return Ok(Err(ToolOutcome {
                structured: Some(serde_json::json!({"task_less": true})),
                ..ToolOutcome::ok(verb.no_task)
            }));
        }
        Some(task) => runtime
            .store
            .task(task.id)?
            .ok_or_else(|| RuntimeError::NotFound(task.id.to_string()))?,
    };
    if task.status.is_terminal() {
        return Ok(Err(ToolOutcome::error(format!(
            "Task {} is already {} and cannot be {} again.",
            task.id,
            task.status.kind(),
            verb.past
        ))));
    }
    // Only the path that really settles the task is refused. A long_lived
    // agent's settle parks it on Inbox, so the question's own park holds.
    if let Some(question) = &effects.asked
        && !ctx.agent.config.long_lived
    {
        return Ok(Err(ToolOutcome::error(still_asking(
            verb.imperative,
            verb.past,
            question,
        ))));
    }
    Ok(Ok(task))
}

/// A long_lived agent parks on Inbox instead of ending. Merged, not
/// assigned: an ask_user earlier in the batch may have recorded that wait.
fn end_or_park(ctx: &TurnCtx, effects: &mut ToolEffects, next: Next) {
    if ctx.agent.config.long_lived {
        merge_wait(effects, WaitCondition::Inbox);
    } else {
        effects.next = Some(next);
    }
}

fn foreign_artifacts_refusal(
    runtime: &Arc<Runtime>,
    ctx: &TurnCtx,
    artifacts: &[ArtifactId],
) -> Result<Option<ToolOutcome>> {
    let Some(id) = first_artifact_outside_scope(runtime, ctx.agent.id, artifacts)? else {
        return Ok(None);
    };
    Ok(Some(ToolOutcome::error(format!(
        "complete_task.artifacts contains artifact `{id}` you did not produce: only \
         artifacts from you or your descendants can be attached. Use the artifact id \
         returned when it was stored."
    ))))
}

fn complete_task(
    runtime: &Arc<Runtime>,
    ctx: &mut TurnCtx,
    effects: &mut ToolEffects,
    summary: String,
    artifacts: Vec<ArtifactId>,
    structured: Option<serde_json::Value>,
) -> Result<ToolOutcome> {
    if let Some(refusal) = foreign_artifacts_refusal(runtime, ctx, &artifacts)? {
        return Ok(refusal);
    }
    let hold = runtime.hold();
    let mut task = match open_task(
        runtime,
        ctx,
        effects,
        &hold,
        SettleVerb {
            imperative: "complete",
            past: "completed",
            no_task: NO_ACTIVE_TASK_TO_COMPLETE,
        },
    )? {
        Ok(task) => task,
        Err(refusal) => return Ok(refusal),
    };
    let result = TaskResult {
        summary,
        artifacts,
        structured,
    };
    mark_completed(&mut task, result.clone(), now())?;
    let mut commit = Commit::new()
        .event(ctx.draft(EventKind::TaskCompleted {
            result: result.clone(),
        }))
        .mutate(Mutation::UpsertTask(task.clone()));
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
    ctx.task = Some(task.clone());
    end_or_park(ctx, effects, Next::Completed(Some(result.summary.clone())));
    Ok(ToolOutcome::ok(format!(
        "Task {} completed. {TURN_ENDS_HERE}",
        task.id
    )))
}

fn fail_task(
    runtime: &Arc<Runtime>,
    ctx: &mut TurnCtx,
    effects: &mut ToolEffects,
    error: String,
    structured: Option<serde_json::Value>,
) -> Result<ToolOutcome> {
    let hold = runtime.hold();
    let mut task = match open_task(
        runtime,
        ctx,
        effects,
        &hold,
        SettleVerb {
            imperative: "fail",
            past: "failed",
            no_task: NO_ACTIVE_TASK_TO_FAIL,
        },
    )? {
        Ok(task) => task,
        Err(refusal) => return Ok(refusal),
    };
    task.transition(
        TaskStatus::Failed {
            error: error.clone(),
            structured: structured.clone(),
        },
        now(),
    )?;
    let mut commit = Commit::new()
        .event(ctx.draft(EventKind::TaskFailed {
            error: error.clone(),
            structured: structured.clone(),
        }))
        .mutate(Mutation::UpsertTask(task.clone()));
    runtime.notify_task_creator(
        &hold,
        &mut commit,
        &task,
        InboxKind::TaskFailed {
            task: task.id,
            error: error.clone(),
            structured,
        },
        ctx.base_actor.clone(),
    )?;
    runtime.apply_held(hold, commit)?;
    ctx.task = Some(task.clone());
    end_or_park(ctx, effects, Next::Failed(error));
    Ok(ToolOutcome::ok(format!(
        "Task {} marked failed. {TURN_ENDS_HERE}",
        task.id
    )))
}

fn wait_for(
    runtime: &Arc<Runtime>,
    ctx: &TurnCtx,
    effects: &mut ToolEffects,
    condition: WaitCondition,
) -> Result<ToolOutcome> {
    if let Some(error) = validate_wait(runtime, ctx, &condition, false)? {
        return Ok(error);
    }
    merge_wait(effects, condition);
    Ok(ToolOutcome::ok(format!(
        "Waiting. {TURN_ENDS_HERE} You will be woken with the results when the \
         condition is met."
    )))
}

fn request_checkpoint(runtime: &Arc<Runtime>, ctx: &TurnCtx, reason: &str) -> Result<ToolOutcome> {
    let hold = runtime.hold();
    if matches!(
        compaction_pending(runtime, ctx.agent.id)?,
        Some(CompactionTrigger::Handoff { .. })
    ) {
        return Ok(ToolOutcome::ok(format!(
            "Checkpoint already requested for the model handoff ({reason})."
        )));
    }
    runtime.apply_held(
        hold,
        Commit::new().event(ctx.draft(EventKind::CompactionRequested {
            trigger: CompactionTrigger::Explicit {
                by: ctx.base_actor.clone(),
            },
        })),
    )?;
    Ok(ToolOutcome::ok(format!(
        "Checkpoint requested ({reason}). It will be built before your next turn."
    )))
}

fn ask_user(
    runtime: &Arc<Runtime>,
    ctx: &TurnCtx,
    effects: &mut ToolEffects,
    question: String,
) -> Result<ToolOutcome> {
    // Honest only while someone waits on the answer: never recorded
    // once the turn ends the agent, and never twice in a turn.
    if let Some(asked) = &effects.asked {
        return Ok(ToolOutcome::error(format!(
            "You already asked the operator \"{}\" in this turn, and only one \
             question can be waited on. {TURN_ENDS_HERE} Ask anything further \
             after they reply.",
            one_line(asked)
        )));
    }
    if let Some(Next::Completed(_) | Next::Failed(_)) = &effects.next {
        return Ok(ToolOutcome::error(format!(
            "This turn already ended the agent, so nothing would be left waiting \
             for an answer and the operator's reply would be refused. {TURN_ENDS_HERE} \
             Put what they need to know in the summary instead, or ask before you \
             finish."
        )));
    }
    // An approval queued anywhere in this batch replaces the turn's
    // wait, and an approval wait is never interrupted by the reply.
    if !effects.pending_approvals.is_empty() {
        return Ok(ToolOutcome::error(format!(
            "Another call in this turn is waiting for the operator to approve it, \
             and that wait would swallow your question: you would not be woken by \
             their answer. {TURN_ENDS_HERE} Ask again once the approval is settled."
        )));
    }
    runtime.apply(Commit::new().event(ctx.draft(EventKind::QuestionAsked {
        question: question.clone(),
    })))?;
    effects.asked = Some(question.clone());
    // Merged, not assigned: a wait this turn already recorded (say a
    // `delegate` it is waiting on) still has to be honoured.
    merge_wait(effects, WaitCondition::Inbox);
    Ok(ToolOutcome::ok(format!(
        "Question recorded for the operator to see. {TURN_ENDS_HERE} You will be \
         woken when they reply."
    )))
}

/// Task settlement, waiting, checkpoints and operator questions.
pub(super) fn apply(
    runtime: &Arc<Runtime>,
    ctx: &mut TurnCtx,
    effects: &mut ToolEffects,
    action: HarnessAction,
) -> Result<ToolOutcome> {
    match action {
        HarnessAction::CompleteTask {
            summary,
            artifacts,
            structured,
        } => complete_task(runtime, ctx, effects, summary, artifacts, structured),
        HarnessAction::FailTask { error, structured } => {
            fail_task(runtime, ctx, effects, error, structured)
        }
        HarnessAction::WaitFor { condition } => wait_for(runtime, ctx, effects, condition),
        HarnessAction::RequestCheckpoint { reason } => request_checkpoint(runtime, ctx, &reason),
        HarnessAction::AskUser { question } => ask_user(runtime, ctx, effects, question),
        other => unreachable!("not a lifecycle action: {other:?}"),
    }
}
