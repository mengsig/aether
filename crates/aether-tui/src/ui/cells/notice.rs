//! Cell family: notices (`• ` dim sentence), errors (`✗` red, `└` cause, hint) and compaction. Owner: W2.
//! Handles `CellModel::{Notice, Error, Compaction}`.

use std::borrow::Cow;

use aether_core::note::NoteLevel;
use ratatui::text::Line;

use unicode_width::UnicodeWidthStr;

use super::text::{
    Layout, cap_layout, clean, plain, span, truncate, under, width_of, wrap_text_layout,
};
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
            kind: kind @ (NoticeKind::Trigger { .. } | NoticeKind::Note(_) | NoticeKind::Echo),
            text,
        } => post(kind, text, width, theme, detail),
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

/// A posted notice: a dashed header naming its kind and subject, then the
/// body behind a gutter. Triggers keep to one body row until details open.
fn post(
    kind: &NoticeKind,
    notice: &NoticeText,
    width: usize,
    theme: &Theme,
    detail: bool,
) -> Layout {
    let text = authored(notice, theme);
    let Post {
        kind: label,
        id,
        body,
    } = Post::of(kind, &text, theme);
    let hue = match kind {
        NoticeKind::Note(NoteLevel::Info) => theme.accent(),
        NoticeKind::Note(NoteLevel::Applied) => theme.success(),
        NoticeKind::Note(NoteLevel::Refused) => theme.failure(),
        _ => theme.secondary(),
    };
    let mut rows = Layout::unmapped(vec![dashed_header(
        &label,
        id.as_deref(),
        hue,
        width,
        theme,
    )]);
    let body = clean(&body);
    if body.trim().is_empty() && !(detail && notice.detail_payload().is_some()) {
        return rows;
    }
    let gutter = vec![span(format!("{} ", theme.glyphs().pipe), theme.rule())];
    let style = match kind {
        NoticeKind::Note(_) => theme.text(),
        _ => theme.secondary(),
    };
    if matches!(kind, NoticeKind::Trigger { .. }) && !detail {
        let flat = body.split_whitespace().collect::<Vec<_>>().join(" ");
        let mut row = gutter.clone();
        row.push(span(flat, style));
        rows.push(Line::from(truncate(row, width, theme)), Default::default());
        return rows;
    }
    rows.append(wrap_text_layout(
        &body,
        style,
        width,
        gutter.clone(),
        gutter.clone(),
    ));
    if detail && let Some(payload) = notice.detail_payload() {
        rows.append(wrap_text_layout(
            payload,
            theme.secondary(),
            width,
            gutter.clone(),
            gutter,
        ));
    }
    rows
}

/// What a posted notice says it is, who it is about, and what it says.
struct Post {
    kind: String,
    id: Option<String>,
    body: String,
}

impl Post {
    fn of(kind: &NoticeKind, text: &str, theme: &Theme) -> Self {
        let (source, rest) = match kind {
            NoticeKind::Trigger { .. } => {
                let (head, rest) = text
                    .split_once(&format!(" {} ", theme.glyphs().dot))
                    .unwrap_or((text, ""));
                match head.strip_prefix("Trigger from ") {
                    Some(source) => (Some(source), rest),
                    None => (None, text),
                }
            }
            _ => (None, text),
        };
        if let Some((kind, id, body)) = titled(rest) {
            return Self {
                kind: kind.to_owned(),
                id: Some(id.to_owned()),
                body: body.to_owned(),
            };
        }
        let label = match kind {
            NoticeKind::Trigger { .. } => "Trigger",
            NoticeKind::Echo => "Command",
            NoticeKind::Note(NoteLevel::Info) => "Note",
            NoticeKind::Note(NoteLevel::Applied) => "Applied",
            NoticeKind::Note(NoteLevel::Refused) => "Refused",
            _ => "Notice",
        };
        Self {
            kind: label.to_owned(),
            id: source.filter(|s| !s.is_empty()).map(str::to_owned),
            body: rest.to_owned(),
        }
    }
}

/// `Outcome (qi_01…): Done: …` as `("Outcome", "qi_01…", "Done: …")`.
fn titled(text: &str) -> Option<(&str, &str, &str)> {
    let (kind, rest) = text.split_once(" (")?;
    let (id, body) = rest.split_once(')')?;
    let word = |c: char| c.is_alphabetic() || c == ' ';
    let kind_ok = kind.chars().next().is_some_and(char::is_uppercase)
        && kind.chars().all(word)
        && kind.len() <= 24;
    let id_ok = !id.is_empty() && id.len() <= 64 && !id.contains(char::is_whitespace);
    if !(kind_ok && id_ok) {
        return None;
    }
    let body = match body.strip_prefix(':') {
        Some(body) => body,
        None if body.is_empty() => body,
        None => return None,
    };
    Some((kind, id, body.trim_start()))
}

/// `╌╌ Outcome · qi_01M8… ╌╌╌╌╌╌╌╌`: the kind, the subject cut with an
/// ellipsis rather than wrapped, and dashes out to the full width.
fn dashed_header(
    kind: &str,
    id: Option<&str>,
    hue: ratatui::style::Style,
    width: usize,
    theme: &Theme,
) -> Line<'static> {
    let g = theme.glyphs();
    let lead = format!("{0}{0} ", g.dash);
    // Room for the id: the lead, the kind, ` · `, a space and two dashes stay.
    let fixed = lead.width() + kind.width() + 3 + 1 + 2;
    let room = width.saturating_sub(fixed);
    let id = id
        .filter(|_| room >= 2)
        .map(|id| truncate(vec![span(clean(id), theme.secondary())], room, theme));
    let mut row = vec![
        span(lead, theme.rule()),
        span(kind.to_owned(), hue.patch(theme.title())),
    ];
    if let Some(id) = id {
        row.push(span(format!(" {} ", g.dot), theme.rule()));
        row.extend(id);
    }
    row.push(span(" ", theme.rule()));
    let used = width_of(&row);
    row.push(span(
        g.dash
            .repeat(width.saturating_sub(used) / g.dash.width().max(1)),
        theme.rule(),
    ));
    Line::from(truncate(row, width, theme))
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
