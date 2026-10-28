//! Where a partial read of maskable text may begin and end. A page, a
//! capped reply or a stream chunk cut inside a secret shows each half
//! unmasked, since neither matches a rule alone.

use std::ops::Range;

use super::Redactor;

/// The least context [`Redactor::clear_span`] reads on each side of a cut:
/// longer than any key shape it guards, a private key block included.
pub const CUT_CONTEXT_BYTES: usize = 16 << 10;

impl Redactor {
    /// How many bytes past each end of a span a window should hold, where
    /// the source has them, for [`Self::clear_span`] to see a secret whole.
    pub fn cut_context(&self) -> usize {
        CUT_CONTEXT_BYTES.max(self.longest_known)
    }

    /// `start..end` of `window` moved off every rule match that crosses it.
    /// A match across `start` is skipped; one across `end` is left out, or
    /// taken whole when it starts at the span's start, so a span never
    /// shrinks to nothing on a secret it starts with. `start <= end` after.
    pub fn clear_span(&self, window: &[u8], start: usize, end: usize) -> (usize, usize) {
        assert!(start <= end, "a span starts before it ends");
        assert!(end <= window.len(), "the span lies inside its window");
        let mut start_matches = self.matches_near(window, start);
        start_matches.sort_by_key(|found| found.start);
        let mut clear_start = start;
        for found in start_matches {
            if found.start < clear_start && found.end > clear_start {
                clear_start = found.end;
            }
        }
        let mut clear_end = end.max(clear_start);
        let mut end_matches = self.matches_near(window, clear_end);
        end_matches.sort_by_key(|found| std::cmp::Reverse(found.end));
        for found in end_matches {
            if found.start < clear_end && found.end > clear_end {
                clear_end = if found.start > clear_start {
                    found.start
                } else {
                    found.end
                };
            }
        }
        debug_assert!(clear_start <= clear_end && clear_end <= window.len());
        (clear_start, clear_end)
    }

    /// Every rule's matches within [`Self::cut_context`] of `cut`, as ranges
    /// of `window`; text runs between invalid bytes are searched apart.
    fn matches_near(&self, window: &[u8], cut: usize) -> Vec<Range<usize>> {
        let context = self.cut_context();
        let low = cut.saturating_sub(context);
        let high = cut.saturating_add(context).min(window.len());
        let mut found = Vec::new();
        let mut at = low;
        for chunk in window[low..high].utf8_chunks() {
            let text = chunk.valid();
            if text.len() >= self.shortest_match {
                for (rule, _) in &self.patterns {
                    found.extend(
                        rule.find_iter(text)
                            .map(|hit| at + hit.start()..at + hit.end()),
                    );
                }
            }
            at += text.len() + chunk.invalid().len();
        }
        debug_assert_eq!(at, high, "every byte of the neighbourhood was walked");
        found
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]

    use super::*;

    const KNOWN: &str = "fleet-token-0123456789abcdef";

    fn known() -> Redactor {
        Redactor::with_known_secrets([KNOWN]).unwrap()
    }

    /// Each half of `text` cut at every point, masked apart, joined.
    fn halves_leak(redactor: &Redactor, text: &str, secret: &str) -> Option<usize> {
        (1..text.len()).find(|&cut| {
            let (_, end) = redactor.clear_span(text.as_bytes(), 0, cut);
            let (start, _) = redactor.clear_span(text.as_bytes(), end, text.len());
            let head = redactor.redact_bytes(&text.as_bytes()[..end]).into_owned();
            let tail = redactor
                .redact_bytes(&text.as_bytes()[start..])
                .into_owned();
            let shown = [head, tail].concat();
            let shown = String::from_utf8_lossy(&shown);
            // Any 8 bytes of the secret shown is a leak.
            secret
                .as_bytes()
                .windows(8)
                .any(|piece| shown.contains(std::str::from_utf8(piece).unwrap()))
        })
    }

    #[test]
    fn no_cut_shows_part_of_a_known_secret_or_a_key_shape() {
        let redactor = known();
        let text = format!("before {KNOWN} after");
        assert_eq!(halves_leak(&redactor, &text, KNOWN), None);
        let key = "sk-ant-api03-AbCdEfGhIjKlMnOpQrStUvWxYz012345";
        let text = format!("token={key}\nnext line");
        assert_eq!(halves_leak(&redactor, &text, key), None);
    }

    #[test]
    fn a_span_starting_on_a_secret_takes_it_whole() {
        let redactor = known();
        let text = format!("{KNOWN} tail");
        let (start, end) = redactor.clear_span(text.as_bytes(), 0, 4);
        assert_eq!((start, end), (0, KNOWN.len()), "the span still moves on");
        assert_eq!(
            redactor.redact_bytes(&text.as_bytes()[..end]),
            b"[REDACTED:known_secret]".as_slice()
        );
    }

    #[test]
    fn a_span_inside_a_secret_skips_past_it() {
        let redactor = known();
        let text = format!("ab {KNOWN} cd");
        let (start, end) = redactor.clear_span(text.as_bytes(), 6, 10);
        assert_eq!(start, 3 + KNOWN.len(), "the rest of the secret is skipped");
        assert_eq!(end, start, "nothing of it is shown");
    }

    #[test]
    fn plain_text_keeps_its_span() {
        let redactor = known();
        let text = "nothing secret here, only words";
        assert_eq!(redactor.clear_span(text.as_bytes(), 3, 17), (3, 17));
        assert_eq!(redactor.cut_context(), CUT_CONTEXT_BYTES);
    }
}
