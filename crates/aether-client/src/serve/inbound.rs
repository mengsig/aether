//! Webhooks in: a signed POST becomes an external trigger.

use std::collections::HashMap;
use std::net::SocketAddr;
use std::sync::Arc;
use std::time::Duration;

use aether_core::id::AgentId;
use aether_protocol::{AgentQuery, Command, Reply};
use http_body_util::{BodyExt as _, Full, LengthLimitError, Limited};
use hyper::body::{Bytes, Incoming};
use hyper::server::conn::http1;
use hyper::service::service_fn;
use hyper::{Method, Request, Response, StatusCode};
use hyper_util::rt::{TokioIo, TokioTimer};
use tokio::net::TcpListener;
use tokio::sync::Semaphore;
use tokio_util::sync::CancellationToken;

use super::replay::{Refused, ReplayGuard};
use super::signature::{self, Stamp};
use super::{
    BODY_READ_TIMEOUT, HEADER_READ_TIMEOUT, MAX_BODY_BYTES, MAX_CONNECTIONS, PRE_AUTH_SLOTS,
    ServeConfig,
};
use crate::sink::{CommandSink, DaemonUnavailable};

/// How long a peer that failed to sign waits for its refusal. A caller with
/// the secret is never delayed.
const BAD_SIGNATURE_DELAY: Duration = Duration::from_millis(250);

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
    let routes: Arc<HashMap<String, Webhook>> = Arc::new(
        config
            .webhooks
            .iter()
            .map(|hook| (hook.path.clone(), hook.clone()))
            .collect(),
    );
    if routes.len() != config.webhooks.len() {
        anyhow::bail!("[serve] has two webhooks on the same path");
    }
    let mut secrets = HashMap::new();
    for hook in &config.webhooks {
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

    // One guard for the whole listener: a nonce is remembered across
    // connections, which is exactly what a replay uses a new one for.
    let guard = Arc::new(ReplayGuard::default());
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
            let service = service_fn(move |request| {
                let sink = sink.clone();
                let routes = routes.clone();
                let guard = guard.clone();
                let pre_auth = pre_auth.clone();
                async move {
                    Ok::<_, std::convert::Infallible>(
                        answer(&*sink, &routes, &guard, &pre_auth, request).await,
                    )
                }
            });
            let mut builder = http1::Builder::new();
            builder.timer(TokioTimer::new());
            builder.header_read_timeout(HEADER_READ_TIMEOUT);
            let served = builder.serve_connection(TokioIo::new(stream), service);
            tokio::select! {
                result = served => if let Err(e) = result {
                    tracing::debug!(%peer, error = %e, "webhook connection ended with an error");
                },
                _ = shutdown.cancelled() => {}
            }
        });
    }
    Ok(())
}

fn refuse(status: StatusCode, why: &str) -> Response<Full<Bytes>> {
    let body = serde_json::json!({ "error": why }).to_string();
    Response::builder()
        .status(status)
        .header(hyper::header::CONTENT_TYPE, "application/json")
        .body(Full::new(Bytes::from(body)))
        .unwrap_or_else(|_| Response::new(Full::new(Bytes::from_static(b"{}"))))
}

/// A header's value, or the empty string. Absent and unreadable are the same
/// answer: neither can be part of a valid signature.
fn header(request: &Request<Incoming>, name: &str) -> String {
    request
        .headers()
        .get(name)
        .and_then(|value| value.to_str().ok())
        .unwrap_or_default()
        .to_owned()
}

async fn answer(
    sink: &dyn CommandSink,
    routes: &HashMap<String, Webhook>,
    guard: &ReplayGuard,
    pre_auth: &Semaphore,
    request: Request<Incoming>,
) -> Response<Full<Bytes>> {
    if request.method() != Method::POST {
        return refuse(StatusCode::METHOD_NOT_ALLOWED, "a webhook is a POST");
    }
    let Some(hook) = routes.get(request.uri().path()).cloned() else {
        return refuse(
            StatusCode::NOT_FOUND,
            "no webhook is configured on this path",
        );
    };
    // Everything below here, up to a proven signature, is unverified work:
    // the bad-signature delay and the body read. Bounded by its own small
    // budget rather than the general connection cap, so a flood of
    // unsigned requests cannot hold every connection slot for
    // `BODY_READ_TIMEOUT` - it can only fill this smaller one. Dropped the
    // moment the delivery is proven authentic, below.
    let Ok(unverified) = pre_auth.try_acquire() else {
        return refuse(
            StatusCode::SERVICE_UNAVAILABLE,
            "too many unverified deliveries are in flight; retry shortly",
        );
    };
    let presented = header(&request, signature::HEADER);
    let stamp = match Stamp::parse(
        &header(&request, signature::HEADER_TIMESTAMP),
        &header(&request, signature::HEADER_NONCE),
    ) {
        Ok(stamp) => stamp,
        // Before the body is read: a delivery that cannot be stamped can
        // never be signed, whatever it carries.
        Err(why) => {
            tokio::time::sleep(BAD_SIGNATURE_DELAY).await;
            return refuse(StatusCode::UNAUTHORIZED, &why);
        }
    };
    let collected = tokio::time::timeout(
        BODY_READ_TIMEOUT,
        Limited::new(request.into_body(), MAX_BODY_BYTES).collect(),
    )
    .await;
    let body = match collected {
        Ok(Ok(collected)) => collected.to_bytes(),
        // Distinguished, because they ask the caller for different things:
        // a smaller body, a faster one, or a connection that stays up.
        Ok(Err(e)) if e.downcast_ref::<LengthLimitError>().is_some() => {
            return refuse(StatusCode::PAYLOAD_TOO_LARGE, "the body is too large");
        }
        Ok(Err(e)) => {
            return refuse(
                StatusCode::BAD_REQUEST,
                &format!("the body did not arrive: {e}"),
            );
        }
        Err(_) => {
            return refuse(
                StatusCode::REQUEST_TIMEOUT,
                "the body did not arrive inside the read timeout",
            );
        }
    };
    // The signature covers the stamp and the exact body bytes, so it is
    // checked before the body is parsed: an unsigned caller must not reach
    // the parser.
    let payload = signature::signed_payload(&stamp, &body);
    if !signature::verify(&hook.secret, &payload, &presented) {
        tokio::time::sleep(BAD_SIGNATURE_DELAY).await;
        return refuse(
            StatusCode::UNAUTHORIZED,
            "the signature does not match the stamp and body",
        );
    }
    // Signed, and now proven to be this delivery rather than a copy of one.
    if let Err(refused) = guard.admit(&hook.path, &stamp, aether_core::time::now().timestamp()) {
        let status = match refused {
            Refused::Replayed | Refused::Stale { .. } | Refused::Ahead { .. } => {
                StatusCode::CONFLICT
            }
            Refused::Saturated | Refused::Unavailable => StatusCode::SERVICE_UNAVAILABLE,
        };
        return refuse(status, &refused.to_string());
    }
    // Proven authentic: the rest (parsing, resolving the agent, dispatching
    // the trigger) is trusted work, not the unverified budget's concern.
    drop(unverified);
    let payload = match serde_json::from_slice::<serde_json::Value>(&body) {
        Ok(payload) => payload,
        Err(e) => return refuse(StatusCode::BAD_REQUEST, &format!("body is not JSON: {e}")),
    };
    let agent = match resolve(sink, &hook.agent).await {
        Ok(agent) => agent,
        Err(Refusal { status, why }) => return refuse(status, &why),
    };
    match sink
        .call(Command::Trigger {
            agent,
            source: hook.source.clone(),
            payload,
            // No key: a webhook sender's own retry semantics are its
            // business, and this relay keeps no outbox to key from.
            idempotency_key: None,
        })
        .await
    {
        Ok(_) => {
            let body = serde_json::json!({ "triggered": agent.to_string() }).to_string();
            Response::builder()
                .status(StatusCode::ACCEPTED)
                .header(hyper::header::CONTENT_TYPE, "application/json")
                .body(Full::new(Bytes::from(body)))
                .unwrap_or_else(|_| Response::new(Full::new(Bytes::from_static(b"{}"))))
        }
        Err(e) if is_unavailable(&e) => unavailable(&e),
        Err(e) => refuse(
            StatusCode::BAD_GATEWAY,
            &format!("the daemon refused it: {e}"),
        ),
    }
}

struct Refusal {
    status: StatusCode,
    why: String,
}

fn is_unavailable(error: &anyhow::Error) -> bool {
    error.downcast_ref::<DaemonUnavailable>().is_some()
}

/// Retryable, unlike an unknown path: the sender should try again shortly.
fn unavailable(error: &anyhow::Error) -> Response<Full<Bytes>> {
    tracing::warn!(%error, "webhook not delivered: the daemon is unavailable");
    refuse(
        StatusCode::SERVICE_UNAVAILABLE,
        &format!("the daemon is unavailable; retry shortly ({error})"),
    )
}

/// An agent id, or the name of exactly one live agent. A name that is not
/// unique is refused rather than guessed at.
async fn resolve(sink: &dyn CommandSink, named: &str) -> Result<AgentId, Refusal> {
    if let Ok(id) = named.parse::<AgentId>() {
        return Ok(id);
    }
    let not_found = |why: String| Refusal {
        status: StatusCode::NOT_FOUND,
        why,
    };
    let listed = sink
        .call(Command::ListAgents {
            // Live only: a hook must not wake an agent that has already
            // finished, and once one has, a replacement with the same name
            // would otherwise make every POST ambiguous for the life of the
            // store.
            query: AgentQuery {
                live_only: true,
                ..Default::default()
            },
        })
        .await
        .map_err(|e| {
            if is_unavailable(&e) {
                return Refusal {
                    status: StatusCode::SERVICE_UNAVAILABLE,
                    why: format!("the daemon is unavailable; retry shortly ({e})"),
                };
            }
            not_found(format!("could not list agents: {e}"))
        })?;
    let Reply::Agents(agents) = listed else {
        return Err(not_found(
            "the daemon answered the agent list with something else".into(),
        ));
    };
    let mut matching = agents.iter().filter(|agent| agent.name == named);
    let Some(first) = matching.next() else {
        return Err(not_found(format!("no live agent is named `{named}`")));
    };
    if matching.next().is_some() {
        return Err(not_found(format!(
            "`{named}` names more than one live agent; configure the hook with an agent id"
        )));
    }
    Ok(first.id)
}
