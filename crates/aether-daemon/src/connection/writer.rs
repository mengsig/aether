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
    /// The client listed `features::DELTA_ORDER`.
    pub delta_order: Arc<AtomicBool>,
    /// The client listed `features::NOESIS`.
    pub noesis: Arc<AtomicBool>,
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
            if !speaking.delta_order.load(Ordering::SeqCst) {
                compat::withhold_delta_order(&mut frame);
            }
            if !speaking.noesis.load(Ordering::SeqCst) {
                compat::withhold_noesis_events(&mut frame);
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

    /// The line `run` writes for one numbered tool-input delta.
    async fn written(delta_order: bool) -> String {
        use tokio::io::AsyncBufReadExt;
        let (write, read) = tokio::io::duplex(4096);
        let (out, frames) = mpsc::channel(1);
        let speaking = Speaking {
            version: Arc::new(AtomicU32::new(aether_protocol::PROTOCOL_VERSION)),
            history_expired: Arc::new(AtomicBool::new(true)),
            delta_order: Arc::new(AtomicBool::new(delta_order)),
            noesis: Arc::new(AtomicBool::new(true)),
        };
        let (_auth, auth) = watch::channel(None);
        let writer = tokio::spawn(run(write, frames, speaking, auth));
        let frame = ServerFrame::Delta {
            sub: 1,
            agent: aether_core::id::AgentId::new(),
            invocation: aether_core::id::InvocationId::new(),
            delta: aether_protocol::Delta::ToolInput {
                name: "bash".into(),
                partial_json: "{".into(),
                tool_use_id: Some("toolu_1".into()),
            },
            invocation_seq: Some(7),
        };
        out.send(frame).await.expect("the writer is reading");
        drop(out);
        let mut line = String::new();
        tokio::io::BufReader::new(read)
            .read_line(&mut line)
            .await
            .expect("a line");
        writer.await.expect("the writer ends with its queue");
        line
    }

    #[tokio::test]
    async fn only_a_client_that_asked_for_delta_order_reads_the_numbering() {
        let numbered = written(true).await;
        assert!(numbered.contains(r#""invocation_seq":7"#), "{numbered}");
        assert!(
            numbered.contains(r#""tool_use_id":"toolu_1""#),
            "{numbered}"
        );
        let plain = written(false).await;
        assert!(!plain.contains("invocation_seq"), "{plain}");
        assert!(!plain.contains("tool_use_id"), "{plain}");
        assert!(plain.contains(r#""partial_json":"{""#), "{plain}");
    }
}
