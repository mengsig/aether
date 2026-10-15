//! Cell family: called: MCP and web tools, `• Called server.tool(args)` with a `└` result preview;
//! memory notes `• Recalled 3 memories` / `• Remembered "…"`. Owner: W2.
//! Handles `CellModel::Called` and `CellModel::Memory`.

use ratatui::text::Line;

use super::text::{
    Layout, PreviewRow, Spans, artifact_rows, child_prefix, clean, hidden_marker, output_lines,
    plain, preview, span, truncate, wrap_layout, wrap_text_layout,
};
use super::{Tone, bullet, verb_style};
use crate::app::transcript::{CallStatus, CallTarget, CellModel, MemoryNote};
use crate::ui::theme::tokens::{Theme, ToolKind};

pub fn display_lines(cell: &CellModel, width: u16, theme: &Theme) -> Vec<Line<'static>> {
    layout(cell, width, false, theme).into_lines()
}

/// Details: the whole result and the full arguments, wrapped.
pub fn detail_lines(cell: &CellModel, width: u16, theme: &Theme) -> Vec<Line<'static>> {
    layout(cell, width, true, theme).into_lines()
}

pub fn plain_lines(cell: &CellModel, width: u16) -> Vec<String> {
    plain(&display_lines(cell, width, &super::plain_theme()))
}

pub fn layout(cell: &CellModel, width: u16, detail: bool, theme: &Theme) -> Layout {
    let width = usize::from(width);
    match cell {
        CellModel::Called {
            target,
            args,
            result,
            status,
            error,
            artifacts,
        } => {
            let mut rows = head(target, args, *status, width, theme, detail);
            match (status, error) {
                (CallStatus::Failed | CallStatus::Denied | CallStatus::Cancelled, Some(error)) => {
                    let text = format!("Error: {}", clean(error));
                    if detail {
                        rows.append(wrap_text_layout(
                            &text,
                            theme.failure(),
                            width,
                            vec![child_prefix(theme, true)],
                            vec![child_prefix(theme, false)],
                        ));
                    } else {
                        let first = text.lines().next().unwrap_or_default().to_owned();
                        let row = vec![child_prefix(theme, true), span(first, theme.failure())];
                        rows.push_row(row, width, theme);
                    }
                }
                _ => rows.append(result_rows(target, result, width, theme, detail)),
            }
            let artifacts = artifact_rows(artifacts, width, detail, theme);
            rows.append(Layout::unmapped(artifacts));
            rows
        }
        CellModel::Memory(note) => memory(note, width, theme, detail),
        _ => Layout::default(),
    }
}

fn head(
    target: &CallTarget,
    args: &str,
    status: CallStatus,
    width: usize,
    theme: &Theme,
    detail: bool,
) -> Layout {
    let (forms, kind, rest): (Forms, ToolKind, Spans) = match target {
        CallTarget::Mcp { server, tool } => (
            ("Calling", "Called", "call"),
            ToolKind::Call,
            call_spans(&format!("{server}.{tool}"), args, theme),
        ),
        CallTarget::Tool { name } => (
            ("Calling", "Called", "call"),
            ToolKind::Call,
            call_spans(name, args, theme),
        ),
        CallTarget::WebSearch { .. } => (
            ("Searching the web", "Searched the web", "search the web"),
            ToolKind::Web,
            Vec::new(),
        ),
        CallTarget::WebFetch { url } => (
            ("Fetching", "Fetched", "fetch"),
            ToolKind::Web,
            vec![span(clean(&short_url(url)), theme.command())],
        ),
    };
    let verb = verb(status, forms);
    let mut row = vec![
        bullet(theme, Tone::of(status)),
        span(" ", theme.text()),
        span(verb, verb_style(theme, kind, status)),
    ];
    if !rest.is_empty() {
        row.push(span(" ", theme.text()));
        row.extend(rest);
    }
    if detail {
        wrap_layout(row, width, Vec::new(), vec![child_prefix(theme, false)])
    } else {
        Layout::unmapped(vec![Line::from(truncate(row, width, theme))])
    }
}

/// A call's verb as present participle, past tense and bare infinitive.
type Forms = (&'static str, &'static str, &'static str);

/// The past tense only once the call succeeded; it never claims what did not happen.
fn verb(status: CallStatus, (doing, done, bare): Forms) -> String {
    match status {
        CallStatus::Awaiting => format!("Wants to {bare}"),
        CallStatus::Running => doing.to_owned(),
        CallStatus::Succeeded => done.to_owned(),
        CallStatus::Failed => format!("Failed to {bare}"),
        CallStatus::Denied => format!("Not allowed to {bare}"),
        CallStatus::Cancelled => format!("Interrupted {}", doing.to_lowercase()),
    }
}

/// `server.tool(arg: "…")`, arguments dim.
fn call_spans(name: &str, args: &str, theme: &Theme) -> Spans {
    vec![
        span(clean(name), theme.command()),
        span(format!("({})", clean(args)), theme.secondary()),
    ]
}

/// `https://docs.rs/x/latest` → `docs.rs/x/latest`.
fn short_url(url: &str) -> String {
    let rest = url.split_once("://").map_or(url, |(_, rest)| rest);
    rest.trim_end_matches('/').to_owned()
}

fn result_rows(
    target: &CallTarget,
    result: &str,
    width: usize,
    theme: &Theme,
    detail: bool,
) -> Layout {
    // A web search leads with its query; the compact view shows nothing else.
    let mut texts: Vec<String> = match target {
        CallTarget::WebSearch { query } => vec![clean(query)],
        _ => Vec::new(),
    };
    if detail || texts.is_empty() {
        let result = clean(result);
        texts.extend(output_lines(&result).into_iter().map(str::to_owned));
    }
    let mut rows = Layout::default();
    if detail {
        for (i, line) in texts.iter().enumerate() {
            rows.append(wrap_text_layout(
                line,
                theme.secondary(),
                width,
                vec![child_prefix(theme, i == 0)],
                vec![child_prefix(theme, false)],
            ));
        }
        return rows;
    }
    let refs: Vec<&str> = texts.iter().map(String::as_str).collect();
    for (i, row) in preview(&refs).into_iter().enumerate() {
        let body = match row {
            PreviewRow::Line(text) => span(text, theme.secondary()),
            PreviewRow::Hidden(n) => hidden_marker(n, theme),
        };
        rows.push_row(vec![child_prefix(theme, i == 0), body], width, theme);
    }
    rows
}

fn memory(note: &MemoryNote, width: usize, theme: &Theme, detail: bool) -> Layout {
    let lead = vec![bullet(theme, Tone::Neutral), span(" ", theme.text())];
    match note {
        MemoryNote::Recalled { names } => {
            let noun = if names.len() == 1 {
                "memory"
            } else {
                "memories"
            };
            let mut row = lead;
            row.push(span(
                format!("Recalled {} {noun}", names.len()),
                theme.secondary(),
            ));
            let mut rows = Layout::unmapped(vec![Line::from(truncate(row, width, theme))]);
            if detail && !names.is_empty() {
                rows.append(wrap_text_layout(
                    &clean(&names.join(", ")),
                    theme.secondary(),
                    width,
                    vec![child_prefix(theme, true)],
                    vec![child_prefix(theme, false)],
                ));
            }
            rows
        }
        MemoryNote::Remembered { text } => {
            let quoted = format!("Remembered \"{}\"", clean(text).trim());
            if detail {
                wrap_text_layout(
                    &quoted,
                    theme.secondary(),
                    width,
                    lead,
                    vec![span("  ", theme.text())],
                )
            } else {
                let first = quoted.lines().next().unwrap_or_default().to_owned();
                let mut row = lead;
                row.push(span(first, theme.secondary()));
                Layout::unmapped(vec![Line::from(truncate(row, width, theme))])
            }
        }
    }
}
