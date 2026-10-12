//! The remote listener: plain HTTP/1.1 on loopback. TLS and the public name
//! belong to whatever fronts it: `tailscale serve`, or another reverse proxy.

use std::net::SocketAddr;
use std::sync::Arc;
use std::time::Duration;

use hyper::server::conn::http1;
use hyper::service::service_fn;
use hyper_util::rt::{TokioIo, TokioTimer};
use tokio::net::{TcpListener, TcpStream};
use tokio::task::JoinSet;
use tokio_util::sync::CancellationToken;

use crate::http::respond;
use crate::lobby::Seat;
use crate::{RemoteConfig, RemoteServer, ws};
use write_deadline::{Watch, WriteDeadline};

mod write_deadline;

pub const DEFAULT_ADDRESS: &str = "127.0.0.1:8377";
pub const DEFAULT_MAX_CONNECTIONS: usize = 32;
pub const DEFAULT_IDLE_TIMEOUT_SECS: u64 = 300;
/// Most WebSockets waiting for their hello to be accepted, however many
/// connections are configured: a hello takes one round trip.
pub const MAX_UNGREETED_SOCKETS: usize = 8;

/// Most bytes of request line and headers read for one request.
const MAX_HEAD_BYTES: usize = 32 * 1024;
/// How long to leave the socket alone after an accept fails, so a
/// descriptor shortage does not become a busy loop.
const ACCEPT_BACKOFF: Duration = Duration::from_millis(100);

/// Whether this listener may be opened at all. Checked before anything is
/// bound, so a refused configuration never holds a port even briefly.
pub fn check(config: &RemoteConfig) -> anyhow::Result<()> {
    anyhow::ensure!(
        config.max_connections > 0,
        "the remote listener needs room for at least one connection"
    );
    anyhow::ensure!(
        !config.idle_timeout.is_zero(),
        "the remote listener needs a positive idle timeout"
    );
    anyhow::ensure!(
        config.address.ip().is_loopback() || config.allow_remote,
        "{} is not a loopback address, and the remote listener speaks plaintext HTTP; front it \
         with `tailscale serve` or a TLS reverse proxy on this machine, or set `[remote] \
         allow_remote = true` to accept plaintext on the network",
        config.address
    );
    Ok(())
}

/// Bind the listener. Returns it and the address it actually bound.
pub async fn bind(config: &RemoteConfig) -> anyhow::Result<(TcpListener, SocketAddr)> {
    check(config)?;
    let listener = TcpListener::bind(config.address).await?;
    let local = listener.local_addr()?;
    Ok((listener, local))
}

/// Accept and serve connections until `shutdown` fires, then close the
/// socket and give the connections still open a moment to end.
pub async fn serve(
    listener: TcpListener,
    server: Arc<RemoteServer>,
    shutdown: CancellationToken,
) -> anyhow::Result<()> {
    // A seat is taken once a peer is accepted, so a full lobby displaces
    // someone only for a newcomer that exists. Until a seat is free the
    // acceptor holds this one stream and the rest wait in the kernel's backlog.
    let lobby = server.http_lobby();
    let mut connections = JoinSet::new();
    loop {
        let accepted = tokio::select! {
            accepted = listener.accept() => accepted,
            _ = shutdown.cancelled() => break,
        };
        reap(&mut connections);
        let stream = match accepted {
            Ok((stream, _)) => stream,
            Err(error) => {
                tracing::warn!(%error, "the remote listener could not accept a connection");
                tokio::time::sleep(ACCEPT_BACKOFF).await;
                continue;
            }
        };
        let seat = tokio::select! {
            seat = lobby.enter() => seat,
            _ = shutdown.cancelled() => break,
        };
        connections.spawn(connection(stream, server.clone(), shutdown.clone(), seat));
    }
    drop(listener);
    drain(connections, server.tuning().drain_timeout).await;
    Ok(())
}

/// Collect the connections that have ended.
fn reap(connections: &mut JoinSet<()>) {
    while let Some(ended) = connections.try_join_next() {
        report(ended);
    }
}

/// A connection task that did not return normally leaves a trace.
fn report(ended: Result<(), tokio::task::JoinError>) {
    let Err(error) = ended else {
        return;
    };
    if error.is_panic() {
        tracing::error!(%error, "a remote connection task panicked");
    } else {
        tracing::warn!(%error, "a remote connection task was cancelled");
    }
}

/// Wait for the connections to end on their own, then end the rest.
async fn drain(mut connections: JoinSet<()>, patience: Duration) {
    let finished = async {
        while let Some(ended) = connections.join_next().await {
            report(ended);
        }
    };
    if tokio::time::timeout(patience, finished).await.is_err() {
        tracing::warn!(
            left = connections.len(),
            "remote connections did not end in time and were dropped"
        );
        connections.shutdown().await;
    }
}

/// Serve one TCP connection: HTTP requests, then a WebSocket if one of
/// them upgraded.
async fn connection(
    stream: TcpStream,
    server: Arc<RemoteServer>,
    stop: CancellationToken,
    seat: Seat,
) {
    if let Err(error) = stream.set_nodelay(true) {
        tracing::debug!(%error, "could not disable Nagle on a remote connection");
    }
    let upgrade = respond::UpgradeSlot::default();
    let service = service_fn(|request| {
        let (server, upgrade) = (&server, &upgrade);
        async move {
            Ok::<_, std::convert::Infallible>(respond::respond(server, request, upgrade).await)
        }
    });
    let watch = Watch::armed();
    let stream = WriteDeadline::new(stream, server.tuning().write_timeout, watch.clone());
    let serving = http1::Builder::new()
        .timer(TokioTimer::new())
        .header_read_timeout(server.tuning().header_timeout)
        .max_buf_size(MAX_HEAD_BYTES)
        .serve_connection(TokioIo::new(stream), service)
        .with_upgrades();
    tokio::pin!(serving);
    let served = tokio::select! {
        served = serving.as_mut() => served,
        _ = stop.cancelled() => {
            serving.as_mut().graceful_shutdown();
            serving.await
        }
        _ = seat.displaced().cancelled() => {
            tracing::debug!("a remote connection gave its place to a newer one");
            return;
        }
    };
    if let Err(error) = served {
        tracing::debug!(%error, "a remote connection ended with an error");
    }
    // An upgraded connection waits in the greeting lobby, not in this one.
    drop(seat);
    let Some(pending) = upgrade.take() else {
        return;
    };
    match pending.on_upgrade.await {
        Ok(upgraded) => {
            watch.disarm();
            let link = server.link(pending.origin, pending.seat, &stop);
            ws::carry(TokioIo::new(upgraded), link).await;
        }
        Err(error) => tracing::debug!(%error, "a WebSocket upgrade did not complete"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    fn config(address: &str) -> RemoteConfig {
        RemoteConfig {
            address: address.parse().expect("addr"),
            allow_remote: false,
            max_connections: DEFAULT_MAX_CONNECTIONS,
            idle_timeout: Duration::from_secs(DEFAULT_IDLE_TIMEOUT_SECS),
            push_previews: false,
            push_subject: None,
        }
    }

    #[test]
    fn a_non_loopback_bind_needs_the_operator_to_mean_it() {
        check(&config(DEFAULT_ADDRESS)).expect("loopback is the default");
        let refused = check(&config("0.0.0.0:8377")).expect_err("no opt-in");
        assert!(refused.to_string().contains("allow_remote"), "{refused}");
        let mut meant = config("0.0.0.0:8377");
        meant.allow_remote = true;
        check(&meant).expect("acknowledged plaintext is allowed");
    }

    #[test]
    fn a_listener_with_no_room_or_no_timeout_is_refused() {
        let mut full = config(DEFAULT_ADDRESS);
        full.max_connections = 0;
        assert!(check(&full).is_err());
        let mut eternal = config(DEFAULT_ADDRESS);
        eternal.idle_timeout = Duration::ZERO;
        assert!(check(&eternal).is_err());
    }
}

#[cfg(test)]
mod live_tests;
