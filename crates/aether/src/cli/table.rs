//! Fixed-width table cells measured in terminal columns, so CJK and emoji
//! text lines up like Latin text and `--ascii` output stays ASCII.

use unicode_width::UnicodeWidthStr as _;

/// How a table spells what is not text: the cut mark and the separator.
#[derive(Clone, Copy)]
pub(super) struct Glyphs {
    ellipsis: &'static str,
    separator: &'static str,
}

impl Glyphs {
    pub(super) fn new(ascii: bool) -> Self {
        if ascii {
            Self {
                ellipsis: "...",
                separator: "-",
            }
        } else {
            Self {
                ellipsis: "…",
                separator: "·",
            }
        }
    }

    pub(super) fn separator(self) -> &'static str {
        self.separator
    }
}

/// True when the locale names a non-UTF-8 charset, as `LC_ALL=C` does.
pub(super) fn locale_is_ascii(
    lc_all: Option<&str>,
    lc_ctype: Option<&str>,
    lang: Option<&str>,
) -> bool {
    let named = [lc_all, lc_ctype, lang]
        .into_iter()
        .flatten()
        .find(|value| !value.is_empty());
    named.is_some_and(|value| {
        let lower = value.to_ascii_lowercase();
        !(lower.contains("utf-8") || lower.contains("utf8"))
    })
}

/// `text` on one line, cut to at most `width` columns, then padded with
/// spaces to exactly `width` columns on the side opposite `align_right`.
pub(super) fn cell(text: &str, width: usize, glyphs: Glyphs, align_right: bool) -> String {
    assert!(
        width >= glyphs.ellipsis.len(),
        "a column must fit its own ellipsis"
    );
    let line = super::sane(text).replace('\n', " ");
    let fitted = fit(&line, width, glyphs.ellipsis);
    let padding = " ".repeat(width.saturating_sub(fitted.width()));
    if align_right {
        format!("{padding}{fitted}")
    } else {
        format!("{fitted}{padding}")
    }
}

/// Columns a [`cell`] needs to show `header` and every value uncut, but no
/// more than `max`, so one long value cannot push the rest off screen.
pub(super) fn column_width<'a>(
    header: &str,
    values: impl IntoIterator<Item = &'a str>,
    max: usize,
) -> usize {
    let widest = values
        .into_iter()
        .map(|value| super::sane(value).replace('\n', " ").width())
        .fold(header.width(), usize::max);
    assert!(header.width() <= max, "a header is never cut");
    widest.min(max)
}

/// A count right-aligned to `width`; never cut, so a large total stays exact.
pub(super) fn number(value: u64, width: usize) -> String {
    format!("{value:>width$}")
}

fn fit(line: &str, width: usize, ellipsis: &str) -> String {
    if line.width() <= width {
        return line.to_owned();
    }
    let room = width - ellipsis.width();
    let mut kept = String::new();
    for c in line.chars() {
        kept.push(c);
        // Measured as a string: emoji presentation sequences are not the sum of their chars.
        if kept.width() > room {
            kept.pop();
            break;
        }
    }
    kept.push_str(ellipsis);
    kept
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn columns_line_up_by_display_width() {
        let glyphs = Glyphs::new(false);
        for name in [
            "plain",
            "日本語",
            "🦀🦀",
            "a日b",
            "a very long agent name indeed",
        ] {
            let shown = cell(name, 10, glyphs, false);
            assert_eq!(shown.width(), 10, "{name:?} -> {shown:?}");
        }
        assert_eq!(cell("日本語日本語", 7, glyphs, false), "日本語…");
    }

    #[test]
    fn a_wide_character_is_never_split_across_the_cut() {
        let shown = cell("日日日日", 6, Glyphs::new(true), false);
        assert_eq!(shown, "日... ", "{shown:?}");
        assert!(shown.width() <= 6);
    }

    #[test]
    fn ascii_glyphs_leave_only_ascii_around_ascii_text() {
        let glyphs = Glyphs::new(true);
        let shown = cell("a name that is far too long", 12, glyphs, false);
        assert!(shown.is_ascii(), "{shown:?}");
        assert!(shown.ends_with("..."), "{shown:?}");
        assert!(glyphs.separator().is_ascii());
    }

    #[test]
    fn emoji_presentation_sequences_never_overflow_the_cell() {
        let shown = cell(&"#\u{FE0F}".repeat(8), 14, Glyphs::new(false), false);
        assert_eq!(shown.width(), 14, "{shown:?}");
    }

    #[test]
    fn a_large_count_keeps_every_digit() {
        assert_eq!(number(123_456_789, 8), "123456789");
        assert_eq!(number(42, 5), "   42");
    }

    #[test]
    fn a_column_fits_its_longest_value_up_to_its_cap() {
        assert_eq!(
            column_width("provider", ["vllm", "openai-responses"], 40),
            16
        );
        assert_eq!(
            column_width("status", ["ok"], 40),
            6,
            "never narrower than its header"
        );
        assert_eq!(
            column_width("name", ["日本語"], 40),
            6,
            "measured in columns"
        );
        assert_eq!(column_width("name", ["x".repeat(99).as_str()], 40), 40);
    }

    #[test]
    fn right_aligned_cells_pad_on_the_left() {
        assert_eq!(cell("42", 5, Glyphs::new(false), true), "   42");
    }

    #[test]
    fn only_a_named_non_utf8_locale_means_ascii() {
        assert!(locale_is_ascii(Some("C"), None, Some("en_US.UTF-8")));
        assert!(!locale_is_ascii(None, None, Some("en_US.UTF-8")));
        assert!(!locale_is_ascii(Some(""), Some("de_DE.utf8"), None));
        assert!(!locale_is_ascii(None, None, None));
    }
}
