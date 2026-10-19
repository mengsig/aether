//! Memories, their feedback, and repository aliases.

use crate::Daemon;
use aether_core::event::Actor;
use aether_core::idempotency::Submission;
use aether_protocol::{Command, ErrorReply, Reply};

use crate::connection::dispatch::blocking;
use crate::connection::map_err;

pub(super) fn owns(command: &Command) -> bool {
    matches!(
        command,
        Command::Memories { .. }
            | Command::CreateMemory { .. }
            | Command::AcceptMemory { .. }
            | Command::RejectMemory { .. }
            | Command::RecordMemoryFeedback { .. }
            | Command::RetractMemoryFeedback { .. }
            | Command::MemoryFeedback { .. }
            | Command::ForgetMemory { .. }
            | Command::RestoreMemory { .. }
            | Command::PinMemory { .. }
            | Command::SyncRepositoryKnowledge { .. }
            | Command::AliasRepository { .. }
            | Command::ListRepositoryAliases
            | Command::MemoriesInContext { .. }
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
        Command::Memories { query } => {
            Ok(Reply::Memories(rt.memories(query.into()).map_err(map_err)?))
        }
        Command::CreateMemory { spec, .. } => {
            let rt = rt.clone();
            let result = blocking(move || rt.create_memory_keyed(spec, by, submission)).await?;
            Ok(Reply::Memory(result.map_err(map_err)?))
        }
        Command::AcceptMemory { id, reason } => Ok(Reply::Memory(
            rt.accept_memory(id, reason, by).map_err(map_err)?,
        )),
        Command::RejectMemory { id, reason } => Ok(Reply::Memory(
            rt.reject_memory(id, reason, by).map_err(map_err)?,
        )),
        Command::ForgetMemory { id, reason } => Ok(Reply::Memory(
            rt.forget_memory(id, reason, by).map_err(map_err)?,
        )),
        Command::RestoreMemory { id } => {
            Ok(Reply::Memory(rt.restore_memory(id, by).map_err(map_err)?))
        }
        Command::PinMemory { id, pinned } => Ok(Reply::Memory(
            rt.pin_memory(id, pinned, by).map_err(map_err)?,
        )),
        Command::MemoriesInContext { task, invocation } => Ok(Reply::MemoriesInContext(
            rt.memories_in_context(task, invocation, &by)
                .map_err(map_err)?,
        )),
        command @ (Command::RecordMemoryFeedback { .. }
        | Command::RetractMemoryFeedback { .. }
        | Command::MemoryFeedback { .. }) => feedback(daemon, command, by, submission).await,
        command @ (Command::SyncRepositoryKnowledge { .. }
        | Command::AliasRepository { .. }
        | Command::ListRepositoryAliases) => repositories(daemon, command, by).await,
        _ => unreachable!("routed to the wrong command family"),
    }
}

async fn feedback(
    daemon: &Daemon,
    command: Command,
    by: Actor,
    submission: Option<Submission>,
) -> Result<Reply, ErrorReply> {
    let rt = daemon.runtime.clone();
    match command {
        Command::RecordMemoryFeedback {
            memory,
            verdict,
            evidence,
            task,
            invocation,
            reason,
            ..
        } => {
            let spec = aether_core::memory_feedback::MemoryFeedbackSpec {
                memory,
                verdict,
                evidence,
                task,
                invocation,
                reason,
            };
            let result =
                blocking(move || rt.record_memory_feedback_keyed(spec, by, submission)).await?;
            Ok(Reply::MemoryFeedback(result.map_err(map_err)?))
        }
        Command::RetractMemoryFeedback { feedback, reason } => {
            let result = blocking(move || rt.retract_memory_feedback(feedback, reason, by)).await?;
            Ok(Reply::MemoryFeedback(result.map_err(map_err)?))
        }
        Command::MemoryFeedback { memory, limit } => {
            let result = blocking(move || rt.memory_feedback(memory, limit, &by)).await?;
            Ok(Reply::MemoryFeedbackList(result.map_err(map_err)?))
        }
        _ => unreachable!("routed to the wrong command family"),
    }
}

async fn repositories(daemon: &Daemon, command: Command, by: Actor) -> Result<Reply, ErrorReply> {
    let rt = daemon.runtime.clone();
    match command {
        // Off the async worker: this scans the tree with a git subprocess
        // per note, then sweeps with two more per file - the create path
        // treats the identical work as blocking for the same reason.
        Command::SyncRepositoryKnowledge { root } => {
            let synced = blocking(move || rt.sync_repository_knowledge(root, by)).await?;
            let (repository, report) = synced.map_err(map_err)?;
            Ok(Reply::MemorySeeded { repository, report })
        }
        // Off the async worker: a root runs git, and the move rewrites every
        // memory filed under the alias in one transaction.
        Command::AliasRepository {
            alias,
            root,
            repository,
            dry_run,
        } => {
            let aliased =
                blocking(move || rt.alias_repository(&alias, root, repository, dry_run, by))
                    .await?;
            Ok(Reply::RepositoryAliased(aliased.map_err(map_err)?))
        }
        Command::ListRepositoryAliases => Ok(Reply::RepositoryAliases(
            rt.repository_aliases().map_err(map_err)?,
        )),
        _ => unreachable!("routed to the wrong command family"),
    }
}
