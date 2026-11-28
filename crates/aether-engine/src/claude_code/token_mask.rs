//! Masks one spawn's token in what its child prints on stdout: in each JSON
//! line however it is escaped, and across the deltas a streamed block arrives in.

use std::borrow::Cow;
use std::collections::BTreeMap;

use aether_core::model::StreamEvent;
use aether_core::redact::KNOWN_SECRET_MASK;

use crate::EngineEvent;

/// `line` with `token` replaced. The token's characters are never escaped by
/// a JSON writer, but any character may be written as `\u` escape, so those
/// escapes are written out as plain characters before the token is looked for.
pub(super) fn mask_line<'a>(token: &str, line: &'a str) -> Cow<'a, str> {
    debug_assert!(!token.is_empty(), "a token is never empty");
    let line: Cow<'a, str> = if line.contains(token) {
        Cow::Owned(line.replace(token, KNOWN_SECRET_MASK))
    } else {
        Cow::Borrowed(line)
    };
    let plain = unescape_plain(&line);
    if !plain.contains(token) {
        return line;
    }
    let masked = plain.replace(token, KNOWN_SECRET_MASK);
    debug_assert!(!masked.contains(token));
    Cow::Owned(masked)
}

/// `text` with every JSON escape of a character that needs none written as
/// that character. Nothing is parsed, so duplicate keys and invalid JSON stay
/// as written; only `"`, `\\` and control characters stay escaped.
fn unescape_plain(text: &str) -> Cow<'_, str> {
    if !text.contains('\\') {
        return Cow::Borrowed(text);
    }
    let mut out = String::with_capacity(text.len());
    let mut rest = text;
    while let Some(at) = rest.find('\\') {
        out.push_str(&rest[..at]);
        let (written, consumed) = unescape_one(&rest[at..]);
        debug_assert!(consumed >= 1 && consumed <= rest.len() - at);
        match written {
            Some(c) => out.push(c),
            None => out.push_str(&rest[at..at + consumed]),
        }
        rest = &rest[at + consumed..];
    }
    out.push_str(rest);
    Cow::Owned(out)
}

/// The escape that starts `text` (at a backslash): the plain character it can
/// be written as, if any, and how many bytes it spans.
fn unescape_one(text: &str) -> (Option<char>, usize) {
    debug_assert!(text.starts_with('\\'));
    let bytes = text.as_bytes();
    match bytes.get(1) {
        Some(b'/') => (Some('/'), 2),
        Some(b'u') => unescape_unicode(text),
        Some(next) if next.is_ascii() => (None, 2),
        // A backslash before a multi-byte character: keep only the backslash.
        _ => (None, 1),
    }
}

fn unescape_unicode(text: &str) -> (Option<char>, usize) {
    let Some(high) = hex4(text, 2) else {
        return (None, 2);
    };
    let (code, consumed) = if (0xD800..0xDC00).contains(&high) {
        match text.get(6..8).filter(|s| *s == "\\u").and(hex4(text, 8)) {
            Some(low) if (0xDC00..0xE000).contains(&low) => {
                (0x10000 + ((high - 0xD800) << 10) + (low - 0xDC00), 12)
            }
            _ => return (None, 6),
        }
    } else {
        (high, 6)
    };
    match char::from_u32(code) {
        Some(c) if c != '"' && c != '\\' && !c.is_control() => (Some(c), consumed),
        _ => (None, consumed),
    }
}

fn hex4(text: &str, start: usize) -> Option<u32> {
    let digits = text.get(start..start + 4)?;
    if !digits.bytes().all(|b| b.is_ascii_hexdigit()) {
        return None;
    }
    u32::from_str_radix(digits, 16).ok()
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

/// Per streamed block, what is held back: for text, the tail that could be
/// the start of the token until the next delta shows whether it is; for tool
/// input, all of it until the block ends, since that JSON may escape the token.
#[derive(Default)]
pub(super) struct HeldDeltas {
    held: BTreeMap<usize, (Piece, String)>,
}

impl HeldDeltas {
    /// `events` with every token masked, even one split across deltas.
    pub(super) fn mask(&mut self, token: &str, events: Vec<EngineEvent>) -> Vec<EngineEvent> {
        let mut out = Vec::with_capacity(events.len());
        for event in events {
            if let EngineEvent::Delta(stream) = &event
                && let Some((piece, index, text)) = Piece::of(stream)
            {
                self.push(token, piece, index, text, &mut out);
                continue;
            }
            match &event {
                EngineEvent::Delta(
                    StreamEvent::BlockStop { index } | StreamEvent::BlockStart { index, .. },
                ) => self.flush(token, *index, &mut out),
                EngineEvent::Delta(
                    StreamEvent::MessageStart { .. }
                    | StreamEvent::MessageStop
                    | StreamEvent::Stop { .. },
                )
                | EngineEvent::Assistant { .. }
                | EngineEvent::Done(_) => self.flush_all(token, &mut out),
                // Notes, usage and background tasks arrive between the deltas
                // of a block that is still open: what it holds stays held.
                _ => {}
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
        if matches!(piece, Piece::ToolInput) {
            self.held.insert(index, (piece, joined));
            return;
        }
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

    /// A text tail still held when its block ends is not the token: it goes
    /// out. Held tool input goes out whole, decoded and masked.
    fn flush(&mut self, token: &str, index: usize, out: &mut Vec<EngineEvent>) {
        if let Some((piece, held)) = self.held.remove(&index) {
            out.push(piece.delta(index, release(token, piece, held)));
        }
    }

    fn flush_all(&mut self, token: &str, out: &mut Vec<EngineEvent>) {
        for (index, (piece, held)) in std::mem::take(&mut self.held) {
            out.push(piece.delta(index, release(token, piece, held)));
        }
        debug_assert!(self.held.is_empty());
    }
}

fn release(token: &str, piece: Piece, held: String) -> String {
    match piece {
        Piece::ToolInput => mask_tool_input(token, &held),
        Piece::Text | Piece::Thinking | Piece::Compaction => held,
    }
}

/// A block's whole tool input with `token` masked, however its JSON escapes it.
fn mask_tool_input(token: &str, input: &str) -> String {
    mask_line(token, input).into_owned()
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
    fn a_background_task_between_two_deltas_keeps_the_prefix_held() {
        let mut held = HeldDeltas::default();
        let task = EngineEvent::BackgroundTask {
            id: "t1".into(),
            running: true,
        };
        let mut events = held.mask(TOKEN, vec![text(0, "env plain-oauth"), task.clone()]);
        events.extend(held.mask(TOKEN, vec![text(0, "-value-0123456789 end")]));
        events.extend(held.mask(
            TOKEN,
            vec![EngineEvent::Delta(StreamEvent::BlockStop { index: 0 })],
        ));
        assert!(events.contains(&task), "the notification still goes out");
        assert_eq!(joined(&events), format!("env {KNOWN_SECRET_MASK} end"));
        for event in &events {
            let shown = format!("{event:?}");
            assert!(!shown.contains("plain-oauth"), "{shown}");
        }
    }

    #[test]
    fn tool_input_escaping_the_token_across_deltas_is_masked() {
        let escaped = TOKEN.replace('p', "\\u0070");
        let input = format!(r#"{{"command":"echo {escaped}"}}"#);
        let (first, second) = input.split_at(input.find("\\u0070").unwrap() + 3);
        let mut held = HeldDeltas::default();
        let mut events = Vec::new();
        for part in [first, second] {
            let delta = EngineEvent::Delta(StreamEvent::ToolInputDelta {
                index: 1,
                partial_json: part.into(),
            });
            events.extend(held.mask(TOKEN, vec![delta]));
        }
        assert!(events.is_empty(), "tool input is held until its block ends");
        events.extend(held.mask(
            TOKEN,
            vec![EngineEvent::Delta(StreamEvent::BlockStop { index: 1 })],
        ));
        let Some(EngineEvent::Delta(StreamEvent::ToolInputDelta { partial_json, .. })) =
            events.first()
        else {
            panic!("the held input goes out first: {events:?}");
        };
        let decoded: serde_json::Value = serde_json::from_str(partial_json).unwrap();
        assert_eq!(decoded["command"], format!("echo {KNOWN_SECRET_MASK}"));
        assert_eq!(
            mask_tool_input(TOKEN, r#"{"a":"é is no token"}"#),
            r#"{"a":"é is no token"}"#,
            "input without the token is passed on as written"
        );
    }

    #[test]
    fn an_escaped_token_under_a_duplicate_key_is_masked() {
        let escaped = TOKEN.replace('p', "\\u0070");
        let input = format!(r#"{{"x":"{escaped}","x":"safe"}}"#);
        let masked = mask_tool_input(TOKEN, &input);
        assert_eq!(
            masked,
            format!(r#"{{"x":"{KNOWN_SECRET_MASK}","x":"safe"}}"#)
        );
        let line = format!(r#"{{"text":"{escaped}","text":"safe"}}"#);
        assert!(!unescape_plain(&mask_line(TOKEN, &line)).contains(TOKEN));
    }

    #[test]
    fn only_escapes_of_plain_characters_are_written_out() {
        assert_eq!(unescape_plain(r#"ap\/b"#), "ap/b");
        assert_eq!(
            unescape_plain(r#"\\u0070"#),
            r#"\\u0070"#,
            "an escaped backslash"
        );
        assert_eq!(unescape_plain(r#""\\u000a\n"#), r#""\\u000a\n"#);
        assert_eq!(unescape_plain(r#"😀 \ud83d x"#), "😀 \\ud83d x");
        assert_eq!(
            unescape_plain(r#"\u00zz\"#),
            r#"\u00zz\"#,
            "malformed stays"
        );
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
