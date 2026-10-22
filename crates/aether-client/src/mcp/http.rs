//! Serve MCP over streamable HTTP: one endpoint, one JSON-RPC message per
//! POST, the answer as a JSON body.
//!
//! A token is mandatory. The endpoint drives the whole fleet - it can create
//! agents that run shell commands on this machine - so an unauthenticated one
//! would be a way in for anything that can reach the port. A request without a
//! matching bearer token is refused before it is parsed, and everything a peer
//! can hold without one (connections, header reads, guesses) is bounded.
//!
//! The token also decides *how much*: the operator's reaches the fleet, a
//! running turn's reaches that turn's own tools and nothing else. Fleet
//! authority must not rest on a credential that travels in a child process's
//! argv.

use std::collections::HashMap;
use std::net::{IpAddr, SocketAddr};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use aether_core::id::AgentId;
use aether_protocol::auth::constant_time_eq;
use aether_protocol::{Command, Reply};
use http_body_util::{BodyExt as _, Full, Limited};
use hyper::body::{Bytes, Incoming};
use hyper::server::conn::http1;
use hyper::service::service_fn;
use hyper::{Method, Request, Response, StatusCode};
use hyper_util::rt::{TokioIo, TokioTimer};
use tokio::net::TcpListener;
use tokio::sync::Semaphore;
use tokio_util::sync::CancellationToken;

use super::{Answer, CommandSink, Surface, TurnSurface};

/// Largest request body accepted. A JSON-RPC call is small; anything larger
/// is a mistake or an attack, and reading it would be the vulnerability.
const MAX_BODY_BYTES: usize = 1 << 20;

/// Shortest operator token accepted, the same rule every tokened transport
/// holds to.
pub use aether_protocol::auth::MIN_TOKEN_CHARS;

/// Connections served at once. A peer that opens more waits in the kernel's
/// backlog instead of costing a task and a pair of file descriptors each.
const MAX_CONNECTIONS: usize = 64;

/// How long a peer may take to send its request head.
const HEADER_READ_TIMEOUT: Duration = Duration::from_secs(5);

/// How long any one connection may stay open. Every call here is a small
/// request and a small answer, so a connection that outlives this is holding
/// a slot rather than using it; a client that needs another opens one.
const CONNECTION_MAX_LIFETIME: Duration = Duration::from_secs(60);

/// A source may fail to authenticate this many times per window before its
/// further failures are answered 429. A valid token is never refused for it.
const MAX_AUTH_FAILURES: u32 = 5;
const AUTH_FAILURE_WINDOW: Duration = Duration::from_secs(60);
/// Held before a throttled failure is answered, so a guesser that ignores
/// 429 is paced per connection (at most `MAX_CONNECTIONS` at once).
const THROTTLED_FAILURE_DELAY: Duration = Duration::from_millis(500);
/// Peers tracked for backoff. Bounded so the map is not itself a way to
/// spend the daemon's memory.
const MAX_TRACKED_PEERS: usize = 1024;

/// Where the endpoint listens and what it accepts.
#[derive(Debug, Clone)]
pub struct HttpConfig {
    pub addr: SocketAddr,
    /// The operator's long-lived bearer token.
    pub token: String,
    /// Allow a bind outside loopback. Off by default: this is a fleet
    /// control plane behind one static string, and Aether terminates no TLS.
    pub allow_remote: bool,
}

/// Serve until `shutdown` fires. Returns the address actually bound, through
/// `bound`, so a caller that asked for port 0 can learn it.
pub async fn serve(
    sink: Arc<dyn CommandSink>,
    config: HttpConfig,
    shutdown: CancellationToken,
    bound: impl FnOnce(SocketAddr),
) -> anyhow::Result<()> {
    let token = config.token.trim().to_owned();
    if token.chars().count() < MIN_TOKEN_CHARS {
        anyhow::bail!(
            "the MCP HTTP endpoint needs a token of at least {MIN_TOKEN_CHARS} characters; \
             this one has {}",
            token.chars().count()
        );
    }
    let listener = TcpListener::bind(config.addr).await?;
    let local = listener.local_addr()?;
    if !local.ip().is_loopback() {
        if !config.allow_remote {
            anyhow::bail!(
                "{local} is not a loopback address, and this endpoint can create agents that \
                 run commands on this machine; pass --allow-remote to mean it"
            );
        }
        tracing::warn!(
            address = %local,
            "mcp server: bound outside loopback over plain HTTP - Aether terminates no TLS, so \
             the bearer token crosses the network in cleartext unless something in front of it \
             does"
        );
    }
    tracing::info!(address = %local, "mcp server listening");
    bound(local);

    let auth = Arc::new(Auth {
        token,
        backoff: Backoff::default(),
    });
    let slots = Arc::new(Semaphore::new(MAX_CONNECTIONS));
    loop {
        // Taken before accepting, so a flood queues in the kernel's backlog
        // rather than in tasks and file descriptors here.
        let Ok(slot) = slots.clone().acquire_owned().await else {
            break;
        };
        let (stream, peer) = tokio::select! {
            accepted = listener.accept() => accepted?,
            _ = shutdown.cancelled() => break,
        };
        let sink = sink.clone();
        let auth = auth.clone();
        let connection = shutdown.clone();
        tokio::spawn(async move {
            let _slot = slot;
            let service = service_fn(move |req| {
                let sink = sink.clone();
                let auth = auth.clone();
                async move {
                    Ok::<_, std::convert::Infallible>(answer(&*sink, &auth, peer.ip(), req).await)
                }
            });
            let served = http1::Builder::new()
                // header_read_timeout needs a timer to arm against.
                .timer(TokioTimer::new())
                .header_read_timeout(HEADER_READ_TIMEOUT)
                .serve_connection(TokioIo::new(stream), service);
            tokio::select! {
                result = served => {
                    if let Err(e) = result {
                        tracing::debug!(%peer, error = %e, "mcp server: connection ended");
                    }
                }
                _ = tokio::time::sleep(CONNECTION_MAX_LIFETIME) => {
                    tracing::debug!(%peer, "mcp server: connection outlived its budget");
                }
                _ = connection.cancelled() => {}
            }
        });
    }
    Ok(())
}

/// What a request must present, and what guessing costs.
struct Auth {
    token: String,
    backoff: Backoff,
}

/// One line naming who authenticated, for the log. The surface itself
/// carries the authority.
fn describe(surface: &Surface) -> String {
    match surface {
        Surface::Fleet => "operator".to_owned(),
        Surface::Turn(turn) => format!("turn of {}", turn.agent),
    }
}

async fn answer(
    sink: &dyn CommandSink,
    auth: &Auth,
    peer: IpAddr,
    request: Request<Incoming>,
) -> Response<Full<Bytes>> {
    // No legitimate caller here is a browser, so every origin is refused
    // rather than allowlisted: it closes DNS rebinding without a setting.
    if request.headers().contains_key(hyper::header::ORIGIN) {
        return status(
            StatusCode::FORBIDDEN,
            "this endpoint does not serve browser origins",
        );
    }
    // Before the method check, so an unauthenticated probe cannot tell this
    // port from a closed one by the status it gets back.
    let Some(surface) = authorize(sink, &auth.token, &request).await else {
        if auth.backoff.failed(peer) {
            tokio::time::sleep(THROTTLED_FAILURE_DELAY).await;
            return status(
                StatusCode::TOO_MANY_REQUESTS,
                "too many failed authentications from here; wait a minute, then retry with the token `aether mcp` prints",
            );
        }
        return status(StatusCode::UNAUTHORIZED, "a valid bearer token is required");
    };
    tracing::debug!(%peer, caller = %describe(&surface), "mcp server: authorized");
    if request.method() != Method::POST {
        return status(StatusCode::METHOD_NOT_ALLOWED, "only POST is served");
    }
    // Limited stops reading at the cap rather than buffering first, so an
    // oversized body is refused instead of held in memory.
    let body = match Limited::new(request.into_body(), MAX_BODY_BYTES)
        .collect()
        .await
        .map(|collected| collected.to_bytes())
    {
        Ok(bytes) => bytes,
        Err(_) => {
            return status(
                StatusCode::PAYLOAD_TOO_LARGE,
                "the request body was too large, or ended early",
            );
        }
    };
    let message: serde_json::Value = match serde_json::from_slice(&body) {
        Ok(v) => v,
        Err(e) => return status(StatusCode::BAD_REQUEST, &format!("malformed JSON: {e}")),
    };
    match super::handle(sink, &surface, &message).await {
        // A notification is accepted and answered with no content, as the
        // streamable HTTP transport requires.
        Answer::None => Response::builder()
            .status(StatusCode::ACCEPTED)
            .body(Full::default())
            .unwrap_or_else(|_| Response::new(Full::default())),
        Answer::Denied(why) => status(StatusCode::FORBIDDEN, &why),
        Answer::Reply(response) => {
            let bytes = serde_json::to_vec(&response).unwrap_or_default();
            Response::builder()
                .status(StatusCode::OK)
                .header("content-type", "application/json")
                .body(Full::from(bytes))
                .unwrap_or_else(|_| Response::new(Full::default()))
        }
    }
}

/// What the bearer token authorizes, or `None`. The operator's token is
/// compared here; a per-turn one is `<agent-id>.<token>` and only the daemon
/// can say whether that turn is still running, so it is asked - and its
/// answer, that turn's own tool list, is the surface the token reaches.
async fn authorize(
    sink: &dyn CommandSink,
    token: &str,
    request: &Request<Incoming>,
) -> Option<Surface> {
    let presented = bearer(request)?;
    if constant_time_eq(presented, token) {
        return Some(Surface::Fleet);
    }
    let (agent, turn_token) = presented.split_once('.')?;
    let agent: AgentId = agent.parse().ok()?;
    match sink
        .call(Command::EngineTools {
            agent,
            token: turn_token.to_owned(),
        })
        .await
    {
        Ok(Reply::EngineTools { tools }) => Some(Surface::Turn(TurnSurface {
            agent,
            token: turn_token.to_owned(),
            tools,
        })),
        _ => None,
    }
}

/// The bearer credential as presented. The scheme is case-insensitive
/// (RFC 7235); the value is not trimmed, so a token that differs only in
/// whitespace is a different token.
fn bearer(request: &Request<Incoming>) -> Option<&str> {
    request
        .headers()
        .get(hyper::header::AUTHORIZATION)?
        .to_str()
        .ok()?
        .split_once(' ')
        .filter(|(scheme, _)| scheme.eq_ignore_ascii_case("bearer"))
        .map(|(_, presented)| presented)
}

/// Counts failed authentications per source in a fixed window. Only failures
/// are throttled: a request that presents a valid token is served whatever
/// its source has failed before, so one bad attempt never locks a client out.
#[derive(Debug, Default)]
struct Backoff {
    peers: Mutex<HashMap<IpAddr, Penalty>>,
}

#[derive(Debug, Clone, Copy)]
struct Penalty {
    failures: u32,
    window_start: Instant,
}

impl Backoff {
    /// Records one failure; true once the source is over its budget and
    /// should be told to wait.
    fn failed(&self, peer: IpAddr) -> bool {
        // A poisoned lock means a panic while recording; throttle rather
        // than let the failure path be the thing that opens the door.
        let Ok(mut peers) = self.peers.lock() else {
            return true;
        };
        let now = Instant::now();
        let failures = match peers.get(&peer) {
            Some(p) if now.duration_since(p.window_start) < AUTH_FAILURE_WINDOW => {
                p.failures.saturating_add(1)
            }
            _ => 1,
        };
        let window_start = match peers.get(&peer) {
            Some(p) if failures > 1 => p.window_start,
            _ => now,
        };
        if peers.len() >= MAX_TRACKED_PEERS && !peers.contains_key(&peer) {
            evict_one(&mut peers);
        }
        peers.insert(
            peer,
            Penalty {
                failures,
                window_start,
            },
        );
        failures > MAX_AUTH_FAILURES
    }
}

/// Drop the peer whose window opened first, so the map stays bounded and
/// a peer still being throttled keeps its count.
fn evict_one(peers: &mut HashMap<IpAddr, Penalty>) {
    let Some(soonest) = peers
        .iter()
        .min_by_key(|(_, penalty)| penalty.window_start)
        .map(|(addr, _)| *addr)
    else {
        return;
    };
    peers.remove(&soonest);
}

fn status(code: StatusCode, message: &str) -> Response<Full<Bytes>> {
    Response::builder()
        .status(code)
        .header("content-type", "text/plain")
        .body(Full::from(message.to_owned()))
        .unwrap_or_else(|_| Response::new(Full::default()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn one_failure_is_not_throttled_but_repeated_ones_are() {
        let backoff = Backoff::default();
        let peer: IpAddr = "10.0.0.1".parse().expect("addr");
        for attempt in 1..=MAX_AUTH_FAILURES {
            assert!(!backoff.failed(peer), "failure {attempt} is within budget");
        }
        assert!(backoff.failed(peer), "the next failure is throttled");
    }

    #[test]
    fn another_source_keeps_its_own_budget() {
        let backoff = Backoff::default();
        let noisy: IpAddr = "10.0.0.2".parse().expect("addr");
        let other: IpAddr = "10.0.0.3".parse().expect("addr");
        for _ in 0..=MAX_AUTH_FAILURES {
            backoff.failed(noisy);
        }
        assert!(
            !backoff.failed(other),
            "someone else's failures are not mine"
        );
    }

    #[test]
    fn the_budget_returns_when_the_window_passes() {
        let backoff = Backoff::default();
        let peer: IpAddr = "10.0.0.4".parse().expect("addr");
        for _ in 0..=MAX_AUTH_FAILURES {
            backoff.failed(peer);
        }
        let mut peers = backoff.peers.lock().expect("lock");
        let penalty = peers.get_mut(&peer).expect("tracked");
        penalty.window_start = Instant::now() - AUTH_FAILURE_WINDOW - Duration::from_secs(1);
        drop(peers);
        assert!(!backoff.failed(peer), "an old window no longer counts");
    }

    #[test]
    fn the_peer_table_is_bounded() {
        let backoff = Backoff::default();
        for i in 0..(MAX_TRACKED_PEERS + 50) {
            let peer: IpAddr = format!("10.{}.{}.{}", i / 65536, (i / 256) % 256, i % 256)
                .parse()
                .expect("addr");
            backoff.failed(peer);
        }
        assert!(
            backoff.peers.lock().expect("lock").len() <= MAX_TRACKED_PEERS,
            "tracking must not itself be a way to spend memory"
        );
    }
}
