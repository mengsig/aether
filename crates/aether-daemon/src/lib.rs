//! The daemon: owns the runtime and the store, serves the protocol over a
//! Unix socket. Every connected client is a projection; none is privileged.

mod agent_temp;
mod authz;
mod background;
mod bridge;
mod connection;
mod crash;
#[cfg(test)]
mod env_tests;
mod instance_lock;
pub mod listen;
mod mcp;
mod projections;
pub mod push;
pub mod remote;
mod secrets;
mod startup;

use std::path::{Path, PathBuf};
use std::sync::Arc;

use aether_model::Models;
pub use aether_runtime::DEFAULT_MAX_CONCURRENT_TURNS;
use aether_runtime::{Runtime, RuntimeConfig};
use aether_store::SqliteStore;
use aether_tools::{McpManager, SearchProvider, ToolRegistry};
use tokio::net::UnixListener;
use tokio::sync::Semaphore;
use tokio_util::sync::CancellationToken;

pub use instance_lock::{LockBusy, socket_lock_is_held, write_private_file};
pub use listen::{ConnectionPolicy, TcpListen};
pub use projections::build_tree;

#[derive(Debug, Clone)]
pub struct DaemonConfig {
    pub socket: PathBuf,
    pub store_path: PathBuf,
    pub config_dir: Option<PathBuf>,
    pub workspace_root: Option<PathBuf>,
    pub max_concurrent_turns: Option<usize>,
    /// Approvals off for every agent (also settable in config.toml).
    pub yolo: bool,
    /// A fully resolved TCP transport. When absent, `[listen]` in
    /// config.toml decides whether there is one.
    pub tcp: Option<TcpListen>,
    /// Let the TCP transport bind outside loopback (also settable in
    /// config.toml as `[listen] allow_remote`).
    pub allow_remote: bool,
}

impl DaemonConfig {
    pub fn from_paths() -> Self {
        Self {
            socket: aether_protocol::paths::socket_path(),
            store_path: aether_protocol::paths::store_path(),
            config_dir: None,
            workspace_root: std::env::current_dir().ok(),
            max_concurrent_turns: None,
            yolo: false,
            tcp: None,
            allow_remote: false,
        }
    }
}

/// The configured semantic pass over memory retrieval, if any. A
/// provider-backed index would be another arm here and another
/// `MemoryIndex` implementation; nothing else changes.
fn memory_index_of(
    choice: aether_model::config::MemoryIndexChoice,
) -> Option<std::sync::Arc<dyn aether_memory::MemoryIndex>> {
    match choice {
        aether_model::config::MemoryIndexChoice::None => None,
        aether_model::config::MemoryIndexChoice::Local => {
            Some(std::sync::Arc::new(aether_memory::LocalEmbeddingIndex))
        }
    }
}

/// `0` means unbounded, not "keep nothing" - the documented reading of
/// `retain_days` / `retain_bytes`.
fn retention_of(
    artifacts: &aether_model::config::ArtifactsSection,
) -> aether_core::artifact::RetentionPolicy {
    aether_core::artifact::RetentionPolicy {
        max_age_days: Some(artifacts.retain_days).filter(|d| *d > 0),
        max_total_bytes: Some(artifacts.retain_bytes).filter(|b| *b > 0),
    }
}

fn storage_artifact_policy(
    config: &aether_model::config::AetherConfig,
) -> aether_core::artifact::RetentionPolicy {
    let mut policy = retention_of(&config.artifacts);
    if let Some(days) = config.storage.artifacts_days {
        if days == 0 {
            return aether_core::artifact::RetentionPolicy::keep_everything();
        }
        policy.max_age_days = Some(u32::from(days));
    }
    policy
}

/// The configured provider, as the search tool wants it. The two structs
/// are field-for-field identical strings, so a transposed pair here would
/// compile and ship; `the_provider_copy_keeps_every_field_in_place` stops it.
/// The configured root, else `aether.build-cache` beside the store. A
/// relative default would resolve inside each agent's workspace, so none.
fn build_cache_root(configured: Option<&Path>, store_path: &Path) -> Option<PathBuf> {
    if let Some(root) = configured {
        debug_assert!(root.is_absolute(), "config load refuses a relative root");
        return Some(root.to_path_buf());
    }
    if !store_path.is_absolute() {
        tracing::warn!(
            "builds keep their workspace's own output: store path {} is not absolute, so \
             there is no default build cache root; set build_cache.root",
            store_path.display()
        );
        return None;
    }
    Some(store_path.with_extension("build-cache"))
}

fn search_provider(s: aether_model::config::WebSearchSection) -> SearchProvider {
    SearchProvider {
        endpoint: s.endpoint,
        query_param: s.query_param,
        results_pointer: s.results_pointer,
        title_field: s.title_field,
        url_field: s.url_field,
        snippet_field: s.snippet_field,
        api_key_env: s.api_key_env,
        api_key_header: s.api_key_header,
    }
}

/// Shared state handed to every connection.
#[derive(Debug)]
pub struct Daemon {
    pub runtime: Arc<Runtime>,
    pub models: Arc<Models>,
    pub instance_id: String,
    pub version: String,
    /// Web Push. `None` only where a daemon is assembled without one.
    pub push: Option<push::Push>,
    /// Remote control; empty until [`remote::attach`] gives the daemon one.
    pub remote: remote::RemoteSlot,
}

/// Run until `shutdown` fires or a client asks to stop.
pub async fn run(config: DaemonConfig, shutdown: CancellationToken) -> anyhow::Result<()> {
    anyhow::ensure!(
        config.max_concurrent_turns != Some(0),
        "max_concurrent_turns must be positive"
    );
    std::os::unix::net::SocketAddr::from_pathname(&config.socket)?;
    let mut clock = startup::StartupClock::start();
    // Both before the store opens and recovery runs, held until `run`
    // returns. The socket lock covers the probe, the unlink and the bind
    // below, which the store lock cannot: a daemon for another store
    // configured with this socket passes that one.
    let instance = instance_lock::InstanceLock::for_store(&config.store_path)?;
    let socket_lock = instance_lock::InstanceLock::for_socket(&config.socket)?;
    // Pin the same target as the lock: a configured directory alias can change later.
    let socket = socket_lock.target();
    let (socket_address, _socket_alias) = pinned_socket_address(socket)?;
    remove_stale_socket(socket, &socket_address).await?;
    let pid_path = aether_protocol::paths::pid_path_for(socket);
    instance_lock::check_private_file_replacement(&pid_path)?;
    clock.finished("preflight");
    let store_path = instance.prepare_store()?;
    crash::report_previous(&store_path);
    crash::begin_run(&store_path);
    let store = Arc::new(SqliteStore::open(&store_path)?);
    clock.finished("store");
    let instance_id = aether_store::Store::instance_id(store.as_ref())?;
    let config_dir = aether_model::config::config_dir(config.config_dir.clone());
    match aether_model::config::ensure_templates(&config_dir) {
        Ok(written) => {
            for path in written {
                tracing::info!(path = %path.display(), "wrote configuration template");
            }
        }
        Err(e) => tracing::warn!(
            dir = %config_dir.display(),
            error = %e,
            "could not write configuration templates"
        ),
    }
    let models = Arc::new(Models::load(config.config_dir.clone())?);
    // Resolved before anything is opened, so a listener the operator asked
    // for and cannot have fails the start rather than the first connection.
    let tcp = match config.tcp.clone() {
        Some(explicit) => Some(explicit),
        None => configured_listener(&models.config().listen, config.allow_remote)?,
    };
    if let Some(listen) = &tcp {
        listen::check(listen)?;
    }
    // Nothing binds until remote control is enabled, but a stanza that
    // could never be honoured fails the start, where the operator is looking.
    let remote_config = remote::configured(&models.config().remote)?;
    let push_config = push::configured(&models.config().remote, &instance_id, &config.store_path)?;
    let yolo = config.yolo || models.config().approval.yolo;
    if yolo {
        tracing::warn!(
            "yolo mode: approvals are off, every tool call runs, engines run unsandboxed"
        );
    } else if models.config().approval.default == aether_core::agent::ApprovalDefault::Yolo {
        tracing::warn!(
            "new agents auto-approve their tool calls ([approval] default = \"yolo\"); scope \
             and the execution backend are the boundary"
        );
    }
    for retired in models.retired_settings() {
        tracing::warn!("{retired}; refused on use until config.toml changes");
    }
    // Validated when the config loaded; re-read here rather than duplicating
    // the rule.
    let wait_timeout = models.config().scheduler.wait_timeout().map_err(|reason| {
        aether_model::config::ConfigError {
            path: "config.toml".into(),
            reason,
        }
    })?;
    let engine_park_grace = models
        .config()
        .scheduler
        .engine_park_grace()
        .map_err(|reason| aether_model::config::ConfigError {
            path: "config.toml".into(),
            reason,
        })?
        .unwrap_or(aether_runtime::DEFAULT_ENGINE_PARK_GRACE);
    let engine_transient_retry_window = models
        .config()
        .scheduler
        .engine_transient_retry()
        .map_err(|reason| aether_model::config::ConfigError {
            path: "config.toml".into(),
            reason,
        })?
        .unwrap_or(aether_runtime::DEFAULT_ENGINE_TRANSIENT_RETRY_WINDOW);
    let engine_stall_timeout = models
        .config()
        .scheduler
        .engine_stall_timeout()
        .map_err(|reason| aether_model::config::ConfigError {
            path: "config.toml".into(),
            reason,
        })?
        .unwrap_or(aether_runtime::DEFAULT_ENGINE_STALL_TIMEOUT);
    let config_error = |reason| aether_model::config::ConfigError {
        path: "config.toml".into(),
        reason,
    };
    let orphan_sweep_interval = models
        .config()
        .scheduler
        .orphan_sweep_interval()
        .map_err(config_error)?
        .unwrap_or(aether_runtime::DEFAULT_ORPHAN_SWEEP_INTERVAL);
    let orphan_idle = models
        .config()
        .scheduler
        .orphan_idle()
        .map_err(config_error)?
        .unwrap_or(aether_runtime::DEFAULT_ORPHAN_IDLE);
    let max_concurrent_engine_turns =
        models
            .config()
            .scheduler
            .engine_concurrency()
            .map_err(|reason| aether_model::config::ConfigError {
                path: "config.toml".into(),
                reason,
            })?;
    let run_budget_usd = models.config().scheduler.run_budget().map_err(|reason| {
        aether_model::config::ConfigError {
            path: "config.toml".into(),
            reason,
        }
    })?;
    let run_hard_stop = models
        .config()
        .scheduler
        .run_hard_stop()
        .map_err(|reason| aether_model::config::ConfigError {
            path: "config.toml".into(),
            reason,
        })?;
    let mut disk_floor = models.config().scheduler.disk_floor().map_err(|reason| {
        aether_model::config::ConfigError {
            path: "config.toml".into(),
            reason,
        }
    })?;
    if let Some(bytes) = models.config().storage.free_space_floor_bytes {
        disk_floor = (bytes > 0).then_some(aether_core::disk::DiskFloor::Bytes(bytes));
    }
    // MCP health transitions become events once the runtime exists, so the
    // channel is created before it and drained after it starts.
    let (mcp_tx, mcp_rx) = tokio::sync::mpsc::unbounded_channel();
    let builtins = tool_registry(models.config(), models.registry(), tcp.as_ref());
    if let Some(background) = builtins.background() {
        background.recover_host_jobs(&store_path)?;
    }
    let mcp = McpManager::with_background(
        mcp::specs(&models.config().mcp),
        Some(mcp_tx),
        builtins.hidden_env(),
        builtins.background().cloned(),
    );
    let tools = Arc::new(builtins.with_mcp(mcp.clone()));
    let bridge = bridge::Bridge::prepare(&store_path);
    // Appended to the whole file name: `a.db` and `a.sqlite` are distinct stores.
    let mut engine_registry = store_path.clone().into_os_string();
    engine_registry.push(".engines");
    let engine_registry = std::path::PathBuf::from(engine_registry);
    let reaped = aether_engine::orphans::init(&engine_registry)
        .map_err(|error| anyhow::anyhow!("preparing {}: {error}", engine_registry.display()))?;
    if !reaped.is_empty() {
        tracing::warn!(pids = ?reaped, "killed engine processes a previous daemon left running");
    }
    let agent_temp_root = agent_temp::prepare(
        models.config().agents.temp_root.as_deref(),
        &store_path,
        &instance_id,
    )?;
    let redactor = secrets::known_secrets_redactor(
        models.config(),
        models.registry(),
        tcp.as_ref(),
        |name| std::env::var(name).ok(),
    )?;
    clock.finished("config");
    let runtime = Runtime::start(
        RuntimeConfig {
            holder: format!("aetherd-{}-{}", instance_id, std::process::id()),
            instance_id: instance_id.as_str().into(),
            max_concurrent_turns: config.max_concurrent_turns.unwrap_or(
                models
                    .config()
                    .scheduler
                    .concurrency()
                    .map_err(anyhow::Error::msg)?,
            ),
            max_concurrent_engine_turns,
            run_budget_usd,
            run_hard_stop,
            wait_timeout,
            resource_wait_timeout: wait_timeout
                .unwrap_or(aether_runtime::DEFAULT_RESOURCE_WAIT_TIMEOUT),
            engine_park_grace,
            engine_transient_retry_window,
            engine_stall_timeout,
            orphan_sweep_interval,
            orphan_idle,
            yolo,
            approval_default: models.config().approval.default,
            network_default: models.config().network.default,
            retention: storage_artifact_policy(models.config()),
            storage_paths: Some(aether_runtime::storage::StoragePaths {
                store: store_path.clone(),
                logs: aether_protocol::paths::log_dir(),
            }),
            memory_index: memory_index_of(models.config().memory.index),
            // Resolved with the engines, after the socket answers.
            default_model: None,
            workspace_root: config.workspace_root.clone(),
            // Beside the resolved store: an engine opens the file from its agent's workspace.
            attachment_dir: Some(store_path.with_extension("attachments")),
            github_checkout_root: Some(config.store_path.with_extension("github-workspaces")),
            github_review_sweep_prs: models
                .config()
                .github_watcher
                .review_sweep_prs()
                .map_err(config_error)?,
            delegated_workspace_root: Some(
                config.store_path.with_extension("delegated-workspaces"),
            ),
            agent_temp_root,
            build_cache_root: build_cache_root(
                models.config().build_cache.root.as_deref(),
                &store_path,
            ),
            bridge: bridge.as_ref().map(|bridge| aether_runtime::BridgeConfig {
                command: bridge.command().to_path_buf(),
                socket: socket_address.clone(),
            }),
            disk_guard: disk_floor.map(|floor| aether_runtime::DiskGuard {
                path: store_path.clone(),
                floor,
            }),
            redactor,
            // The daemon runs on a multi-threaded runtime and no `LocalSet`.
            store_writes_off_worker: true,
            ..Default::default()
        },
        store.clone(),
        models.clone(),
        tools,
    )
    .await?;
    clock.finished("runtime");

    let mut connections = tokio::task::JoinSet::new();
    let mut socket_published = None;
    let mut pid_published = None;
    let result = async {
        aether_tools::exec::sweep_orphan_containers(&instance_id).await;
        clock.finished("containers");

        mcp::spawn_recorder(&runtime, mcp_rx, &mut connections, &shutdown);
        if !mcp.is_empty() {
            // Discovery talks to other processes; do not hold up the socket for it.
            let discovering = mcp.clone();
            background::isolate(&mut connections, &runtime, "MCP discovery", &shutdown, true, async move { discovering.discover_all().await });
        }

        let listener = bind_private_socket(socket, &mut socket_published)?;
        instance_lock::write_private_file_tracked(
            &pid_path,
            std::process::id().to_string().as_bytes(),
            &mut pid_published,
        )?;
        tracing::info!(socket = %socket.display(), pid = std::process::id(), "aetherd listening");
        clock.finished("listening");

        let push = push::Push::start(
            push_config,
            store.clone(),
            Arc::new(aether_remote::push::NetTransport),
            &runtime,
            &mut connections,
            &shutdown,
        )?;
        let daemon = Arc::new(Daemon {
            runtime: runtime.clone(),
            models,
            instance_id,
            version: env!("CARGO_PKG_VERSION").to_owned(),
            push: Some(push),
            remote: remote::RemoteSlot::default(),
        });
        // Supervised with the connections: it opens the listener the stored
        // settings ask for, and closes it when the daemon stops.
        let remote_control = remote::attach(&daemon, remote_config, shutdown.clone()).await?;
        background::isolate(&mut connections, &runtime, "remote listener control", &shutdown, false, remote_control.run(shutdown.clone()));

        // A failure to open the second transport is a failure to start: a daemon
        // the operator asked to reach over the network must not come up without
        // it and look healthy.
        if let Some(listen) = tcp {
            let (tcp, local) = listen::bind_tcp(&listen).await?;
            tracing::info!(address = %local, "aetherd listening on tcp");
            background::isolate(&mut connections, &runtime, "TCP listener", &shutdown, false, serve_tcp(daemon.clone(), tcp, listen, shutdown.clone()));
        }

        background::github(&mut connections, &runtime, &shutdown);
        clock.finished("serving");
        loop {
            tokio::select! {
                accepted = listener.accept() => match accepted {
                    Ok((stream, _)) => {
                        let daemon = daemon.clone();
                        let shutdown = shutdown.clone();
                        connections.spawn(async move {
                            // The socket's file mode is its authorization, and a
                            // local client is never dropped for being quiet.
                            let policy = ConnectionPolicy::default();
                            if let Err(e) =
                                connection::serve(daemon, stream, policy, None, None, shutdown).await
                            {
                                tracing::debug!(error = %e, "connection ended with error");
                            }
                        });
                    }
                    Err(e) => {
                        tracing::error!(error = %e, "accept failed");
                        accept_backoff(&e, &shutdown).await;
                    },
                },
                Some(result) = connections.join_next() => {
                    if let Err(error) = result {
                        tracing::error!(%error, "connection task failed");
                    }
                }
                _ = shutdown.cancelled() => break Ok(()),
            }
        }
    }.await;
    tracing::info!("aetherd shutting down");
    shutdown.cancel();
    shutdown_connections(&runtime, &mut connections).await;
    let store_sync = match store.sync() {
        Ok(true) => Ok(()),
        // A reader on an older snapshot keeps frames in the WAL; the next open still reads them.
        Ok(false) => {
            tracing::warn!("final WAL sync left frames for a reader; they stay in the WAL");
            Ok(())
        }
        Err(error) => Err(anyhow::Error::new(error).context("final store sync")),
    };
    let socket_cleanup = if let Some(identity) = socket_published {
        remove_daemon_file(socket, &identity)
    } else {
        Ok(())
    };
    // A rejected PID entry must be left intact, including a foreign symlink or hard link.
    let pid_cleanup = if let Some(file) = pid_published {
        file.metadata()
            .map_err(anyhow::Error::new)
            .and_then(|identity| remove_daemon_file(&pid_path, &identity))
    } else {
        Ok(())
    };
    let bridge_cleanup = bridge.map_or(Ok(()), bridge::Bridge::remove);
    if result.is_ok() && store_sync.is_ok() {
        crash::end_run(&store_path);
    }
    result
        .and(store_sync)
        .and(socket_cleanup)
        .and(pid_cleanup)
        .and(bridge_cleanup)
}

/// A valid configured alias can resolve to a target longer than `sun_path`.
/// Keep a private, stable address for probing and engine connections in that case.
fn pinned_socket_address(socket: &Path) -> anyhow::Result<(PathBuf, Option<tempfile::TempDir>)> {
    use anyhow::Context as _;
    use std::os::unix::fs::{PermissionsExt, symlink};

    if std::os::unix::net::SocketAddr::from_pathname(socket).is_ok() {
        return Ok((socket.to_path_buf(), None));
    }
    instance_lock::check_shared_parent(Path::new("/tmp"))?;
    let alias = tempfile::Builder::new()
        .permissions(std::fs::Permissions::from_mode(0o700))
        .tempdir_in("/tmp")
        .context("creating a short daemon socket address")?;
    let address = alias.path().join("s");
    symlink(socket, &address).context("linking the pinned daemon socket")?;
    Ok((address, Some(alias)))
}

fn remove_daemon_file(path: &Path, identity: &std::fs::Metadata) -> anyhow::Result<()> {
    use std::os::unix::fs::MetadataExt;

    let current = match std::fs::symlink_metadata(path) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(error) => {
            return Err(anyhow::Error::new(error).context(format!("checking {}", path.display())));
        }
    };
    // Validated canonical ancestors protect this check/unlink from other users.
    anyhow::ensure!(
        current.dev() == identity.dev()
            && current.ino() == identity.ino()
            && current.uid() == identity.uid()
            && current.file_type() == identity.file_type(),
        "daemon file {} was replaced; leaving it intact",
        path.display()
    );
    match std::fs::remove_file(path) {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(error) => {
            tracing::warn!(path = %path.display(), %error, "could not remove daemon file");
            Err(anyhow::Error::new(error).context(format!("removing {}", path.display())))
        }
    }
}

async fn shutdown_connections(runtime: &Runtime, connections: &mut tokio::task::JoinSet<()>) {
    // Pending engine approvals need runtime cancellation before their connections can finish.
    runtime.shutdown().await;
    while let Some(result) = connections.join_next().await {
        if let Err(error) = result {
            tracing::error!(%error, "connection task failed");
        }
    }
}

/// Publish an owner-only socket without changing its parent's permissions.
fn bind_private_socket(
    socket: &Path,
    published: &mut Option<std::fs::Metadata>,
) -> anyhow::Result<UnixListener> {
    use anyhow::Context as _;
    use std::os::unix::fs::PermissionsExt;

    let parent = socket
        .parent()
        .filter(|p| !p.as_os_str().is_empty())
        .unwrap_or(Path::new("."));
    instance_lock::check_shared_parent(parent)?;
    // Short names leave room for long configured socket parents.
    let staging = tempfile::Builder::new()
        .prefix("")
        .rand_bytes(3)
        .permissions(std::fs::Permissions::from_mode(0o700))
        .tempdir_in(parent)
        .with_context(|| format!("staging the socket {}", socket.display()))?;
    let staged_socket = staging.path().join("s");
    // A short basename can fit even when the longer staged address does not.
    let alias = if std::os::unix::net::SocketAddr::from_pathname(&staged_socket).is_err() {
        instance_lock::check_shared_parent(Path::new("/tmp"))?;
        let alias = tempfile::Builder::new()
            .permissions(std::fs::Permissions::from_mode(0o700))
            .tempdir_in("/tmp")
            .context("creating a short socket staging alias")?;
        std::os::unix::fs::symlink(
            std::fs::canonicalize(staging.path())?,
            alias.path().join("d"),
        )
        .context("linking the socket staging directory")?;
        Some(alias)
    } else {
        None
    };
    let bind_path = alias
        .as_ref()
        .map(|alias| alias.path().join("d/s"))
        .unwrap_or_else(|| staged_socket.clone());
    let listener = UnixListener::bind(&bind_path)
        .with_context(|| format!("binding the staged socket for {}", socket.display()))?;
    std::fs::set_permissions(&staged_socket, std::fs::Permissions::from_mode(0o600))
        .with_context(|| format!("hardening the socket {}", socket.display()))?;
    let identity = std::fs::symlink_metadata(&staged_socket)
        .with_context(|| format!("checking the staged socket {}", socket.display()))?;
    // A foreign listener can bind after our probe; publication must not replace it.
    #[cfg(any(target_os = "linux", target_os = "android", target_vendor = "apple"))]
    rustix::fs::renameat_with(
        rustix::fs::CWD,
        &staged_socket,
        rustix::fs::CWD,
        socket,
        rustix::fs::RenameFlags::NOREPLACE,
    )
    .map_err(std::io::Error::from)
    .with_context(|| format!("publishing the socket {}", socket.display()))?;
    #[cfg(not(any(target_os = "linux", target_os = "android", target_vendor = "apple")))]
    std::fs::hard_link(&staged_socket, socket)
        .with_context(|| format!("publishing the socket {}", socket.display()))?;
    *published = Some(identity);
    if let Some(alias) = alias {
        alias.close().context("removing the socket staging alias")?;
    }
    staging
        .close()
        .context("removing the socket staging directory")?;
    Ok(listener)
}

/// Clear a socket whose daemon is gone. Called under the socket lock, so no
/// other aether daemon can bind between the probe and the bind; something
/// still answering here is a foreign listener and keeps its socket.
async fn remove_stale_socket(socket: &Path, address: &Path) -> anyhow::Result<()> {
    use std::os::unix::fs::FileTypeExt as _;

    match std::fs::symlink_metadata(socket) {
        Ok(found) => anyhow::ensure!(
            found.file_type().is_socket(),
            "{} exists and is not a socket; refusing to replace it",
            socket.display()
        ),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(e) => {
            return Err(
                anyhow::Error::new(e).context(format!("checking the socket {}", socket.display()))
            );
        }
    }
    match tokio::net::UnixStream::connect(address).await {
        Ok(_) => anyhow::bail!(
            "an aether daemon is already listening on {}",
            socket.display()
        ),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(e) if e.kind() == std::io::ErrorKind::ConnectionRefused => {}
        Err(e) => {
            return Err(
                anyhow::Error::new(e).context(format!("probing the socket {}", socket.display()))
            );
        }
    }
    match std::fs::remove_file(socket) {
        Err(e) if e.kind() != std::io::ErrorKind::NotFound => Err(anyhow::Error::new(e)
            .context(format!("removing the stale socket {}", socket.display()))),
        _ => Ok(()),
    }
}

/// Configured tools hide daemon and provider credentials from tool children.
/// One owner; engine authentication keeps a separate hide list.
fn tool_registry(
    config: &aether_model::config::AetherConfig,
    providers: &aether_model::registry::Registry,
    tcp: Option<&TcpListen>,
) -> ToolRegistry {
    ToolRegistry::with_builtins_and_search(config.web.search.clone().map(search_provider))
        .hiding(daemon_secret_env(config, tcp))
        .hiding_from_tools(provider_secret_env(providers))
}

/// Each built-in and configured provider's API key and bearer variables.
fn provider_secret_env(providers: &aether_model::registry::Registry) -> Vec<String> {
    let builtins = aether_model::registry::Registry::builtin();
    builtins
        .providers()
        .iter()
        .chain(providers.providers())
        .flat_map(|provider| {
            provider
                .auth
                .api_key_env
                .iter()
                .chain(&provider.auth.bearer_env)
                .cloned()
        })
        .collect()
}

/// Every variable naming a secret this daemon reads: the TCP transport's
/// token, each `[serve]` hook secret and each MCP server's `token_env`. They join the daemon's own reach in
/// the environment stripped from every tool child, engine and MCP server, so
/// an agent's shell cannot read the credential that drives the fleet.
///
/// `[listen] token_env` is named unconditionally, not only when the listener
/// is open: a variable the operator declared a secret stays hidden whether
/// or not this daemon is the process using it. `tcp` is the listener this
/// daemon actually resolved to, which may carry a different variable name -
/// `DaemonConfig.tcp` lets an embedder set one directly, bypassing
/// `[listen]` entirely, and that name must be hidden too.
fn daemon_secret_env(
    config: &aether_model::config::AetherConfig,
    tcp: Option<&TcpListen>,
) -> Vec<String> {
    let mut names = vec![config.listen.token_env.clone()];
    names.extend(tcp.and_then(|listen| listen.token_env.clone()));
    names.extend(config.serve.webhooks.iter().map(|h| h.secret_env.clone()));
    names.extend(
        config
            .mcp
            .servers
            .iter()
            .filter_map(|s| s.token_env.clone()),
    );
    names.extend(
        config
            .serve
            .subscriptions
            .iter()
            .filter_map(|s| s.secret_env.clone()),
    );
    names
}

/// The TCP transport `[listen]` asks for, with its token read from the
/// environment. `Ok(None)` means the section names no address.
fn configured_listener(
    section: &aether_model::config::ListenSection,
    allow_remote: bool,
) -> anyhow::Result<Option<TcpListen>> {
    let bad = |reason: String| aether_model::config::ConfigError {
        path: "config.toml".into(),
        reason,
    };
    let Some(addr) = section.address().map_err(bad)? else {
        return Ok(None);
    };
    Ok(Some(TcpListen {
        addr,
        token: section.token().map_err(bad)?,
        token_env: Some(section.token_env.clone()),
        allow_remote: allow_remote || section.allow_remote,
        max_connections: section.max_connections,
        idle_timeout: std::time::Duration::from_secs(section.idle_timeout_secs),
        handshake_timeout: listen::HANDSHAKE_TIMEOUT,
    }))
}

async fn accept_backoff(error: &std::io::Error, shutdown: &CancellationToken) {
    let code = error.raw_os_error();
    if code == Some(rustix::io::Errno::MFILE.raw_os_error())
        || code == Some(rustix::io::Errno::NFILE.raw_os_error())
    {
        tokio::select! {
            _ = tokio::time::sleep(std::time::Duration::from_millis(100)) => {},
            _ = shutdown.cancelled() => {},
        }
    }
}

/// Accept TCP connections until `shutdown`. The slot is taken before the
/// accept, so a flood queues in the kernel's backlog rather than in tasks and
/// file descriptors here.
async fn serve_tcp(
    daemon: Arc<Daemon>,
    listener: tokio::net::TcpListener,
    listen: TcpListen,
    shutdown: CancellationToken,
) {
    let policy = ConnectionPolicy {
        token: Some(listen.token.as_str().into()),
        idle_timeout: Some(listen.idle_timeout),
        handshake_timeout: listen.handshake_timeout,
        device: None,
    };
    // Two caps, not one. A peer that has authorized holds an `authenticated`
    // slot for as long as it likes; one that has not can only hold a
    // `handshaking` slot, and the handshake deadline bounds that - so
    // unauthenticated peers cannot fill the transport.
    let handshaking = Arc::new(Semaphore::new(listen.max_connections));
    let authenticated = Arc::new(Semaphore::new(listen.max_connections));
    let mut connections = tokio::task::JoinSet::new();
    loop {
        // Selected against shutdown as well: at the cap this waits, and
        // a daemon stopping then must not wait with it.
        let slot = tokio::select! {
            slot = handshaking.clone().acquire_owned() => match slot {
                Ok(slot) => slot,
                Err(_) => break,
            },
            _ = shutdown.cancelled() => break,
        };
        let accepted = tokio::select! {
            a = listener.accept() => a,
            _ = shutdown.cancelled() => break,
            Some(result) = connections.join_next() => {
                if let Err(error) = result {
                    tracing::error!(%error, "tcp connection task failed");
                }
                continue;
            }
        };
        let (stream, peer) = match accepted {
            Ok(pair) => pair,
            Err(e) => {
                tracing::error!(error = %e, "tcp accept failed");
                accept_backoff(&e, &shutdown).await;
                continue;
            }
        };
        // Frames are small and answered one at a time; waiting to coalesce
        // them only adds latency.
        let _ = stream.set_nodelay(true);
        tracing::debug!(%peer, "tcp connection accepted");
        let daemon = daemon.clone();
        let policy = policy.clone();
        let gate = connection::Gate::new(slot, authenticated.clone());
        let shutdown = shutdown.clone();
        connections.spawn(async move {
            if let Err(e) =
                connection::serve(daemon, stream, policy, Some(peer), Some(gate), shutdown).await
            {
                tracing::debug!(error = %e, "tcp connection ended with error");
            }
        });
    }
    while let Some(result) = connections.join_next().await {
        if let Err(error) = result {
            tracing::error!(%error, "tcp connection task failed");
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_build_cache_root_is_configured_or_beside_an_absolute_store() {
        let store = Path::new("/srv/aether/aether.db");
        let configured = Path::new("/var/cache/aether");
        assert_eq!(
            build_cache_root(Some(configured), store).as_deref(),
            Some(configured)
        );
        assert_eq!(
            build_cache_root(None, store),
            Some(PathBuf::from("/srv/aether/aether.build-cache"))
        );
        assert_eq!(build_cache_root(None, Path::new("data/aether.db")), None);
    }

    #[tokio::test(start_paused = true)]
    async fn descriptor_exhaustion_backs_off_but_shutdown_interrupts_it() {
        for errno in [rustix::io::Errno::MFILE, rustix::io::Errno::NFILE] {
            let error = std::io::Error::from_raw_os_error(errno.raw_os_error());
            let shutdown = CancellationToken::new();
            let started = tokio::time::Instant::now();
            accept_backoff(&error, &shutdown).await;
            assert_eq!(started.elapsed(), std::time::Duration::from_millis(100));
            let waiting = accept_backoff(&error, &shutdown);
            tokio::pin!(waiting);
            assert!(futures::poll!(&mut waiting).is_pending());
            shutdown.cancel();
            waiting.await;
            assert_eq!(started.elapsed(), std::time::Duration::from_millis(100));
        }
        let started = tokio::time::Instant::now();
        accept_backoff(
            &std::io::Error::from(std::io::ErrorKind::Interrupted),
            &CancellationToken::new(),
        )
        .await;
        assert_eq!(started.elapsed(), std::time::Duration::ZERO);
    }

    #[tokio::test]
    async fn a_private_socket_is_published_without_chmodding_its_parent() {
        use std::os::unix::fs::PermissionsExt;

        let dir = tempfile::tempdir().unwrap();
        std::fs::set_permissions(dir.path(), std::fs::Permissions::from_mode(0o1777)).unwrap();
        let socket = dir.path().join("aetherd.sock");
        let listener = bind_private_socket(&socket, &mut None).unwrap();
        assert_eq!(
            std::fs::metadata(dir.path()).unwrap().permissions().mode() & 0o7777,
            0o1777
        );
        assert_eq!(
            std::fs::metadata(&socket).unwrap().permissions().mode() & 0o777,
            0o600
        );
        assert_eq!(
            std::fs::read_dir(dir.path()).unwrap().count(),
            1,
            "staging removed"
        );
        let peer = tokio::net::UnixStream::connect(&socket).await.unwrap();
        listener.accept().await.unwrap();
        drop(peer);
    }

    #[cfg(any(target_os = "linux", target_vendor = "apple"))]
    #[tokio::test]
    async fn a_short_socket_basename_in_a_long_parent_still_binds() {
        use std::os::unix::ffi::OsStrExt;

        let dir = tempfile::tempdir_in("/tmp").unwrap();
        let parent_length = if cfg!(target_vendor = "apple") {
            98
        } else {
            102
        };
        let name_length = parent_length - dir.path().as_os_str().as_bytes().len() - 1;
        let parent = dir.path().join("a".repeat(name_length));
        std::fs::create_dir(&parent).unwrap();
        let socket = parent.join("s");
        std::os::unix::net::SocketAddr::from_pathname(&socket).expect("configured address fits");
        let listener =
            bind_private_socket(&socket, &mut None).expect("staging must preserve valid addresses");
        let peer = tokio::net::UnixStream::connect(&socket).await.unwrap();
        listener.accept().await.unwrap();
        assert_eq!(std::fs::read_dir(&parent).unwrap().count(), 1);
        drop(peer);
    }

    #[tokio::test]
    async fn publishing_a_socket_does_not_replace_a_foreign_listener() {
        let dir = tempfile::tempdir().unwrap();
        let socket = dir.path().join("aetherd.sock");
        let foreign = UnixListener::bind(&socket).unwrap();
        bind_private_socket(&socket, &mut None).expect_err("foreign listener keeps its pathname");
        let peer = tokio::net::UnixStream::connect(&socket).await.unwrap();
        foreign.accept().await.unwrap();
        assert_eq!(
            std::fs::read_dir(dir.path()).unwrap().count(),
            1,
            "failed staging cleaned"
        );
        drop(peer);
    }

    #[tokio::test]
    async fn a_regular_file_at_the_socket_path_is_refused_and_kept() {
        let dir = tempfile::tempdir().unwrap();
        let socket = dir.path().join("aetherd.sock");
        std::fs::write(&socket, "precious").unwrap();
        let error = remove_stale_socket(&socket, &socket).await.unwrap_err();
        assert!(error.to_string().contains("not a socket"), "{error}");
        assert_eq!(std::fs::read_to_string(&socket).unwrap(), "precious");
    }

    #[tokio::test]
    async fn a_symlink_at_the_socket_path_is_refused_and_its_target_kept() {
        let dir = tempfile::tempdir().unwrap();
        let target = dir.path().join("target");
        std::fs::write(&target, "precious").unwrap();
        let socket = dir.path().join("aetherd.sock");
        std::os::unix::fs::symlink(&target, &socket).unwrap();
        remove_stale_socket(&socket, &socket).await.unwrap_err();
        assert_eq!(std::fs::read_to_string(&target).unwrap(), "precious");
        assert!(
            std::fs::symlink_metadata(&socket)
                .unwrap()
                .file_type()
                .is_symlink()
        );
    }

    #[tokio::test]
    async fn a_directory_at_the_socket_path_is_refused() {
        let dir = tempfile::tempdir().unwrap();
        let socket = dir.path().join("aetherd.sock");
        std::fs::create_dir(&socket).unwrap();
        remove_stale_socket(&socket, &socket).await.unwrap_err();
        assert!(socket.is_dir());
    }

    #[tokio::test]
    async fn a_stale_socket_is_replaced() {
        let dir = tempfile::tempdir().unwrap();
        let socket = dir.path().join("aetherd.sock");
        drop(std::os::unix::net::UnixListener::bind(&socket).unwrap());
        assert!(socket.exists(), "closing a listener leaves its pathname");
        remove_stale_socket(&socket, &socket).await.unwrap();
        assert!(!socket.exists());
    }

    #[test]
    fn the_provider_copy_keeps_every_field_in_place() {
        // Eight identically typed strings: transposing two compiles fine and
        // sends queries to the wrong endpoint or reads the wrong field, so
        // each one is named against a distinct value.
        let section = aether_model::config::WebSearchSection {
            endpoint: "https://search.example/api".into(),
            query_param: "query-param".into(),
            results_pointer: "/results-pointer".into(),
            title_field: "title-field".into(),
            url_field: "url-field".into(),
            snippet_field: "snippet-field".into(),
            api_key_env: Some("KEY_ENV".into()),
            api_key_header: "Key-Header".into(),
        };
        let provider = search_provider(section);
        assert_eq!(provider.endpoint, "https://search.example/api");
        assert_eq!(provider.query_param, "query-param");
        assert_eq!(provider.results_pointer, "/results-pointer");
        assert_eq!(provider.title_field, "title-field");
        assert_eq!(provider.url_field, "url-field");
        assert_eq!(provider.snippet_field, "snippet-field");
        assert_eq!(provider.api_key_env.as_deref(), Some("KEY_ENV"));
        assert_eq!(provider.api_key_header, "Key-Header");
    }

    /// The token that drives the whole fleet must never be readable by an
    /// agent's shell: `exec::pipes` strips whatever is on this list, so the
    /// list is where the token has to be.
    #[test]
    fn every_secret_the_config_names_is_hidden_from_tool_children() {
        let mut config = aether_model::config::AetherConfig::default();
        config.listen.token_env = "FLEET_TOKEN".into();
        config
            .serve
            .webhooks
            .push(aether_model::config::WebhookSection {
                path: "/hooks/ci".into(),
                agent: "builder".into(),
                source: "ci".into(),
                secret_env: "HOOK_SECRET".into(),
            });
        config
            .serve
            .subscriptions
            .push(aether_model::config::SubscriptionSection {
                url: "https://example.com/events".into(),
                secret_env: Some("SUBSCRIPTION_SECRET".into()),
                ..Default::default()
            });

        config
            .mcp
            .servers
            .push(aether_model::config::McpServerSection {
                name: "issues".into(),
                url: Some("https://issues.example/mcp".into()),
                token_env: Some("MCP_TOKEN".into()),
                command: None,
                args: Vec::new(),
                env: Default::default(),
                cwd: None,
                auth: Default::default(),
                classes: Default::default(),
                send_caller_meta: false,
            });

        let tools = tool_registry(&config, &aether_model::registry::Registry::builtin(), None);
        let hidden = tools.hidden_env();
        for name in [
            "FLEET_TOKEN",
            "HOOK_SECRET",
            "SUBSCRIPTION_SECRET",
            "MCP_TOKEN",
        ] {
            assert!(hidden.contains(&name.to_owned()), "{name} reaches a child");
            assert!(
                tools.engine_hidden_env().contains(&name.to_owned()),
                "{name} reaches an engine"
            );
        }
        // Adding to the list never replaces the daemon's own reach.
        for name in aether_tools::registry::DAEMON_ENV {
            assert!(hidden.contains(&name.to_owned()), "{name} reaches a child");
        }
    }

    /// `daemon_secret_env` used to read only
    /// `config.listen.token_env`, so an embedder that sets `DaemonConfig.tcp`
    /// directly - bypassing `[listen]` - had its token's variable name
    /// unhidden from tool children even though the config's own (unused)
    /// `token_env` default was hidden regardless.
    #[test]
    fn an_embedder_resolved_listener_hides_its_own_token_env_too() {
        let config = aether_model::config::AetherConfig::default();
        let addr: std::net::SocketAddr = "127.0.0.1:0".parse().expect("addr");
        let mut tcp = TcpListen::new(addr, "0123456789abcdef0123456789abcdef");
        tcp.token_env = Some("EMBEDDER_TOKEN".into());

        let tools = tool_registry(
            &config,
            &aether_model::registry::Registry::builtin(),
            Some(&tcp),
        );
        let hidden = tools.hidden_env();
        assert!(hidden.contains(&"EMBEDDER_TOKEN".to_owned()));
        assert!(
            tools
                .engine_hidden_env()
                .contains(&"EMBEDDER_TOKEN".to_owned())
        );
        // The config's own configured name is still hidden too, whether or
        // not this resolved listener is the one actually using it.
        assert!(hidden.contains(&config.listen.token_env));
    }

    #[test]
    fn overridden_auth_keeps_builtin_credentials_hidden() {
        let mut providers = aether_model::registry::Registry::builtin();
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("providers.toml");
        std::fs::write(
            &path,
            "[providers.anthropic.auth]\napi_key_env = ['CUSTOM_KEY']\n",
        )
        .unwrap();
        let overrides = aether_model::registry::load_providers_file(&path)
            .unwrap()
            .unwrap();
        providers.apply_overrides(overrides).unwrap();
        let tools = tool_registry(
            &aether_model::config::AetherConfig::default(),
            &providers,
            None,
        );
        for name in ["CUSTOM_KEY", "ANTHROPIC_API_KEY"] {
            assert!(tools.hidden_env().contains(&name.to_owned()));
        }
    }

    #[test]
    fn storage_artifact_override_preserves_legacy_and_disables_all_limits_at_zero() {
        let mut config = aether_model::config::AetherConfig::default();
        assert_eq!(
            storage_artifact_policy(&config),
            retention_of(&config.artifacts)
        );
        config.storage.artifacts_days = Some(14);
        assert_eq!(storage_artifact_policy(&config).max_age_days, Some(14));
        config.storage.artifacts_days = Some(0);
        assert_eq!(
            storage_artifact_policy(&config),
            aether_core::artifact::RetentionPolicy::keep_everything()
        );
    }

    #[test]
    fn zero_retention_limits_mean_unbounded_not_empty() {
        let none = aether_model::config::ArtifactsSection {
            retain_days: 0,
            retain_bytes: 0,
        };
        let policy = retention_of(&none);
        assert_eq!(
            policy,
            aether_core::artifact::RetentionPolicy::keep_everything()
        );
        let bounded = aether_model::config::ArtifactsSection {
            retain_days: 7,
            retain_bytes: 1024,
        };
        let policy = retention_of(&bounded);
        assert_eq!(policy.max_age_days, Some(7));
        assert_eq!(policy.max_total_bytes, Some(1024));
    }
}
