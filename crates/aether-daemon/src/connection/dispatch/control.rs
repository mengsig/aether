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
        } => {
            let message = rt
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
            let seq = rt.append_note(agent, &note, by).map_err(map_err)?;
            Ok(Reply::NoteAppended { seq })
        }
        Command::Cancel { agent, reason, .. } => {
            rt.cancel_agent_keyed(agent, reason, by, submission)
                .map_err(map_err)?;
            Ok(Reply::Ok)
        }
        Command::Interrupt { agent, .. } => {
            let was_running = rt
                .interrupt_agent_keyed(agent, by, submission)
                .map_err(map_err)?;
            Ok(Reply::Interrupted { was_running })
        }
        Command::Trigger {
            agent,
            source,
            payload,
            ..
        } => {
            rt.trigger_agent_keyed(agent, source, payload, by, submission)
                .map_err(map_err)?;
            Ok(Reply::Ok)
        }
        Command::Restart { agent, .. } => Ok(Reply::Agent(
            rt.restart_agent_keyed(agent, by, submission)
                .map_err(map_err)?,
        )),
        Command::Wake { agent } => Ok(Reply::Agent(rt.wake_agent(agent, by).map_err(map_err)?)),
        Command::Pause { agent, subtree, .. } => {
            let outcome = rt
                .pause_agent_keyed(agent, subtree, by, submission)
                .map_err(map_err)?;
            Ok(Reply::Paused {
                agents: outcome.changed,
                mid_turn: outcome.mid_turn,
            })
        }
        Command::Resume { agent, subtree, .. } => {
            let agents = rt
                .resume_agent_keyed(agent, subtree, by, submission)
                .map_err(map_err)?;
            Ok(Reply::Agents(agents))
        }
        Command::SetApprovals {
            agent,
            policy,
            subtree,
            ..
        } => Ok(Reply::Agents(
            rt.set_approvals_keyed(agent, policy, subtree, by, submission)
                .map_err(map_err)?,
        )),
        Command::Compact { agent } => {
            rt.request_compaction(agent, by).map_err(map_err)?;
            Ok(Reply::Ok)
        }
        _ => unreachable!("routed to the wrong command family"),
    }
}
