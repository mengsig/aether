//! Selecting and batching what a job reads (parity X1): coverage is a
//! SHA-256 per message, the uncovered set is derived from it, and a batch
//! never splits a message. A source's text is cut into chunks at blank lines.

use std::collections::BTreeMap;

use crate::graph::ids::fingerprint;
use crate::graph::store::Covered;

/// The most estimated tokens of prose one extractor call reads, unless a
/// single message alone is larger.
pub const BATCH_TOKENS: u64 = 6_000;
/// The most estimated tokens of one source chunk, unless a paragraph is larger.
pub const CHUNK_TOKENS: u64 = 1_500;

/// One message of a session's teacher prose, or one chunk of a source.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Message {
    /// Unique within its session or source.
    pub message_ref: String,
    pub text: String,
}

/// A cheap token estimate: four characters a token, rounded up.
pub fn tokens(text: &str) -> u64 {
    (text.chars().count() as u64).div_ceil(4)
}

/// The coverage row reading `message` of `source` writes.
pub fn covered(source: &str, message: &Message) -> Covered {
    Covered {
        source: source.to_string(),
        message_ref: message.message_ref.clone(),
        fingerprint: fingerprint(message.text.as_bytes()),
        tokens: tokens(&message.text),
    }
}

/// The messages not yet read as they now stand: never covered, or covered
/// with another fingerprint. Empty prose has nothing to read.
pub fn uncovered(messages: Vec<Message>, coverage: &BTreeMap<String, String>) -> Vec<Message> {
    let mut refs: Vec<&str> = messages.iter().map(|m| m.message_ref.as_str()).collect();
    refs.sort_unstable();
    let before = refs.len();
    refs.dedup();
    assert_eq!(
        before,
        refs.len(),
        "message refs are unique within a source"
    );
    messages
        .into_iter()
        .filter(|message| !message.text.trim().is_empty())
        .filter(|message| {
            let print = coverage.get(&message.message_ref);
            print.is_none_or(|print| *print != fingerprint(message.text.as_bytes()))
        })
        .collect()
}

/// Consecutive messages grouped up to `budget` tokens; a message is never
/// split, so one larger than `budget` is a batch of its own.
pub fn batches(messages: Vec<Message>, budget: u64) -> Vec<Vec<Message>> {
    assert!(budget > 0, "a batch budget is positive");
    let total = messages.len();
    let mut out: Vec<Vec<Message>> = Vec::new();
    let mut used = 0;
    for message in messages {
        let cost = tokens(&message.text);
        match out.last_mut() {
            Some(batch) if used + cost <= budget => batch.push(message),
            _ => {
                out.push(vec![message]);
                used = 0;
            }
        }
        used += cost;
    }
    debug_assert_eq!(out.iter().map(Vec::len).sum::<usize>(), total);
    out
}

/// A source's text as chunks of whole paragraphs, `chunk-<n>` in order. The
/// same text always gives the same chunks, so coverage stays meaningful.
pub fn chunks(text: &str) -> Vec<Message> {
    let normalized = text.replace("\r\n", "\n");
    let paragraphs: Vec<&str> = normalized
        .split("\n\n")
        .map(str::trim)
        .filter(|p| !p.is_empty())
        .collect();
    let mut out: Vec<Message> = Vec::new();
    let mut current = String::new();
    for paragraph in paragraphs {
        let joined = tokens(&current) + tokens(paragraph);
        if !current.is_empty() && joined > CHUNK_TOKENS {
            out.push(chunk(out.len(), std::mem::take(&mut current)));
        }
        if !current.is_empty() {
            current.push_str("\n\n");
        }
        current.push_str(paragraph);
    }
    if !current.is_empty() {
        out.push(chunk(out.len(), current));
    }
    debug_assert!(out.iter().all(|m| !m.text.is_empty()));
    out
}

fn chunk(index: usize, text: String) -> Message {
    Message {
        message_ref: format!("chunk-{index}"),
        text,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn message(r: &str, chars: usize) -> Message {
        Message {
            message_ref: r.into(),
            text: "x".repeat(chars),
        }
    }

    #[test]
    fn batches_never_split_a_message_and_respect_the_budget() {
        let messages = vec![
            message("a", 40),
            message("b", 40),
            message("c", 400),
            message("d", 4),
        ];
        let out = batches(messages, 20);
        let refs: Vec<Vec<&str>> = out
            .iter()
            .map(|b| b.iter().map(|m| m.message_ref.as_str()).collect())
            .collect();
        assert_eq!(refs, vec![vec!["a", "b"], vec!["c"], vec!["d"]]);
        assert!(batches(Vec::new(), 20).is_empty());
    }

    #[test]
    fn covered_prose_stays_covered_until_it_changes() {
        let messages = vec![message("a", 8), message("b", 8)];
        let mut coverage = BTreeMap::new();
        let first = covered("ns_1", &messages[0]);
        coverage.insert(first.message_ref.clone(), first.fingerprint.clone());
        let left = uncovered(messages.clone(), &coverage);
        assert_eq!(left, vec![messages[1].clone()]);
        let mut edited = messages.clone();
        edited[0].text.push('!');
        assert_eq!(uncovered(edited, &coverage).len(), 2);
    }

    #[test]
    fn chunks_keep_paragraphs_whole_and_are_stable() {
        let paragraph = "word ".repeat(1_000);
        let text = format!("{paragraph}\n\nshort one\n\n{paragraph}");
        let one = chunks(&text);
        assert_eq!(
            one,
            chunks(&text.replace('\n', "\r\n").replace("\r\r", "\r"))
        );
        assert_eq!(one.len(), 2);
        assert!(one[0].text.ends_with("short one"));
        assert_eq!(one[1].message_ref, "chunk-1");
    }
}
