//! Cell family: edited: `• Edited <file> (+a -d)` with a capped diff preview. Owner: W2.
//! Handles `CellModel::Edited`.

use ratatui::text::Line;

use super::text::{Spans, child_prefix, clean, hidden_marker, plain, span, truncate, wrap};
use super::{PREVIEW_MAX_ROWS, Tone, bullet, verb_style};
use crate::app::transcript::{CallStatus, CellModel, DiffLine, DiffSign, FileChange};
use crate::ui::theme::tokens::{Theme, ToolKind};

pub fn display_lines(cell: &CellModel, width: u16, theme: &Theme) -> Vec<Line<'static>> {
    lines(cell, usize::from(width), theme, false)
}

/// Details: every diff line, context included, for every file.
pub fn detail_lines(cell: &CellModel, width: u16, theme: &Theme) -> Vec<Line<'static>> {
    lines(cell, usize::from(width), theme, true)
}

pub fn plain_lines(cell: &CellModel, width: u16) -> Vec<String> {
    plain(&display_lines(cell, width, &super::plain_theme()))
}

fn lines(cell: &CellModel, width: usize, theme: &Theme, detail: bool) -> Vec<Line<'static>> {
    let CellModel::Edited { files, status } = cell else {
        return Vec::new();
    };
    let mut head = vec![bullet(theme, Tone::of(*status)), span(" ", theme.text())];
    let [file] = files.as_slice() else {
        let (added, removed) = files
            .iter()
            .fold((0, 0), |(a, d), f| (a + f.added, d + f.removed));
        head.push(span(
            verb(*status, None),
            verb_style(theme, ToolKind::Edit, *status),
        ));
        head.push(span(format!(" {} files ", files.len()), theme.text()));
        head.extend(counts(added, removed, theme));
        let mut rows = vec![Line::from(truncate(head, width, theme))];
        for (i, file) in files.iter().enumerate() {
            let mut row = vec![
                child_prefix(theme, i == 0),
                span(clean(&file.path), theme.command()),
            ];
            row.push(span(" ", theme.text()));
            row.extend(counts(file.added, file.removed, theme));
            rows.push(Line::from(truncate(row, width, theme)));
            if detail {
                rows.extend(diff_rows(&file.diff, width, theme, true, "      "));
            }
        }
        return rows;
    };
    let verb_style = verb_style(theme, ToolKind::Edit, *status);
    head.push(span(verb(*status, Some(file.change)), verb_style));
    head.push(span(" ", theme.text()));
    head.push(span(clean(&file.path), theme.command()));
    head.push(span(" ", theme.text()));
    head.extend(counts(file.added, file.removed, theme));
    let mut rows = wrap(head, width, Vec::new(), vec![child_prefix(theme, false)]);
    let mut diff = diff_rows(&file.diff, width, theme, detail, "");
    if !detail && diff.len() > PREVIEW_MAX_ROWS {
        let hidden = diff.len() - (PREVIEW_MAX_ROWS - 1);
        diff.truncate(PREVIEW_MAX_ROWS - 1);
        diff.push(Line::from(hidden_marker(hidden, theme)));
    }
    for (i, mut row) in diff.into_iter().enumerate() {
        row.spans.insert(0, child_prefix(theme, i == 0));
        rows.push(row);
    }
    rows
}

fn verb(status: CallStatus, change: Option<FileChange>) -> &'static str {
    match (status, change) {
        (CallStatus::Failed, _) => "Failed to edit",
        (CallStatus::Denied, _) => "Denied edit",
        (CallStatus::Cancelled, _) => "Interrupted edit",
        (CallStatus::Awaiting, _) => "Wants to edit",
        (_, Some(FileChange::Added)) => "Added",
        (_, Some(FileChange::Deleted)) => "Deleted",
        (CallStatus::Running, _) => "Editing",
        _ => "Edited",
    }
}

/// `(+38 -4)`: dim parentheses, green additions, red removals.
fn counts(added: u32, removed: u32, theme: &Theme) -> Spans {
    vec![
        span("(", theme.secondary()),
        span(format!("+{added}"), theme.diff_add()),
        span(" ", theme.text()),
        span(format!("-{removed}"), theme.diff_remove()),
        span(")", theme.secondary()),
    ]
}

/// ` 41 +text` rows; context lines only in details. `indent` precedes each row.
fn diff_rows(
    diff: &[DiffLine],
    width: usize,
    theme: &Theme,
    context: bool,
    indent: &str,
) -> Vec<Line<'static>> {
    let shown: Vec<&DiffLine> = diff
        .iter()
        .filter(|l| context || l.sign != DiffSign::Context)
        .collect();
    let digits = shown
        .iter()
        .filter_map(|l| l.line_no)
        .max()
        .map_or(1, |n| n.to_string().len())
        .max(3);
    shown
        .into_iter()
        .map(|line| {
            let number = line
                .line_no
                .map_or_else(|| " ".repeat(digits), |n| format!("{n:>digits$}"));
            let (sign, style) = match line.sign {
                DiffSign::Add => ('+', theme.diff_add()),
                DiffSign::Remove => ('-', theme.diff_remove()),
                DiffSign::Context => (' ', theme.secondary()),
            };
            let row = vec![
                span(indent, theme.text()),
                span(format!("{number} "), theme.secondary()),
                span(format!("{sign}{}", clean(&line.text)), style),
            ];
            // Room for the `└ ` gutter the caller adds.
            Line::from(truncate(row, width.saturating_sub(4), theme))
        })
        .collect()
}
