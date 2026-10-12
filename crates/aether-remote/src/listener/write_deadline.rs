//! A deadline on writing an HTTP response: the only bound on a peer that asks for a file and
//! never reads it (docs/remote.md, Request checks).

use std::future::Future as _;
use std::io;
use std::pin::Pin;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::task::{Context, Poll};
use std::time::Duration;

use tokio::io::{AsyncRead, AsyncWrite, ReadBuf};
use tokio::time::Sleep;

/// Turned off when the connection becomes a WebSocket, whose pump tells a
/// slow peer from a stalled one itself and closes it with a code.
#[derive(Debug)]
pub(crate) struct Watch(AtomicBool);

impl Watch {
    pub(crate) fn armed() -> Arc<Self> {
        Arc::new(Self(AtomicBool::new(true)))
    }

    pub(crate) fn disarm(&self) {
        self.0.store(false, Ordering::SeqCst);
    }
}

/// A stream whose writes fail once the peer has taken nothing for
/// `patience`. Progress resets the clock, so a slow link is not a stall.
pub(crate) struct WriteDeadline<S> {
    io: S,
    patience: Duration,
    watch: Arc<Watch>,
    /// Running while a write is waiting on the peer.
    stalled: Option<Pin<Box<Sleep>>>,
}

impl<S> WriteDeadline<S> {
    pub(crate) fn new(io: S, patience: Duration, watch: Arc<Watch>) -> Self {
        assert!(!patience.is_zero(), "a write needs some time to be taken");
        Self {
            io,
            patience,
            watch,
            stalled: None,
        }
    }

    /// Pass a write's outcome on, unless it has been pending for too long.
    fn timed<T>(
        &mut self,
        cx: &mut Context<'_>,
        written: Poll<io::Result<T>>,
    ) -> Poll<io::Result<T>> {
        if written.is_ready() || !self.watch.0.load(Ordering::SeqCst) {
            self.stalled = None;
            return written;
        }
        let patience = self.patience;
        let stalled = self
            .stalled
            .get_or_insert_with(|| Box::pin(tokio::time::sleep(patience)));
        match stalled.as_mut().poll(cx) {
            Poll::Pending => Poll::Pending,
            Poll::Ready(()) => {
                let error = io::Error::new(io::ErrorKind::TimedOut, "the peer stopped reading");
                Poll::Ready(Err(error))
            }
        }
    }
}

impl<S: AsyncRead + Unpin> AsyncRead for WriteDeadline<S> {
    fn poll_read(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &mut ReadBuf<'_>,
    ) -> Poll<io::Result<()>> {
        Pin::new(&mut self.io).poll_read(cx, buf)
    }
}

impl<S: AsyncWrite + Unpin> AsyncWrite for WriteDeadline<S> {
    fn poll_write(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &[u8],
    ) -> Poll<io::Result<usize>> {
        let written = Pin::new(&mut self.io).poll_write(cx, buf);
        self.timed(cx, written)
    }

    fn poll_write_vectored(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        bufs: &[io::IoSlice<'_>],
    ) -> Poll<io::Result<usize>> {
        let written = Pin::new(&mut self.io).poll_write_vectored(cx, bufs);
        self.timed(cx, written)
    }

    fn is_write_vectored(&self) -> bool {
        self.io.is_write_vectored()
    }

    fn poll_flush(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        let flushed = Pin::new(&mut self.io).poll_flush(cx);
        self.timed(cx, flushed)
    }

    fn poll_shutdown(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        Pin::new(&mut self.io).poll_shutdown(cx)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::io::{AsyncReadExt as _, AsyncWriteExt as _};

    const PATIENCE: Duration = Duration::from_secs(10);

    #[tokio::test(start_paused = true)]
    async fn a_write_nobody_takes_fails_at_the_deadline() {
        let (ours, _theirs) = tokio::io::duplex(16);
        let mut stream = WriteDeadline::new(ours, PATIENCE, Watch::armed());
        let started = tokio::time::Instant::now();
        let refused = stream.write_all(&[7u8; 64]).await.expect_err("never read");
        assert_eq!(refused.kind(), io::ErrorKind::TimedOut);
        assert_eq!(started.elapsed(), PATIENCE);
    }

    #[tokio::test(start_paused = true)]
    async fn a_slow_reader_that_keeps_reading_is_not_cut_off() {
        let (ours, mut theirs) = tokio::io::duplex(16);
        let mut stream = WriteDeadline::new(ours, PATIENCE, Watch::armed());
        let reading = tokio::spawn(async move {
            let mut taken = 0;
            let mut chunk = [0u8; 16];
            while taken < 64 {
                tokio::time::sleep(PATIENCE - Duration::from_secs(1)).await;
                taken += theirs.read(&mut chunk).await.expect("read");
            }
            taken
        });
        stream.write_all(&[7u8; 64]).await.expect("taken, slowly");
        assert_eq!(reading.await.expect("joined"), 64);
        assert!(stream.stalled.is_none(), "no clock runs with nothing owed");
    }

    #[tokio::test(start_paused = true)]
    async fn a_disarmed_watch_leaves_writes_to_wait() {
        let (ours, _theirs) = tokio::io::duplex(16);
        let watch = Watch::armed();
        let mut stream = WriteDeadline::new(ours, PATIENCE, watch.clone());
        watch.disarm();
        let writing = stream.write_all(&[7u8; 64]);
        let waited = tokio::time::timeout(PATIENCE * 3, writing).await;
        assert!(waited.is_err(), "the write is still pending");
    }
}
