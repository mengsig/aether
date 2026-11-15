use crate::wait::{RuntimeOracle, settlement_in_flight};
use crate::{Result, Runtime};
use aether_core::{
    agent::{Agent, AgentStatus, BlockReason},
    event::{Actor, EventDraft, EventKind},
    id::AgentId,
    time::now,
};
use aether_core::{
    agent::{WaitCondition, WaitOracle},
    inbox::InboxKind,
    model::MessageRole,
    task::TaskStatusKind,
};
use aether_store::{Commit, EventFilter, Mutation};

/// Whether an unconsumed person message should lift this block right now
/// (VISION §3.6 limit 1). A dependency gate and an orphaned lease wait yield;
/// an inspection needs a person to look first, and a live turn owns its own
/// lease wait.
pub(super) fn person_reaches(
    runtime: &Runtime,
    agent: &Agent,
    reason: &BlockReason,
) -> Result<bool> {
    let ours = match reason {
        BlockReason::Dependency { .. } => true,
        BlockReason::Resource { .. } => !runtime.sched.lock().running.contains_key(&agent.id),
        BlockReason::Inspection { .. } => false,
        // A person reaching a suspended agent probes now instead of waiting
        // out the schedule; the turn re-dispatches and may exhaust again.
        BlockReason::Exhausted { .. } => true,
    };
    if !ours {
        return Ok(false);
    }
    person_message_pending(runtime, agent)
}

pub(super) fn person_message_pending(runtime: &Runtime, agent: &Agent) -> Result<bool> {
    let items = runtime.store.inbox(agent.id, agent.inbox_cursor)?;
    Ok(items.iter().any(|item| {
        matches!(
            &item.kind,
            InboxKind::Message {
                from: Actor::User { .. },
                ..
            }
        )
    }))
}

pub(super) fn wake_reason(condition: &WaitCondition, oracle: &RuntimeOracle<'_>) -> String {
    match condition {
        WaitCondition::Inbox => "inbox item".into(),
        WaitCondition::Task { .. } | WaitCondition::All { .. } | WaitCondition::AtLeast { .. } => {
            "awaited work finished".into()
        }
        // Name the branch that fired: a deadline reads very differently
        // from finished work.
        WaitCondition::Any { of } => of
            .iter()
            .find(|c| c.is_satisfied(oracle))
            .map(|c| wake_reason(c, oracle))
            .unwrap_or_else(|| "awaited work finished".into()),
        WaitCondition::Approval { .. } => "approval resolved".into(),
        WaitCondition::MessageFrom { .. } => "message arrived".into(),
        WaitCondition::Deadline { .. } => "deadline reached".into(),
    }
}

/// Inbox items that should reach a waiting agent now: words from a person or
/// from this agent's own supervisor, a question from a descendant that waits
/// on this agent's answer, or a child that failed or was cancelled and will
/// never report success. Any other agent's message - a child's report, a
/// sibling, an unrelated agent - waits until the condition is satisfied.
pub(crate) fn urgent_inbox(runtime: &Runtime, agent: &Agent) -> Result<Option<String>> {
    let items = runtime.store.inbox(agent.id, agent.inbox_cursor)?;
    for item in items
        .iter()
        .filter(|item| !settlement_in_flight(runtime, &item.kind))
    {
        if let Some(reason) = urgent_item(runtime, agent, &item.kind)? {
            return Ok(Some(reason));
        }
    }
    Ok(None)
}

fn urgent_item(runtime: &Runtime, agent: &Agent, kind: &InboxKind) -> Result<Option<String>> {
    let reason = match kind {
        InboxKind::Message {
            from: Actor::User { .. },
            ..
        } => "message from the user".to_owned(),
        // A supervisor steering a subtree speaks with the same urgency as a
        // person: it must be heard before its child's fan-out finishes.
        InboxKind::Message {
            from: Actor::Agent { id },
            ..
        } if agent.parent == Some(*id) => "message from your parent".to_owned(),
        InboxKind::Message {
            from: Actor::Agent { id },
            ..
        } => {
            if !awaits_reply(runtime, agent.id, *id)? {
                return Ok(None);
            }
            format!("question from {id}, which is waiting for your answer")
        }
        InboxKind::TaskFailed { .. } | InboxKind::ChildFailed { .. } => {
            "a delegated task failed".to_owned()
        }
        InboxKind::ChildSuspended { .. } => {
            "a delegated agent is suspended on provider exhaustion".to_owned()
        }
        InboxKind::TaskCancelled { .. } => "a delegated task was cancelled".to_owned(),
        InboxKind::ExternalTrigger { source, .. } => format!("external trigger from {source}"),
        _ => return Ok(None),
    };
    Ok(Some(reason))
}

/// Whether `sender`, a descendant of `recipient`, is parked waiting for a
/// message from it. Holding its question until `recipient`'s own wait ends
/// would deadlock both when that wait is on the sender's task.
fn awaits_reply(runtime: &Runtime, recipient: AgentId, sender: AgentId) -> Result<bool> {
    if sender == recipient {
        return Ok(false);
    }
    let Some(asker) = runtime.store.agent(sender)? else {
        return Ok(false);
    };
    let AgentStatus::Waiting { condition } = &asker.status else {
        return Ok(false);
    };
    if !condition.message_senders().contains(&recipient) {
        return Ok(false);
    }
    Ok(runtime.store.is_descendant(recipient, sender)?)
}

/// When only a deadline fired, tell the agent what is still open so it can
/// choose: keep waiting, cancel, or proceed without.
fn deadline_note(condition: &WaitCondition, oracle: &RuntimeOracle<'_>) -> Option<String> {
    let WaitCondition::Any { of } = condition else {
        return None;
    };
    let by_deadline = of
        .iter()
        .any(|c| matches!(c, WaitCondition::Deadline { .. }) && c.is_satisfied(oracle));
    let work_done = of
        .iter()
        .any(|c| !matches!(c, WaitCondition::Deadline { .. }) && c.is_satisfied(oracle));
    if !by_deadline || work_done {
        return None;
    }
    if condition.task_ids().is_empty() && condition.message_senders().is_empty() {
        return Some("Your requested wake time has arrived. Continue the planned follow-up, or wait for new work if nothing remains.".into());
    }
    let mut open: Vec<String> = condition
        .task_ids()
        .into_iter()
        .filter(|t| !oracle.task_is_terminal(*t))
        .map(|t| format!("task {t} still has not finished"))
        .collect();
    open.extend(
        condition
            .message_senders()
            .into_iter()
            .filter(|a| !oracle.has_unconsumed_message_from(*a))
            .map(|a| format!("a message from {a} still has not arrived")),
    );
    let open = if open.is_empty() {
        "the work it is waiting for still has not happened".to_owned()
    } else {
        open.join(", ")
    };
    Some(format!(
        "The wait timed out and {open}. Decide: keep waiting (wait_for), cancel or replace it, \
         or proceed without it."
    ))
}

pub(super) fn satisfied_wait_note(
    runtime: &Runtime,
    agent: &Agent,
    condition: &WaitCondition,
    oracle: &RuntimeOracle<'_>,
) -> Result<Option<String>> {
    if let Some(note) = deadline_note(condition, oracle) {
        return Ok(Some(note));
    }
    if has_work(runtime, agent)? {
        return Ok(None);
    }
    // A task reports to its creator, which need not be this waiter (for
    // example, an ancestor waiting on a grandchild). Ready alone would be
    // parked by has_work; persist an input with the observed task states. A wait is not a grant
    // to read another task's result or failure details.
    let mut outcomes = Vec::new();
    for id in condition.task_ids() {
        if let Some(task) = runtime.store.task(id)? {
            outcomes.push(serde_json::json!({"task": id, "state": task.status.kind()}));
        }
    }
    Ok(Some(format!(
        "Your requested wait condition is satisfied. Continue the assigned work. \
         Awaited task states: {}",
        serde_json::json!(outcomes)
    )))
}

/// Move an agent the tick decided to wake to Ready. `None` when the row moved
/// under the pass - it is re-read and written under one hold on the write
/// path, so a durable flag set in between (a pause) is never written back over
/// by the snapshot the tick was reading.
pub(super) fn wake(
    runtime: &Runtime,
    agent: Agent,
    reason: String,
    interrupted: Option<WaitCondition>,
    note: Option<String>,
) -> Result<Option<Agent>> {
    let hold = runtime.hold();
    let Some(mut agent) = runtime
        .store
        .agent(agent.id)?
        .filter(|a| a.status == agent.status && !a.paused)
    else {
        return Ok(None);
    };
    agent.transition(AgentStatus::Ready, now())?;
    // Only ever set, never cleared: a caller with nothing to record must not
    // discard a wait the agent chose and is still holding.
    if interrupted.is_some() {
        agent.resume_wait = interrupted;
    }
    let mut commit = Commit::new().event(
        EventDraft::new(
            Actor::Scheduler,
            EventKind::AgentResumed { reason, via: None },
        )
        .agent(agent.id)
        .run(agent.run),
    );
    if let Some(reason) = note {
        runtime.push_inbox(
            &hold,
            &mut commit,
            &agent,
            InboxKind::Wake { reason },
            Actor::Scheduler,
        )?;
    }
    commit.push_mutation(Mutation::UpsertAgent(agent.clone()));
    runtime.apply_held(hold, commit)?;
    Ok(Some(agent))
}

/// Park a ready agent with nothing to act on. Re-read under the same hold as
/// `wake`, and for the same reason.
pub(super) fn park(runtime: &Runtime, agent: Agent) -> Result<()> {
    let condition = WaitCondition::Inbox;
    let hold = runtime.hold();
    let Some(mut agent) = runtime
        .store
        .agent(agent.id)?
        .filter(|a| a.status == AgentStatus::Ready)
    else {
        return Ok(());
    };
    agent.transition(
        AgentStatus::Waiting {
            condition: condition.clone(),
        },
        now(),
    )?;
    let commit = Commit::new()
        .event(
            EventDraft::new(Actor::Scheduler, EventKind::AgentWaiting { condition })
                .agent(agent.id)
                .run(agent.run),
        )
        .mutate(Mutation::UpsertAgent(agent));
    runtime.apply_held(hold, commit)?;
    Ok(())
}

/// Whether a ready agent has something a model turn can act on.
pub(crate) fn has_work(runtime: &Runtime, agent: &Agent) -> Result<bool> {
    if has_unread_inbox(runtime, agent)? {
        return Ok(true);
    }
    has_transcript_work(runtime, agent)
}

pub(crate) fn has_transcript_work(runtime: &Runtime, agent: &Agent) -> Result<bool> {
    let last = runtime.store.latest_events(
        1,
        &EventFilter {
            types: vec!["agent.message".into()],
            ..EventFilter::own_events(agent.id)
        },
    )?;
    match last.first().map(|e| &e.kind) {
        // A user turn (tool results, nudge) is waiting for a reply.
        Some(EventKind::AgentMessage {
            role: MessageRole::User,
            ..
        }) => Ok(true),
        Some(EventKind::AgentMessage {
            role: MessageRole::Assistant,
            ..
        }) => Ok(false),
        // No transcript yet: only a fresh task justifies a first turn.
        _ => Ok(match agent.current_task {
            Some(task) => runtime.store.task(task)?.is_some_and(|t| {
                matches!(
                    t.status.kind(),
                    TaskStatusKind::Assigned | TaskStatusKind::Active
                )
            }),
            None => false,
        }),
    }
}

pub(crate) fn has_unread_inbox(runtime: &Runtime, agent: &Agent) -> Result<bool> {
    Ok(runtime.store.inbox_last_seq(agent.id)? > agent.inbox_cursor)
}
