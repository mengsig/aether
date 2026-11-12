//! Memory 2.0's commands (docs/memory/MEMORY-V2.md), and the standing every
//! memory reply carries; the writer withholds it from a client without
//! `memory_v2`.

use crate::Daemon;
use aether_core::event::Actor;
use aether_protocol::{Command, ErrorReply, Reply};

use crate::connection::dispatch::blocking;
use crate::connection::map_err;

pub(super) fn owns(command: &Command) -> bool {
    matches!(
        command,
        Command::MemoryRetrievalExplain { .. }
            | Command::ConsolidateMemories { .. }
            | Command::MemoryExport { .. }
            | Command::MemoryImport { .. }
            | Command::MemoryMigrationStatus
    )
}

/// Every command here reads or rewrites many rows, so each runs off the
/// async worker.
pub(super) async fn dispatch(
    daemon: &Daemon,
    command: Command,
    by: Actor,
) -> Result<Reply, ErrorReply> {
    let rt = daemon.runtime.clone();
    match command {
        Command::MemoryRetrievalExplain { task, invocation } => {
            let explained =
                blocking(move || rt.memory_retrieval_explain(task, invocation, &by)).await?;
            Ok(Reply::MemoryRetrievalExplain(explained.map_err(map_err)?))
        }
        Command::ConsolidateMemories { scope, dry_run } => {
            let report =
                blocking(move || rt.consolidate_memories_report(scope, dry_run, by)).await?;
            Ok(Reply::MemoryConsolidation(report.map_err(map_err)?))
        }
        Command::MemoryExport { scope } => {
            let export = blocking(move || rt.export_memories(scope, &by)).await?;
            Ok(Reply::MemoryExport(export.map_err(map_err)?))
        }
        Command::MemoryImport { artifact, dry_run } => {
            let import = blocking(move || rt.import_memories(&artifact, dry_run, by)).await?;
            Ok(Reply::MemoryImport(import.map_err(map_err)?))
        }
        Command::MemoryMigrationStatus => {
            let status = blocking(move || rt.memory_migration_status()).await?;
            Ok(Reply::MemoryMigrationStatus(status.map_err(map_err)?))
        }
        _ => unreachable!("routed to the wrong command family"),
    }
}

/// `reply` with each record's standing filled; any other reply as it was.
pub(super) fn with_standing(daemon: &Daemon, mut reply: Reply) -> Result<Reply, ErrorReply> {
    let records = match &mut reply {
        Reply::Memories(records) => records.as_mut_slice(),
        Reply::Memory(record) => std::slice::from_mut(record),
        _ => return Ok(reply),
    };
    daemon.runtime.with_standing(records).map_err(map_err)?;
    Ok(reply)
}
