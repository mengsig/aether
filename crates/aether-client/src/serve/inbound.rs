//! Webhooks in: a signed POST becomes an external trigger.

use std::collections::HashMap;
use std::net::SocketAddr;
use std::sync::Arc;

use hyper::server::conn::http1;
use hyper::service::service_fn;
use hyper_util::rt::{TokioIo, TokioTimer};
use tokio::net::TcpListener;
use tokio::sync::Semaphore;
use tokio_util::sync::CancellationToken;

use super::replay::ReplayGuard;
use super::request::answer;
use super::signature;
use super::{HEADER_READ_TIMEOUT, MAX_CONNECTIONS, PRE_AUTH_SLOTS, ServeConfig};
use crate::sink::CommandSink;

/// One inbound hook: a path, the agent it wakes, and the secret it is signed
/// with.
#[derive(Clone)]
pub struct Webhook {
    /// Request path, matched exactly.
    pub path: String,
    /// The agent to trigger: an agent id, or the name of exactly one live
    /// agent.
    pub agent: String,
    /// Recorded on the inbox item, so an agent can tell its callers apart.
    pub source: String,
    /// Must differ from every other route's and outbound subscription's secret.
    pub secret: String,
}

/// Manual, so that one `{:?}` added later cannot put a hook secret in a log.
impl std::fmt::Debug for Webhook {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Webhook")
            .field("path", &self.path)
            .field("agent", &self.agent)
            .field("source", &self.source)
            .field("secret", &"<redacted>")
            .finish()
    }
}

/// Serve the configured hooks until `shutdown`. Returns at once when no
/// address is configured, so a serve that is outbound-only needs no listener.
pub async fn serve_webhooks(
    sink: Arc<dyn CommandSink>,
    config: &ServeConfig,
    shutdown: CancellationToken,
    bound: impl FnOnce(Option<SocketAddr>),
) -> anyhow::Result<()> {
    let Some(address) = config.address else {
        if !config.webhooks.is_empty() {
            anyhow::bail!("[serve] configures webhooks but no address to receive them on");
        }
        bound(None);
        return Ok(());
    };
    if !address.ip().is_loopback() && !config.allow_remote {
        anyhow::bail!(
            "{address} is not a loopback address, and a webhook creates work on this machine; \
             put a TLS terminator in front of it, or set allow_remote to accept plaintext"
        );
    }
    let routes = Arc::new(validate_routes(config)?);
    let replay_path = config.replay_path.clone();
    let guard =
        Arc::new(tokio::task::spawn_blocking(move || ReplayGuard::open(&replay_path)).await??);
    let listener = TcpListener::bind(address).await?;
    let local = listener.local_addr()?;
    if !local.ip().is_loopback() {
        tracing::warn!(
            address = %local,
            "aether serve: listening outside loopback in plaintext - the hook secret protects the \
             body, not the connection"
        );
    }
    tracing::info!(address = %local, hooks = routes.len(), "aether serve listening");
    bound(Some(local));

    listen(sink, routes, guard, listener, shutdown).await
}

fn validate_routes(config: &ServeConfig) -> anyhow::Result<HashMap<String, Webhook>> {
    let routes: HashMap<String, Webhook> = config
        .webhooks
        .iter()
        .map(|hook| (hook.path.clone(), hook.clone()))
        .collect();
    if routes.len() != config.webhooks.len() {
        anyhow::bail!("[serve] has two webhooks on the same path");
    }
    let mut secrets = HashMap::new();
    for hook in &config.webhooks {
        if hook.secret.len() < signature::MIN_SECRET_BYTES {
            anyhow::bail!(
                "webhook route {} needs a secret of at least {} bytes; set its secret_env variable to a strong random secret",
                hook.path,
                signature::MIN_SECRET_BYTES
            );
        }
        if let Some(path) = secrets.insert(&hook.secret, &hook.path) {
            anyhow::bail!(
                "webhook routes {path} and {} must use distinct secrets",
                hook.path
            );
        }
        for subscription in &config.subscriptions {
            if hook.secret == subscription.secret {
                anyhow::bail!(
                    "webhook route {} and outbound subscription {} must use distinct secrets",
                    hook.path,
                    subscription.url
                );
            }
        }
    }
    Ok(routes)
}

async fn listen(
    sink: Arc<dyn CommandSink>,
    routes: Arc<HashMap<String, Webhook>>,
    guard: Arc<ReplayGuard>,
    listener: TcpListener,
    shutdown: CancellationToken,
) -> anyhow::Result<()> {
    let slots = Arc::new(Semaphore::new(MAX_CONNECTIONS));
    // A separate, smaller budget for work that has not yet proven itself:
    // the bad-signature delay and the body read. See `PRE_AUTH_SLOTS`.
    let pre_auth = Arc::new(Semaphore::new(PRE_AUTH_SLOTS));
    loop {
        // Taken before accepting, so a flood queues in the kernel's backlog.
        let slot = tokio::select! {
            slot = slots.clone().acquire_owned() => match slot {
                Ok(slot) => slot,
                Err(_) => break,
            },
            _ = shutdown.cancelled() => break,
        };
        let (stream, peer) = tokio::select! {
            accepted = listener.accept() => match accepted {
                Ok(pair) => pair,
                Err(e) => { tracing::error!(error = %e, "accept failed"); continue }
            },
            _ = shutdown.cancelled() => break,
        };
        let sink = sink.clone();
        let routes = routes.clone();
        let guard = guard.clone();
        let pre_auth = pre_auth.clone();
        let shutdown = shutdown.clone();
        tokio::spawn(async move {
            let _slot = slot;
            serve_connection(stream, peer, sink, routes, guard, pre_auth, shutdown).await;
        });
    }
    Ok(())
}

async fn serve_connection(
    stream: tokio::net::TcpStream,
    peer: SocketAddr,
    sink: Arc<dyn CommandSink>,
    routes: Arc<HashMap<String, Webhook>>,
    guard: Arc<ReplayGuard>,
    pre_auth: Arc<Semaphore>,
    shutdown: CancellationToken,
) {
    let service = service_fn(move |request| {
        let (sink, routes, guard, pre_auth) = (
            sink.clone(),
            routes.clone(),
            guard.clone(),
            pre_auth.clone(),
        );
        async move {
            Ok::<_, std::convert::Infallible>(
                answer(&*sink, &routes, guard, &pre_auth, request).await,
            )
        }
    });
    let mut builder = http1::Builder::new();
    builder
        .timer(TokioTimer::new())
        .header_read_timeout(HEADER_READ_TIMEOUT);
    let served = builder.serve_connection(TokioIo::new(stream), service);
    tokio::select! {
        result = served => if let Err(e) = result {
            tracing::debug!(%peer, error = %e, "webhook connection ended with an error");
        },
        _ = shutdown.cancelled() => {}
    }
}
