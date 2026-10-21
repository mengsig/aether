//! Cell family: notices (`• ` dim sentence), errors (`✗` red, `└` cause, hint) and compaction. Owner: W2.
//! Handles `CellModel::{Notice, Error, Compaction}`.

use std::borrow::Cow;

use aether_core::note::NoteLevel;
use ratatui::text::Line;

use super::text::{Layout, cap_layout, clean, plain, span, truncate, under, wrap_text_layout};
use super::{Tone, bullet};
use crate::app::transcript::{CellModel, CompactionState, NoticeKind, NoticeText};
use crate::format;
use crate::ui::theme::tokens::Theme;

/// Rows an error's cause takes in the compact view.
const CAUSE_ROWS: usize = 3;

pub fn display_lines(cell: &CellModel, width: u16, theme: &Theme) -> Vec<Line<'static>> {
    layout(cell, width, false, theme).into_lines()
}

/// Details: an error's whole cause.
pub fn detail_lines(cell: &CellModel, width: u16, theme: &Theme) -> Vec<Line<'static>> {
    layout(cell, width, true, theme).into_lines()
}

pub fn plain_lines(cell: &CellModel, width: u16) -> Vec<String> {
    plain(&display_lines(cell, width, &super::plain_theme()))
}

pub fn layout(cell: &CellModel, width: u16, detail: bool, theme: &Theme) -> Layout {
    let width = usize::from(width);
    match cell {
        CellModel::Notice {
            kind: NoticeKind::ClientError,
            text,
        } => error(text, None, None, width, theme, detail),
        CellModel::Notice {
            kind: NoticeKind::Trigger { .. },
            text,
        } if !detail => one_row(&authored(text, theme), width, theme),
        CellModel::Notice {
            kind: NoticeKind::Note(level),
            text,
        } => note(*level, &authored(text, theme), width, theme),
        CellModel::Notice { text, .. } => {
            let mut rows = sentence(Tone::Neutral, &authored(text, theme), width, theme);
            if detail && let Some(payload) = text.detail_payload() {
                rows.append(sentence(Tone::Neutral, payload, width, theme));
            }
            rows
        }
        CellModel::Error {
            message,
            cause,
            hint,
        } => error(
            message,
            cause.as_deref(),
            hint.as_ref().map(|h| authored(h, theme)).as_deref(),
            width,
            theme,
            detail,
        ),
        CellModel::Compaction(state) => compaction(state, width, theme, detail),
        _ => Layout::default(),
    }
}

const COMPACTION_FAILED_HINT: &str =
    "Nothing was lost: the agent keeps its full context and carries on. Run /compact to try again.";

fn compaction(state: &CompactionState, width: usize, theme: &Theme, detail: bool) -> Layout {
    match state {
        CompactionState::Active => {
            let text = format!(
                "Compacting context {} summarizing older turns, usually within a minute or two",
                theme.glyphs().dot
            );
            let row = vec![
                bullet(theme, Tone::Active),
                span(" ", theme.text()),
                span(text, theme.text()),
            ];
            Layout::unmapped(vec![Line::from(truncate(row, width, theme))])
        }
        CompactionState::Done {
            before,
            after,
            by,
            duration_ms,
        } => {
            let dot = theme.glyphs().dot;
            let mut text = "Compacted context".to_owned();
            // An unknown side says nothing rather than guess.
            if let (Some(before), Some(after)) = (before, after) {
                text.push_str(&format!(
                    " {dot} {} {} {} tokens",
                    format::tokens(*before),
                    arrow(theme),
                    format::tokens(*after)
                ));
            }
            if let Some(by) = by {
                text.push_str(&format!(" {dot} by {}", folder_name(by)));
            }
            if let Some(ms) = duration_ms {
                text.push_str(&format!(" in {}", format::took(*ms)));
            }
            sentence(Tone::Neutral, &text, width, theme)
        }
        CompactionState::Failed { error: e } => error(
            "Compaction failed",
            Some(e),
            Some(COMPACTION_FAILED_HINT),
            width,
            theme,
            detail,
        ),
    }
}

/// The engine or provider that folded a context, as people know it.
fn folder_name(provider: &str) -> &str {
    match provider {
        "claude-code" => "Claude Code",
        "anthropic" => "the Anthropic API",
        other => other,
    }
}

fn authored<'a>(text: &'a NoticeText, theme: &Theme) -> Cow<'a, str> {
    if theme.ascii() {
        text.ascii(theme.glyphs().dot)
    } else {
        Cow::Borrowed(text)
    }
}

fn arrow(theme: &Theme) -> &'static str {
    if theme.ascii() { "->" } else { "→" }
}

/// `• ` and the sentence on one dim row, cut at the width; details wrap it whole.
fn one_row(text: &str, width: usize, theme: &Theme) -> Layout {
    let flat = clean(text).split_whitespace().collect::<Vec<_>>().join(" ");
    let row = vec![
        bullet(theme, Tone::Neutral),
        span(" ", theme.text()),
        span(flat, theme.secondary()),
    ];
    Layout::unmapped(vec![Line::from(truncate(row, width, theme))])
}

/// A label (`note`, `applied` or `refused`), then the client's text at full
/// brightness: apart from the dim status notices and from anything the agent said.
fn note(level: NoteLevel, text: &str, width: usize, theme: &Theme) -> Layout {
    let (label, hue) = match level {
        NoteLevel::Info => ("note", theme.accent()),
        NoteLevel::Applied => ("applied", theme.success()),
        NoteLevel::Refused => ("refused", theme.failure()),
    };
    let first = vec![
        span(label, hue.patch(theme.title())),
        span(" ", theme.text()),
    ];
    let indent = " ".repeat(label.len() + 1);
    wrap_text_layout(
        &clean(text),
        theme.text(),
        width,
        first,
        vec![span(indent, theme.text())],
    )
}

/// `• ` and one dim sentence, wrapped under a 2-column indent.
fn sentence(tone: Tone, text: &str, width: usize, theme: &Theme) -> Layout {
    let first = vec![bullet(theme, tone), span(" ", theme.text())];
    wrap_text_layout(
        &clean(text),
        theme.secondary(),
        width,
        first,
        vec![span("  ", theme.text())],
    )
}

/// `✗ message` red, `└ cause` dim, then the action hint dim.
fn error(
    message: &str,
    cause: Option<&str>,
    hint: Option<&str>,
    width: usize,
    theme: &Theme,
    detail: bool,
) -> Layout {
    let first = vec![
        span(theme.glyphs().failed, theme.failure().patch(theme.title())),
        span(" ", theme.text()),
    ];
    let mut rows = wrap_text_layout(
        &clean(message),
        theme.failure(),
        width,
        first,
        vec![span("  ", theme.text())],
    );
    let inner = width.saturating_sub(4);
    let mut below = Layout::default();
    if let Some(cause) = cause.filter(|c| !c.trim().is_empty()) {
        let cause = wrap_text_layout(
            &clean(cause.trim()),
            theme.secondary(),
            inner,
            Vec::new(),
            Vec::new(),
        );
        below.append(if detail {
            cause
        } else {
            cap_layout(cause, CAUSE_ROWS, inner, theme)
        });
    }
    if let Some(hint) = hint {
        below.append(wrap_text_layout(
            &clean(hint),
            theme.secondary(),
            inner,
            Vec::new(),
            Vec::new(),
        ));
    }
    rows.append(under(below, theme));
    rows
}
