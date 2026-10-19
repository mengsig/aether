//! Commands that steer a running agent: messages, notes, and lifecycle
//! controls.

use crate::Daemon;
use aether_core::event::Actor;
use aether_core::idempotency::Submission;
use aether_core::model::ContentBlock;
use aether_protocol::{Command, ErrorReply, Reply};

use crate::connection::{map_err, messages};

pub(super) fn owns(command: &Command) -> bool {
    matches!(
        command,
        Command::ReplyToQuestion { .. }
            | Command::SendMessage { .. }
            | Command::AppendNote { .. }
            | Command::Cancel { .. }
            | Command::Interrupt { .. }
            | Command::Trigger { .. }
            | Command::Restart { .. }
            | Command::Wake { .. }
            | Command::Pause { .. }
            | Command::Resume { .. }
            | Command::SetApprovals { .. }
            | Command::Compact { .. }
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
        Command::ReplyToQuestion {
            to,
            question_seq,
            content,
            ..
        } => reply_to_question(daemon, to, question_seq, content, by, submission),
        command @ Command::SendMessage { .. } => {
            messages::dispatch(daemon, command, by, submission).await
        }
        Command::AppendNote {
            agent,
            text,
            links,
            context,
            level,
        } => {
            let note = aether_core::note::Note {
                text,
                links,
                context,
                level,
            };
            append_note(daemon, agent, &note, by)
        }
        Command::Cancel { agent, reason, .. } => cancel(daemon, agent, reason, by, submission),
        Command::Interrupt { agent, .. } => interrupt(daemon, agent, by, submission),
        Command::Trigger {
            agent,
            source,
            payload,
            ..
        } => trigger(daemon, agent, source, payload, by, submission),
        Command::Restart { agent, .. } => Ok(Reply::Agent(
            rt.restart_agent_keyed(agent, by, submission)
                .map_err(map_err)?,
        )),
        Command::Wake { agent } => Ok(Reply::Agent(rt.wake_agent(agent, by).map_err(map_err)?)),
        Command::Pause { agent, subtree, .. } => pause(daemon, agent, subtree, by, submission),
        Command::Resume { agent, subtree, .. } => resume(daemon, agent, subtree, by, submission),
        Command::SetApprovals {
            agent,
            policy,
            subtree,
            ..
        } => Ok(Reply::Agents(
            rt.set_approvals_keyed(agent, policy, subtree, by, submission)
                .map_err(map_err)?,
        )),
        Command::Compact { agent } => compact(daemon, agent, by),
        _ => unreachable!("routed to the wrong command family"),
    }
}

fn reply_to_question(
    daemon: &Daemon,
    to: aether_core::id::AgentId,
    question_seq: u64,
    content: String,
    by: Actor,
    submission: Option<Submission>,
) -> Result<Reply, ErrorReply> {
    let message = daemon
        .runtime
        .reply_to_question(
            to,
            question_seq,
            vec![ContentBlock::text(content)],
            by,
            submission,
        )
        .map_err(map_err)?;
    Ok(Reply::MessageSent { message })
}

fn append_note(
    daemon: &Daemon,
    agent: aether_core::id::AgentId,
    note: &aether_core::note::Note,
    by: Actor,
) -> Result<Reply, ErrorReply> {
    let seq = daemon
        .runtime
        .append_note(agent, note, by)
        .map_err(map_err)?;
    Ok(Reply::NoteAppended { seq })
}

fn pause(
    daemon: &Daemon,
    agent: aether_core::id::AgentId,
    subtree: bool,
    by: Actor,
    submission: Option<Submission>,
) -> Result<Reply, ErrorReply> {
    let outcome = daemon
        .runtime
        .pause_agent_keyed(agent, subtree, by, submission)
        .map_err(map_err)?;
    Ok(Reply::Paused {
        agents: outcome.changed,
        mid_turn: outcome.mid_turn,
    })
}

fn cancel(
    daemon: &Daemon,
    agent: aether_core::id::AgentId,
    reason: Option<String>,
    by: Actor,
    submission: Option<Submission>,
) -> Result<Reply, ErrorReply> {
    daemon
        .runtime
        .cancel_agent_keyed(agent, reason, by, submission)
        .map_err(map_err)?;
    Ok(Reply::Ok)
}

fn interrupt(
    daemon: &Daemon,
    agent: aether_core::id::AgentId,
    by: Actor,
    submission: Option<Submission>,
) -> Result<Reply, ErrorReply> {
    let was_running = daemon
        .runtime
        .interrupt_agent_keyed(agent, by, submission)
        .map_err(map_err)?;
    Ok(Reply::Interrupted { was_running })
}

fn trigger(
    daemon: &Daemon,
    agent: aether_core::id::AgentId,
    source: String,
    payload: serde_json::Value,
    by: Actor,
    submission: Option<Submission>,
) -> Result<Reply, ErrorReply> {
    daemon
        .runtime
        .trigger_agent_keyed(agent, source, payload, by, submission)
        .map_err(map_err)?;
    Ok(Reply::Ok)
}

fn resume(
    daemon: &Daemon,
    agent: aether_core::id::AgentId,
    subtree: bool,
    by: Actor,
    submission: Option<Submission>,
) -> Result<Reply, ErrorReply> {
    let agents = daemon
        .runtime
        .resume_agent_keyed(agent, subtree, by, submission)
        .map_err(map_err)?;
    Ok(Reply::Agents(agents))
}

fn compact(
    daemon: &Daemon,
    agent: aether_core::id::AgentId,
    by: Actor,
) -> Result<Reply, ErrorReply> {
    daemon
        .runtime
        .request_compaction(agent, by)
        .map_err(map_err)?;
    Ok(Reply::Ok)
}
