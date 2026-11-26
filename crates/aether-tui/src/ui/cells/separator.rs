//! Cell family: turn separator: `── Worked for 6m 14s • $1.84 • 5 agents • 14:32 ──` all dim. Owner: W2.
//! Handles `CellModel::Separator`.

use ratatui::text::Line;
use unicode_width::UnicodeWidthStr;

use super::text::{money, plain, span, truncate};
use crate::app::transcript::CellModel;
use crate::format;
use crate::ui::theme::tokens::Theme;

pub fn display_lines(cell: &CellModel, width: u16, theme: &Theme) -> Vec<Line<'static>> {
    let CellModel::Separator {
        elapsed_ms,
        cost,
        agents,
        finished_at,
    } = cell
    else {
        return Vec::new();
    };
    let width = usize::from(width);
    let g = theme.glyphs();
    let mut parts = vec![format!("Worked for {}", format::took(*elapsed_ms))];
    parts.extend(cost.map(money));
    if *agents > 1 {
        parts.push(format!("{agents} agents"));
    }
    parts.push(
        finished_at
            .with_timezone(&chrono::Local)
            .format("%H:%M")
            .to_string(),
    );
    let rule = g.rule;
    let lead = format!("{rule}{rule} {} ", parts.join(&format!(" {} ", g.bullet)));
    let fill = width.saturating_sub(lead.width()) / rule.width().max(1);
    let row = vec![
        span(lead, theme.rule()),
        span(rule.repeat(fill), theme.rule()),
    ];
    vec![Line::from(truncate(row, width, theme))]
}

pub fn detail_lines(cell: &CellModel, width: u16, theme: &Theme) -> Vec<Line<'static>> {
    display_lines(cell, width, theme)
}

pub fn plain_lines(cell: &CellModel, width: u16) -> Vec<String> {
    plain(&display_lines(cell, width, &super::plain_theme()))
}

/// `── 3 new ──────` in the accent hue: where what the operator has not seen begins.
pub fn new_divider(count: usize, width: u16, theme: &Theme) -> Line<'static> {
    let rule = theme.glyphs().rule;
    let lead = format!("{rule}{rule} {count} new ");
    let fill = usize::from(width).saturating_sub(lead.width()) / rule.width().max(1);
    let style = theme.accent();
    let row = vec![
        span(lead, style.patch(theme.title())),
        span(rule.repeat(fill), style),
    ];
    Line::from(truncate(row, usize::from(width), theme))
}
