use super::Run;
use ratatui::{style::Style, text::Span};
use unicode_segmentation::UnicodeSegmentation;

use crate::ui::text_width::{display_width, grapheme_width};

/// Pad or truncate `s` to exactly `width` columns.
pub(super) fn fit(s: &str, width: usize) -> String {
    let mut out = String::new();
    let mut w = 0;
    for grapheme in s.graphemes(true) {
        let gw = grapheme_width(grapheme);
        if w + gw > width {
            break;
        }
        out.push_str(grapheme);
        w += gw;
    }
    out.push_str(&" ".repeat(width.saturating_sub(w)));
    out
}

/// Split at grapheme boundaries, keeping oversized graphemes intact on their own row.
/// A narrower terminal viewport may clip an oversized grapheme.
pub(super) fn split_width(s: &str, width: usize) -> Vec<String> {
    let mut pieces = Vec::new();
    let mut current = String::new();
    let mut w = 0;
    for grapheme in s.graphemes(true) {
        let gw = grapheme_width(grapheme);
        if w + gw > width && !current.is_empty() {
            pieces.push(std::mem::take(&mut current));
            w = 0;
        }
        current.push_str(grapheme);
        w += gw;
    }
    if !current.is_empty() || pieces.is_empty() {
        pieces.push(current);
    }
    pieces
}

/// Greedy word wrap of plain `s` into rows of at most `width` columns, splitting
/// words longer than a row. Always returns at least one row.
pub(super) fn wrap_text(s: &str, width: usize) -> Vec<String> {
    if width == 0 {
        return vec![String::new()];
    }
    let mut rows = Vec::new();
    let mut row = String::new();
    for word in s.split_whitespace() {
        let word_width = display_width(word);
        let row_width = display_width(&row);
        if !row.is_empty() && row_width + 1 + word_width <= width {
            row.push(' ');
            row.push_str(word);
            continue;
        }
        if !row.is_empty() {
            rows.push(std::mem::take(&mut row));
        }
        if word_width <= width {
            row.push_str(word);
            continue;
        }
        let mut pieces = split_width(word, width);
        row = pieces.pop().unwrap_or_default();
        rows.extend(pieces);
    }
    rows.push(row);
    rows
}

/// A word or a gap between words, possibly spanning several styles.
#[derive(Debug)]
struct Token {
    pieces: Vec<(String, Style)>,
    width: usize,
    kind: TokenKind,
}

impl Token {
    fn align_styles_to_graphemes(&mut self) {
        if self.pieces.len() > 1 {
            let text: String = self.pieces.iter().map(|(text, _)| text.as_str()).collect();
            let mut pieces: Vec<(String, Style)> = Vec::new();
            let mut piece_index = 0;
            let mut piece_end = self.pieces[0].0.len();
            // Ratatui renders spans separately, so a grapheme uses its leading style
            // even when an inline style boundary falls inside it.
            for (offset, grapheme) in text.grapheme_indices(true) {
                while offset >= piece_end {
                    piece_index += 1;
                    piece_end += self.pieces[piece_index].0.len();
                }
                let style = self.pieces[piece_index].1;
                match pieces.last_mut() {
                    Some((text, last_style)) if *last_style == style => text.push_str(grapheme),
                    _ => pieces.push((grapheme.to_owned(), style)),
                }
            }
            self.pieces = pieces;
        }
        self.width = self
            .pieces
            .iter()
            .map(|(text, _)| display_width(text))
            .sum();
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum TokenKind {
    Word,
    Space,
    Break,
}

fn tokenize(runs: &[Run]) -> Vec<Token> {
    let mut tokens: Vec<Token> = Vec::new();
    let push = |tokens: &mut Vec<Token>, text: &str, style: Style, kind: TokenKind| {
        if text.is_empty() {
            return;
        }
        let width = display_width(text);
        if let Some(last) = tokens.last_mut()
            && last.kind == kind
            && kind != TokenKind::Break
        {
            last.width += width;
            match last.pieces.last_mut() {
                Some((t, s)) if *s == style => t.push_str(text),
                _ => last.pieces.push((text.to_owned(), style)),
            }
            return;
        }
        tokens.push(Token {
            pieces: vec![(text.to_owned(), style)],
            width,
            kind,
        });
    };
    for run in runs {
        if run.text == "\n" {
            tokens.push(Token {
                pieces: Vec::new(),
                width: 0,
                kind: TokenKind::Break,
            });
            continue;
        }
        let mut word = String::new();
        let mut space = String::new();
        for c in run.text.chars() {
            if c == '\n' || c.is_whitespace() {
                if !word.is_empty() {
                    push(&mut tokens, &word, run.style, TokenKind::Word);
                    word.clear();
                }
                space.push(' ');
            } else {
                if !space.is_empty() {
                    push(&mut tokens, &space, run.style, TokenKind::Space);
                    space.clear();
                }
                word.push(c);
            }
        }
        push(&mut tokens, &word, run.style, TokenKind::Word);
        push(&mut tokens, &space, run.style, TokenKind::Space);
    }
    for token in &mut tokens {
        if token.kind == TokenKind::Word {
            token.align_styles_to_graphemes();
        }
    }
    tokens
}

/// A wrapped row, and the spaces dropped before it when it soft-wraps on from the row above.
pub(super) type Wrapped = (Vec<Span<'static>>, Option<usize>);

/// Greedy word wrap over styled runs, splitting long words at grapheme boundaries.
/// A grapheme wider than the row stays intact on its own row and may be clipped by the terminal.
pub(super) fn wrap_runs(runs: &[Run], width: usize) -> Vec<Wrapped> {
    let width = width.max(1);
    let mut lines: Vec<Wrapped> = Vec::new();
    let mut line: Wrapped = (Vec::new(), None);
    let mut line_w = 0usize;
    let mut pending_space: Option<Token> = None;
    let flush = |line: &mut Wrapped, line_w: &mut usize, lines: &mut Vec<_>, next| {
        lines.push(std::mem::replace(line, (Vec::new(), next)));
        *line_w = 0;
    };
    let push = |line: &mut Vec<Span<'static>>, text: String, style: Style| match line.last_mut() {
        Some(last) if last.style == style => last.content.to_mut().push_str(&text),
        _ => line.push(Span::styled(text, style)),
    };
    for token in tokenize(runs) {
        match token.kind {
            TokenKind::Break => {
                pending_space = None;
                flush(&mut line, &mut line_w, &mut lines, None);
            }
            TokenKind::Space => {
                if line_w > 0 {
                    pending_space = Some(token);
                }
            }
            TokenKind::Word => {
                let space_w = pending_space.as_ref().map(|t| t.width).unwrap_or(0);
                if line_w > 0 && line_w + space_w + token.width > width {
                    pending_space = None;
                    flush(&mut line, &mut line_w, &mut lines, Some(space_w));
                }
                if let Some(space) = pending_space.take() {
                    for (text, style) in space.pieces {
                        push(&mut line.0, text, style);
                    }
                    line_w += space.width;
                }
                if token.width <= width {
                    for (text, style) in token.pieces {
                        push(&mut line.0, text, style);
                    }
                    line_w += token.width;
                    continue;
                }
                for (text, style) in token.pieces {
                    for grapheme in text.graphemes(true) {
                        let gw = grapheme_width(grapheme);
                        if line_w + gw > width && line_w > 0 {
                            flush(&mut line, &mut line_w, &mut lines, Some(0));
                        }
                        push(&mut line.0, grapheme.to_owned(), style);
                        line_w += gw;
                    }
                }
            }
        }
    }
    if !line.0.is_empty() {
        lines.push(line);
    }
    lines
}

#[cfg(test)]
mod tests {
    use super::*;
    use unicode_width::UnicodeWidthStr;

    fn wrapped_text(s: &str, width: usize) -> Vec<String> {
        wrap_runs(
            &[Run {
                text: s.to_owned(),
                style: Style::default(),
            }],
            width,
        )
        .iter()
        .map(|(spans, _)| spans.iter().map(|span| span.content.as_ref()).collect())
        .collect()
    }

    #[test]
    fn overlong_words_use_grapheme_display_widths() {
        assert_eq!(wrapped_text("abcdef👩‍💻X", 8), ["abcdef👩‍💻", "X"]);
    }

    #[test]
    fn splitting_uses_grapheme_display_widths() {
        for cluster in ["👩‍💻", "🇩🇰", "👍🏽", "e\u{301}"] {
            let prefix = "a".repeat(8 - cluster.width());
            let text = format!("{prefix}{cluster}X");
            assert_eq!(
                split_width(&text, 8),
                [format!("{prefix}{cluster}"), "X".into()]
            );
        }
    }

    #[test]
    fn oversized_graphemes_stay_intact_on_their_own_row() {
        let expected = ["a", "👩‍💻", "X"];
        assert_eq!(split_width("a👩‍💻X", 1), expected);
        assert_eq!(wrapped_text("a👩‍💻X", 1), expected);
        assert_eq!(split_width("👩‍💻", 1), ["👩‍💻"]);
        assert_eq!(wrapped_text("👩‍💻", 1), ["👩‍💻"]);
    }

    #[test]
    fn splitting_empty_and_zero_width_input_makes_progress() {
        assert_eq!(split_width("", 0), [""]);
        assert_eq!(split_width("a👩‍💻X", 0), ["a", "👩‍💻", "X"]);
        assert_eq!(wrapped_text("a👩‍💻X", 0), ["a", "👩‍💻", "X"]);
        assert_eq!(split_width("\u{301}a", 1), ["\u{301}a"]);
        assert_eq!(wrapped_text("\u{301}ab", 1), ["\u{301}a", "b"]);
    }

    #[test]
    fn fitting_keeps_complete_graphemes() {
        assert_eq!(fit("abcdef👩‍💻X", 8), "abcdef👩‍💻");
        assert_eq!(fit("abcdef👩‍💻X", 7), "abcdef ");
        assert_eq!(fit("👩‍💻", 1), " ");
        assert_eq!(fit("👩‍💻", 0), "");
        assert_eq!(fit("e\u{301}X", 1), "e\u{301}");
    }

    #[test]
    fn control_graphemes_have_no_rendered_width() {
        assert_eq!(display_width("\0ab"), 2);
        assert_eq!(split_width("\0abc", 2), ["\0ab", "c"]);
        assert_eq!(wrapped_text("\0abc", 2), ["\0ab", "c"]);
        assert_eq!(fit("\0abc", 2), "\0ab");
    }
}
