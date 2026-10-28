//! Where a partial read of maskable text may begin and end. A page, a
//! capped reply or a stream chunk cut inside a secret shows each half
//! unmasked, since neither matches a rule alone.

use std::ops::Range;
use std::sync::LazyLock;

use regex::bytes::Regex;

use super::{PRIVATE_KEY_MASK, Redactor};

/// The least context a window holds on each side of a cut, for
/// [`Redactor::clear_span`] to see any secret around the cut whole.
pub const CUT_CONTEXT_BYTES: usize = 16 << 10;

/// How far before a page its window should reach, where the source has it,
/// so that a token or private key block begun that far back is seen whole.
pub const PAGE_LOOKBACK_BYTES: usize = 1 << 20;

/// Runs and gaps a rule match is followed across beyond the context: a
/// value, its blank space and `=`, and its key name, on either side.
const REACH_HOPS: usize = 5;

/// Most times a cut moves on past one secret onto the next. Each pass
/// searches afresh, and matches of a secret repeated back to back fall
/// differently from each neighbourhood, so passes are bounded.
const MAX_PASSES: usize = 8;

/// Private key block edges over bytes, so an invalid byte inside a block
/// cannot hide it the way it splits the text rules' UTF-8 runs.
static BLOCK_BEGIN: LazyLock<Regex> =
    LazyLock::new(|| byte_rule(r"-----BEGIN [A-Z ]*PRIVATE KEY-----"));
static BLOCK_END: LazyLock<Regex> =
    LazyLock::new(|| byte_rule(r"-----END [A-Z ]*PRIVATE KEY-----"));
/// A window that starts inside a block, as the text rule for a cut key reads it.
static HEADLESS_BLOCK: LazyLock<Regex> =
    LazyLock::new(|| byte_rule(r"\A[A-Za-z0-9+/=\r\n ]*-----END [A-Z ]*PRIVATE KEY-----"));

fn byte_rule(pattern: &str) -> Regex {
    // A rule that cannot compile must stop the process, not vanish.
    #[allow(clippy::expect_used)]
    Regex::new(pattern).expect("a built-in key block rule compiles")
}

/// Whether `byte` belongs to the alphabet of a key-shaped token, which a
/// rule match can run on through for any length.
fn is_key_byte(byte: u8) -> bool {
    byte.is_ascii_alphanumeric() || b"_-.~+/=".contains(&byte)
}

/// Whether `byte` may stand between a key name and its value: the rules'
/// `\s` takes Unicode blank space too, every byte of which is non-ASCII.
fn is_gap_byte(byte: u8) -> bool {
    !byte.is_ascii() || b" \t\n\r\x0b\x0c\"':".contains(&byte)
}

impl Redactor {
    /// How many bytes past each end of a span a window should hold, where
    /// the source has them, for [`Self::clear_span`] to see a secret whole.
    pub fn cut_context(&self) -> usize {
        CUT_CONTEXT_BYTES.max(self.longest_known)
    }

    /// `start..end` of `window` moved off every secret that crosses it: a
    /// private key block or a rule match. One across `start` is skipped; one
    /// across `end` is left out, or taken whole when it starts at the span's
    /// start, so a span never shrinks to nothing on a secret it starts with.
    pub fn clear_span(&self, window: &[u8], start: usize, end: usize) -> (usize, usize) {
        assert!(start <= end, "a span starts before it ends");
        assert!(end <= window.len(), "the span lies inside its window");
        let blocks = key_blocks(window);
        let clear_start = self.skip_forward(window, &blocks, start);
        let clear_end = self.clear_end(window, &blocks, clear_start, end.max(clear_start));
        debug_assert!(start <= clear_start && clear_start <= clear_end);
        debug_assert!(clear_end <= window.len());
        (clear_start, clear_end)
    }

    /// The first point from `at` on that no secret crosses.
    fn skip_forward(&self, window: &[u8], blocks: &[Range<usize>], mut at: usize) -> usize {
        for _ in 0..MAX_PASSES {
            let Some(crossing) = self.across(window, blocks, at) else {
                break;
            };
            debug_assert!(crossing.end > at, "each pass moves forward");
            at = crossing.end;
        }
        at
    }

    /// `end` moved back before every secret that crosses it, or past one
    /// that reaches back to `start`, a point nothing crosses.
    fn clear_end(
        &self,
        window: &[u8],
        blocks: &[Range<usize>],
        start: usize,
        mut end: usize,
    ) -> usize {
        debug_assert!(start <= end, "the end is sought from the cleared start on");
        for _ in 0..MAX_PASSES {
            let Some(crossing) = self.across(window, blocks, end) else {
                break;
            };
            if crossing.start <= start {
                return self.skip_forward(window, blocks, crossing.end);
            }
            debug_assert!(crossing.start < end, "each pass moves back");
            end = crossing.start;
        }
        end
    }

    /// The hull of every secret with `at` strictly inside it, if any.
    fn across(&self, window: &[u8], blocks: &[Range<usize>], at: usize) -> Option<Range<usize>> {
        if at == 0 || at >= window.len() {
            return None;
        }
        let mut hull: Option<Range<usize>> = None;
        let crossing = blocks
            .iter()
            .cloned()
            .chain(self.matches_near(window, at))
            .filter(|found| found.start < at && at < found.end);
        for found in crossing {
            hull = Some(match hull {
                Some(seen) => seen.start.min(found.start)..seen.end.max(found.end),
                None => found,
            });
        }
        debug_assert!(hull.as_ref().is_none_or(|h| h.start < at && at < h.end));
        hull
    }

    /// Every text rule's matches within [`Self::cut_context`] of `cut`, and
    /// on over a token or blank gap at either edge, as ranges of `window`.
    /// Text runs between invalid bytes are searched apart; private key
    /// blocks are left to [`key_blocks`].
    fn matches_near(&self, window: &[u8], cut: usize) -> Vec<Range<usize>> {
        let context = self.cut_context();
        let low = reach_back(window, cut.saturating_sub(context));
        let high = reach_forward(window, cut.saturating_add(context).min(window.len()));
        let mut found = Vec::new();
        let mut at = low;
        for chunk in window[low..high].utf8_chunks() {
            let text = chunk.valid();
            if text.len() >= self.shortest_match {
                let rules = self
                    .patterns
                    .iter()
                    .filter(|(_, mask)| *mask != PRIVATE_KEY_MASK);
                for (rule, _) in rules {
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

/// Where a match crossing `at` may begin: back over a token `at` lies in,
/// then over gaps and runs in turn, so a long blank gap hides no key name.
fn reach_back(window: &[u8], mut at: usize) -> usize {
    assert!(at <= window.len(), "the reach starts inside the window");
    for hop in 0..REACH_HOPS {
        let class = if hop % 2 == 0 {
            is_key_byte
        } else {
            is_gap_byte
        };
        at = window[..at]
            .iter()
            .rposition(|&byte| !class(byte))
            .map_or(0, |before| before + 1);
    }
    at
}

/// Where a match crossing `at` may end, as [`reach_back`] reads forward.
fn reach_forward(window: &[u8], mut at: usize) -> usize {
    assert!(at <= window.len(), "the reach starts inside the window");
    for hop in 0..REACH_HOPS {
        let class = if hop % 2 == 0 {
            is_key_byte
        } else {
            is_gap_byte
        };
        at = window[at..]
            .iter()
            .position(|&byte| !class(byte))
            .map_or(window.len(), |after| at + after);
    }
    at
}

/// Every private key block in `window`, in order: BEGIN to its END, or to
/// the window's end without one, and a lead that ends in an END.
fn key_blocks(window: &[u8]) -> Vec<Range<usize>> {
    let mut blocks = Vec::new();
    let mut at = 0;
    if let Some(headless) = HEADLESS_BLOCK.find(window) {
        blocks.push(0..headless.end());
        at = headless.end();
    }
    while let Some(begin) = BLOCK_BEGIN.find_at(window, at) {
        let end = BLOCK_END
            .find_at(window, begin.end())
            .map_or(window.len(), |end| end.end());
        blocks.push(begin.start()..end);
        at = end;
    }
    debug_assert!(blocks.windows(2).all(|pair| pair[0].end <= pair[1].start));
    blocks
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

    #[test]
    fn a_long_run_that_is_no_key_is_cut_anywhere() {
        let redactor = Redactor::default();
        let text = format!("x {} tail", "A".repeat(40_000));
        let spans = [(0, 4), (20_000, 20_008), (39_998, 40_006)];
        for (start, end) in spans {
            assert_eq!(
                redactor.clear_span(text.as_bytes(), start, end),
                (start, end)
            );
        }
    }

    #[test]
    fn a_long_unicode_blank_gap_does_not_hide_a_bearer_name() {
        let redactor = Redactor::default();
        let gap = "\u{2003}\u{0b}".repeat(4_500);
        let text = format!("log Bearer{gap}0123456789abcdef tail");
        let value = 10 + gap.len()..10 + gap.len() + 16;
        assert!(
            gap.len() > CUT_CONTEXT_BYTES,
            "the name lies beyond the context"
        );
        let (start, end) = redactor.clear_span(text.as_bytes(), value.start, value.end);
        assert_eq!((start, end), (value.end, value.end), "the value is skipped");
    }

    #[test]
    fn a_long_blank_gap_does_not_hide_a_bearer_name() {
        let redactor = Redactor::default();
        let text = format!("log Bearer{}0123456789abcdef tail", " ".repeat(20_000));
        let value = 20_010..20_026;
        let (start, end) = redactor.clear_span(text.as_bytes(), value.start, value.end);
        assert_eq!((start, end), (value.end, value.end), "the value is skipped");
        let (_, end) = redactor.clear_span(text.as_bytes(), 0, 10_000);
        assert_eq!(end, 4, "a span ending in the gap leaves the name out");
    }

    #[test]
    fn a_token_longer_than_the_context_is_hidden_from_its_middle() {
        let redactor = Redactor::default();
        let token = format!("sk-ant-{}", "A".repeat(40_000));
        let text = format!("x {token} tail");
        let token_end = 2 + token.len();
        let (start, end) = redactor.clear_span(text.as_bytes(), 20_000, 20_008);
        assert_eq!(
            (start, end),
            (token_end, token_end),
            "nothing of it is shown"
        );
        let (_, end) = redactor.clear_span(text.as_bytes(), 0, 20_000);
        assert_eq!(end, 2, "a page ends before it");
    }

    #[test]
    fn a_cut_inside_a_key_block_keeps_the_text_after_its_end() {
        let redactor = Redactor::default();
        let text = "head -----BEGIN PRIVATE KEY-----\nQUJDREVGR0hJSktMTU5PUA==\n\
                    -----END PRIVATE KEY----- TAIL MUST SURVIVE";
        let block = 5..text.find(" TAIL").unwrap();
        for cut in block.start + 1..block.end {
            let (start, _) = redactor.clear_span(text.as_bytes(), cut, cut);
            assert_eq!(start, block.end, "a span from {cut} starts after the block");
            let (_, end) = redactor.clear_span(text.as_bytes(), 0, cut);
            assert_eq!(end, block.start, "a span to {cut} ends before the block");
        }
    }

    #[test]
    fn an_invalid_byte_inside_a_key_block_does_not_hide_it() {
        let redactor = Redactor::default();
        let text = b"-----BEGIN PRIVATE KEY-----\n\xffsensitive!payload!0123456789\n\
                     -----END PRIVATE KEY----- tail";
        let block_end = text.len() - b" tail".len();
        assert_eq!(redactor.clear_span(text, 29, 45), (block_end, block_end));
        let (_, end) = redactor.clear_span(text, 0, 45);
        assert_eq!(
            end, block_end,
            "a span from the block's start takes it whole"
        );
        assert_eq!(
            redactor.redact_bytes(&text[..end]),
            b"[REDACTED:private_key]".as_slice()
        );
    }
}
