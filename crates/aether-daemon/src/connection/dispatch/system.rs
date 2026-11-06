//! Daemon-wide commands: liveness, storage, status, MCP, providers, and the
//! host, remote and push services.

use crate::Daemon;
use crate::authz::AuthContext;
use aether_core::event::Actor;
use aether_protocol::{Command, DaemonStatus, ErrorCode, ErrorReply, Reply, StatusStale};
use std::sync::Arc;

use crate::connection::dispatch::blocking;
use crate::connection::{host_services, map_err, providers, push, remote_admin, store_err};
use crate::status_reads::{Answer, Figures, PATIENCE};

pub(super) fn owns(command: &Command) -> bool {
    matches!(
        command,
        Command::Hello(_)
            | Command::Ping
            | Command::StorageStatus
            | Command::StorageGc { .. }
            | Command::PreviewRetention
            | Command::Status
            | Command::Shutdown
            | Command::Subscribe { .. }
            | Command::Unsubscribe { .. }
            | Command::ResolveSubmission { .. }
            | Command::McpServers { .. }
            | Command::GetGithubWatcher
            | Command::ConfigureGithubWatcher { .. }
            | Command::ListModels
            | Command::AuthStatus
            | Command::Providers
            | Command::RefreshProviderUsage { .. }
            | Command::GetAttention
            | Command::CompletePath { .. }
            | Command::ListWorkspaces { .. }
            | Command::ValidateWorkspace { .. }
            | Command::GetGitSummary { .. }
            | Command::GetRemote
            | Command::ConfigureRemote { .. }
            | Command::StartPairing { .. }
            | Command::GetPairing { .. }
            | Command::ConfirmPairing { .. }
            | Command::CancelPairing { .. }
            | Command::ListDevices { .. }
            | Command::RevokeDevice { .. }
            | Command::UpdateDevice { .. }
            | Command::GetPushConfig
            | Command::SubscribePush { .. }
            | Command::UnsubscribePush
            | Command::TestPush
    )
}

pub(super) async fn dispatch(
    daemon: &Daemon,
    command: Command,
    by: Actor,
    auth: &AuthContext,
) -> Result<Reply, ErrorReply> {
    let rt = &daemon.runtime;
    match command {
        Command::Hello(_) => Err(ErrorReply::invalid("already greeted")),
        // Nothing but a frame in: the point is to hold a quiet connection
        // open under the idle timeout.
        Command::Ping => Ok(Reply::Ok),
        Command::StorageStatus | Command::StorageGc { .. } => storage(daemon, command).await,
        Command::PreviewRetention => preview_retention(daemon).await,
        Command::Status => status(daemon).await,
        Command::Shutdown | Command::Subscribe { .. } | Command::Unsubscribe { .. } => {
            Err(ErrorReply::internal("handled by the connection loop"))
        }
        Command::ResolveSubmission { .. } => Err(ErrorReply::internal(
            "a resolution is answered before dispatch",
        )),
        Command::McpServers { refresh, server } => mcp_servers(daemon, refresh, server).await,
        Command::GetGithubWatcher => Ok(Reply::GithubWatcher(
            rt.github_watcher_status().map_err(map_err)?,
        )),
        Command::ConfigureGithubWatcher {
            config,
            expected_revision,
        } => configure_github_watcher(daemon, config, expected_revision, by),
        Command::ListModels => providers::list_models(daemon).await,
        Command::AuthStatus => providers::auth_status(daemon).await,

        Command::Providers => providers::providers(daemon).await,
        Command::RefreshProviderUsage { provider } => {
            refresh_provider_usage(daemon, provider.as_deref()).await
        }

        command @ (Command::GetAttention
        | Command::CompletePath { .. }
        | Command::ListWorkspaces { .. }
        | Command::ValidateWorkspace { .. }
        | Command::GetGitSummary { .. }) => host_services::dispatch(daemon, command).await,
        command @ (Command::GetRemote
        | Command::ConfigureRemote { .. }
        | Command::StartPairing { .. }
        | Command::GetPairing { .. }
        | Command::ConfirmPairing { .. }
        | Command::CancelPairing { .. }
        | Command::ListDevices { .. }
        | Command::RevokeDevice { .. }
        | Command::UpdateDevice { .. }) => remote_admin::dispatch(daemon, command, by, auth).await,
        command @ (Command::GetPushConfig
        | Command::SubscribePush { .. }
        | Command::UnsubscribePush
        | Command::TestPush) => push::dispatch(daemon, command, auth).await,
        _ => unreachable!("routed to the wrong command family"),
    }
}

async fn storage(daemon: &Daemon, command: Command) -> Result<Reply, ErrorReply> {
    let (dry_run, prune_build_output) = match command {
        Command::StorageGc {
            dry_run,
            prune_build_output,
        } => (dry_run, prune_build_output),
        _ => (true, false),
    };
    let reading = daemon.runtime.clone();
    let report = blocking(move || {
        reading
            .storage_gc_with(dry_run, prune_build_output)
            .map_err(map_err)
    })
    .await??;
    Ok(Reply::StorageReport(report))
}

async fn status(daemon: &Daemon) -> Result<Reply, ErrorReply> {
    let rt = &daemon.runtime;
    let reading = rt.clone();
    // Reads every agent and takes `sched`: shared, off the async workers, and
    // waited for only `PATIENCE`, so an overloaded store cannot time it out.
    let answer = daemon
        .status_reads
        .answer(PATIENCE, move || read_figures(&reading))
        .await?;
    let (figures, stale) = resolve(answer);
    let (mut s, last_seq) = match figures {
        Some(figures) => (figures.runtime.clone(), figures.last_seq),
        None => (rt.status_settings(), 0),
    };
    if let Some(stale) = &stale {
        s.problems
            .insert(0, stale_problem(stale, aether_core::time::now()));
    }
    Ok(Reply::Status(DaemonStatus {
        problems: s.problems,
        instance_id: daemon.instance_id.clone(),
        version: daemon.version.clone(),
        pid: std::process::id(),
        started_at: rt.started_at(),
        last_seq,
        agents_live: s.agents_live,
        agents_running: s.agents_running,
        agents_waiting: s.agents_waiting,
        agents_blocked: s.agents_blocked,
        agents_suspended: s.agents_suspended,
        agents_paused: s.agents_paused,
        tasks_active: s.tasks_active,
        pending_approvals: s.pending_approvals,
        default_model: rt.default_model(),
        usage_total: s.usage_total,
        yolo: s.yolo,
        engines: s.engines,
        providers: s.providers,
        resource_leases: s.resource_leases,
        runs: s.runs,
        approval_default: s.approval_default,
        network_default: s.network_default,
        max_concurrent_turns: Some(s.max_concurrent_turns),
        max_concurrent_engine_turns: s.max_concurrent_engine_turns,
        engine_turns_running: s.engine_turns_running,
        run_budget_usd: s.run_budget_usd,
        run_hard_stop_usd: s.run_hard_stop_usd,
        run_hard_stop_tokens: s.run_hard_stop_tokens,
        mcp: s.mcp,
        low_disk: s.low_disk,
        writes: Some(s.writes),
        bridge: rt
            .config()
            .bridge
            .as_ref()
            .map(|bridge| bridge.command.display().to_string()),
        stale,
        unavailable: Vec::new(),
    }))
}

/// The figures to show, and whether they are stale. None are shown only
/// when no read has finished since the daemon started.
fn resolve(answer: Answer<Figures>) -> (Option<Arc<Figures>>, Option<StatusStale>) {
    let (figures, stale) = match answer {
        Answer::Fresh(figures) => (Some(figures), None),
        Answer::Stale(last) => {
            let stale = StatusStale {
                read_at: last.as_ref().map(|(at, _)| *at),
                waited_ms: PATIENCE.as_millis() as u64,
            };
            (last.map(|(_, figures)| figures), Some(stale))
        }
    };
    assert_eq!(
        figures.is_some(),
        stale.as_ref().is_none_or(|stale| stale.read_at.is_some()),
        "figures are shown exactly when some read finished"
    );
    assert!(
        stale.as_ref().is_none_or(|stale| stale.waited_ms > 0),
        "a stale answer waited for a fresh one first"
    );
    (figures, stale)
}

/// One read for `status`: the runtime's figures, a crash left behind, and
/// the log position.
fn read_figures(reading: &aether_runtime::Runtime) -> Result<Figures, ErrorReply> {
    let mut runtime = reading.status().map_err(map_err)?;
    if let Some(paths) = &reading.config().storage_paths
        && let Some(problem) = aether_core::crash::problem(&paths.store)
    {
        runtime.problems.push(problem);
    }
    let last_seq = reading.store().last_seq().map_err(store_err)?;
    Ok(Figures { runtime, last_seq })
}

/// The line a stale status leads its problems with, which older clients
/// show too: they do not know `stale`.
fn stale_problem(stale: &StatusStale, at: aether_core::time::Timestamp) -> String {
    let waited = stale.waited_ms as f64 / 1000.0;
    match stale.read_at {
        Some(read_at) => {
            let age = (at - read_at).num_seconds().max(0);
            format!(
                "status is {age}s old: reading the store and scheduler took longer than \
                 {waited:.0}s, so these figures are from {}; a fresh read is still running",
                read_at.format("%Y-%m-%dT%H:%M:%SZ")
            )
        }
        None => format!(
            "status figures are unknown: the daemon's first read of the store and scheduler \
             has taken longer than {waited:.0}s and is still running, so the counts are zero \
             because they are not known"
        ),
    }
}

async fn mcp_servers(
    daemon: &Daemon,
    refresh: bool,
    server: Option<String>,
) -> Result<Reply, ErrorReply> {
    let Some(mcp) = daemon.runtime.tools().mcp().cloned() else {
        return Ok(Reply::McpServers(Vec::new()));
    };
    if refresh {
        match &server {
            // A named server that cannot be reached is the answer the
            // caller asked for, so its health is returned, not an error.
            Some(name) => {
                if let Err(e) = mcp.discover(name).await
                    && matches!(e, aether_tools::McpError::UnknownServer(_))
                {
                    return Err(ErrorReply::new(ErrorCode::NotFound, e.to_string()));
                }
            }
            None => mcp.discover_all().await,
        }
    }
    let health = mcp.health();
    Ok(Reply::McpServers(match server {
        Some(name) => health.into_iter().filter(|h| h.name == name).collect(),
        None => health,
    }))
}

async fn preview_retention(daemon: &Daemon) -> Result<Reply, ErrorReply> {
    let reading = daemon.runtime.clone();
    let preview = blocking(move || reading.preview_retention().map_err(map_err)).await??;
    Ok(Reply::RetentionPreview(preview))
}

fn configure_github_watcher(
    daemon: &Daemon,
    config: aether_core::github::GithubWatcherConfig,
    expected_revision: u64,
    by: Actor,
) -> Result<Reply, ErrorReply> {
    if !matches!(by, Actor::User { .. }) {
        return Err(ErrorReply::new(
            ErrorCode::Unauthorized,
            "only an operator may configure the GitHub watcher",
        ));
    }
    Ok(Reply::GithubWatcher(
        daemon
            .runtime
            .configure_github_watcher(config, expected_revision)
            .map_err(map_err)?,
    ))
}

async fn refresh_provider_usage(
    daemon: &Daemon,
    provider: Option<&str>,
) -> Result<Reply, ErrorReply> {
    Ok(Reply::ProviderUsage(
        daemon
            .runtime
            .refresh_provider_usage(provider)
            .await
            .map_err(map_err)?,
    ))
}
