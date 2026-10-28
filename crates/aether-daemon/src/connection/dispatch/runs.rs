//! Run-level reads, replay, and export/import.

use crate::Daemon;
use aether_protocol::{Command, ErrorReply, Reply};

use crate::connection::dispatch::blocking;
use crate::connection::{map_err, store_err};

pub(super) fn owns(command: &Command) -> bool {
    matches!(
        command,
        Command::ListRuns { .. }
            | Command::GetRunReport { .. }
            | Command::Replay { .. }
            | Command::ExportRun { .. }
            | Command::ImportRun { .. }
            | Command::GetTree { .. }
    )
}

pub(super) async fn dispatch(daemon: &Daemon, command: Command) -> Result<Reply, ErrorReply> {
    let rt = &daemon.runtime;
    match command {
        Command::ListRuns { limit } => {
            let store = rt.store().clone();
            let runs = blocking(move || {
                crate::projections::list_runs(store.as_ref(), limit.clamp(1, 5000))
            })
            .await?;
            Ok(Reply::Runs(runs.map_err(store_err)?))
        }
        Command::GetRunReport { run } => {
            let store = rt.store().clone();
            let report =
                blocking(move || crate::projections::build_report(store.as_ref(), run)).await?;
            Ok(Reply::RunReport(Box::new(report.map_err(store_err)?)))
        }
        // The daemon reads and writes these paths, not the caller: a run can
        // be far larger than a socket frame. Reaching the socket already
        // means being able to create agents that run commands here, so this
        // is not a wider reach than the caller had.
        Command::Replay { agent } => {
            let store = rt.store().clone();
            let frames =
                blocking(move || crate::projections::replay(store.as_ref(), agent)).await?;
            Ok(Reply::Replay(frames.map_err(store_err)?))
        }
        // Off the async worker: a transfer holds the store lock for as long
        // as the run is large, and half a gigabyte of import would otherwise
        // stall every other client's command behind it.
        Command::ExportRun { run, path } => {
            let store = rt.store().clone();
            let redactor = rt.config().redactor.clone();
            let exported = blocking(move || {
                aether_runtime::transfer::export_run(store.as_ref(), run, &path, &redactor)
            })
            .await?;
            Ok(Reply::RunExported(exported.map_err(map_err)?))
        }
        Command::ImportRun { path } => {
            let rt = rt.clone();
            let imported =
                blocking(move || aether_runtime::transfer::import_run(&rt, &path)).await?;
            Ok(Reply::RunImported(imported.map_err(map_err)?))
        }
        Command::GetTree {
            run,
            include_archived,
        } => {
            let store = rt.store().clone();
            let tree = blocking(move || {
                crate::projections::build_tree(store.as_ref(), run, include_archived)
            })
            .await?;
            Ok(Reply::Tree(tree.map_err(store_err)?))
        }
        _ => unreachable!("routed to the wrong command family"),
    }
}
