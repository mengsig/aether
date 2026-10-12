//! Byte pump from SSH stdio to the host socket; no handshake or autostart.
use anyhow::Context as _;
use std::path::PathBuf;
use tokio::io::{AsyncRead, AsyncWrite, AsyncWriteExt};

pub(crate) async fn bridge(socket: PathBuf, stdio: bool) -> anyhow::Result<()> {
    anyhow::ensure!(stdio, "`aether bridge` needs --stdio");
    anyhow::ensure!(
        !socket.as_os_str().is_empty(),
        "daemon socket path is empty"
    );
    let stream = tokio::net::UnixStream::connect(&socket)
        .await
        .with_context(|| format!("no daemon at {}: start it on this host", socket.display()))?;
    pump(stream, tokio::io::stdin(), tokio::io::stdout()).await
}

async fn pump<R: AsyncRead + Unpin, W: AsyncWrite + Unpin>(
    stream: tokio::net::UnixStream,
    mut input: R,
    mut output: W,
) -> anyhow::Result<()> {
    let (mut read, mut write) = stream.into_split();
    let upstream = async {
        tokio::io::copy(&mut input, &mut write)
            .await
            .context("bridge stdin to daemon")?;
        write.shutdown().await.context("bridge half-close")
    };
    let downstream = async {
        tokio::io::copy(&mut read, &mut output)
            .await
            .context("bridge daemon to stdout")?;
        output.flush().await.context("bridge stdout flush")
    };
    tokio::pin!(upstream, downstream);
    tokio::select! {
        result = &mut upstream => { result?; downstream.await },
        result = &mut downstream => result,
    }
}

#[cfg(test)]
#[path = "bridge_tests.rs"]
mod tests;
