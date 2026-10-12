use std::sync::Arc;
use std::sync::atomic::{AtomicU32, Ordering};

use aether_protocol::{ErrorReply, ServerFrame, compat};
use futures::SinkExt;
use tokio::io::AsyncWrite;
use tokio::sync::{mpsc, watch};
use tokio_util::codec::{FramedWrite, LinesCodec};

use crate::authz::AuthContext;

pub(super) async fn run<W: AsyncWrite + Unpin>(
    write: W,
    mut out: mpsc::Receiver<ServerFrame>,
    version: Arc<AtomicU32>,
    auth: watch::Receiver<Option<AuthContext>>,
) {
    let mut sink = FramedWrite::new(write, LinesCodec::new_with_max_length(super::MAX_LINE));
    let sending = async {
        while let Some(mut frame) = out.recv().await {
            if let Some(auth) = auth.borrow().as_ref() {
                auth.redact_frame(&mut frame);
            }
            let line = match encode(version.load(Ordering::SeqCst), &frame) {
                Ok(line) => line,
                Err(error) => {
                    tracing::error!(%error, "could not encode frame; closing its connection");
                    return;
                }
            };
            debug_assert!(
                line.ends_with('\n'),
                "the encoder supplies the line terminator"
            );
            debug_assert_eq!(
                line.bytes().filter(|byte| *byte == b'\n').count(),
                1,
                "one encoded frame per line"
            );
            if let Err(error) = sink.send(line.trim_end().to_owned()).await {
                tracing::debug!(%error, "connection writer failed");
                return;
            }
        }
    };
    tokio::select! {
        biased;
        _ = revoked(auth.clone()) => {},
        _ = sending => {},
    }
}

fn encode(version: u32, frame: &ServerFrame) -> anyhow::Result<String> {
    assert!(
        version >= compat::MIN_PROTOCOL_VERSION,
        "writer uses an admitted protocol"
    );
    assert!(
        version <= aether_protocol::PROTOCOL_VERSION,
        "writer cannot speak a future protocol"
    );
    match compat::encode_for(version, frame) {
        Ok(line) => Ok(line),
        Err(error) => {
            tracing::error!(%error, "could not encode frame");
            let ServerFrame::Reply { id, .. } = frame else {
                return Err(error.into());
            };
            let fallback = ServerFrame::Reply {
                id: *id,
                result: Err(ErrorReply::internal(format!(
                    "could not encode reply: {error}"
                ))),
            };
            Ok(compat::encode_for(version, &fallback)?)
        }
    }
}

async fn revoked(mut auth: watch::Receiver<Option<AuthContext>>) {
    loop {
        let context = auth.borrow_and_update().clone();
        if let Some(closed) = context.as_ref().and_then(AuthContext::closed) {
            closed.cancelled().await;
            return;
        }
        if auth.changed().await.is_err() {
            return std::future::pending().await;
        }
    }
}
