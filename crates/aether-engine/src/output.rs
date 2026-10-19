//! Bounds on native process diagnostics and JSONL records.

use futures::StreamExt;
use std::collections::VecDeque;
use tokio::io::{AsyncRead, AsyncReadExt};
use tokio_util::codec::{FramedRead, LinesCodec, LinesCodecError};

pub(crate) const MAX_LINE_BYTES: usize = 16 * 1024 * 1024;
const STDERR_HEAD_BYTES: usize = 8 * 1024;
const STDERR_TAIL_BYTES: usize = 64 * 1024;

pub(crate) struct BoundedLines<R> {
    lines: FramedRead<R, LinesCodec>,
}

impl<R: AsyncRead + Unpin> BoundedLines<R> {
    pub(crate) fn new(reader: R) -> Self {
        Self {
            lines: FramedRead::new(reader, LinesCodec::new_with_max_length(MAX_LINE_BYTES)),
        }
    }

    pub(crate) async fn next_line(&mut self) -> Result<Option<String>, std::io::Error> {
        self.lines
            .next()
            .await
            .transpose()
            .map_err(|error| match error {
                LinesCodecError::Io(error) => error,
                LinesCodecError::MaxLineLengthExceeded => std::io::Error::new(
                    std::io::ErrorKind::InvalidData,
                    format!("native JSONL record exceeds {MAX_LINE_BYTES} bytes"),
                ),
            })
    }
}

#[derive(Default)]
pub(crate) struct StderrCapture {
    pub(crate) text: String,
    pub(crate) session_lost: bool,
}

#[cfg(test)]
pub(crate) async fn drain_stderr(reader: impl AsyncRead + Unpin) -> StderrCapture {
    let redactor = aether_core::redact::Redactor::default();
    drain_stderr_until(reader, tokio_util::sync::CancellationToken::new(), redactor).await
}

/// Capture stderr's head and tail, masked by `redactor`: the turn's, which
/// also knows the daemon's secret values.
pub(crate) async fn drain_stderr_until(
    mut reader: impl AsyncRead + Unpin,
    stop: tokio_util::sync::CancellationToken,
    redactor: aether_core::redact::Redactor,
) -> StderrCapture {
    let mut head = Vec::with_capacity(STDERR_HEAD_BYTES);
    let mut tail = VecDeque::with_capacity(STDERR_TAIL_BYTES);
    let mut chunk = [0u8; 8192];
    let mut truncated = false;
    let mut scan = Vec::with_capacity(chunk.len() + 32);
    let (mut missing_conversation, mut session, mut not_found) = (false, false, false);
    let read_error = loop {
        let read = tokio::select! {
            read = reader.read(&mut chunk) => read,
            _ = stop.cancelled() => Err(std::io::Error::new(std::io::ErrorKind::TimedOut, "stderr did not close after process cleanup")),
        };
        match read {
            Ok(0) => break None,
            Ok(read) => {
                let keep = read.min(STDERR_HEAD_BYTES - head.len());
                head.extend_from_slice(&chunk[..keep]);
                tail.extend(&chunk[..read]);
                // Preserve resume-failure detection even when its diagnostic falls out of the tail.
                scan.extend_from_slice(&chunk[..read]);
                let text = String::from_utf8_lossy(&scan);
                missing_conversation |= text.contains("No conversation found");
                session |= text.contains("session");
                not_found |= text.contains("not found");
                scan.drain(..scan.len().saturating_sub(32));
                if tail.len() > STDERR_TAIL_BYTES {
                    tail.drain(..tail.len() - STDERR_TAIL_BYTES);
                    truncated = true;
                }
            }
            Err(error) => break Some(error),
        }
    };
    // Each side is masked on its own: a key cut by the gap has no marker
    // the redactor could pair across the separator.
    let mut output = if truncated {
        format!(
            "{}\n[stderr truncated]\n{}",
            redactor.redact(&String::from_utf8_lossy(&head)),
            redactor.redact(&String::from_utf8_lossy(tail.make_contiguous()))
        )
    } else {
        redactor
            .redact(&String::from_utf8_lossy(tail.make_contiguous()))
            .into_owned()
    };
    if let Some(error) = read_error {
        output.push_str(&format!("\n[stderr read error: {error}]"));
    }
    StderrCapture {
        text: output,
        session_lost: missing_conversation || session && not_found,
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn stderr_flood_preserves_bounded_head_and_tail_and_drains_invalid_utf8() {
        let mut bytes = b"head\n".to_vec();
        bytes.extend(vec![0xff; STDERR_TAIL_BYTES * 3]);
        bytes.extend(b"\ntail\n");
        let output = drain_stderr(&bytes[..]).await.text;
        assert!(output.starts_with("head\n"));
        assert!(output.contains("[stderr truncated]"));
        assert!(output.ends_with("\ntail\n"));
        assert!(output.len() <= 3 * (STDERR_HEAD_BYTES + STDERR_TAIL_BYTES) + 32);
        assert_eq!(drain_stderr(&b"ordinary\n"[..]).await.text, "ordinary\n");
        let keyed =
            drain_stderr(&b"bad key sk-ant-api03-NOTAREALKEYbutkeyshaped0123456789\n"[..]).await;
        assert!(!keyed.text.contains("NOTAREALKEY") && keyed.text.contains("[REDACTED"));
    }

    #[tokio::test]
    async fn a_private_key_cut_by_the_truncation_gap_is_masked() {
        let mut bytes = vec![b'x'; STDERR_HEAD_BYTES + 100];
        bytes.extend(b"\n-----BEGIN PRIVATE KEY-----\nSECRETBODYAAA\n");
        bytes.extend(vec![b'y'; STDERR_TAIL_BYTES]);
        bytes.extend(b"\nSECRETBODYBBB\n-----END PRIVATE KEY-----\nafter\n");
        let output = drain_stderr(&bytes[..]).await.text;
        assert!(output.contains("[stderr truncated]"));
        assert!(
            !output.contains("SECRETBODY"),
            "{}",
            &output[output.len() - 200..]
        );
        assert!(output.ends_with("after\n"));
    }

    #[tokio::test]
    async fn stderr_truncation_preserves_session_loss_in_discarded_diagnostics() {
        let mut bytes = vec![b'x'; STDERR_HEAD_BYTES + 100];
        bytes.extend(b"No conversation found");
        bytes.extend(vec![b'x'; STDERR_TAIL_BYTES * 2]);
        let capture = drain_stderr(&bytes[..]).await;
        assert!(capture.session_lost);
        assert!(!capture.text.contains("No conversation found"));
    }

    #[tokio::test]
    async fn stderr_session_loss_markers_survive_chunk_boundaries() {
        for marker in ["No conversation found", "session not found"] {
            for split in 1..marker.len() {
                let mut bytes = vec![b'x'; 8192 - split];
                bytes.extend(marker.as_bytes());
                bytes.extend(vec![b'x'; STDERR_TAIL_BYTES * 2]);
                assert!(
                    drain_stderr(&bytes[..]).await.session_lost,
                    "{marker}: {split}"
                );
            }
        }
    }

    #[tokio::test]
    async fn oversized_jsonl_record_returns_an_explicit_error() {
        let bytes = vec![b'x'; MAX_LINE_BYTES + 1];
        let mut lines = BoundedLines::new(&bytes[..]);
        let error = lines.next_line().await.unwrap_err();
        assert_eq!(error.kind(), std::io::ErrorKind::InvalidData);
        assert!(error.to_string().contains("exceeds"));
    }

    #[tokio::test]
    async fn jsonl_records_preserve_crlf_and_unterminated_final_lines() {
        let mut lines = BoundedLines::new(&b"one\r\ntwo"[..]);
        assert_eq!(lines.next_line().await.unwrap().as_deref(), Some("one"));
        assert_eq!(lines.next_line().await.unwrap().as_deref(), Some("two"));
        assert!(lines.next_line().await.unwrap().is_none());
    }
}

#[cfg(test)]
mod interrupted_stderr_tests {
    use super::*;
    #[tokio::test]
    async fn ending_a_stalled_capture_keeps_the_diagnostic_and_session_marker() {
        use tokio::io::AsyncWriteExt;
        let (mut writer, reader) = tokio::io::duplex(1024);
        writer
            .write_all(b"session not found: resume failed\n")
            .await
            .unwrap();
        let stop = tokio_util::sync::CancellationToken::new();
        let stopping = stop.clone();
        let task = tokio::spawn(drain_stderr_until(
            reader,
            stop,
            aether_core::redact::Redactor::default(),
        ));
        writer.write_all(&vec![b'x'; 2048]).await.unwrap();
        stopping.cancel();
        let captured = task.await.unwrap();
        assert!(captured.session_lost);
        assert!(captured.text.contains("resume failed"));
        assert!(captured.text.contains("stderr did not close"));
    }
}
