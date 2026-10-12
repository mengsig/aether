//! Byte transports under the client: a Unix socket speaking NDJSON, or an
//! in-memory channel pair for tests and embedded use.

use std::pin::Pin;

use aether_protocol::{ClientFrame, ServerFrame, decode, encode};
use futures::{Stream, StreamExt};
use tokio::io::{AsyncRead, AsyncWrite, AsyncWriteExt};
use tokio::net::{TcpStream, UnixStream};
use tokio::sync::mpsc;
use tokio_util::codec::{FramedRead, LinesCodec};

use crate::ClientError;

/// Longest accepted frame; protects against a runaway peer.
const MAX_LINE: usize = 64 * 1024 * 1024;

/// The other end of an in-memory transport, held by a fake or embedded daemon.
#[derive(Debug)]
pub struct MemoryPeer {
    /// Frames the client sent.
    pub incoming: mpsc::Receiver<ClientFrame>,
    /// Frames to deliver to the client.
    pub outgoing: mpsc::Sender<ServerFrame>,
}

/// A connection to a daemon, before it is split into reader and writer.
pub enum Transport {
    Unix(UnixStream),
    Stdio {
        read: tokio::process::ChildStdout,
        write: tokio::process::ChildStdin,
    },
    Tcp(TcpStream),
    Memory {
        incoming: mpsc::Receiver<ServerFrame>,
        outgoing: mpsc::Sender<ClientFrame>,
    },
}

impl std::fmt::Debug for Transport {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Stdio { .. } => f.write_str("Transport::Stdio"),
            Self::Unix(_) => f.write_str("Transport::Unix"),
            Self::Tcp(_) => f.write_str("Transport::Tcp"),
            Self::Memory { .. } => f.write_str("Transport::Memory"),
        }
    }
}

/// Incoming frames, already decoded.
pub type FrameStream = Pin<Box<dyn Stream<Item = Result<ServerFrame, ClientError>> + Send>>;

/// Outgoing frames. The writer task owns the socket half.
pub type FrameSink = mpsc::Sender<ClientFrame>;

impl Transport {
    /// Build a connected in-memory pair.
    pub fn memory(capacity: usize) -> (Transport, MemoryPeer) {
        let (client_tx, client_rx) = mpsc::channel(capacity);
        let (server_tx, server_rx) = mpsc::channel(capacity);
        (
            Transport::Memory {
                incoming: server_rx,
                outgoing: client_tx,
            },
            MemoryPeer {
                incoming: client_rx,
                outgoing: server_tx,
            },
        )
    }

    /// Split into a decoded frame stream and a frame sink. Spawns the writer
    /// task; it ends when the sink is dropped or the peer goes away.
    pub fn split(self) -> (FrameStream, FrameSink) {
        match self {
            Transport::Stdio { read, write } => split_stream(read, write),
            Transport::Unix(stream) => {
                let (read, write) = stream.into_split();
                split_stream(read, write)
            }
            Transport::Tcp(stream) => {
                let (read, write) = stream.into_split();
                split_stream(read, write)
            }
            Transport::Memory { incoming, outgoing } => {
                let reader = tokio_stream_from(incoming).map(Ok);
                (Box::pin(reader), outgoing)
            }
        }
    }
}

/// Decode inbound lines, and spawn the one task that owns the write half.
/// It ends when the sink is dropped or the peer goes away.
fn split_stream<R, W>(read: R, mut write: W) -> (FrameStream, FrameSink)
where
    R: AsyncRead + Send + Unpin + 'static,
    W: AsyncWrite + Send + Unpin + 'static,
{
    let reader =
        FramedRead::new(read, LinesCodec::new_with_max_length(MAX_LINE)).map(|line| match line {
            Ok(line) => decode::<ServerFrame>(&line).map_err(ClientError::Codec),
            Err(tokio_util::codec::LinesCodecError::Io(e)) => Err(ClientError::Io(e)),
            Err(tokio_util::codec::LinesCodecError::MaxLineLengthExceeded) => {
                Err(ClientError::Io(std::io::Error::other("frame too large")))
            }
        });
    let (tx, mut rx) = mpsc::channel::<ClientFrame>(256);
    tokio::spawn(async move {
        while let Some(frame) = rx.recv().await {
            let line = match encode(&frame) {
                Ok(line) => line,
                Err(e) => {
                    tracing::error!("failed to encode frame: {e}");
                    break;
                }
            };
            if let Err(e) = write.write_all(line.as_bytes()).await {
                tracing::debug!("connection write failed: {e}");
                break;
            }
        }
        if let Err(error) = write.shutdown().await {
            tracing::debug!(%error, "transport shutdown failed");
        }
    });
    (Box::pin(reader), tx)
}

fn tokio_stream_from(mut rx: mpsc::Receiver<ServerFrame>) -> impl Stream<Item = ServerFrame> {
    futures::stream::poll_fn(move |cx| rx.poll_recv(cx))
}
