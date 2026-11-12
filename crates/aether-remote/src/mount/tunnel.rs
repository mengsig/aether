//! A mounted WebSocket: frames relayed one by one between the client and the target, each
//! checked against the lease first. When the lease ends the client gets 1008 and the target
//! gets nothing more, not even a close frame.

use std::time::Duration;

use futures::{SinkExt as _, StreamExt as _};
use hyper::upgrade::{OnUpgrade, Upgraded};
use hyper_util::rt::TokioIo;
use tokio::io::{AsyncRead, AsyncWrite};
use tokio::sync::OwnedSemaphorePermit;
use tokio::time::Instant;
use tokio_tungstenite::WebSocketStream;
use tokio_tungstenite::tungstenite::protocol::frame::coding::CloseCode;
use tokio_tungstenite::tungstenite::protocol::{CloseFrame, Role, WebSocketConfig};
use tokio_tungstenite::tungstenite::{Error as WsError, Message};
use tokio_util::sync::CancellationToken;

use super::Ticket;
use crate::{Tuning, ws};

/// Why the lease ending closed a tunnel, as the client reads it.
const LEASE_ENDED: &str = "the listener's publication status could not be confirmed";

/// A mounted upgrade the target accepted, waiting for the client's side.
pub(crate) struct Tunnel {
    pub client: OnUpgrade,
    pub target: TokioIo<Upgraded>,
    pub ticket: Ticket,
    /// Fires when the mounts change under the tunnel.
    pub remounted: CancellationToken,
    /// One of the listener's mounted-connection places, held while the tunnel is open.
    pub slot: OwnedSemaphorePermit,
}

/// How a relay stopped.
#[derive(Debug, PartialEq)]
enum End {
    LeaseEnded,
    Stopping,
    Remounted,
    /// A side sent a close frame, passed on to the other.
    ClientClosed(Option<CloseFrame>),
    TargetClosed(Option<CloseFrame>),
    /// A side went away without a close, or broke the protocol.
    ClientGone,
    TargetGone,
    /// The client sent a message over the frame limit.
    TooBig,
}

/// Once the client's side of the upgrade is ready, relay frames until either side ends, the
/// lease ends, the mounts change, or the listener stops.
pub(crate) async fn carry(tunnel: Tunnel, tuning: Tuning, stop: CancellationToken) {
    let client = match tunnel.client.await {
        Ok(client) => TokioIo::new(client),
        Err(error) => {
            return tracing::debug!(%error, "a mounted WebSocket upgrade did not complete");
        }
    };
    let ends = Ends {
        ticket: tunnel.ticket,
        remounted: tunnel.remounted,
        stop,
    };
    relay_frames(client, tunnel.target, &ends, tuning).await;
    drop(tunnel.slot);
}

/// Everything that ends a tunnel from outside it.
pub(crate) struct Ends {
    pub ticket: Ticket,
    pub remounted: CancellationToken,
    pub stop: CancellationToken,
}

pub(crate) async fn relay_frames<C, T>(client: C, target: T, ends: &Ends, tuning: Tuning)
where
    C: AsyncRead + AsyncWrite + Unpin,
    T: AsyncRead + AsyncWrite + Unpin,
{
    let config = WebSocketConfig::default()
        .max_message_size(Some(ws::MAX_MESSAGE))
        .max_frame_size(Some(ws::MAX_MESSAGE));
    let mut client = WebSocketStream::from_raw_socket(client, Role::Server, Some(config)).await;
    let mut target = WebSocketStream::from_raw_socket(target, Role::Client, Some(config)).await;
    let ticket = &ends.ticket;
    let end = tokio::select! {
        biased;
        _ = ticket.ended() => End::LeaseEnded,
        _ = ends.stop.cancelled() => End::Stopping,
        _ = ends.remounted.cancelled() => End::Remounted,
        end = relay(&mut client, &mut target, ticket, &tuning) => end,
    };
    // A side that failed because the lease ended failed for that reason.
    let end = if ticket.live() { end } else { End::LeaseEnded };
    finish(&mut client, &mut target, end, ticket, tuning.close_timeout).await;
}

/// Relay data frames both ways. Ping and Pong belong to each hop: each side's pings are
/// answered here, and the client is pinged so that one gone silent is let go.
async fn relay<C, T>(
    client: &mut WebSocketStream<C>,
    target: &mut WebSocketStream<T>,
    ticket: &Ticket,
    tuning: &Tuning,
) -> End
where
    C: AsyncRead + AsyncWrite + Unpin,
    T: AsyncRead + AsyncWrite + Unpin,
{
    assert!(
        tuning.ping_interval > Duration::ZERO,
        "a ping interval is set"
    );
    let stall = tuning.stall_timeout;
    let mut ping =
        tokio::time::interval_at(Instant::now() + tuning.ping_interval, tuning.ping_interval);
    let mut heard = Instant::now();
    loop {
        tokio::select! {
            _ = ping.tick() => {
                if heard.elapsed() >= tuning.pong_timeout {
                    return End::ClientGone;
                }
                if !pass(client, Message::Ping(Default::default()), stall).await {
                    return End::ClientGone;
                }
            }
            read = client.next() => {
                heard = Instant::now();
                let message = match read {
                    Some(Ok(Message::Close(frame))) => return End::ClientClosed(frame),
                    Some(Ok(message)) => message,
                    Some(Err(WsError::Capacity(_))) => return End::TooBig,
                    Some(Err(_)) | None => return End::ClientGone,
                };
                if hop_only(&message) {
                    continue;
                }
                // The lease is checked frame by frame: its timer may not have fired yet.
                if !ticket.live() {
                    return End::LeaseEnded;
                }
                if !pass(target, message, stall).await {
                    return End::TargetGone;
                }
            }
            read = target.next() => {
                let message = match read {
                    Some(Ok(Message::Close(frame))) => return End::TargetClosed(frame),
                    Some(Ok(message)) => message,
                    Some(Err(_)) | None => return End::TargetGone,
                };
                if hop_only(&message) {
                    continue;
                }
                if !ticket.live() {
                    return End::LeaseEnded;
                }
                if !pass(client, message, stall).await {
                    return End::ClientGone;
                }
            }
        }
    }
}

/// A peer's own Ping and Pong stay on its hop: tungstenite has answered the Ping already.
fn hop_only(message: &Message) -> bool {
    matches!(
        message,
        Message::Ping(_) | Message::Pong(_) | Message::Frame(_)
    )
}

/// Send one message, giving up on a peer that takes none of it for `stall`.
async fn pass<S>(to: &mut WebSocketStream<S>, message: Message, stall: Duration) -> bool
where
    S: AsyncRead + AsyncWrite + Unpin,
{
    matches!(
        tokio::time::timeout(stall, to.send(message)).await,
        Ok(Ok(()))
    )
}

fn closing(code: CloseCode, reason: &'static str) -> Option<CloseFrame> {
    Some(CloseFrame {
        code,
        reason: reason.into(),
    })
}

/// Take leave of both sides as `end` calls for. The target hears nothing without a live lease.
async fn finish<C, T>(
    client: &mut WebSocketStream<C>,
    target: &mut WebSocketStream<T>,
    end: End,
    ticket: &Ticket,
    patience: Duration,
) where
    C: AsyncRead + AsyncWrite + Unpin,
    T: AsyncRead + AsyncWrite + Unpin,
{
    let (to_client, to_target) = match end {
        End::LeaseEnded => (closing(CloseCode::Policy, LEASE_ENDED), None),
        End::Stopping => {
            let going = closing(CloseCode::Away, "the listener is closing");
            (going.clone(), going)
        }
        End::Remounted => {
            let going = closing(CloseCode::Away, "the mount was changed");
            (going.clone(), going)
        }
        End::ClientClosed(frame) => (None, frame.or_else(|| closing(CloseCode::Normal, ""))),
        End::TargetClosed(frame) => (frame.or_else(|| closing(CloseCode::Normal, "")), None),
        End::ClientGone => (None, closing(CloseCode::Away, "the client went away")),
        End::TargetGone => (
            closing(CloseCode::Error, "the mounted daemon went away"),
            None,
        ),
        End::TooBig => (
            closing(CloseCode::Size, "message too big"),
            closing(CloseCode::Away, "the client sent a message too big"),
        ),
    };
    let leaving = async {
        take_leave(client, to_client).await;
        // Even the echo of a close is a frame, and none reaches the target without a lease.
        if ticket.live() {
            take_leave(target, to_target).await;
        }
    };
    if tokio::time::timeout(patience, leaving).await.is_err() {
        tracing::debug!("a mounted WebSocket did not close in time and was dropped");
    }
}

/// Close with `frame`, or with none, flush the echo of a close the peer sent.
async fn take_leave<S>(socket: &mut WebSocketStream<S>, frame: Option<CloseFrame>)
where
    S: AsyncRead + AsyncWrite + Unpin,
{
    let left = match frame {
        Some(frame) => socket.close(Some(frame)).await,
        None => socket.flush().await,
    };
    match left {
        Ok(()) | Err(WsError::ConnectionClosed | WsError::AlreadyClosed) => {}
        Err(error) => tracing::debug!(%error, "a mounted WebSocket could not be closed cleanly"),
    }
}
