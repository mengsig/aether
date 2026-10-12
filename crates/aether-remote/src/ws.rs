//! The WebSocket carrier: the daemon protocol, unchanged, with one JSON
//! frame per text message instead of one per line.

pub(crate) mod meter;
mod pump;

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use aether_protocol::remote::DeviceProof;
use aether_protocol::{ErrorCode, ErrorReply};
use base64::Engine as _;
use hyper::HeaderMap;
use hyper::header::{
    CONNECTION, HeaderName, SEC_WEBSOCKET_KEY, SEC_WEBSOCKET_PROTOCOL, SEC_WEBSOCKET_VERSION,
    UPGRADE,
};
use tokio::io::{AsyncRead, AsyncWrite};
use tokio::sync::{OwnedSemaphorePermit, Semaphore};
use tokio_tungstenite::WebSocketStream;
use tokio_tungstenite::tungstenite::protocol::{Role, WebSocketConfig};
use tokio_util::sync::CancellationToken;

use crate::auth::{AuthError, Challenge, DeviceAuthenticator, DeviceGate, DeviceSession};
use crate::connections::Registered;
use crate::lobby::Seat;
use crate::{Ended, RemoteHost, Tuning};
use meter::{Meter, Metered};
use pump::{Closing, Farewell, PumpEnd};

/// The subprotocol a client must offer and the daemon selects.
pub const SUBPROTOCOL: &str = "aether";

/// Largest message accepted before the hello is authorized.
pub const PRE_AUTH_MAX_MESSAGE: usize = 64 * 1024;
/// Largest message accepted afterwards; the socket's own frame cap.
pub const MAX_MESSAGE: usize = 64 * 1024 * 1024;
/// How often the daemon pings a connection that is otherwise quiet.
pub const PING_INTERVAL_SECS: u64 = 30;
/// A peer heard from for this long is taken for gone: three pings unanswered.
pub const PONG_TIMEOUT_SECS: u64 = 90;
/// A peer that takes no bytes for this long while owed some is too slow.
pub const STALL_TIMEOUT_SECS: u64 = 30;
/// The only WebSocket version there is.
const VERSION: &str = "13";
/// Bytes in the daemon-to-carrier pipe; the framed outbound message and
/// the WebSocket write buffer are additional.
const OUTBOUND_QUEUE_BYTES: usize = 256 * 1024;
/// What the socket reads at a time; small, since most frames are.
const READ_BUFFER_BYTES: usize = 16 * 1024;

/// Close codes the daemon sends. See docs/remote.md.
pub mod close {
    pub const NORMAL: u16 = 1000;
    pub const GOING_AWAY: u16 = 1001;
    /// A binary message: the carrier is text only.
    pub const UNSUPPORTED_DATA: u16 = 1003;
    pub const PROTOCOL: u16 = 1002;
    pub const TOO_BIG: u16 = 1009;
    /// A daemon fault ended the connection.
    pub const INTERNAL: u16 = 1011;
    /// The peer was not reading what it subscribed to.
    pub const SLOW: u16 = 1013;
    /// The hello was refused.
    pub const UNAUTHORIZED: u16 = 4401;
    /// The device was revoked or its role changed while connected.
    pub const REVOKED: u16 = 4403;
    /// No accepted hello in time, or idle past the timeout.
    pub const TIMEOUT: u16 = 4408;
    /// The listener was disabled, or the connection's origin removed.
    pub const DISABLED: u16 = 4503;

    /// The code for a connection the daemon finished serving.
    pub fn for_ended(ended: crate::Ended) -> u16 {
        match ended {
            crate::Ended::Closed => NORMAL,
            crate::Ended::Refused => UNAUTHORIZED,
            crate::Ended::Revoked => REVOKED,
            crate::Ended::TimedOut => TIMEOUT,
            crate::Ended::ShuttingDown => GOING_AWAY,
        }
    }
}

/// The `Sec-WebSocket-Accept` value answering a client's key.
pub fn accept_key(key: &[u8]) -> String {
    tokio_tungstenite::tungstenite::handshake::derive_accept_key(key)
}

/// The comma-separated tokens of every `name` header.
fn tokens<'a>(headers: &'a HeaderMap, name: &HeaderName) -> impl Iterator<Item = &'a str> {
    headers
        .get_all(name)
        .into_iter()
        .filter_map(|value| value.to_str().ok())
        .flat_map(|value| value.split(','))
        .map(str::trim)
}

/// Check an upgrade request and answer its key, or say what is wrong.
pub(crate) fn accept(headers: &HeaderMap) -> Result<String, &'static str> {
    let upgrading = tokens(headers, &CONNECTION).any(|t| t.eq_ignore_ascii_case("upgrade"))
        && tokens(headers, &UPGRADE).any(|t| t.eq_ignore_ascii_case("websocket"));
    if !upgrading {
        return Err("`/ws` answers only a WebSocket upgrade");
    }
    if !tokens(headers, &SEC_WEBSOCKET_VERSION).eq([VERSION]) {
        return Err("only WebSocket version 13 is served");
    }
    let mut keys = headers.get_all(SEC_WEBSOCKET_KEY).into_iter();
    let (Some(key), None) = (keys.next(), keys.next()) else {
        return Err("exactly one Sec-WebSocket-Key is required");
    };
    let nonce = base64::engine::general_purpose::STANDARD.decode(key.as_bytes());
    if nonce.is_ok_and(|nonce| nonce.len() == 16) {
        // Subprotocol names are case-sensitive tokens.
        if tokens(headers, &SEC_WEBSOCKET_PROTOCOL).any(|t| t == SUBPROTOCOL) {
            return Ok(accept_key(key.as_bytes()));
        }
        return Err("the `aether` subprotocol must be offered in Sec-WebSocket-Protocol");
    }
    Err("Sec-WebSocket-Key must be 16 bytes of base64")
}

/// What one carried connection needs from the server that accepted it.
pub(crate) struct Link {
    pub host: Arc<dyn RemoteHost>,
    pub authenticator: Arc<dyn DeviceAuthenticator>,
    pub registered: Registered,
    /// Where the connection waits until its hello is accepted.
    pub seat: Seat,
    /// What it holds from then on: one of the device sessions.
    pub session_slots: Arc<Semaphore>,
    /// The allowlisted `Origin` of the upgrade request.
    pub origin: String,
    /// Ends this connection: the listener closing, or its origin removed.
    pub stop: CancellationToken,
    /// Fires only when the whole daemon is stopping.
    pub daemon_stopping: CancellationToken,
    pub tuning: Tuning,
}

/// What a connection holds: a seat in the greeting lobby, traded for a
/// session slot when its hello is accepted.
#[derive(Debug)]
struct Standing {
    seat: Option<Seat>,
    session: Option<OwnedSemaphorePermit>,
}

/// One connection's use of the authenticator: answers a single hello, for
/// its own challenge, and tells the carrier when that hello was accepted.
#[derive(Debug)]
struct Admission {
    inner: Arc<dyn DeviceAuthenticator>,
    nonce: String,
    spent: AtomicBool,
    meter: Arc<Meter>,
    registered: Registered,
    session_slots: Arc<Semaphore>,
    standing: parking_lot::Mutex<Standing>,
}

impl DeviceAuthenticator for Admission {
    fn authenticate(
        &self,
        challenge: &Challenge,
        proof: &DeviceProof,
    ) -> Result<DeviceSession, AuthError> {
        let ours = challenge.nonce == self.nonce;
        if !ours || self.spent.swap(true, Ordering::SeqCst) {
            return Err(AuthError::ChallengeSpent);
        }
        // Reserved first, so no hello is recorded as accepted and then dropped.
        let slot = Arc::clone(&self.session_slots)
            .try_acquire_owned()
            .map_err(|_| AuthError::Busy)?;
        let session = self.inner.authenticate(challenge, proof)?;
        self.registered
            .identify(session.identity.id, session.closed.clone());
        self.meter.bind_session(session.closed.clone());
        self.meter.authenticate();
        let mut standing = self.standing.lock();
        debug_assert!(standing.session.is_none(), "one hello per connection");
        standing.session = Some(slot);
        // Leaving the lobby: from here no newcomer can take this place.
        drop(standing.seat.take());
        Ok(session)
    }
}

/// Serve the daemon protocol on one upgraded connection until it ends, then
/// close it with the code docs/remote.md gives the reason.
pub(crate) async fn carry<S>(io: S, link: Link)
where
    S: AsyncRead + AsyncWrite + Send + Unpin + 'static,
{
    let meter = Meter::new();
    let config = WebSocketConfig::default()
        .max_message_size(Some(MAX_MESSAGE))
        .max_frame_size(Some(MAX_MESSAGE))
        .read_buffer_size(READ_BUFFER_BYTES);
    let metered = Metered::new(io, meter.clone(), PRE_AUTH_MAX_MESSAGE);
    let mut socket = WebSocketStream::from_raw_socket(metered, Role::Server, Some(config)).await;
    let timeout = link.tuning.close_timeout;
    let challenge = match Challenge::issue(link.host.instance_id(), &link.origin) {
        Ok(challenge) => challenge,
        Err(error) => {
            tracing::error!(%error, "could not issue a device challenge");
            let closing = Closing::fault("the daemon could not issue a challenge");
            return pump::finish(&mut socket, Farewell::Close(closing), timeout).await;
        }
    };
    let displaced = link.seat.displaced().clone();
    let gate = DeviceGate {
        authenticator: Arc::new(Admission {
            inner: link.authenticator.clone(),
            nonce: challenge.nonce.clone(),
            spent: AtomicBool::new(false),
            meter: meter.clone(),
            registered: link.registered,
            session_slots: link.session_slots,
            standing: parking_lot::Mutex::new(Standing {
                seat: Some(link.seat),
                session: None,
            }),
        }),
        challenge,
    };
    let (ours, theirs) = tokio::io::duplex(OUTBOUND_QUEUE_BYTES);
    // Joined, not raced: the pump ending drops its end of the stream, which
    // ends the daemon's side; the daemon ending is the pump's end of input.
    let (served, pumped) = tokio::join!(
        link.host.serve(theirs, gate, link.stop.clone()),
        pump::run(&mut socket, ours, &meter, &displaced, &link.tuning),
    );
    let farewell = farewell(served, pumped, &link.daemon_stopping);
    pump::finish(&mut socket, farewell, timeout).await;
}

/// How to take leave, given how the daemon and the pump each finished.
fn farewell(
    served: anyhow::Result<Ended>,
    pumped: PumpEnd,
    daemon_stopping: &CancellationToken,
) -> Farewell {
    let ended = match (pumped, served) {
        (PumpEnd::Violation(closing), _) => return Farewell::Close(closing),
        (PumpEnd::PeerGone, _) => return Farewell::Nothing,
        (PumpEnd::PeerClosed, _) => return Farewell::Echo,
        (PumpEnd::DaemonDone, Ok(ended)) => ended,
        (PumpEnd::DaemonDone, Err(error)) => {
            tracing::error!(error = %format!("{error:#}"), "serving a device connection failed");
            let closing = Closing::fault("the daemon could not serve this connection");
            return Farewell::Close(closing);
        }
    };
    if ended != Ended::ShuttingDown {
        // The daemon already said why, in a reply frame of its own.
        return Farewell::Close(Closing::quiet(close::for_ended(ended), ""));
    }
    let (code, why) = match daemon_stopping.is_cancelled() {
        true => (close::GOING_AWAY, "the daemon is shutting down"),
        false => (
            close::DISABLED,
            "remote access was disabled, or this origin was removed",
        ),
    };
    Farewell::Close(Closing {
        code,
        reason: why,
        notice: Some(ErrorReply::new(ErrorCode::Unavailable, why)),
    })
}

#[cfg(test)]
mod tests;
