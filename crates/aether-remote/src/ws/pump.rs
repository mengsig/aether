//! Moving frames between an open WebSocket and the daemon's line stream,
//! and saying goodbye in a way the peer can read.

use std::time::Duration;

use aether_protocol::{ErrorCode, ErrorReply, ServerFrame};
use futures::{Sink, SinkExt, Stream, StreamExt};
use tokio::io::{
    AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt, DuplexStream, ReadHalf, WriteHalf,
};
use tokio_tungstenite::WebSocketStream;
use tokio_tungstenite::tungstenite::protocol::CloseFrame;
use tokio_tungstenite::tungstenite::{Error as WsError, Message, error::CapacityError};
use tokio_util::codec::{FramedRead, LinesCodec};
use tokio_util::sync::CancellationToken;

use super::meter::{Meter, Metered};
use super::{MAX_MESSAGE, PRE_AUTH_MAX_MESSAGE, close};
use crate::Tuning;

/// How a connection is closed: the code, and the reply frame that says why.
#[derive(Debug, Clone, PartialEq)]
pub(super) struct Closing {
    pub code: u16,
    /// The close frame's reason; short, for a log.
    pub reason: &'static str,
    /// Sent with id 0 before the close, when the daemon has not said it.
    pub notice: Option<ErrorReply>,
}

impl Closing {
    pub(super) fn quiet(code: u16, reason: &'static str) -> Self {
        Self {
            code,
            reason,
            notice: None,
        }
    }

    fn with_notice(code: u16, error: ErrorCode, reason: &'static str) -> Self {
        Self {
            code,
            reason,
            notice: Some(ErrorReply::new(error, reason)),
        }
    }

    pub(super) fn fault(reason: &'static str) -> Self {
        Self::with_notice(close::INTERNAL, ErrorCode::Internal, reason)
    }
}

/// Why the pump stopped.
#[derive(Debug, Clone, PartialEq)]
pub(super) enum PumpEnd {
    /// The daemon closed its side; how it ended says which code to send.
    DaemonDone,
    /// The peer sent a close frame.
    PeerClosed,
    /// The transport failed; nobody is left to tell.
    PeerGone,
    /// The carrier is ending the connection itself.
    Violation(Closing),
}

/// Carry frames both ways until one side is done, or until `displaced`
/// says the connection's place in the greeting lobby went to a newcomer.
pub(super) async fn run<S>(
    socket: &mut WebSocketStream<Metered<S>>,
    daemon: DuplexStream,
    meter: &Meter,
    displaced: &CancellationToken,
    tuning: &Tuning,
) -> PumpEnd
where
    S: AsyncRead + AsyncWrite + Unpin,
{
    let (from_daemon, to_daemon) = tokio::io::split(daemon);
    let (mut sink, mut stream) = socket.split();
    // Whichever half finishes first decides; dropping the other here drops
    // the carrier's end of the daemon stream, so the daemon sees it close.
    tokio::select! {
        biased;
        _ = meter.revoked() => PumpEnd::Violation(Closing::with_notice(close::REVOKED, ErrorCode::Forbidden, "this device was revoked or its role changed")),
        end = inbound(&mut stream, to_daemon, meter) => end,
        end = outbound(&mut sink, from_daemon, meter, tuning) => end,
        end = crowded_out(displaced, meter) => end,
    }
}

/// Ends a connection that lost its place before its hello was accepted.
async fn crowded_out(displaced: &CancellationToken, meter: &Meter) -> PumpEnd {
    displaced.cancelled().await;
    if meter.is_authenticated() {
        // Its place was a session slot by then, which nobody can take.
        return std::future::pending().await;
    }
    // Like any peer that never greeted, it is owed no reply frame.
    PumpEnd::Violation(Closing::quiet(
        close::TIMEOUT,
        "no hello was accepted before another connection needed the place",
    ))
}

/// Peer to daemon: one text message becomes one line.
async fn inbound<St>(stream: &mut St, mut daemon: WriteHalf<DuplexStream>, meter: &Meter) -> PumpEnd
where
    St: Stream<Item = Result<Message, WsError>> + Unpin,
{
    loop {
        let message = match stream.next().await {
            None => return PumpEnd::PeerGone,
            Some(Err(error)) => return read_failure(error, meter),
            Some(Ok(message)) => message,
        };
        meter.mark_heard();
        let text = match message {
            Message::Text(text) => text,
            Message::Binary(_) => {
                return PumpEnd::Violation(Closing::with_notice(
                    close::UNSUPPORTED_DATA,
                    ErrorCode::InvalidArgument,
                    "this carrier is text only: one JSON frame per text message",
                ));
            }
            Message::Close(_) => return PumpEnd::PeerClosed,
            Message::Ping(_) | Message::Pong(_) | Message::Frame(_) => continue,
        };
        let line = text.strip_suffix('\n').unwrap_or(&text);
        if let Some(violation) = refuse_line(line, meter) {
            return PumpEnd::Violation(violation);
        }
        let written = async {
            daemon.write_all(line.as_bytes()).await?;
            daemon.write_all(b"\n").await
        };
        if written.await.is_err() {
            // The daemon stopped reading. The outbound half still holds what
            // it said last, and reports the end once that is delivered.
            return std::future::pending().await;
        }
    }
}

/// What is wrong with a line the daemon must not be handed.
fn refuse_line(line: &str, meter: &Meter) -> Option<Closing> {
    if !meter.is_authenticated() && line.len() > PRE_AUTH_MAX_MESSAGE {
        return Some(too_big());
    }
    if line.contains('\n') {
        return Some(Closing::with_notice(
            close::UNSUPPORTED_DATA,
            ErrorCode::InvalidArgument,
            "a message is one compact JSON frame with no newline inside it",
        ));
    }
    None
}

fn too_big() -> Closing {
    Closing::with_notice(
        close::TOO_BIG,
        ErrorCode::InvalidArgument,
        "the message exceeds the size limit: 64 KiB before the hello is accepted, 64 MiB after",
    )
}

fn read_failure(error: WsError, meter: &Meter) -> PumpEnd {
    match error {
        WsError::Capacity(CapacityError::MessageTooLong { .. }) => PumpEnd::Violation(too_big()),
        WsError::Io(_) if meter.invalid() => PumpEnd::Violation(Closing::quiet(
            close::PROTOCOL,
            "invalid WebSocket frame header",
        )),
        WsError::Io(_) if meter.overran() => PumpEnd::Violation(too_big()),
        WsError::Utf8(_) => PumpEnd::Violation(Closing::with_notice(
            close::UNSUPPORTED_DATA,
            ErrorCode::InvalidArgument,
            "a text message must be UTF-8",
        )),
        other => {
            tracing::debug!(error = %other, "a device connection failed");
            PumpEnd::PeerGone
        }
    }
}

/// Daemon to peer: one line becomes one text message, with a ping whenever
/// the interval passes.
async fn outbound<Si>(
    sink: &mut Si,
    daemon: ReadHalf<DuplexStream>,
    meter: &Meter,
    tuning: &Tuning,
) -> PumpEnd
where
    Si: Sink<Message, Error = WsError> + Unpin,
{
    let mut lines = FramedRead::new(daemon, LinesCodec::new_with_max_length(MAX_MESSAGE));
    let first_ping = tokio::time::Instant::now() + tuning.ping_interval;
    let mut ping = tokio::time::interval_at(first_ping, tuning.ping_interval);
    ping.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
    loop {
        let message = tokio::select! {
            line = lines.next() => match line {
                None => return PumpEnd::DaemonDone,
                Some(Ok(line)) => Message::text(line),
                Some(Err(error)) => {
                    tracing::error!(%error, "could not read a frame from the daemon");
                    return PumpEnd::Violation(Closing::fault("the daemon sent a frame the carrier cannot relay"));
                }
            },
            _ = ping.tick() => {
                if meter.since_heard() >= tuning.pong_timeout {
                    return PumpEnd::Violation(Closing::with_notice(
                        close::TIMEOUT,
                        ErrorCode::Unavailable,
                        "closed after keepalive pings went unanswered",
                    ));
                }
                Message::Ping(Default::default())
            }
        };
        if let Err(end) = send(sink, message, meter, tuning.stall_timeout).await {
            return end;
        }
    }
}

/// Send one message, giving up on a peer that takes nothing for `stall`.
/// Progress is what counts, so a large frame on a slow link is not a stall.
async fn send<Si>(
    sink: &mut Si,
    message: Message,
    meter: &Meter,
    stall: Duration,
) -> Result<(), PumpEnd>
where
    Si: Sink<Message, Error = WsError> + Unpin,
{
    meter.mark_write_progress();
    meter
        .pending_write
        .store(true, std::sync::atomic::Ordering::SeqCst);
    let sending = sink.send(message);
    tokio::pin!(sending);
    loop {
        let patience = stall.saturating_sub(meter.since_write_progress());
        tokio::select! {
            sent = &mut sending => {
                if sent.is_ok() {
                    meter.pending_write.store(false, std::sync::atomic::Ordering::SeqCst);
                }
                return sent.map_err(|error| {
                    tracing::debug!(%error, "a device connection stopped taking frames");
                    PumpEnd::PeerGone
                });
            },
            _ = tokio::time::sleep(patience) => {}
        }
        if meter.since_write_progress() >= stall {
            return Err(PumpEnd::Violation(Closing::with_notice(
                close::SLOW,
                ErrorCode::Unavailable,
                "closed for not keeping up with what the connection subscribed to",
            )));
        }
    }
}

/// How a connection is taken leave of.
#[derive(Debug)]
pub(super) enum Farewell {
    /// The peer is gone; nobody is left to tell.
    Nothing,
    /// The peer closed first; its close frame is answered.
    Echo,
    /// The carrier closes, saying why.
    Close(Closing),
}

/// End the connection as `farewell` says, within `timeout`.
pub(super) async fn finish<S>(
    socket: &mut WebSocketStream<Metered<S>>,
    farewell: Farewell,
    timeout: Duration,
) where
    S: AsyncRead + AsyncWrite + Unpin,
{
    if socket.get_ref().revoked_with_pending_write() {
        return;
    }
    if socket.get_ref().is_revoked() {
        socket.get_mut().allow_revoked_close();
    }
    let leaving = async {
        match &farewell {
            Farewell::Nothing => Ok(()),
            // Reading the peer's close queued the answer; this sends it.
            Farewell::Echo => socket.flush().await,
            Farewell::Close(closing) => close(socket, closing).await,
        }
    };
    match tokio::time::timeout(timeout, leaving).await {
        Ok(Ok(())) | Ok(Err(WsError::ConnectionClosed)) => {}
        Ok(Err(error)) => tracing::debug!(%error, ?farewell, "a close was cut short"),
        Err(_) => tracing::debug!(?farewell, "the peer did not finish closing in time"),
    }
}

/// Close from this side: the reason as a reply frame with id 0, the close
/// frame, then whatever the peer still sends until it closes too.
async fn close<S>(
    socket: &mut WebSocketStream<Metered<S>>,
    closing: &Closing,
) -> Result<(), WsError>
where
    S: AsyncRead + AsyncWrite + Unpin,
{
    if let Some(notice) = closing.notice.clone() {
        let frame = ServerFrame::Reply {
            id: 0,
            result: Err(notice),
        };
        match serde_json::to_string(&frame) {
            Ok(json) => socket.send(Message::text(json)).await?,
            Err(error) => tracing::error!(%error, "could not encode a closing notice"),
        }
    }
    let frame = CloseFrame {
        code: closing.code.into(),
        reason: closing.reason.into(),
    };
    socket.close(Some(frame)).await?;
    // Unread input when a socket is dropped resets it, which can cost the
    // peer the frames just sent. Read to its close frame instead.
    while let Some(Ok(message)) = socket.next().await {
        if message.is_close() {
            return Ok(());
        }
    }
    // The frame reader gave up, as it does after a violation: discard raw.
    let mut discarded = [0u8; 4096];
    while socket.get_mut().raw().read(&mut discarded).await? > 0 {}
    Ok(())
}
