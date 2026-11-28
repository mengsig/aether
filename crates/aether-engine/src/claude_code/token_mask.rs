//! Masks one spawn's token in what its child prints on stdout: in each JSON
//! line however it is escaped, and across the deltas a streamed block arrives in.

use std::borrow::Cow;
use std::collections::BTreeMap;

use aether_core::model::StreamEvent;
use aether_core::redact::KNOWN_SECRET_MASK;

use crate::EngineEvent;

/// `line` with `token` replaced. The token's characters are never escaped by
/// a JSON writer, but any character may be written as `\u` escape, so a line
/// holding one is decoded and masked again if the decoded form holds the token.
pub(super) fn mask_line<'a>(token: &str, line: &'a str) -> Cow<'a, str> {
    debug_assert!(!token.is_empty(), "a token is never empty");
    let line: Cow<'a, str> = if line.contains(token) {
        Cow::Owned(line.replace(token, KNOWN_SECRET_MASK))
    } else {
        Cow::Borrowed(line)
    };
    if !line.contains("\\u") {
        return line;
    }
    // A line that is not JSON is passed on: the parser refuses it as well.
    let Ok(decoded) = serde_json::from_str::<serde_json::Value>(&line) else {
        return line;
    };
    let normalized = decoded.to_string();
    if !normalized.contains(token) {
        return line;
    }
    let masked = normalized.replace(token, KNOWN_SECRET_MASK);
    debug_assert!(!masked.contains(token));
    Cow::Owned(masked)
}

/// The kinds of streamed block text a token can be split across.
#[derive(Clone, Copy)]
enum Piece {
    Text,
    Thinking,
    ToolInput,
    Compaction,
}

impl Piece {
    fn of(event: &StreamEvent) -> Option<(Self, usize, &str)> {
        match event {
            StreamEvent::TextDelta { index, text } => Some((Self::Text, *index, text)),
            StreamEvent::ThinkingDelta { index, text } => Some((Self::Thinking, *index, text)),
            StreamEvent::ToolInputDelta {
                index,
                partial_json,
            } => Some((Self::ToolInput, *index, partial_json)),
            StreamEvent::CompactionDelta { index, content } => {
                Some((Self::Compaction, *index, content))
            }
            _ => None,
        }
    }

    fn delta(self, index: usize, text: String) -> EngineEvent {
        EngineEvent::Delta(match self {
            Self::Text => StreamEvent::TextDelta { index, text },
            Self::Thinking => StreamEvent::ThinkingDelta { index, text },
            Self::ToolInput => StreamEvent::ToolInputDelta {
                index,
                partial_json: text,
            },
            Self::Compaction => StreamEvent::CompactionDelta {
                index,
                content: text,
            },
        })
    }
}

/// Per streamed block, the tail of its text that could be the start of the
/// token, held back until the next delta shows whether it is.
#[derive(Default)]
pub(super) struct HeldDeltas {
    held: BTreeMap<usize, (Piece, String)>,
}

impl HeldDeltas {
    /// `events` with every token masked, even one split across deltas.
    pub(super) fn mask(&mut self, token: &str, events: Vec<EngineEvent>) -> Vec<EngineEvent> {
        let mut out = Vec::with_capacity(events.len());
        for event in events {
            let EngineEvent::Delta(stream) = &event else {
                self.flush_all(&mut out);
                out.push(event);
                continue;
            };
            if let Some((piece, index, text)) = Piece::of(stream) {
                self.push(token, piece, index, text, &mut out);
                continue;
            }
            match stream {
                StreamEvent::BlockStop { index } => self.flush(*index, &mut out),
                _ => self.flush_all(&mut out),
            }
            out.push(event);
        }
        out
    }

    fn push(
        &mut self,
        token: &str,
        piece: Piece,
        index: usize,
        text: &str,
        out: &mut Vec<EngineEvent>,
    ) {
        let mut joined = self
            .held
            .remove(&index)
            .map(|(_, held)| held)
            .unwrap_or_default();
        joined.push_str(text);
        let masked = if joined.contains(token) {
            joined.replace(token, KNOWN_SECRET_MASK)
        } else {
            joined
        };
        let keep = token_prefix_tail(token, &masked);
        debug_assert!(keep < token.len(), "a whole token was masked already");
        let (sent, held) = masked.split_at(masked.len() - keep);
        if !held.is_empty() {
            self.held.insert(index, (piece, held.to_owned()));
        }
        if !sent.is_empty() {
            out.push(piece.delta(index, sent.to_owned()));
        }
    }

    fn flush(&mut self, index: usize, out: &mut Vec<EngineEvent>) {
        if let Some((piece, held)) = self.held.remove(&index) {
            out.push(piece.delta(index, held));
        }
    }

    /// A tail still held when its block ends is not the token: it goes out.
    fn flush_all(&mut self, out: &mut Vec<EngineEvent>) {
        for (index, (piece, held)) in std::mem::take(&mut self.held) {
            out.push(piece.delta(index, held));
        }
    }
}

/// Bytes at the end of `text` that are a proper prefix of `token`, longest first.
fn token_prefix_tail(token: &str, text: &str) -> usize {
    let longest = text.len().min(token.len() - 1);
    (1..=longest)
        .rev()
        .find(|&len| {
            let start = text.len() - len;
            text.is_char_boundary(start) && token.starts_with(&text[start..])
        })
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    const TOKEN: &str = "plain-oauth-value-0123456789";

    fn text(index: usize, text: &str) -> EngineEvent {
        EngineEvent::Delta(StreamEvent::TextDelta {
            index,
            text: text.into(),
        })
    }

    fn joined(events: &[EngineEvent]) -> String {
        events
            .iter()
            .filter_map(|event| match event {
                EngineEvent::Delta(StreamEvent::TextDelta { text, .. }) => Some(text.as_str()),
                _ => None,
            })
            .collect()
    }

    #[test]
    fn a_unicode_escaped_token_is_masked_once_decoded() {
        let escaped = TOKEN.replace('p', "\\u0070");
        let line = format!(r#"{{"type":"assistant","text":"env {escaped} end"}}"#);
        let masked = mask_line(TOKEN, &line);
        let decoded: serde_json::Value = serde_json::from_str(&masked).unwrap();
        assert_eq!(decoded["text"], format!("env {KNOWN_SECRET_MASK} end"));
        let plain = r#"{"text":"café is not a token"}"#;
        assert_eq!(
            mask_line(TOKEN, plain),
            plain,
            "nothing to mask stays as is"
        );
    }

    #[test]
    fn a_token_split_across_deltas_is_masked_whole() {
        let mut held = HeldDeltas::default();
        let mut events = Vec::new();
        for part in ["env pla", "in-oauth-val", "ue-0123456789 done", " s"] {
            events.extend(held.mask(TOKEN, vec![text(0, part)]));
        }
        let stop = EngineEvent::Delta(StreamEvent::BlockStop { index: 0 });
        events.extend(held.mask(TOKEN, vec![stop.clone()]));
        assert_eq!(joined(&events), format!("env {KNOWN_SECRET_MASK} done s"));
        assert_eq!(
            events.last(),
            Some(&stop),
            "held text goes out before the stop"
        );
        for event in &events {
            let shown = format!("{event:?}");
            assert!(!shown.contains("in-oauth-val"), "{shown}");
        }
    }

    #[test]
    fn ordinary_deltas_pass_through_unchanged() {
        let mut held = HeldDeltas::default();
        let events = held.mask(TOKEN, vec![text(1, "hello world"), text(2, "x")]);
        assert_eq!(events, vec![text(1, "hello world"), text(2, "x")]);
        assert_eq!(token_prefix_tail(TOKEN, "a plain"), 5);
        assert_eq!(token_prefix_tail(TOKEN, "none"), 0);
    }
}
