use super::{
    CancellationToken, Client, ClientError, DaemonConfig, Path, PathBuf, StartingDaemon, acted,
    connect, daemon_command,
};
use anyhow::Context as _;

#[cfg(test)]
#[allow(clippy::expect_used)]
mod tests;

/// Run `[serve]`: webhooks in, event subscriptions out, until interrupted.
pub async fn serve(socket: PathBuf, allow_remote: bool) -> anyhow::Result<()> {
    let config = load_serve_config(allow_remote)?;
    let _log = crate::logging::init("serve.log", true)?;
    let client = connect(&socket).await?;
    // Live from here on: an event that predates this process was the previous
    // one's to deliver, and replaying the log would post it all again.
    let events = if config.subscriptions.is_empty() {
        None
    } else {
        Some(client.subscribe(None, Default::default(), false).await?)
    };
    let reconnect_to = socket.clone();
    let sink = std::sync::Arc::new(aether_client::ReconnectingSink::new(
        client.clone(),
        move || {
            let socket = reconnect_to.clone();
            async move { connect(&socket).await }
        },
    ));
    let shutdown = tokio_util::sync::CancellationToken::new();
    let stopping = shutdown.clone();
    tokio::spawn(async move {
        if tokio::signal::ctrl_c().await.is_ok() {
            stopping.cancel();
        }
    });
    let stream = serve_events(client, events);
    aether_client::serve::run(sink, config, stream, shutdown, |bound| match bound {
        Some(address) => println!("aether serve listening on http://{address}"),
        None => println!("aether serve: posting events, no inbound listener configured"),
    })
    .await
}

fn load_serve_config(allow_remote: bool) -> anyhow::Result<aether_client::serve::ServeConfig> {
    let models = aether_model::Models::load(None)?;
    let section = models.config().serve.clone();
    let address = section
        .address()
        .map_err(|reason| anyhow::anyhow!(reason))?;
    let mut webhooks = Vec::new();
    for hook in &section.webhooks {
        webhooks.push(aether_client::serve::Webhook {
            path: hook.path.clone(),
            agent: hook.agent.clone(),
            source: hook.source.clone(),
            secret: webhook_secret(&hook.secret_env, &hook.path)?,
        });
    }
    let mut subscriptions = Vec::new();
    for out in &section.subscriptions {
        // `ServeSection::address` already refused a subscription without one.
        let name = out.secret_env.as_deref().unwrap_or_default();
        subscriptions.push(aether_client::serve::Subscription {
            url: out.url.clone(),
            events: out.events.clone(),
            secret: secret_from(name, &out.url)?,
            max_attempts: out.max_attempts,
            dead_letter: out.dead_letter.clone(),
        });
    }
    Ok(aether_client::serve::ServeConfig {
        address,
        allow_remote: allow_remote || section.allow_remote,
        replay_path: aether_protocol::paths::data_dir().join("webhook-replay.db"),
        webhooks,
        subscriptions,
    })
}

fn serve_events(
    client: Client,
    events: Option<aether_client::Subscription>,
) -> impl futures::Stream<Item = aether_core::event::Event> + Send {
    let last = events.as_ref().map_or(0, |events| events.last_seq());
    futures::stream::unfold(
        (client, events, last),
        |(client, events, mut last)| async move {
            let mut events = events?;
            loop {
                match events.next().await? {
                    aether_client::SubscriptionFrame::Event(event) => {
                        last = event.seq;
                        return Some((*event, (client, Some(events), last)));
                    }
                    aether_client::SubscriptionFrame::Delta { .. } => continue,
                    aether_client::SubscriptionFrame::Lagged { capacity } => {
                        tracing::warn!(
                            capacity,
                            last,
                            "webhook event subscription overflowed; replaying undelivered events"
                        );
                        events = match client
                            .subscribe(Some(last), Default::default(), false)
                            .await
                        {
                            Ok(events) => events,
                            Err(error) => {
                                tracing::error!(%error, "cannot resume webhook event delivery");
                                return None;
                            }
                        };
                    }
                }
            }
        },
    )
}

fn webhook_secret(variable: &str, route: &str) -> anyhow::Result<String> {
    let secret = secret_from(variable, route)?;
    validate_webhook_secret(&secret, variable, route)?;
    Ok(secret)
}

fn validate_webhook_secret(secret: &str, variable: &str, route: &str) -> anyhow::Result<()> {
    anyhow::ensure!(
        secret.len() >= aether_client::serve::signature::MIN_SECRET_BYTES,
        "{variable} for webhook route {route} must contain at least 32 bytes; set {variable} to a strong random secret before running aether serve"
    );
    Ok(())
}

/// A secret from the environment. Aether stores none, so an unset variable is
/// a refusal to start rather than an unsigned door.
fn secret_from(variable: &str, what: &str) -> anyhow::Result<String> {
    std::env::var(variable)
        .ok()
        .map(|secret| secret.trim().to_owned())
        .filter(|secret| !secret.is_empty())
        .with_context(|| format!("{variable} is unset; {what} is signed with it"))
}

pub async fn daemon(
    socket: PathBuf,
    foreground: bool,
    detached: bool,
    workspace: Option<PathBuf>,
    max_concurrent: Option<usize>,
    yolo: bool,
    allow_remote: bool,
) -> anyhow::Result<()> {
    if !foreground {
        anyhow::ensure!(
            max_concurrent != Some(0),
            "--max-concurrent must be positive"
        );
        let command = daemon_command(&socket, yolo, max_concurrent, allow_remote)?;
        let starting = StartingDaemon::spawn(command, &socket)?;
        // Watched the same way auto-start watches its own child: a detached
        // daemon that dies before it listens must not be reported as started.
        aether_client::watch_startup(starting, &socket).await?;
        println!("aetherd started (socket {})", socket.display());
        return Ok(());
    }
    // Held here rather than in `run_daemon`: dropping the guard stops the log
    // writer, and `record_startup_failure` below still has something to say.
    let _log = crate::logging::init("aetherd.log", !detached).inspect_err(|e| {
        if detached {
            record_startup_failure(&socket, e);
        }
    })?;
    let result = run_daemon(
        socket.clone(),
        detached,
        workspace,
        max_concurrent,
        yolo,
        allow_remote,
    )
    .await;
    report_startup_result(&socket, detached, &result);
    result
}

/// A detached daemon has no terminal and its stderr is /dev/null, so its
/// startup log is the only account of a failure the client that started it
/// can read. A foreground one already said it where its caller can see.
pub(super) fn report_startup_result(socket: &Path, detached: bool, result: &anyhow::Result<()>) {
    if detached && let Err(e) = result {
        record_startup_failure(socket, e);
    }
}

/// Leave `error` where the client that started this daemon looks for it.
///
/// The startup log is not appended to: it answers "why did the daemon I just
/// started not come up", and the client clears it before each start.
pub(super) fn record_startup_failure(socket: &Path, error: &anyhow::Error) {
    let path = aether_protocol::paths::startup_log_path_for(socket);
    // Only this user: a daemon that will not start quotes its configuration.
    let written =
        aether_daemon::write_private_file(&path, format!("Error: {error:#}\n").as_bytes());
    if let Err(e) = written {
        // `error` is still returned to the caller; losing this file costs the
        // client only the words, and it reports the exit status regardless.
        tracing::error!(path = %path.display(), error = %e, "cannot record the startup failure");
    }
}

/// Become the daemon. The caller decides what to do with a failure here; a
/// detached daemon has nowhere to print one. Logging is already up.
async fn run_daemon(
    socket: PathBuf,
    detached: bool,
    workspace: Option<PathBuf>,
    max_concurrent: Option<usize>,
    yolo: bool,
    allow_remote: bool,
) -> anyhow::Result<()> {
    let mut config = DaemonConfig::from_paths();
    config.socket = socket;
    config.max_concurrent_turns = max_concurrent;
    config.yolo = yolo;
    config.allow_remote = allow_remote;
    if let Some(ws) = workspace {
        config.workspace_root = Some(
            std::fs::canonicalize(&ws).with_context(|| format!("workspace {}", ws.display()))?,
        );
    }
    let shutdown = CancellationToken::new();
    let signal_token = shutdown.clone();
    tokio::spawn(async move {
        let mut term =
            tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate()).ok();
        tokio::select! {
            _ = tokio::signal::ctrl_c() => {}
            _ = async { match term.as_mut() { Some(t) => { t.recv().await; } None => std::future::pending::<()>().await } } => {}
        }
        signal_token.cancel();
    });
    if !detached {
        eprintln!(
            "aetherd running in the foreground; logs also go to {}",
            crate::logging::log_path_hint("aetherd.log")
        );
    }
    aether_daemon::run(config, shutdown).await
}

/// Ask a compatible daemon to stop; only a confirmed refusal allows a safe signal.
pub async fn stop(socket: PathBuf, json: bool) -> anyhow::Result<()> {
    let connected = tokio::time::timeout(std::time::Duration::from_secs(5), connect(&socket))
        .await
        .context("timed out stopping the daemon during its handshake; no signal sent")?;
    match connected {
        Ok(client) => match client.shutdown().await {
            Ok(()) => acted(
                json,
                "aetherd stopping",
                serde_json::json!({"stopping": true}),
            ),
            Err(error) if error.is_connection_failure() && daemon_is_alive(&socket)? => {
                already_stopping(json)
            }
            Err(error) => Err(error.into()),
        },
        Err(refused) if is_version_skew(&refused) => async {
            tokio::time::timeout(
                std::time::Duration::from_secs(10),
                stop_unaskable_daemon(&socket, json),
            )
            .await
            .context(
                "timed out confirming or stopping the refusing daemon; no forced kill attempted",
            )?
        }
        .await
        .with_context(|| format!("could not stop the daemon that refused this build: {refused:#}")),
        Err(e) if is_connection_failure(&e) && daemon_is_alive(&socket)? => already_stopping(json),
        Err(e) => Err(e),
    }
}

/// A daemon that stopped listening but still holds its lock is on its way
/// out, most likely asked to stop by another caller.
fn daemon_is_alive(socket: &Path) -> anyhow::Result<bool> {
    aether_daemon::socket_lock_is_held(socket)
        .with_context(|| format!("checking whether a daemon still holds {}", socket.display()))
}

fn is_connection_failure(error: &anyhow::Error) -> bool {
    error
        .downcast_ref::<ClientError>()
        .is_some_and(ClientError::is_connection_failure)
}

fn already_stopping(json: bool) -> anyhow::Result<()> {
    acted(
        json,
        "aetherd already stopping",
        serde_json::json!({"stopping": true, "already": true}),
    )
}

pub async fn stop_endpoint(endpoint: aether_client::Endpoint, json: bool) -> anyhow::Result<()> {
    if let Some(path) = endpoint.local_path() {
        return stop(path.to_path_buf(), json).await;
    }
    connect(&endpoint).await?.shutdown().await?;
    acted(
        json,
        "remote daemon stopping",
        serde_json::json!({"stopping": true}),
    )
}

/// True when the daemon answered and refused this build's protocol. `connect`
/// adds context, so the client's own error is found through the chain.
pub(super) fn is_version_skew(error: &anyhow::Error) -> bool {
    matches!(
        error.downcast_ref::<ClientError>(),
        Some(ClientError::Incompatible(_))
    )
}

/// Pin the peer before confirming its refusal on that same connection.
/// A replacement that accepts our handshake is asked to stop, never signalled.
pub(super) async fn stop_unaskable_daemon(socket: &Path, json: bool) -> anyhow::Result<()> {
    let stream = tokio::net::UnixStream::connect(socket)
        .await
        .with_context(|| format!("connecting to {} to identify its daemon", socket.display()))?;
    let peer = crate::daemon_stop::Peer::identify(&stream);
    match Client::open(aether_client::Transport::Unix(stream)).await {
        Err(ClientError::Incompatible(_)) => {
            let peer = peer.context(
                "cannot safely signal this daemon; use its original binary with the same --socket to stop it, or stop its service/foreground terminal",
            )?;
            let pid = peer.pid();
            peer.terminate().await?;
            acted(
                json,
                &format!(
                    "aetherd (pid {pid}) stopped after SIGTERM; it speaks another protocol version"
                ),
                serde_json::json!({"stopped": true, "pid": pid, "signalled": "SIGTERM"}),
            )
        }
        Ok(client) => {
            client.shutdown().await?;
            acted(
                json,
                "aetherd stopping",
                serde_json::json!({"stopping": true}),
            )
        }
        Err(e) => Err(anyhow::Error::new(e).context(format!(
            "the daemon on {} was neither asked nor signalled",
            socket.display()
        ))),
    }
}
