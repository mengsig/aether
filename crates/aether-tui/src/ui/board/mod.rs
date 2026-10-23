//! The Ctrl+B board (SPEC §1.2, §2): agent tree on top, inspector below a
//! hairline. Data comes from `app::board`. Owner: W3.

pub mod inspector;
pub mod tree;

use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::style::Style;
use ratatui::text::{Line, Span};
use ratatui::widgets::Clear;
use unicode_width::UnicodeWidthStr;

use aether_core::time::Timestamp;

use crate::app::App;
use crate::app::board::BoardStatus;
pub use crate::app::board::{BoardModel, BoardRow, InspectorModel};
use crate::app::transcript::Cost;
use crate::ui::layout::board_inner;
use crate::ui::theme::tokens::Theme;

/// The tree may take at most this share of the board height.
pub const TREE_MAX_PERCENT: u16 = 55;
/// The inspector always keeps at least this many rows.
pub const INSPECTOR_MIN_ROWS: u16 = 14;
/// Header, its gap, and one agent: below this the tree shows nothing useful.
const TREE_MIN_ROWS: u16 = 4;
/// Blank, hairline, blank between tree and inspector.
const DIVIDER_ROWS: u16 = 3;
/// One brightness cycle of the running `●` (§2.1).
pub const PULSE_PERIOD_MS: i64 = 2_000;
/// Frame spacing while the pulse runs; smooth enough for a 2 s brightness swing.
const PULSE_FRAME_MS: u64 = 100;

/// Tree scroll inside the board, plus the tree's per-second label cache; kept
/// across frames so the cursor stays in view.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct BoardScroll {
    pub tree_offset: usize,
    pub wheel_delta: isize,
    pub manual_scroll: bool,
    pub last_anchor: Option<aether_core::id::AgentId>,
    pub tree_area: Rect,
    pub rendered_revision: u64,
    labels: tree::Labels,
}

/// Paint the whole board (header, tree, hairline, inspector) into `area`
/// (already padded via `ui::layout::board_inner`).
pub fn render(frame: &mut Frame, area: Rect, app: &App, scroll: &mut BoardScroll, theme: &Theme) {
    scroll.rendered_revision = app.board.revision;
    scroll.tree_area = Rect::default();
    frame.render_widget(Clear, area);
    // Row 0 is breathing room above the header.
    let body = Rect {
        y: area.y.saturating_add(1),
        height: area.height.saturating_sub(1),
        ..area
    };
    let model = &app.board.model;
    let tree_rows = tree_height(tree::desired_height(model), body.height);
    let ctx = tree::TreeContext {
        cursor: app.board.focused().then(|| app.inspected_agent()).flatten(),
        viewed: app.selected,
        now: app.now,
        revision: app.board.revision,
    };
    tree::render(
        frame,
        Rect {
            height: tree_rows,
            ..body
        },
        model,
        ctx,
        scroll,
        theme,
    );

    let below = body.height - tree_rows;
    if below <= DIVIDER_ROWS {
        return;
    }
    let hairline = Rect {
        y: body.y + tree_rows + 1,
        height: 1,
        ..body
    };
    let rule = theme.glyphs().rule.repeat(usize::from(body.width));
    frame.buffer_mut().set_stringn(
        hairline.x,
        hairline.y,
        rule,
        usize::from(body.width),
        theme.rule(),
    );
    let Some(inspected) = app
        .inspected_agent()
        .and_then(|id| InspectorModel::build(app, id))
    else {
        return;
    };
    let inspector_area = Rect {
        y: body.y + tree_rows + DIVIDER_ROWS,
        height: below - DIVIDER_ROWS,
        ..body
    };
    inspector::render(frame, inspector_area, &inspected, app.now, theme);
}

/// The board as a full-width overlay below 100 columns: clears everything it
/// covers, padding included, then paints the board inside.
pub fn render_overlay(
    frame: &mut Frame,
    area: Rect,
    app: &App,
    scroll: &mut BoardScroll,
    theme: &Theme,
) {
    frame.render_widget(Clear, area);
    render(frame, board_inner(area), app, scroll, theme);
}

/// Milliseconds until the board next changes on its own: the running pulse,
/// else a live elapsed clock (1 Hz), else the inspector's `… ago`. `None`
/// when nothing on it moves.
pub fn next_frame_in_ms(app: &App, theme: &Theme) -> Option<u64> {
    let rows = &app.board.model.rows;
    if !app.board.open || rows.is_empty() {
        return None;
    }
    let now = app.now;
    let pulses = theme.animates();
    let live =
        |status: BoardStatus, elapsed: Option<u64>| !status.is_finished() && elapsed.is_some();
    if pulses && rows.iter().any(|r| r.status == BoardStatus::Running) {
        return Some(PULSE_FRAME_MS);
    }
    if rows.iter().any(|r| live(r.status, r.elapsed_ms(now))) {
        return Some(1_000);
    }
    // The inspector can show agents the tree folds away.
    let inspected = app
        .inspected_agent()
        .and_then(|id| InspectorModel::build(app, id))?;
    let statuses = std::iter::once((inspected.status, inspected.elapsed_ms(now))).chain(
        inspected
            .children
            .iter()
            .map(|c| (c.status, c.elapsed_ms(now))),
    );
    let mut clock = None;
    for (status, elapsed) in statuses {
        if pulses && status == BoardStatus::Running {
            return Some(PULSE_FRAME_MS);
        }
        if live(status, elapsed) {
            clock = Some(1_000);
        }
    }
    clock.or_else(|| {
        let activity = inspected.last_activity?;
        let ago = u64::try_from((now - activity.at).num_milliseconds()).unwrap_or(0);
        Some(ago_changes_in(ago))
    })
}

/// `Last activity`'s age: `12s ago`, then `4m ago`, `2h ago`, `3d ago`.
pub(crate) fn ago_text(ms: u64) -> String {
    let secs = ms / 1_000;
    match secs {
        0..60 => format!("{secs}s ago"),
        60..3_600 => format!("{}m ago", secs / 60),
        3_600..86_400 => format!("{}h ago", secs / 3_600),
        _ => format!("{}d ago", secs / 86_400),
    }
}

/// Milliseconds until [`ago_text`] of `ms` next reads differently.
fn ago_changes_in(ms: u64) -> u64 {
    let unit: u64 = match ms / 1_000 {
        0..60 => 1_000,
        60..3_600 => 60_000,
        3_600..86_400 => 3_600_000,
        _ => 86_400_000,
    };
    unit - ms % unit
}

/// Tree rows: what it wants, within 55 % and leaving the inspector its minimum.
fn tree_height(want: u16, height: u16) -> u16 {
    let share =
        u16::try_from(u32::from(height) * u32::from(TREE_MAX_PERCENT) / 100).unwrap_or(height);
    let room = height.saturating_sub(INSPECTOR_MIN_ROWS + DIVIDER_ROWS);
    let cap = share.min(room).max(TREE_MIN_ROWS).min(height);
    want.min(cap)
}

/// The status glyph in its §2.1 style; `now` drives the running pulse.
pub fn status_glyph(status: BoardStatus, theme: &Theme, now: Timestamp) -> Span<'static> {
    let g = theme.glyphs();
    let glyph = match status {
        BoardStatus::Running => g.running,
        BoardStatus::Waiting => g.waiting,
        BoardStatus::Idle => g.idle,
        BoardStatus::NeedsYou => g.needs_you,
        BoardStatus::Blocked { .. } => g.blocked,
        BoardStatus::Paused => g.paused,
        BoardStatus::Completed => g.done,
        BoardStatus::Failed => g.failed,
        BoardStatus::Cancelled => g.cancelled,
    };
    let style = match status {
        BoardStatus::Running => running_style(theme, now),
        other => status_style(other, theme),
    };
    Span::styled(glyph, style)
}

/// Each status in its own hue, held still; [`status_glyph`] pulses running.
pub fn status_style(status: BoardStatus, theme: &Theme) -> Style {
    match status {
        BoardStatus::Running => theme.running(),
        BoardStatus::Waiting => theme.waiting(),
        BoardStatus::Idle => theme.secondary(),
        BoardStatus::NeedsYou => theme.attention().patch(theme.title()),
        BoardStatus::Blocked { .. } => theme.blocked(),
        BoardStatus::Paused | BoardStatus::Cancelled => theme.paused(),
        BoardStatus::Completed => theme.success(),
        BoardStatus::Failed => theme.failure(),
    }
}

/// The running hue breathing on a 2 s triangle wave; still under reduced motion.
fn running_style(theme: &Theme, now: Timestamp) -> Style {
    let t = now.timestamp_millis().rem_euclid(PULSE_PERIOD_MS) as f32 / PULSE_PERIOD_MS as f32;
    theme.pulse(1.0 - (2.0 * t - 1.0).abs())
}

/// The inspector's status chip word.
pub fn status_word(status: BoardStatus) -> &'static str {
    match status {
        BoardStatus::Running => "Running",
        BoardStatus::Waiting => "Waiting",
        BoardStatus::Idle => "Idle",
        BoardStatus::NeedsYou => "Needs you",
        BoardStatus::Blocked { .. } => "Blocked",
        BoardStatus::Paused => "Paused",
        BoardStatus::Completed => "Done",
        BoardStatus::Failed => "Failed",
        BoardStatus::Cancelled => "Cancelled",
    }
}

/// `$0.38`, `~$0.09` for an estimate or floor, and nothing when unknown:
/// the board never prints a bare `—` for a price.
pub fn cost_text(cost: Option<Cost>) -> String {
    let Some(cost) = cost else {
        return String::new();
    };
    let prefix = if cost.estimated { "~" } else { "" };
    crate::format::cost_with_basis(format!("{prefix}${:.2}", cost.usd), cost.basis)
}

/// A finished agent's span reads `<1s` when short; a live clock keeps `0s`.
pub(crate) fn elapsed_text(status: BoardStatus, ms: Option<u64>) -> String {
    let format = if status.is_finished() {
        crate::format::took
    } else {
        crate::format::elapsed_compact
    };
    ms.map(format).unwrap_or_default()
}

/// `left`, then `right` flush against the right edge of `width` columns. The
/// left side gives way first, from its end; `right` is dropped only when it
/// cannot fit at all.
pub(crate) fn split_line(
    left: Vec<Span<'static>>,
    right: Vec<Span<'static>>,
    width: usize,
    theme: &Theme,
) -> Line<'static> {
    let room = left_room(&right, width);
    if room == width {
        return Line::from(fit_spans(left, width, theme));
    }
    let mut spans = fit_spans(left, room, theme);
    let used: usize = spans.iter().map(|s| s.content.width()).sum();
    spans.push(Span::raw(" ".repeat(room + 1 - used)));
    spans.extend(right);
    Line::from(spans)
}

/// Columns [`split_line`] leaves its left side beside `right`; all of `width`
/// when `right` cannot fit and is dropped.
pub(crate) fn left_room(right: &[Span<'static>], width: usize) -> usize {
    let right_width: usize = right.iter().map(|s| s.content.width()).sum();
    if right_width + 1 > width {
        width
    } else {
        width - right_width - 1
    }
}

/// Cut `spans` to `max` columns, ending in the theme's ellipsis where text was lost.
pub(crate) fn fit_spans(
    spans: Vec<Span<'static>>,
    max: usize,
    theme: &Theme,
) -> Vec<Span<'static>> {
    let mut out = Vec::with_capacity(spans.len());
    let mut used = 0;
    for span in spans {
        let w = span.content.width();
        if used + w <= max {
            used += w;
            out.push(span);
            continue;
        }
        // A lone ellipsis says nothing; drop the span instead.
        if max - used >= 2 {
            out.push(Span::styled(
                truncate(&span.content, max - used, theme),
                span.style,
            ));
        }
        break;
    }
    out
}

pub(crate) fn truncate(text: &str, max: usize, theme: &Theme) -> String {
    let marker = theme.glyphs().ellipsis.chars().next().unwrap_or('~');
    crate::format::truncate_with_marker(text, max, marker)
}

#[cfg(test)]
mod tests;
