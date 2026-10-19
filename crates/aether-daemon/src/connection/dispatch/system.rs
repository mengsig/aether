//! Daemon-wide commands: liveness, storage, status, MCP, providers, and the
//! host, remote and push services.

use crate::Daemon;
use crate::authz::AuthContext;
use aether_core::event::Actor;
use aether_protocol::{Command, DaemonStatus, ErrorCode, ErrorReply, Reply};

use crate::connection::dispatch::blocking;
use crate::connection::{host_services, map_err, providers, push, remote_admin, store_err};

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
    // Reads every agent and takes `sched`: on a slow disk that must not park an async worker.
    let reading = rt.clone();
    let (s, last_seq) = blocking(move || {
        let status = reading.status().map_err(map_err)?;
        let last_seq = reading.store().last_seq().map_err(store_err)?;
        Ok::<_, ErrorReply>((status, last_seq))
    })
    .await??;
    Ok(Reply::Status(DaemonStatus {
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
        mcp: s.mcp,
        low_disk: s.low_disk,
        bridge: rt
            .config()
            .bridge
            .as_ref()
            .map(|bridge| bridge.command.display().to_string()),
    }))
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
