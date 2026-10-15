//! Cell family: exec: `• Ran <cmd>`, `└` output preview, exit/failure styling. Owner: W2.
//! Handles `CellModel::Exec`.

use std::borrow::Cow;

use ratatui::text::Line;

use super::text::{
    Layout, PreviewRow, Spans, artifact_rows, cap_layout, child_prefix, clean, hidden_marker,
    output_lines, plain, preview, span, wrap_layout, wrap_text_layout,
};
use super::{Tone, bullet, verb_style};
use crate::app::transcript::{CallStatus, CellModel, inner_command};
use crate::format;
use crate::ui::theme::tokens::{Theme, ToolKind};

/// Rows the command may take in the compact view before it is cut.
const COMMAND_ROWS: usize = 2;

pub fn display_lines(cell: &CellModel, width: u16, theme: &Theme) -> Vec<Line<'static>> {
    layout(cell, width, false, theme).into_lines()
}

pub fn layout(cell: &CellModel, width: u16, detail: bool, theme: &Theme) -> Layout {
    let CellModel::Exec { artifacts, .. } = cell else {
        return Layout::default();
    };
    let width = usize::from(width);
    let mut rows = if detail {
        full(cell, width, theme)
    } else {
        compact(cell, width, theme)
    };
    let artifacts = artifact_rows(artifacts, width, detail, theme);
    rows.append(Layout::unmapped(artifacts));
    rows
}

fn compact(cell: &CellModel, width: usize, theme: &Theme) -> Layout {
    let CellModel::Exec { output, status, .. } = cell else {
        return Layout::default();
    };
    let mut rows = cap_layout(head(cell, width, false, theme), COMMAND_ROWS, width, theme);
    let output = clean(output);
    let lines = output_lines(&output);
    if lines.is_empty() {
        if !matches!(status, CallStatus::Awaiting | CallStatus::Running) {
            let row = vec![
                child_prefix(theme, true),
                span("(no output)", theme.secondary()),
            ];
            rows.push_row(row, width, theme);
        }
        return rows;
    }
    let shown = preview(&lines);
    let last = shown.len() - 1;
    for (i, row) in shown.into_iter().enumerate() {
        let body = match row {
            PreviewRow::Hidden(n) => hidden_marker(n, theme),
            PreviewRow::Line(text) => {
                let style = if *status == CallStatus::Failed && i == last {
                    theme.failure()
                } else {
                    theme.secondary()
                };
                span(text, style)
            }
        };
        rows.push_row(vec![child_prefix(theme, i == 0), body], width, theme);
    }
    rows
}

pub fn detail_lines(cell: &CellModel, width: u16, theme: &Theme) -> Vec<Line<'static>> {
    layout(cell, width, true, theme).into_lines()
}

/// Details: the whole command in the head, every output line, then `✓ exit 0 • 1.2s`.
fn full(cell: &CellModel, width: usize, theme: &Theme) -> Layout {
    let CellModel::Exec {
        status,
        output,
        exit,
        duration_ms,
        ..
    } = cell
    else {
        return Layout::default();
    };
    let mut rows = head(cell, width, true, theme);
    let output = clean(output);
    let lines = output_lines(&output);
    let last = lines.len().saturating_sub(1);
    for (i, line) in lines.iter().enumerate() {
        let style = if *status == CallStatus::Failed && i == last {
            theme.failure()
        } else {
            theme.secondary()
        };
        let first = vec![child_prefix(theme, i == 0)];
        rows.append(wrap_text_layout(
            line,
            style,
            width,
            first,
            vec![child_prefix(theme, false)],
        ));
    }
    if let Some(outcome) = outcome(*status, *exit, *duration_ms, theme) {
        let mut row = vec![child_prefix(theme, lines.is_empty())];
        row.extend(outcome);
        rows.push_row(row, width, theme);
    }
    rows
}

pub fn plain_lines(cell: &CellModel, width: u16) -> Vec<String> {
    plain(&display_lines(cell, width, &super::plain_theme()))
}

/// `• Ran cargo test (exit 101)`, the command wrapped behind `│`. Only
/// details show the shell wrapper an engine ran it under.
fn head(cell: &CellModel, width: usize, detail: bool, theme: &Theme) -> Layout {
    let CellModel::Exec {
        command,
        status,
        exit,
        is_search,
        ..
    } = cell
    else {
        return Layout::default();
    };
    let verb = match status {
        CallStatus::Awaiting => "Wants to run",
        CallStatus::Running => "Running",
        CallStatus::Denied => "Denied",
        CallStatus::Cancelled => "Interrupted",
        CallStatus::Succeeded | CallStatus::Failed => "Ran",
    };
    let first = vec![
        bullet(theme, Tone::of(*status)),
        span(" ", theme.text()),
        span(verb, verb_style(theme, ToolKind::Shell, *status)),
        span(" ", theme.text()),
    ];
    let command = if detail {
        Cow::Borrowed(command.as_str())
    } else {
        inner_command(command)
    };
    let mut body = vec![span(clean(&command), theme.command())];
    if *status == CallStatus::Failed
        && let Some(code) = exit
    {
        let style = if *code == 1 || *is_search {
            theme.secondary()
        } else {
            theme.failure()
        };
        body.push(span(format!(" (exit {code})"), style));
    }
    let more = vec![span(
        format!("  {} ", theme.glyphs().pipe),
        theme.secondary(),
    )];
    wrap_layout(body, width, first, more)
}

/// `✓ exit 0 • 1.2s`; `None` while the command runs.
fn outcome(
    status: CallStatus,
    exit: Option<i32>,
    duration_ms: Option<u64>,
    theme: &Theme,
) -> Option<Spans> {
    let g = theme.glyphs();
    let (glyph, style, word) = match status {
        CallStatus::Awaiting | CallStatus::Running => return None,
        CallStatus::Succeeded => (g.done, theme.success(), None),
        CallStatus::Failed => (g.failed, theme.failure(), None),
        CallStatus::Denied => (g.failed, theme.failure(), Some("denied")),
        CallStatus::Cancelled => (g.cancelled, theme.secondary(), Some("interrupted")),
    };
    let mut parts = Vec::new();
    parts.extend(word.map(str::to_owned));
    parts.extend(exit.map(|code| format!("exit {code}")));
    parts.extend(duration_ms.map(format::duration_ms));
    let sep = format!(" {} ", g.bullet);
    Some(vec![
        span(glyph, style),
        span(" ", theme.text()),
        span(parts.join(&sep), theme.secondary()),
    ])
}
