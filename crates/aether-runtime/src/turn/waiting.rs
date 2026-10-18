use super::*;

pub(crate) fn has_direct_user_message(items: &[aether_core::inbox::InboxItem]) -> bool {
    items.iter().any(|item| {
        matches!(
            &item.kind,
            InboxKind::Message {
                from: Actor::User { .. } | Actor::Client { .. },
                ..
            }
        )
    })
}

/// Validate a wait without changing turn state. Native jobs are a wake source
/// only while their owning engine session remains alive.
pub(crate) fn validate_wait(
    runtime: &Runtime,
    ctx: &TurnCtx,
    condition: &WaitCondition,
    native_jobs: bool,
) -> Result<Option<ToolOutcome>> {
    if ctx.user_reply_pending {
        return Ok(Some(ToolOutcome::error(
            "The user sent a message. Reply in visible assistant text before waiting again.",
        )));
    }
    if !native_jobs
        && *condition == WaitCondition::Inbox
        && bare_wait_has_no_wake_source(runtime, &ctx.agent)?
    {
        return Ok(Some(ToolOutcome::error(
            "Nothing can wake you: no children, pending tasks, timers or unread inbox items, \
             and your task is still assigned. Finish it (`complete_task`), give it up with \
             the reason (`fail_task`), delegate the remaining work, or wait for something \
             specific (`message_from`, `tasks`, `timeout_seconds`/`wake_at`).",
        )));
    }
    for id in condition.task_ids() {
        let found = runtime.store.task(id)?;
        let in_scope = match &found {
            Some(task) => task_in_caller_scope(runtime, ctx.agent.id, task)?,
            None => false,
        };
        if !in_scope {
            let mut tasks = runtime.store.tasks(&TaskFilter {
                run: Some(ctx.agent.run),
                ..Default::default()
            })?;
            tasks.retain(|task| {
                task.owner == Some(ctx.agent.id)
                    || task.creator == Actor::Agent { id: ctx.agent.id }
            });
            tasks.sort_by_key(|task| std::cmp::Reverse(task.created_at));
            let recent: Vec<_> = tasks
                .iter()
                .take(32)
                .map(|task| task.id.to_string())
                .collect();
            let reason = if found.is_some() {
                "belongs to a task tree you may not wait on"
            } else {
                "is unknown"
            };
            return Ok(Some(ToolOutcome::error(format!(
                "wait_for.tasks contains task `{id}` you may not wait on: it {reason}. Copy delegate.task_id or delegate.wait_for_args exactly; never change an agent ID's prefix. Recent task IDs created by or assigned to you ({} of {}): {}",
                recent.len(),
                tasks.len(),
                serde_json::to_string(&recent)
                    .map_err(|error| RuntimeError::Internal(error.to_string()))?
            ))));
        }
    }
    Ok(None)
}

/// Why an asking turn may not also end the task: a question nobody waits on
/// is one the operator cannot answer. `verb`/`done`, e.g. "fail"/"failed".
pub(super) fn still_asking(verb: &str, done: &str, question: &str) -> String {
    format!(
        "You asked the operator \"{}\" in this turn, so the task cannot be {done} yet - \
         a question nobody is waiting on can never be answered. {TURN_ENDS_HERE} You will \
         be woken with their reply; {verb} the task then.",
        one_line(question)
    )
}

/// The agent's own question quoted back into a tool reply: line breaks
/// flattened and length bounded, so it cannot bury the instruction after it.
pub(super) fn one_line(question: &str) -> String {
    const MAX: usize = 120;
    let flat: String = question
        .chars()
        .take(MAX)
        .map(|c| if breaks_a_line(c) { ' ' } else { c })
        .collect();
    let flat = flat.trim();
    if question.chars().nth(MAX).is_some() {
        format!("{flat}…")
    } else {
        flat.to_owned()
    }
}

/// Control characters plus the separators a terminal or client still renders
/// as a break (U+2028 line, U+2029 paragraph).
pub(super) fn breaks_a_line(c: char) -> bool {
    c.is_control() || c == '\u{2028}' || c == '\u{2029}'
}

/// What every parking action tells the engine, so none of them reads as an
/// ordinary result it can follow up on.
pub(crate) const TURN_ENDS_HERE: &str =
    "Your turn ends here - stop now and make no further tool calls.";

/// Informational, not an error: a root or between-task agent has nothing to
/// complete. It must read differently from a real failure.
pub(crate) const NO_ACTIVE_TASK_TO_COMPLETE: &str = "You have no active task to complete right now. There's nothing to mark done: answer the \
caller directly, or reach them with ask_user (a human) or send_message \
(another agent).";

pub(crate) const NO_ACTIVE_TASK_TO_FAIL: &str = "You have no active task to fail right now. There's nothing to mark failed: answer \
the caller directly, or reach them with ask_user (a human) or send_message \
(another agent).";

pub(super) fn bare_wait_has_no_wake_source(runtime: &Runtime, agent: &Agent) -> Result<bool> {
    if agent.config.long_lived {
        return Ok(false);
    }
    let Some(task_id) = agent.current_task else {
        return Ok(false);
    };
    let assigned = runtime.store.task(task_id)?.is_some_and(|task| {
        matches!(
            task.status.kind(),
            TaskStatusKind::Assigned | TaskStatusKind::Active
        )
    });
    if !assigned || crate::scheduler::has_unread_inbox(runtime, agent)? {
        return Ok(false);
    }

    let subtree = runtime.store.subtree_ids(agent.id)?;
    let live = runtime.store.agents(&AgentFilter {
        live_only: true,
        ..Default::default()
    })?;
    if live
        .iter()
        .any(|other| other.id != agent.id && subtree.contains(&other.id))
    {
        return Ok(false);
    }
    let pending_tasks = runtime.store.tasks(&TaskFilter {
        statuses: vec![
            TaskStatusKind::Queued,
            TaskStatusKind::Assigned,
            TaskStatusKind::Active,
            TaskStatusKind::Blocked,
        ],
        ..Default::default()
    })?;
    if pending_tasks
        .iter()
        .any(|task| task.id != task_id && task.creator == Actor::agent(agent.id))
    {
        return Ok(false);
    }
    Ok(!runtime
        .store
        .schedules(Some(agent.id))?
        .iter()
        .any(|schedule| schedule.status.next_due().is_some()))
}

/// A task is in `caller`'s scope for `wait_for` when it is bound (by owner
/// or creator) to `caller`'s own subtree, or when it is bound to nobody -
/// an ownerless task created directly by a user/system actor, open for
/// whoever gets there first.
pub(super) fn task_in_caller_scope(
    runtime: &Runtime,
    caller: AgentId,
    task: &Task,
) -> Result<bool> {
    let owner = task.owner;
    let creator = task.creator.agent_id();
    if owner.is_none() && creator.is_none() {
        return Ok(true);
    }
    for id in [owner, creator].into_iter().flatten() {
        if runtime.store.is_descendant(caller, id)? {
            return Ok(true);
        }
    }
    Ok(false)
}

/// The first `complete_task.artifacts` id, if any, that was not produced by
/// `caller` or one of its descendants.
pub(super) fn first_artifact_outside_scope(
    runtime: &Runtime,
    caller: AgentId,
    artifacts: &[ArtifactId],
) -> Result<Option<ArtifactId>> {
    if artifacts.is_empty() {
        return Ok(None);
    }
    let subtree = runtime.store.subtree_ids(caller)?;
    for id in artifacts {
        let owned = runtime
            .store
            .artifact(*id)?
            .is_some_and(|a| a.agent_id.is_some_and(|owner| subtree.contains(&owner)));
        if !owned {
            return Ok(Some(*id));
        }
    }
    Ok(None)
}

pub(super) fn merge_wait(effects: &mut ToolEffects, condition: WaitCondition) {
    effects.next = Some(match effects.next.take() {
        // Same condition twice (e.g. two long_lived settles after one
        // ask_user): keep it singular, or a bare Inbox loses its exemption
        // from with_deadline's wait_timeout and parks for 30 minutes, not
        // indefinitely.
        Some(Next::Waiting(existing)) if existing == condition => Next::Waiting(existing),
        Some(Next::Waiting(existing)) => Next::Waiting(WaitCondition::All {
            of: vec![existing, condition],
        }),
        Some(other) => other,
        None => Next::Waiting(condition),
    });
}
