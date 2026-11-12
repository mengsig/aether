//! The target's socket, checked against the lease on every read and write. Cancellation
//! ends a relay soon after the lease does; this makes "nothing after" hold to the byte.

use std::io;
use std::pin::Pin;
use std::task::{Context, Poll};

use tokio::io::{AsyncRead, AsyncWrite, ReadBuf};

use super::lease::Standing;

/// A stream to a mounted target that fails every read and write once its lease is over.
#[derive(Debug)]
pub(crate) struct Leased<S> {
    inner: S,
    standing: Standing,
}

impl<S> Leased<S> {
    pub(crate) fn new(inner: S, standing: Standing) -> Self {
        Self { inner, standing }
    }

    fn check(&self) -> io::Result<()> {
        match self.standing.live() {
            true => Ok(()),
            false => Err(io::Error::new(
                io::ErrorKind::BrokenPipe,
                "the mount lease ended",
            )),
        }
    }
}

impl<S: AsyncRead + Unpin> AsyncRead for Leased<S> {
    fn poll_read(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &mut ReadBuf<'_>,
    ) -> Poll<io::Result<()>> {
        self.check()?;
        Pin::new(&mut self.inner).poll_read(cx, buf)
    }
}

impl<S: AsyncWrite + Unpin> AsyncWrite for Leased<S> {
    fn poll_write(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &[u8],
    ) -> Poll<io::Result<usize>> {
        self.check()?;
        Pin::new(&mut self.inner).poll_write(cx, buf)
    }

    fn poll_write_vectored(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        bufs: &[io::IoSlice<'_>],
    ) -> Poll<io::Result<usize>> {
        self.check()?;
        Pin::new(&mut self.inner).poll_write_vectored(cx, bufs)
    }

    fn is_write_vectored(&self) -> bool {
        self.inner.is_write_vectored()
    }

    fn poll_flush(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        self.check()?;
        Pin::new(&mut self.inner).poll_flush(cx)
    }

    /// A shutdown sends no data, so it is allowed after the lease: it only hangs up.
    fn poll_shutdown(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        Pin::new(&mut self.inner).poll_shutdown(cx)
    }
}
