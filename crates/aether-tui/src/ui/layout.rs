//! Pure frame geometry (SPEC §1): board width and docking, bottom-pane
//! stacking, and what gives way when rows run short.

use ratatui::layout::Rect;

/// At or above this width the board docks; below it is a full-width overlay.
pub const BOARD_DOCK_MIN_COLS: u16 = 100;
/// First-run default: the board starts open when the terminal is this wide.
pub const BOARD_DEFAULT_OPEN_COLS: u16 = 140;
pub const BOARD_MIN_WIDTH: u16 = 40;
pub const BOARD_MAX_WIDTH: u16 = 56;
/// Columns the transcript and composer keep beside a board the operator widened.
pub const MAIN_MIN_COLS: u16 = 40;
/// Columns of padding inside the board on each side.
pub const BOARD_PADDING: u16 = 2;
/// Below this height the inspector drops its low-priority rows (§1.2).
pub const INSPECTOR_SHORT_ROWS: u16 = 30;
/// Composer block: one padding row above and below its input rows (§3.2).
pub const COMPOSER_PADDING_ROWS: u16 = 2;
pub const COMPOSER_MAX_INPUT_ROWS: u16 = 8;

/// `clamp(round(cols × 0.30), 40, 56)`.
pub fn board_width(cols: u16) -> u16 {
    let share = (u32::from(cols) * 30 + 50) / 100;
    u16::try_from(share)
        .unwrap_or(u16::MAX)
        .clamp(BOARD_MIN_WIDTH, BOARD_MAX_WIDTH)
}

/// Widest docked board that still leaves the main column [`MAIN_MIN_COLS`].
pub fn board_max_width(cols: u16) -> u16 {
    cols.saturating_sub(MAIN_MIN_COLS + 2).max(BOARD_MIN_WIDTH)
}

/// The operator's width clamped to fit `cols`; `None` keeps [`board_width`].
pub fn chosen_board_width(cols: u16, chosen: Option<u16>) -> u16 {
    let Some(chosen) = chosen else {
        return board_width(cols);
    };
    let max = board_max_width(cols);
    debug_assert!(BOARD_MIN_WIDTH <= max, "{cols} columns: max {max}");
    chosen.clamp(BOARD_MIN_WIDTH, max)
}

/// One Alt+- / Alt+= step from `current`: 5 % of `cols`, as the viewer steps.
pub fn step_board_width(cols: u16, current: u16, wider: bool) -> u16 {
    let step = u16::try_from((u32::from(cols) * 5 + 50) / 100)
        .unwrap_or(u16::MAX)
        .max(1);
    let next = if wider {
        current.saturating_add(step)
    } else {
        current.saturating_sub(step)
    };
    let width = chosen_board_width(cols, Some(next));
    debug_assert!((BOARD_MIN_WIDTH..=board_max_width(cols)).contains(&width));
    width
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BoardMode {
    Hidden,
    /// Right-hand column of `width`, plus a separator and a gutter.
    Docked {
        width: u16,
    },
    /// Covers transcript and bottom pane; the hint row stays.
    Overlay,
}

pub fn board_mode(cols: u16, open: bool) -> BoardMode {
    sized_board_mode(cols, open, None)
}

/// [`board_mode`] with the operator's docked width, see [`chosen_board_width`].
pub fn sized_board_mode(cols: u16, open: bool, chosen: Option<u16>) -> BoardMode {
    if !open {
        BoardMode::Hidden
    } else if cols >= BOARD_DOCK_MIN_COLS {
        BoardMode::Docked {
            width: chosen_board_width(cols, chosen),
        }
    } else {
        BoardMode::Overlay
    }
}

/// Rows each bottom-pane part asks for; zero means absent.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct BottomRows {
    pub needs_you: u16,
    pub pending: u16,
    pub agents_strip: u16,
    pub status_row: u16,
    /// Composer block or the active view, padding included.
    pub composer: u16,
    pub status_line: u16,
    pub hint: u16,
}

impl BottomRows {
    fn total(&self) -> u16 {
        self.needs_you
            + self.pending
            + self.agents_strip
            + self.status_row
            + self.composer
            + self.status_line
            + self.hint
    }
}

/// Every region of one frame. Absent parts are zero-height rects.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct FrameLayout {
    pub transcript: Rect,
    pub needs_you: Rect,
    pub pending: Rect,
    pub agents_strip: Rect,
    pub status_row: Rect,
    pub composer: Rect,
    pub status_line: Rect,
    pub hint: Rect,
    /// Board column (docked) or overlay area; padding not yet applied.
    pub board: Option<Rect>,
    /// The one-column `│` between transcript and docked board.
    pub separator: Option<Rect>,
}

/// The blank "composer gap" row above the bottom pane.
const GAP_ROWS: u16 = 1;
const MIN_TRANSCRIPT_ROWS: u16 = 1;

pub fn frame_layout(area: Rect, board: BoardMode, want: BottomRows) -> FrameLayout {
    let (left, board_rect, separator) = split_columns(area, board);
    let rows = fit_rows(left.height, want);
    let mut out = stack(left, rows);
    out.board = board_rect;
    out.separator = separator;
    if board == BoardMode::Overlay {
        out.board = Some(Rect {
            height: area.height.saturating_sub(out.hint.height),
            ..area
        });
    }
    out
}

/// Inner area of the board after its horizontal padding.
pub fn board_inner(board: Rect) -> Rect {
    let pad = BOARD_PADDING.min(board.width / 2);
    Rect {
        x: board.x + pad,
        width: board.width - pad * 2,
        ..board
    }
}

fn split_columns(area: Rect, board: BoardMode) -> (Rect, Option<Rect>, Option<Rect>) {
    let BoardMode::Docked { width } = board else {
        return (area, None, None);
    };
    // Board, then the separator, then one gutter column before the transcript.
    let width = width.min(area.width.saturating_sub(2));
    let left_width = area.width - width - 2;
    let left = Rect {
        width: left_width,
        ..area
    };
    let separator = Rect {
        x: area.x + left_width + 1,
        width: 1,
        ..area
    };
    let board_rect = Rect {
        x: separator.x + 1,
        width,
        ..area
    };
    (left, Some(board_rect), Some(separator))
}

/// Trim `want` to `height`: drop parts least-important first (pending, agents
/// strip, needs-you, status row, status line), then shrink the composer to one row.
fn fit_rows(height: u16, want: BottomRows) -> BottomRows {
    let mut rows = want;
    let budget = height.saturating_sub(MIN_TRANSCRIPT_ROWS + GAP_ROWS);
    let drop_order: [fn(&mut BottomRows) -> &mut u16; 5] = [
        |r| &mut r.pending,
        |r| &mut r.agents_strip,
        |r| &mut r.needs_you,
        |r| &mut r.status_row,
        |r| &mut r.status_line,
    ];
    for part in drop_order {
        if rows.total() <= budget {
            return rows;
        }
        let excess = rows.total() - budget;
        let slot = part(&mut rows);
        *slot = slot.saturating_sub(excess);
    }
    let min_composer = want.composer.min(1);
    if rows.total() > budget {
        let excess = rows.total() - budget;
        rows.composer = rows.composer.saturating_sub(excess).max(min_composer);
    }
    if rows.total() > height {
        rows.hint = rows.hint.min(height.saturating_sub(rows.composer));
    }
    rows
}

fn stack(left: Rect, rows: BottomRows) -> FrameLayout {
    let bottom = rows.total().min(left.height);
    let room = left.height - bottom;
    let gap = if room > MIN_TRANSCRIPT_ROWS {
        GAP_ROWS
    } else {
        0
    };
    let transcript = Rect {
        height: room - gap,
        ..left
    };
    let mut y = left.y + room;
    let mut next = |h: u16| {
        let h = h.min(left.y + left.height - y);
        let rect = Rect {
            y,
            height: h,
            ..left
        };
        y += h;
        rect
    };
    FrameLayout {
        transcript,
        needs_you: next(rows.needs_you),
        pending: next(rows.pending),
        agents_strip: next(rows.agents_strip),
        status_row: next(rows.status_row),
        composer: next(rows.composer),
        status_line: next(rows.status_line),
        hint: next(rows.hint),
        board: None,
        separator: None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn area(w: u16, h: u16) -> Rect {
        Rect::new(0, 0, w, h)
    }

    fn typical() -> BottomRows {
        BottomRows {
            needs_you: 1,
            pending: 2,
            agents_strip: 1,
            status_row: 1,
            composer: 3,
            status_line: 1,
            hint: 1,
        }
    }

    #[test]
    fn board_width_follows_the_clamped_share() {
        assert_eq!(board_width(160), 48);
        assert_eq!(board_width(100), 40);
        assert_eq!(board_width(120), 40);
        assert_eq!(board_width(300), 56);
        assert_eq!(board_width(0), 40);
    }

    #[test]
    fn a_chosen_width_is_clamped_to_the_board_and_transcript_minimums() {
        assert_eq!(chosen_board_width(160, None), board_width(160));
        assert_eq!(chosen_board_width(160, Some(0)), BOARD_MIN_WIDTH);
        assert_eq!(chosen_board_width(160, Some(70)), 70);
        assert_eq!(
            chosen_board_width(160, Some(u16::MAX)),
            160 - MAIN_MIN_COLS - 2
        );
        assert_eq!(board_max_width(100), 58);
        for cols in [0, 1, 41, 82] {
            assert_eq!(board_max_width(cols), BOARD_MIN_WIDTH, "{cols}");
            assert_eq!(chosen_board_width(cols, Some(u16::MAX)), BOARD_MIN_WIDTH);
        }
    }

    #[test]
    fn a_step_is_five_percent_of_the_terminal_within_the_bounds() {
        assert_eq!(step_board_width(140, 42, true), 49);
        assert_eq!(step_board_width(140, 49, false), 42);
        assert_eq!(step_board_width(220, 56, true), 67);
        assert_eq!(step_board_width(100, 42, false), BOARD_MIN_WIDTH);
        assert_eq!(step_board_width(100, 55, true), board_max_width(100));
        assert_eq!(step_board_width(100, u16::MAX, true), board_max_width(100));
        assert_eq!(step_board_width(u16::MAX, 0, false), BOARD_MIN_WIDTH);
    }

    #[test]
    fn a_chosen_width_docks_only_from_one_hundred_columns() {
        assert_eq!(sized_board_mode(99, true, Some(60)), BoardMode::Overlay);
        assert_eq!(sized_board_mode(140, false, Some(60)), BoardMode::Hidden);
        assert_eq!(
            sized_board_mode(140, true, Some(60)),
            BoardMode::Docked { width: 60 }
        );
        for cols in [100, 140, 220] {
            let mode = sized_board_mode(cols, true, Some(u16::MAX));
            let l = frame_layout(area(cols, 40), mode, typical());
            let board = l.board.unwrap();
            assert_eq!(l.transcript.width, MAIN_MIN_COLS, "{cols}");
            assert_eq!(l.separator.unwrap().x, l.transcript.width + 1);
            assert_eq!(board.x + board.width, cols);
        }
    }

    #[test]
    fn board_docks_at_one_hundred_columns() {
        assert_eq!(board_mode(99, true), BoardMode::Overlay);
        assert_eq!(board_mode(100, true), BoardMode::Docked { width: 40 });
        assert_eq!(board_mode(160, false), BoardMode::Hidden);
    }

    #[test]
    fn docked_board_leaves_separator_and_gutter() {
        let l = frame_layout(area(160, 44), board_mode(160, true), typical());
        let board = l.board.unwrap();
        let sep = l.separator.unwrap();
        assert_eq!(board.width, 48);
        assert_eq!(board.x + board.width, 160);
        assert_eq!(sep.x + 1, board.x);
        assert_eq!(l.transcript.width, 160 - 48 - 2);
        assert_eq!(board.height, 44);
    }

    #[test]
    fn bottom_pane_stacks_in_spec_order_and_fills_the_screen() {
        let l = frame_layout(area(120, 40), BoardMode::Hidden, typical());
        let order = [
            l.needs_you,
            l.pending,
            l.agents_strip,
            l.status_row,
            l.composer,
            l.status_line,
            l.hint,
        ];
        for pair in order.windows(2) {
            assert_eq!(pair[0].y + pair[0].height, pair[1].y);
        }
        assert_eq!(l.hint.y + l.hint.height, 40);
        // One blank composer-gap row between transcript and the bottom pane.
        assert_eq!(l.transcript.height + 1, l.needs_you.y);
    }

    #[test]
    fn short_screens_drop_optional_rows_before_the_composer() {
        let l = frame_layout(area(80, 9), BoardMode::Hidden, typical());
        assert_eq!(l.pending.height, 0);
        assert_eq!(l.agents_strip.height, 0);
        assert_eq!(l.composer.height, 3);
        assert_eq!(l.hint.height, 1);
        assert!(l.transcript.height >= 1);
    }

    #[test]
    fn tiny_screens_never_overflow() {
        for h in 0..12 {
            for w in [0, 1, 39, 80, 99, 100, 160] {
                for open in [false, true] {
                    let a = area(w, h);
                    let l = frame_layout(a, board_mode(w, open), typical());
                    for r in [l.transcript, l.composer, l.hint, l.status_line] {
                        assert!(r.y + r.height <= h, "{w}x{h} {r:?}");
                        assert!(r.x + r.width <= w, "{w}x{h} {r:?}");
                    }
                }
            }
        }
    }

    #[test]
    fn overlay_covers_everything_but_the_hint_row() {
        let l = frame_layout(area(80, 34), board_mode(80, true), typical());
        let board = l.board.unwrap();
        assert_eq!(board.width, 80);
        assert_eq!(board.height, 33);
        assert_eq!(l.hint.y, 33);
        assert!(l.separator.is_none());
    }

    #[test]
    fn board_inner_applies_padding() {
        let inner = board_inner(Rect::new(110, 0, 48, 44));
        assert_eq!(inner, Rect::new(112, 0, 44, 44));
    }
}
