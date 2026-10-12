//! Byte accounting under the WebSocket: what the socket library cannot
//! bound for us, and what tells a slow peer from a busy one.
//!
//! Headers are screened before the library can reserve their payloads.
//! Data budgets depend on admission; protocol validity never does.

use std::io;
use std::pin::Pin;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::task::{Context, Poll};
use std::time::{Duration, Instant};

use tokio::io::{AsyncRead, AsyncWrite, ReadBuf};

/// Longest WebSocket frame header: two bytes, a 64-bit length and a mask.
const MAX_HEADER: usize = 14;
/// Opcodes from here up are control frames, which carry no message data.
const FIRST_CONTROL_OPCODE: u8 = 0x8;

/// Follows frame boundaries in a client's byte stream and adds up each
/// message, so an oversized one is refused at its header.
#[derive(Debug, Default)]
struct FrameScreen {
    header: [u8; MAX_HEADER],
    /// Header bytes collected so far.
    have: usize,
    /// Payload bytes of the current frame still to come.
    payload_left: u64,
    /// Data bytes declared so far by the message being assembled.
    message: u64,
}

/// A header that cannot be handed to the allocating frame parser.
#[derive(Debug, PartialEq, Eq)]
enum FrameError {
    TooLong,
    Invalid,
}

impl FrameScreen {
    fn feed(&mut self, mut bytes: &[u8], limit: u64) -> Result<(), FrameError> {
        debug_assert!(limit > 0, "the admission budget permits a greeting");
        while !bytes.is_empty() {
            debug_assert!(
                self.have < MAX_HEADER,
                "a complete header must have been consumed"
            );
            if self.payload_left > 0 {
                let skipped = self.payload_left.min(bytes.len() as u64);
                self.payload_left -= skipped;
                bytes = &bytes[skipped as usize..];
                continue;
            }
            self.header[self.have] = bytes[0];
            self.have += 1;
            bytes = &bytes[1..];
            if let Some(frame) = Frame::parse(&self.header[..self.have]) {
                self.have = 0;
                self.begin(frame, limit)?;
            }
        }
        Ok(())
    }

    fn begin(&mut self, frame: Frame, limit: u64) -> Result<(), FrameError> {
        debug_assert_eq!(self.payload_left, 0, "one frame at a time");
        debug_assert_eq!(self.have, 0, "the header is complete");
        if frame.reserved || !matches!(frame.opcode, 0x0..=0x2 | 0x8..=0xa) {
            return Err(FrameError::Invalid);
        }
        if frame.opcode >= FIRST_CONTROL_OPCODE && (!frame.last || frame.length > 125) {
            return Err(FrameError::Invalid);
        }
        self.payload_left = frame.length;
        if frame.opcode >= FIRST_CONTROL_OPCODE {
            return Ok(());
        }
        self.message = self.message.saturating_add(frame.length);
        if self.message > limit {
            return Err(FrameError::TooLong);
        }
        if frame.last {
            self.message = 0;
        }
        Ok(())
    }
}

#[derive(Debug, PartialEq, Eq)]
struct Frame {
    reserved: bool,
    last: bool,
    opcode: u8,
    length: u64,
}

impl Frame {
    /// The frame a complete header announces; `None` while bytes are missing.
    fn parse(header: &[u8]) -> Option<Frame> {
        let (&first, &second) = (header.first()?, header.get(1)?);
        let extended = match second & 0x7f {
            126 => 2,
            127 => 8,
            _ => 0,
        };
        let mask = if second & 0x80 == 0 { 0 } else { 4 };
        if header.len() < 2 + extended + mask {
            return None;
        }
        debug_assert_eq!(header.len(), 2 + extended + mask, "fed a byte at a time");
        let length = match extended {
            0 => u64::from(second & 0x7f),
            _ => header[2..2 + extended]
                .iter()
                .fold(0u64, |length, byte| (length << 8) | u64::from(*byte)),
        };
        Some(Frame {
            reserved: first & 0x70 != 0,
            last: first & 0x80 != 0,
            opcode: first & 0x0f,
            length,
        })
    }
}

/// Shared between the socket's I/O and the task pumping it.
#[derive(Debug)]
pub(crate) struct Meter {
    started: Instant,
    /// Once set, inbound data uses the authenticated budget.
    authenticated: AtomicBool,
    /// Set when a message outgrew its admission budget.
    overran: AtomicBool,
    invalid: AtomicBool,
    pub(crate) pending_write: AtomicBool,
    session: tokio::sync::watch::Sender<Option<tokio_util::sync::CancellationToken>>,
    /// Milliseconds after `started` when the peer last took bytes from us.
    last_write_ms: AtomicU64,
    /// Milliseconds after `started` when the peer last sent a message.
    last_heard_ms: AtomicU64,
}

impl Meter {
    pub(crate) fn new() -> Arc<Self> {
        Arc::new(Self {
            started: Instant::now(),
            authenticated: AtomicBool::new(false),
            overran: AtomicBool::new(false),
            invalid: AtomicBool::new(false),
            pending_write: AtomicBool::new(false),
            session: tokio::sync::watch::channel(None).0,
            last_write_ms: AtomicU64::new(0),
            last_heard_ms: AtomicU64::new(0),
        })
    }

    pub(crate) fn authenticate(&self) {
        self.authenticated.store(true, Ordering::SeqCst);
    }

    pub(crate) fn bind_session(&self, closed: tokio_util::sync::CancellationToken) {
        assert!(
            self.session.borrow().is_none(),
            "one admitted device per socket"
        );
        assert!(
            !self.is_authenticated(),
            "bind cancellation before admission"
        );
        self.session.send_replace(Some(closed));
    }

    pub(crate) fn is_revoked(&self) -> bool {
        self.session
            .borrow()
            .as_ref()
            .is_some_and(|closed| closed.is_cancelled())
    }

    pub(crate) async fn revoked(&self) {
        let mut session = self.session.subscribe();
        loop {
            let closed = session.borrow_and_update().clone();
            if let Some(closed) = closed {
                closed.cancelled().await;
                return;
            }
            assert!(session.changed().await.is_ok(), "the meter owns the sender");
        }
    }

    pub(crate) fn invalid(&self) -> bool {
        self.invalid.load(Ordering::SeqCst)
    }

    pub(crate) fn is_authenticated(&self) -> bool {
        self.authenticated.load(Ordering::SeqCst)
    }

    pub(crate) fn overran(&self) -> bool {
        self.overran.load(Ordering::SeqCst)
    }

    fn now_ms(&self) -> u64 {
        u64::try_from(self.started.elapsed().as_millis()).unwrap_or(u64::MAX)
    }

    fn since(&self, stamp: &AtomicU64) -> Duration {
        let last = Duration::from_millis(stamp.load(Ordering::SeqCst));
        self.started.elapsed().saturating_sub(last)
    }

    /// How long since the peer last accepted any bytes.
    pub(crate) fn since_write_progress(&self) -> Duration {
        self.since(&self.last_write_ms)
    }

    /// Start the stall clock: the peer is owed bytes from now.
    pub(crate) fn mark_write_progress(&self) {
        self.last_write_ms
            .fetch_max(self.now_ms(), Ordering::SeqCst);
    }

    /// How long since the peer last sent anything, a pong included.
    pub(crate) fn since_heard(&self) -> Duration {
        self.since(&self.last_heard_ms)
    }

    pub(crate) fn mark_heard(&self) {
        self.last_heard_ms
            .fetch_max(self.now_ms(), Ordering::SeqCst);
    }
}

/// Screens frame headers at both admission budgets and stops revoked writes.
#[derive(Debug)]
pub(crate) struct Metered<S> {
    inner: S,
    meter: Arc<Meter>,
    screen: FrameScreen,
    limit: u64,
    closing: bool,
}

impl<S> Metered<S> {
    pub(crate) fn new(inner: S, meter: Arc<Meter>, pre_auth_limit: usize) -> Self {
        assert!(pre_auth_limit > 0, "a peer must be able to send its hello");
        assert!(
            pre_auth_limit <= super::PRE_AUTH_MAX_MESSAGE,
            "greeting buffers stay independent of the authenticated cap"
        );
        Self {
            inner,
            meter,
            screen: FrameScreen::default(),
            limit: pre_auth_limit as u64,
            closing: false,
        }
    }

    pub(crate) fn is_revoked(&self) -> bool {
        self.meter.is_revoked()
    }

    pub(crate) fn revoked_with_pending_write(&self) -> bool {
        self.meter.is_revoked() && self.meter.pending_write.load(Ordering::SeqCst)
    }

    pub(crate) fn allow_revoked_close(&mut self) {
        assert!(
            self.meter.is_revoked(),
            "only a revoked socket bypasses the write gate"
        );
        assert!(
            !self.meter.pending_write.load(Ordering::SeqCst),
            "no data may be flushed on revocation"
        );
        self.closing = true;
    }

    /// The stream underneath, for discarding what a closed peer still sends.
    pub(crate) fn raw(&mut self) -> &mut S {
        &mut self.inner
    }
}

impl<S: AsyncRead + Unpin> AsyncRead for Metered<S> {
    fn poll_read(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &mut ReadBuf<'_>,
    ) -> Poll<io::Result<()>> {
        let this = self.get_mut();
        if this.meter.overran() || this.meter.invalid() {
            return Poll::Ready(Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "WebSocket frame screen already refused this stream",
            )));
        }
        let before = buf.filled().len();
        let polled = Pin::new(&mut this.inner).poll_read(cx, buf);
        let read = matches!(polled, Poll::Ready(Ok(())));
        if !read {
            return polled;
        }
        let limit = if this.meter.is_authenticated() {
            super::MAX_MESSAGE as u64
        } else {
            this.limit
        };
        if let Err(error) = this.screen.feed(&buf.filled()[before..], limit) {
            match error {
                FrameError::TooLong => this.meter.overran.store(true, Ordering::SeqCst),
                FrameError::Invalid => this.meter.invalid.store(true, Ordering::SeqCst),
            }
            return Poll::Ready(Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "WebSocket frame violates the protocol or admission byte budget",
            )));
        }
        polled
    }
}

impl<S: AsyncWrite + Unpin> AsyncWrite for Metered<S> {
    fn poll_write(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &[u8],
    ) -> Poll<io::Result<usize>> {
        if self.meter.is_revoked() && !self.closing {
            return Poll::Ready(Err(io::Error::new(
                io::ErrorKind::PermissionDenied,
                "device session revoked",
            )));
        }
        let polled = Pin::new(&mut self.inner).poll_write(cx, buf);
        if let Poll::Ready(Ok(written)) = polled
            && written > 0
        {
            self.meter.mark_write_progress();
        }
        polled
    }

    fn poll_flush(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        if self.meter.is_revoked() && !self.closing {
            return Poll::Ready(Err(io::Error::new(
                io::ErrorKind::PermissionDenied,
                "device session revoked",
            )));
        }
        Pin::new(&mut self.inner).poll_flush(cx)
    }

    fn poll_shutdown(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        Pin::new(&mut self.inner).poll_shutdown(cx)
    }
}

#[cfg(test)]
mod tests;
