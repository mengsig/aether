//! Cell family: user and user-from cells: tinted block, `› ` prompt, `task from aether · ` prefix. Owner: W2.
//! Handles `CellModel::User`.

use ratatui::text::Line;

use unicode_width::UnicodeWidthStr;

use super::text::{
    Layout, RowSource, Spans, clean, inline_code, plain, span, truncate, width_of, wrap_layout,
};
use crate::app::transcript::{CellModel, UserFrom};
use crate::ui::theme::tokens::Theme;

pub fn display_lines(cell: &CellModel, width: u16, theme: &Theme) -> Vec<Line<'static>> {
    layout(cell, width, false, theme).into_lines()
}

pub fn layout(cell: &CellModel, width: u16, _detail: bool, theme: &Theme) -> Layout {
    let CellModel::User { from, text, images } = cell else {
        return Layout::default();
    };
    let first = lead(from, theme);
    // Image lines are drawn verbatim: a click finds them by their text.
    let (words, image_lines) = crate::app::split_image_lines(text, images);
    let mut body = inline_code(&clean(&words), theme.text(), theme);
    for line in image_lines {
        let gap = if body.is_empty() { "" } else { "\n" };
        body.push(span(format!("{gap}{}", clean(&line)), theme.text()));
    }
    // The block keeps a 2-column right margin inside its tint (mock h).
    let wrap_width = usize::from(width).saturating_sub(2);
    let mut rows = Layout::default();
    rows.push(Line::default(), RowSource::default());
    rows.append(wrap_layout(
        body,
        wrap_width,
        first,
        vec![span("  ", theme.text())],
    ));
    rows.push(Line::default(), RowSource::default());
    for (row, _) in rows.rows_mut() {
        row.style = theme.tint();
    }
    rows
}

/// The pinned header row: the prompt on one tinted row, cut with an ellipsis.
pub fn pinned_line(cell: &CellModel, width: u16, theme: &Theme) -> Option<Line<'static>> {
    let CellModel::User { from, text, .. } = cell else {
        return None;
    };
    let mut spans = lead(from, theme);
    let prefix: String = text
        .chars()
        .take(usize::from(width).saturating_mul(4))
        .collect();
    let flat = clean(&prefix)
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ");
    spans.extend(inline_code(&flat, theme.secondary(), theme));
    // A trailing `…` marks an excerpt, not a prompt waiting for input.
    let room = usize::from(width).saturating_sub(1);
    let tail = format!(" {}", theme.glyphs().ellipsis);
    if width_of(&spans) + tail.width() <= room {
        spans.push(span(tail, theme.secondary()));
    } else {
        let cut = width_of(&spans).min(room).saturating_sub(1);
        spans = truncate(spans, cut, theme);
    }
    Some(Line::from(spans).style(theme.tint()))
}

/// `› ` bold in the user hue, then `task from aether · ` for work another agent sent.
fn lead(from: &UserFrom, theme: &Theme) -> Spans {
    let prompt = theme.user().patch(theme.title());
    let mut spans = vec![span(format!("{} ", theme.glyphs().prompt), prompt)];
    let dot = theme.glyphs().dot;
    match from {
        UserFrom::Operator => {}
        UserFrom::Task { from } => spans.push(span(
            format!("task from {} {dot} ", clean(from)),
            theme.secondary(),
        )),
        UserFrom::Message { from } => spans.push(span(
            format!("from {} {dot} ", clean(from)),
            theme.secondary(),
        )),
    }
    spans
}

pub fn detail_lines(cell: &CellModel, width: u16, theme: &Theme) -> Vec<Line<'static>> {
    display_lines(cell, width, theme)
}

pub fn plain_lines(cell: &CellModel, width: u16) -> Vec<String> {
    plain(&display_lines(cell, width, &super::plain_theme()))
}
