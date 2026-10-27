use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};

use aether_protocol::{ErrorCode, ErrorReply, ServerFrame, compat};
use futures::SinkExt;
use tokio::io::AsyncWrite;
use tokio::sync::{mpsc, watch};
use tokio_util::codec::{FramedWrite, LinesCodec};

use crate::authz::AuthContext;

/// What the hello settled about the shape of this connection's frames.
pub(super) struct Speaking {
    pub version: Arc<AtomicU32>,
    /// The client listed `features::HISTORY_EXPIRED`.
    pub history_expired: Arc<AtomicBool>,
}

pub(super) async fn run<W: AsyncWrite + Unpin>(
    write: W,
    mut out: mpsc::Receiver<ServerFrame>,
    speaking: Speaking,
    auth: watch::Receiver<Option<AuthContext>>,
) {
    let mut sink = FramedWrite::new(write, LinesCodec::new_with_max_length(super::MAX_LINE));
    let sending = async {
        while let Some(mut frame) = out.recv().await {
            if let Some(auth) = auth.borrow().as_ref() {
                auth.redact_frame(&mut frame);
            }
            if !speaking.history_expired.load(Ordering::SeqCst) {
                legacy_history_expired(&mut frame);
            }
            let line = match encode(speaking.version.load(Ordering::SeqCst), &frame) {
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

/// A client without the feature gets the `NotFound` every daemon before it
/// sent, with the same message and no span.
fn legacy_history_expired(frame: &mut ServerFrame) {
    let ServerFrame::Reply {
        result: Err(error), ..
    } = frame
    else {
        return;
    };
    if error.code != ErrorCode::HistoryExpired {
        return;
    }
    debug_assert!(
        error.expired_span().is_some(),
        "history_expired always names its span"
    );
    *error = ErrorReply::new(ErrorCode::NotFound, std::mem::take(&mut error.message));
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

#[cfg(test)]
mod tests {
    use super::*;

    fn refusal(error: ErrorReply) -> ServerFrame {
        ServerFrame::Reply {
            id: 7,
            result: Err(error),
        }
    }

    #[test]
    fn a_client_without_the_feature_gets_the_older_not_found() {
        let message = "event history expired within sequence range 2..=10; obtain a fresh state snapshot before resuming replay";
        let mut frame = refusal(ErrorReply::history_expired(2, 10, message));
        legacy_history_expired(&mut frame);
        assert_eq!(
            frame,
            refusal(ErrorReply::new(ErrorCode::NotFound, message))
        );
        let line = encode(aether_protocol::PROTOCOL_VERSION, &frame).unwrap();
        assert!(!line.contains("first_seq"), "{line}");

        let mut other = refusal(ErrorReply::not_found("agent agt_x"));
        let before = other.clone();
        legacy_history_expired(&mut other);
        assert_eq!(other, before);
    }
}
