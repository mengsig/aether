//! Cell family: assistant markdown: `• ` dim gutter on the first row, 2-column indent. Owner: W2.
//! Handles `CellModel::Assistant` and a host's `CellModel::Post`, whose bullet is accented.

use ratatui::text::Line;

use ratatui::text::Span;
use unicode_width::UnicodeWidthStr;

use super::text::{Layout, plain, span};
use crate::app::transcript::CellModel;
use crate::ui::markdown;
use crate::ui::theme::tokens::Theme;

pub fn display_lines(cell: &CellModel, width: u16, theme: &Theme) -> Vec<Line<'static>> {
    layout(cell, width, false, theme).into_lines()
}

pub fn layout(cell: &CellModel, width: u16, _detail: bool, theme: &Theme) -> Layout {
    let (text, bullet_style) = match cell {
        CellModel::Assistant { text } => (text, theme.secondary()),
        CellModel::Post { text } => (text, theme.accent()),
        _ => return Layout::default(),
    };
    let mut rows = markdown::render_layout(text, text_width(width), theme.text(), theme);
    let (bullet, indent) = gutters(theme, bullet_style);
    for (i, (row, source)) in rows.rows_mut().enumerate() {
        if let Some(lead) = gutter(i, row, &bullet, &indent) {
            source.prefix += lead.content.width();
            row.spans.insert(0, lead.clone());
        }
    }
    rows
}

/// Columns the reply's text wraps at, inside the 2-column gutter; the live
/// stream wraps at the same width.
pub fn text_width(width: u16) -> usize {
    usize::from(width).saturating_sub(2)
}

fn gutters(theme: &Theme, bullet: ratatui::style::Style) -> (Span<'static>, Span<'static>) {
    let bullet = span(format!("{} ", theme.glyphs().bullet), bullet);
    (bullet, span("  ", theme.text()))
}

/// `• ` before the first row, two spaces before the rest; blank rows stay empty.
fn gutter<'a>(
    row: usize,
    line: &Line<'_>,
    bullet: &'a Span<'static>,
    indent: &'a Span<'static>,
) -> Option<&'a Span<'static>> {
    if row == 0 {
        Some(bullet)
    } else {
        (!line.spans.is_empty()).then_some(indent)
    }
}

/// [`gutter`] for rows that start at row `first` of the reply.
pub fn gutter_from(
    first: usize,
    rows: Vec<Line<'static>>,
    theme: &Theme,
) -> impl Iterator<Item = Line<'static>> {
    let (bullet, indent) = gutters(theme, theme.secondary());
    rows.into_iter().enumerate().map(move |(i, mut row)| {
        if let Some(lead) = gutter(first + i, &row, &bullet, &indent) {
            row.spans.insert(0, lead.clone());
        }
        row
    })
}

pub fn detail_lines(cell: &CellModel, width: u16, theme: &Theme) -> Vec<Line<'static>> {
    display_lines(cell, width, theme)
}

pub fn plain_lines(cell: &CellModel, width: u16) -> Vec<String> {
    plain(&display_lines(cell, width, &super::plain_theme()))
}
