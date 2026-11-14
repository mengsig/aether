//! Transcript cell rendering (SPEC §3): one `ChatCell` impl over
//! `app::transcript::CellModel`, dispatching to one module per cell family.
//! Owner: W2 (except `header.rs`, W1).

pub mod agent;
pub mod approval_record;
pub mod assistant;
pub mod called;
pub mod compact;
pub mod edited;
pub mod exec;
pub mod explored;
pub mod header;
pub mod notice;
pub mod question;
pub mod reasoning;
pub mod separator;
pub mod text;
pub mod user;

use aether_core::time::Timestamp;
use ratatui::style::Style;
use ratatui::text::{Line, Span};

use crate::app::transcript::{CallStatus, CompactionState};
pub use crate::app::transcript::{Cell, CellId, CellModel, CellRevision};
use crate::format;
use crate::ui::theme::tokens::{Theme, ThemeMode, ThemeSpec, ToolKind};
use text::Layout;

/// Output previews (§3): up to 5 rows show whole; longer ones show head 2,
/// `… +N lines (ctrl+t to view)`, tail 3.
pub const PREVIEW_MAX_ROWS: usize = 5;
pub const PREVIEW_HEAD_ROWS: usize = 2;
pub const PREVIEW_TAIL_ROWS: usize = 3;

/// How a cell paints itself. Lines are `'static` so `LayoutCache` can keep them.
pub trait ChatCell {
    /// Compact view (§3.1 "Shape").
    fn display_lines(&self, width: u16, theme: &Theme) -> Vec<Line<'static>>;

    /// Ctrl+T view: full output, `$ cmd`, exit/duration, reasoning, child summaries.
    fn detail_lines(&self, width: u16, theme: &Theme) -> Vec<Line<'static>> {
        self.display_lines(width, theme)
    }

    /// The compact or detail rows, with where each sits in its source line.
    /// Cells that do not track their wrapping match row by row.
    fn layout(&self, width: u16, detail: bool, theme: &Theme) -> Layout {
        Layout::unmapped(if detail {
            self.detail_lines(width, theme)
        } else {
            self.display_lines(width, theme)
        })
    }

    fn desired_height(&self, width: u16, detail: bool, theme: &Theme) -> u16 {
        let lines = if detail {
            self.detail_lines(width, theme)
        } else {
            self.display_lines(width, theme)
        };
        u16::try_from(lines.len()).unwrap_or(u16::MAX)
    }

    /// `Some(period_ms)` while the cell animates (pulsing bullet, wait timer).
    fn animation_tick(&self) -> Option<u64> {
        None
    }

    /// Text `/copy` takes from this cell, if any.
    fn copy_source(&self) -> Option<&str> {
        None
    }

    /// Unstyled lines for `--plain`; same grammar as `display_lines`.
    fn plain_lines(&self, width: u16) -> Vec<String>;
}

/// Frame period for pulsing bullets and the live wait timer.
pub const PULSE_FRAME_MS: u64 = 100;

impl ChatCell for CellModel {
    fn display_lines(&self, width: u16, theme: &Theme) -> Vec<Line<'static>> {
        let render = match self {
            CellModel::Header { .. } => header::display_lines,
            CellModel::User { .. } => user::display_lines,
            CellModel::Assistant { .. } | CellModel::Post { .. } => assistant::display_lines,
            CellModel::Reasoning { .. } => reasoning::display_lines,
            CellModel::Exec { .. } => exec::display_lines,
            CellModel::Explored { .. } => explored::display_lines,
            CellModel::Edited { .. } => edited::display_lines,
            CellModel::Called { .. } | CellModel::Memory(_) => called::display_lines,
            CellModel::Spawned { .. }
            | CellModel::Waiting { .. }
            | CellModel::ChildResult { .. }
            | CellModel::Sent { .. } => agent::display_lines,
            CellModel::ApprovalRecord { .. } => approval_record::display_lines,
            CellModel::Question { .. } => question::display_lines,
            CellModel::Error { .. } | CellModel::Notice { .. } | CellModel::Compaction(_) => {
                notice::display_lines
            }
            CellModel::Separator { .. } => separator::display_lines,
        };
        render(self, width, theme)
    }

    fn detail_lines(&self, width: u16, theme: &Theme) -> Vec<Line<'static>> {
        let render = match self {
            CellModel::Header { .. } => header::detail_lines,
            CellModel::User { .. } => user::detail_lines,
            CellModel::Assistant { .. } | CellModel::Post { .. } => assistant::detail_lines,
            CellModel::Reasoning { .. } => reasoning::detail_lines,
            CellModel::Exec { .. } => exec::detail_lines,
            CellModel::Explored { .. } => explored::detail_lines,
            CellModel::Edited { .. } => edited::detail_lines,
            CellModel::Called { .. } | CellModel::Memory(_) => called::detail_lines,
            CellModel::Spawned { .. }
            | CellModel::Waiting { .. }
            | CellModel::ChildResult { .. }
            | CellModel::Sent { .. } => agent::detail_lines,
            CellModel::ApprovalRecord { .. } => approval_record::detail_lines,
            CellModel::Question { .. } => question::detail_lines,
            CellModel::Error { .. } | CellModel::Notice { .. } | CellModel::Compaction(_) => {
                notice::detail_lines
            }
            CellModel::Separator { .. } => separator::detail_lines,
        };
        render(self, width, theme)
    }

    fn layout(&self, width: u16, detail: bool, theme: &Theme) -> Layout {
        let render = match self {
            CellModel::User { .. } => user::layout,
            CellModel::Assistant { .. } | CellModel::Post { .. } => assistant::layout,
            CellModel::Reasoning { .. } => reasoning::layout,
            CellModel::Exec { .. } => exec::layout,
            CellModel::Called { .. } | CellModel::Memory(_) => called::layout,
            CellModel::Error { .. } | CellModel::Notice { .. } | CellModel::Compaction(_) => {
                notice::layout
            }
            _ => {
                return Layout::unmapped(if detail {
                    self.detail_lines(width, theme)
                } else {
                    self.display_lines(width, theme)
                });
            }
        };
        render(self, width, detail, theme)
    }

    fn animation_tick(&self) -> Option<u64> {
        is_live(self).then_some(PULSE_FRAME_MS)
    }

    fn copy_source(&self) -> Option<&str> {
        match self {
            CellModel::User { text, .. }
            | CellModel::Assistant { text }
            | CellModel::Post { text }
            | CellModel::Reasoning { text, .. }
            | CellModel::Question { text, .. } => Some(text),
            CellModel::Exec { output, .. } => Some(output),
            CellModel::Called { result, .. } => Some(result),
            CellModel::ChildResult { result, .. } => Some(result),
            CellModel::Error { message, .. } => Some(message),
            _ => None,
        }
        .map(String::as_str)
        .filter(|text| !text.is_empty())
    }

    fn plain_lines(&self, width: u16) -> Vec<String> {
        let render = match self {
            CellModel::Header { .. } => header::plain_lines,
            CellModel::User { .. } => user::plain_lines,
            CellModel::Assistant { .. } | CellModel::Post { .. } => assistant::plain_lines,
            CellModel::Reasoning { .. } => reasoning::plain_lines,
            CellModel::Exec { .. } => exec::plain_lines,
            CellModel::Explored { .. } => explored::plain_lines,
            CellModel::Edited { .. } => edited::plain_lines,
            CellModel::Called { .. } | CellModel::Memory(_) => called::plain_lines,
            CellModel::Spawned { .. }
            | CellModel::Waiting { .. }
            | CellModel::ChildResult { .. }
            | CellModel::Sent { .. } => agent::plain_lines,
            CellModel::ApprovalRecord { .. } => approval_record::plain_lines,
            CellModel::Question { .. } => question::plain_lines,
            CellModel::Error { .. } | CellModel::Notice { .. } | CellModel::Compaction(_) => {
                notice::plain_lines
            }
            CellModel::Separator { .. } => separator::plain_lines,
        };
        let mut lines = render(self, width);
        for line in &mut lines {
            if let Some(kept) = line.strip_suffix(text::DETAILS_HINT) {
                line.truncate(kept.len());
            }
        }
        lines
    }
}

/// Cells whose bullet pulses: running calls, an open explore group, a wait, compaction.
fn is_live(cell: &CellModel) -> bool {
    match cell {
        CellModel::Exec { status, .. }
        | CellModel::Edited { status, .. }
        | CellModel::Called { status, .. } => *status == CallStatus::Running,
        CellModel::Explored { active, .. } => *active,
        CellModel::Waiting { ended, .. } => ended.is_none(),
        CellModel::Compaction(state) => *state == CompactionState::Active,
        _ => false,
    }
}

/// Draw-time touches on one cached row, `(index, of rows)`: the pulsing
/// bullet at `phase` and a live wait's ` · since 41s`. Cached lines stay
/// static, so this is per frame.
pub fn decorate_live(
    line: &mut Line<'static>,
    (row, rows): (usize, usize),
    cell: &CellModel,
    theme: &Theme,
    phase: f32,
    now: Timestamp,
) {
    if !is_live(cell) {
        return;
    }
    if row == 0
        && let Some(bullet) = line.spans.first_mut()
    {
        bullet.style = theme.pulse(phase).patch(theme.title());
    }
    if let CellModel::Waiting {
        since, ended: None, ..
    } = cell
        && row + 1 == rows
    {
        let last = line;
        let ms = u64::try_from((now - *since).num_milliseconds()).unwrap_or(0);
        let text = format!(
            " {} since {}",
            theme.glyphs().dot,
            format::elapsed_compact(ms)
        );
        last.spans.push(Span::styled(text, theme.secondary()));
    }
}

/// How a cell's leading bullet reads (§3 rules).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Tone {
    Neutral,
    Success,
    Failure,
    /// Pulses at draw time ([`decorate_live`]).
    Active,
    /// Held for the operator: the accent `◆`.
    Asking,
}

impl Tone {
    pub fn of(status: CallStatus) -> Self {
        match status {
            CallStatus::Awaiting => Tone::Asking,
            CallStatus::Running => Tone::Active,
            CallStatus::Succeeded => Tone::Success,
            CallStatus::Failed => Tone::Failure,
            CallStatus::Denied | CallStatus::Cancelled => Tone::Neutral,
        }
    }
}

pub fn bullet(theme: &Theme, tone: Tone) -> Span<'static> {
    let style = match tone {
        Tone::Neutral => theme.secondary(),
        Tone::Success => theme.success().patch(theme.title()),
        Tone::Failure => theme.failure().patch(theme.title()),
        Tone::Active => theme.pulse(1.0).patch(theme.title()),
        Tone::Asking => {
            let style = theme.attention().patch(theme.title());
            return Span::styled(theme.glyphs().needs_you, style);
        }
    };
    Span::styled(theme.glyphs().bullet, style)
}

/// A tool call's verb in its kind's hue; a call that never ran stays plain.
pub fn verb_style(theme: &Theme, kind: ToolKind, status: CallStatus) -> Style {
    match status {
        CallStatus::Denied | CallStatus::Cancelled => theme.title(),
        _ => theme.tool(kind).patch(theme.title()),
    }
}

/// `--plain` renders through the mono unicode theme, then drops styles.
pub fn plain_theme() -> Theme {
    Theme::resolve(ThemeSpec {
        mode: ThemeMode::Mono,
        ..ThemeSpec::default()
    })
}

#[cfg(test)]
mod tests;
