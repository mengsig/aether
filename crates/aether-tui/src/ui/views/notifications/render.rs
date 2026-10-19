//! Painting the notification center: the grouped list, and one notification
//! with its body, its answer or the choices that answer it.

use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::text::{Line, Span};
use unicode_width::UnicodeWidthStr;

use aether_core::notification::{Notification, NotificationKind, NotificationState};

use super::super::picker::{PickerRow, RowState, label_column, picker_line};
use super::super::{
    BODY_ROWS, CHROME_ROWS, Hint, Window, body_room, clean, render_empty, render_frame, row, sep,
    title,
};
use super::{Detail, NotificationsView, agent_of, choices};
use crate::app::App;
use crate::ui::markdown;
use crate::ui::theme::tokens::Theme;

const LIST_HINTS: &[Hint] = &[
    ("↑↓", "choose"),
    ("enter", "open"),
    ("r", "read"),
    ("x", "resolve"),
    ("o", "go to chat"),
    ("esc", "back"),
];
const ANSWER_HINTS: &[Hint] = &[
    ("↑↓", "choose"),
    ("enter", "answer"),
    ("x", "resolve"),
    ("o", "go to chat"),
    ("esc", "list"),
];
const READ_HINTS: &[Hint] = &[
    ("pgup pgdn", "scroll"),
    ("x", "resolve"),
    ("o", "go to chat"),
    ("esc", "list"),
];
const DRAFT_HINTS: &[Hint] = &[("enter", "send"), ("esc", "stop writing")];
const EMPTY_TEXT: &str =
    "Nothing yet: agents post with the notify tool, services such as Archon over the socket";

/// The list wants a row per notification plus a header per non-empty state.
pub(super) fn list_height(app: &App) -> u16 {
    let rows = app.notifications.items().len() + groups(app);
    super::super::frame_height(rows)
}

pub(super) fn list(
    frame: &mut Frame,
    area: Rect,
    view: &NotificationsView,
    app: &App,
    theme: &Theme,
) {
    let heading = list_title(app, theme);
    let rows = app.notifications.grouped();
    let Some(cursor) = view.position(&rows) else {
        render_empty(frame, area, heading, EMPTY_TEXT, theme);
        return;
    };
    let entries: Vec<PickerRow> = rows.iter().map(|n| list_row(n, app, theme)).collect();
    let refs: Vec<&PickerRow> = entries.iter().collect();
    let label_width = label_column(&refs, usize::from(area.width).saturating_sub(4));
    let mut lines = Vec::with_capacity(rows.len() + 3);
    let mut cursor_line = 0;
    let mut group = None;
    for (index, n) in rows.iter().enumerate() {
        if group != Some(n.state) {
            group = Some(n.state);
            lines.push(row(state_heading(n.state), theme.secondary()));
        }
        if index == cursor {
            cursor_line = lines.len();
        }
        let state = if index == cursor {
            RowState::Selected
        } else {
            RowState::Plain
        };
        lines.push(picker_line(
            theme,
            &entries[index],
            None,
            label_width,
            state,
        ));
    }
    debug_assert_eq!(lines.len(), rows.len() + groups(app), "one header a group");
    let window = Window::following(lines.len(), body_room(area), cursor_line);
    render_frame(
        frame,
        area,
        heading,
        window.lines(lines, theme),
        LIST_HINTS,
        theme,
    );
}

/// `Notifications  3 unread · 1 waiting on you`.
fn list_title(app: &App, theme: &Theme) -> Vec<Span<'static>> {
    let center = &app.notifications;
    let mut meta = vec![format!("{} unread", center.unread())];
    if center.urgent_unread() > 0 {
        meta.push(format!("{} waiting on you", center.urgent_unread()));
    }
    title("Notifications", meta.join(&sep(theme)), theme)
}

fn groups(app: &App) -> usize {
    let items = app.notifications.items();
    NotificationState::ALL
        .into_iter()
        .filter(|state| items.iter().any(|n| n.state == *state))
        .count()
}

fn state_heading(state: NotificationState) -> &'static str {
    match state {
        NotificationState::Unread => "Unread",
        NotificationState::Read => "Read",
        NotificationState::Resolved => "Resolved",
    }
}

/// `◆ Merge #231 into next?   question · archon · 2m`.
fn list_row(n: &Notification, app: &App, theme: &Theme) -> PickerRow {
    let mark = if n.kind.is_urgent() && n.state != NotificationState::Resolved {
        theme.glyphs().needs_you
    } else {
        " "
    };
    let dot = sep(theme);
    let detail = format!(
        "{}{dot}{}{dot}{}",
        n.kind.as_str(),
        poster(n, app),
        crate::format::relative(n.created_at, app.now)
    );
    PickerRow::new(format!("{mark} {}", clean(&n.title)), detail)
}

/// The posting agent by name, or the service's own name.
fn poster(n: &Notification, app: &App) -> String {
    match n.poster_agent() {
        Some(agent) => app.agent_name(agent),
        None => clean(&n.source),
    }
}

/// Rows of the text part of the detail: the title, the body, the link and
/// the answer, wrapped to `width`.
fn text_lines(n: &Notification, width: u16, app: &App, theme: &Theme) -> Vec<Line<'static>> {
    let wrap = usize::from(width).saturating_sub(4).max(1);
    let mut lines = vec![row(clean(&n.title), theme.text().patch(theme.title()))];
    if !n.body.trim().is_empty() {
        lines.push(Line::default());
        let body = markdown::render(&n.body, wrap, theme.text(), theme);
        lines.extend(body.into_iter().map(super::super::indent));
    }
    if let Some(link) = &n.link {
        lines.push(Line::default());
        lines.push(row(format!("link {}", clean(link)), theme.link()));
    }
    lines.extend(outcome_lines(n, app, theme));
    lines
}

/// How it was settled: the answer given, or that it was resolved without one.
fn outcome_lines(n: &Notification, app: &App, theme: &Theme) -> Vec<Line<'static>> {
    let Some(answer) = &n.answer else {
        if n.state != NotificationState::Resolved {
            return Vec::new();
        }
        return vec![Line::default(), row("Resolved", theme.secondary())];
    };
    let chosen = answer.action.as_deref().map(|id| {
        n.actions
            .iter()
            .find(|action| action.id == id)
            .map_or_else(|| clean(id), |action| clean(&action.label))
    });
    let mut said: Vec<String> = chosen.into_iter().collect();
    if let Some(text) = &answer.text {
        said.push(format!("\"{}\"", clean(text)));
    }
    let when = crate::format::relative(answer.at, app.now);
    vec![
        Line::default(),
        row(
            format!("Answered {}{}{when}", said.join(" "), sep(theme)),
            theme.success(),
        ),
    ]
}

/// The choices under an answerable notification: its actions, then writing
/// an answer of one's own when it takes text.
fn choice_lines(n: &Notification, detail: &Detail, theme: &Theme) -> Vec<Line<'static>> {
    let mut lines = Vec::with_capacity(choices(n));
    let label_width = n.actions.iter().map(|a| a.label.width()).max().unwrap_or(0);
    for (index, action) in n.actions.iter().enumerate() {
        let state = if detail.draft.is_none() && index == detail.choice {
            RowState::Selected
        } else {
            RowState::Plain
        };
        let label = PickerRow::new(clean(&action.label), "");
        lines.push(picker_line(theme, &label, Some(index), label_width, state));
    }
    if n.free_text {
        lines.push(draft_line(n, detail, theme));
    }
    debug_assert_eq!(lines.len(), choices(n), "a row per choice");
    lines
}

/// `write an answer`, or the words being typed with a caret after them.
fn draft_line(n: &Notification, detail: &Detail, theme: &Theme) -> Line<'static> {
    let number = n.actions.len();
    match &detail.draft {
        Some(draft) => {
            let prompt = theme.glyphs().prompt;
            Line::from(vec![
                Span::styled(format!("{prompt} "), theme.accent().patch(theme.title())),
                Span::styled(clean(draft), theme.text()),
                Span::styled(theme.glyphs().caret.to_owned(), theme.accent()),
            ])
        }
        None => {
            let state = if detail.choice == number {
                RowState::Selected
            } else {
                RowState::Plain
            };
            let label = PickerRow::new("write an answer", "a");
            let shown = (number < 9).then_some(number);
            picker_line(theme, &label, shown, label.label.width(), state)
        }
    }
}

/// The text part's height at `width`, so scrolling stops at its end.
pub(super) fn text_len(n: &Notification, width: u16, app: &App) -> usize {
    text_lines(n, width, app, &app.theme).len()
}

pub(super) fn detail_height(detail: &Detail, width: u16, app: &App) -> u16 {
    let Some(n) = app.notifications.get(detail.id) else {
        return super::super::frame_height(1);
    };
    let text = text_len(n, width, app).clamp(1, BODY_ROWS);
    let chosen = match choices(n) {
        0 => 0,
        rows => rows + 1,
    };
    CHROME_ROWS + u16::try_from(text + chosen).unwrap_or(u16::MAX)
}

pub(super) fn detail(frame: &mut Frame, area: Rect, detail: &Detail, app: &App, theme: &Theme) {
    let Some(n) = app.notifications.get(detail.id) else {
        render_empty(
            frame,
            area,
            title("Notification", "", theme),
            "It is gone",
            theme,
        );
        return;
    };
    let choices = if n.is_answerable() {
        choice_lines(n, detail, theme)
    } else {
        Vec::new()
    };
    let room = body_room(area);
    // The choices stay in view; the text above them scrolls.
    let text_room = room.saturating_sub(choices.len() + usize::from(!choices.is_empty()));
    let text = text_lines(n, area.width, app, theme);
    let mut body = Window::scrolled(text.len(), text_room, detail.scroll).lines(text, theme);
    if !choices.is_empty() {
        body.push(Line::default());
        body.extend(choices);
    }
    let hints = if detail.draft.is_some() {
        DRAFT_HINTS
    } else if n.is_answerable() {
        ANSWER_HINTS
    } else {
        READ_HINTS
    };
    render_frame(frame, area, detail_title(n, app, theme), body, hints, theme);
}

/// `Question  from archon · 2m · unread`.
fn detail_title(n: &Notification, app: &App, theme: &Theme) -> Vec<Span<'static>> {
    let dot = sep(theme);
    let mut meta = format!(
        "from {}{dot}{}{dot}{}",
        poster(n, app),
        crate::format::relative(n.created_at, app.now),
        n.state.as_str()
    );
    if agent_of(n).is_some() {
        meta.push_str(&format!("{dot}o opens the chat"));
    }
    let style = if n.kind.is_urgent() {
        theme.attention().patch(theme.title())
    } else {
        theme.title()
    };
    let mut spans = vec![Span::styled(kind_label(n.kind), style)];
    spans.push(Span::styled(format!("  {meta}"), theme.secondary()));
    spans
}

fn kind_label(kind: NotificationKind) -> &'static str {
    match kind {
        NotificationKind::Question => "Question",
        NotificationKind::Blocker => "Blocker",
        NotificationKind::Decision => "Decision",
        NotificationKind::Outcome => "Outcome",
        NotificationKind::Info => "Info",
    }
}
