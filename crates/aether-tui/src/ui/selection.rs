//! Pane-local selection; transcript endpoints refer to cells rather than screen rows.

use aether_core::id::AgentId;
use crossterm::event::MouseEvent;
use ratatui::buffer::Buffer;
use ratatui::layout::{Position, Rect};
use unicode_segmentation::UnicodeSegmentation;
use unicode_width::UnicodeWidthStr;

use crate::app::App;
use crate::app::transcript::CellId;
use crate::ui::transcript::{self, ViewCache};

pub const MAX_COPY_BYTES: usize = 1024 * 1024;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Pane {
    Transcript,
    Composer,
    Board,
    View,
}

#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct FrameRegions {
    pub transcript: Rect,
    pub composer: Rect,
    pub composer_block: Rect,
    pub board: Rect,
    pub view: Rect,
}

impl FrameRegions {
    pub fn hit(&self, x: u16, y: u16) -> Option<Pane> {
        [Pane::Board, Pane::View, Pane::Composer, Pane::Transcript]
            .into_iter()
            .find(|&pane| {
                self.rect(pane).contains(Position::new(x, y))
                    || pane == Pane::Composer && self.composer_block.contains(Position::new(x, y))
            })
    }

    pub fn rect(&self, pane: Pane) -> Rect {
        match pane {
            Pane::Transcript => self.transcript,
            Pane::Composer => self.composer,
            Pane::Board => self.board,
            Pane::View => self.view,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TextPoint {
    pub cell: CellId,
    pub row: usize,
    pub column: u16,
    pub stream_generation: Option<u64>,
}

#[derive(Debug)]
pub struct Selection {
    pub pane: Pane,
    pub rect: Rect,
    pub agent: Option<AgentId>,
    pub start: TextPoint,
    pub end: TextPoint,
    pub start_excerpt: Option<String>,
    pub end_excerpt: Option<String>,
    pub dragging: bool,
    pub moved: bool,
    /// A double click selected a whole line: drags leave it alone.
    pub line: bool,
    pub pointer: MouseEvent,
    pub rows: Vec<String>,
    pub epoch: crate::app::transcript::TranscriptEpoch,
    pub input_revision: u64,
    pub board_revision: u64,
    pub details: bool,
}

#[derive(Debug, Default)]
pub struct MouseState {
    pub transcript_area: Rect,
    pub displayed_top: crate::ui::transcript::Anchor,
    pub pinned: Option<TextPoint>,
    pub selection: Option<Selection>,
    pub pending_board_click: Option<AgentId>,
    pub extend_after_scroll: bool,
    /// Time and cell of the previous left press, for double-click detection.
    pub last_down: Option<(std::time::Instant, u16, u16)>,
}

impl MouseState {
    pub fn clear_selection(&mut self) {
        self.selection = None;
        self.pending_board_click = None;
        self.extend_after_scroll = false;
    }
}

pub fn clamp(rect: Rect, x: u16, y: u16) -> Option<(u16, u16)> {
    if rect.is_empty() {
        return None;
    }
    Some((
        x.clamp(rect.x, rect.right() - 1),
        y.clamp(rect.y, rect.bottom() - 1),
    ))
}

pub fn point(app: &App, cache: &mut ViewCache, pane: Pane, x: u16, y: u16) -> Option<TextPoint> {
    let rect = cache.regions.rect(pane);
    let (x, y) = clamp(rect, x, y)?;
    if pane == Pane::Transcript {
        transcript::selection_point(app, cache, x, y)
    } else {
        Some(TextPoint {
            stream_generation: None,
            cell: CellId(0),
            row: usize::from(y - rect.y),
            column: x - rect.x,
        })
    }
}

pub fn pinned_excerpt(app: &App, cache: &ViewCache, point: TextPoint, y: u16) -> Option<String> {
    let pinned = cache.mouse.pinned?;
    if y != cache.regions.transcript.y || (point.cell, point.row) != (pinned.cell, pinned.row) {
        return None;
    }
    let t = app.transcripts.get(&app.selected?)?;
    let cell = t
        .cells
        .get(t.position(point.cell)?)
        .or_else(|| t.active.as_ref().filter(|c| c.id == point.cell))?;
    let line = crate::ui::cells::user::pinned_line(
        &cell.model,
        cache.regions.transcript.width,
        &app.theme,
    )?;
    Some(line.spans.iter().map(|s| s.content.as_ref()).collect())
}

pub fn validate(app: &App, cache: &mut ViewCache) {
    let invalid = cache.mouse.selection.as_ref().is_some_and(|s| {
        app.mouse == crate::prefs::MouseMode::Off
            || s.agent != app.selected
            || s.details != app.details
            || s.pane == Pane::Transcript
                && s.epoch
                    != app
                        .selected
                        .and_then(|id| app.transcripts.get(&id))
                        .map(|t| t.epoch)
                        .unwrap_or_default()
            || s.pane == Pane::Composer && s.input_revision != app.input.revision
            || s.pane == Pane::Board && s.board_revision != app.board.revision
    });
    let invalid = invalid || !transcript::selection_resolves(app, cache);
    if invalid {
        cache.mouse.clear_selection();
    }
}

pub fn extract(app: &App, cache: &mut ViewCache) -> Result<String, String> {
    validate(app, cache);
    let Some(selection) = cache.mouse.selection.as_ref() else {
        return Ok(String::new());
    };
    if selection.pane == Pane::Transcript {
        return transcript::selected_text(app, cache);
    }
    let mut a = selection.start;
    let mut b = selection.end;
    if (a.row, a.column) > (b.row, b.column) {
        std::mem::swap(&mut a, &mut b);
    }
    if selection.pane == Pane::Composer && !app.input.text.is_empty() {
        let first = cache
            .composer
            .rows
            .get(cache.composer.scroll + a.row)
            .ok_or("selected draft row no longer available")?;
        let last = cache
            .composer
            .rows
            .get(cache.composer.scroll + b.row)
            .ok_or("selected draft row no longer available")?;
        let from = first
            .selected_range(usize::from(a.column), usize::from(selection.rect.width))
            .start;
        let to = last.selected_range(0, usize::from(b.column) + 1).end;
        let source = app
            .input
            .text
            .get(from..to)
            .ok_or("selected draft range no longer available")?;
        if source.len() > MAX_COPY_BYTES {
            return Err("selection exceeds the 1 MiB clipboard limit".into());
        }
        return Ok(source
            .split('\n')
            .map(str::trim_end)
            .collect::<Vec<_>>()
            .join("\n"));
    }
    let mut text = String::new();
    for row in a.row..=b.row {
        let line = selection
            .rows
            .get(row)
            .ok_or("selection has not been rendered")?;
        append_row(
            &mut text,
            line,
            if row == a.row { a.column } else { 0 },
            if row == b.row {
                b.column.saturating_add(1)
            } else {
                selection.rect.width
            },
        )?;
    }
    text.pop();
    Ok(text)
}

pub fn append_row(out: &mut String, text: &str, start: u16, end: u16) -> Result<(), String> {
    let mut column = 0;
    let mut row = String::new();
    for g in text.graphemes(true) {
        let width = g.width();
        if column < usize::from(end) && column + width > usize::from(start) {
            row.push_str(g);
        }
        column += width;
    }
    let row = row.trim_end();
    if out.len() + row.len() + 1 > MAX_COPY_BYTES {
        return Err("selection exceeds the 1 MiB clipboard limit".into());
    }
    out.push_str(row);
    out.push('\n');
    Ok(())
}

pub fn highlight(buffer: &mut Buffer, app: &App, cache: &mut ViewCache) {
    let Some(selection) = cache.mouse.selection.as_ref() else {
        return;
    };
    let pane = selection.pane;
    let pointer = selection.pointer;
    let rect = cache.regions.rect(pane);
    if rect != selection.rect {
        if pane == Pane::Transcript
            && !rect.is_empty()
            && rect.x == selection.rect.x
            && rect.width == selection.rect.width
        {
            if let Some(s) = cache.mouse.selection.as_mut() {
                s.rect = rect;
            }
        } else {
            cache.mouse.clear_selection();
            return;
        }
    }
    if cache.mouse.extend_after_scroll {
        if let Some(end) = point(app, cache, pane, pointer.column, pointer.row) {
            let excerpt = pinned_excerpt(app, cache, end, pointer.row);
            if let Some(s) = cache.mouse.selection.as_mut() {
                s.end = end;
                s.end_excerpt = excerpt;
            }
        }
        cache.mouse.extend_after_scroll = false;
    }
    let transcript_rows = if cache
        .mouse
        .selection
        .as_ref()
        .is_some_and(|s| s.pane == Pane::Transcript)
    {
        transcript::selection_rows(app, cache)
    } else {
        Vec::new()
    };
    let Some(s) = cache.mouse.selection.as_mut() else {
        return;
    };
    if s.rows.is_empty() && s.pane != Pane::Transcript {
        s.rows = (rect.y..rect.bottom())
            .map(|y| {
                let mut row = String::new();
                let mut x = rect.x;
                while x < rect.right() {
                    let symbol = buffer[(x, y)].symbol();
                    row.push_str(symbol);
                    x = x.saturating_add(u16::try_from(symbol.width().max(1)).unwrap_or(1));
                }
                row.trim_end().to_owned()
            })
            .collect();
    }
    if !s.moved {
        return;
    }
    let order = |p: TextPoint| {
        if s.pane == Pane::Transcript {
            transcript::selection_order(app, p)
        } else {
            Some((0, p.row, p.column))
        }
    };
    let (Some(mut a), Some(mut b)) = (order(s.start), order(s.end)) else {
        return;
    };
    if a > b {
        std::mem::swap(&mut a, &mut b);
    }
    for y in rect.y..rect.bottom() {
        let row = usize::from(y - rect.y);
        let p = if s.pane == Pane::Transcript {
            transcript_rows.get(row).copied().flatten()
        } else {
            Some(TextPoint {
                stream_generation: None,
                cell: CellId(0),
                row,
                column: 0,
            })
        };
        let Some(key) = p.and_then(order) else {
            continue;
        };
        if (key.0, key.1) < (a.0, a.1) || (key.0, key.1) > (b.0, b.1) {
            continue;
        }
        let mut start = if (key.0, key.1) == (a.0, a.1) { a.2 } else { 0 };
        let mut end = if (key.0, key.1) == (b.0, b.1) {
            b.2.saturating_add(1).min(rect.width)
        } else {
            rect.width
        };
        let used = (rect.x..rect.right())
            .rfind(|&x| !buffer[(x, y)].symbol().trim().is_empty())
            .map_or(0, |x| {
                (x - rect.x)
                    .saturating_add(
                        u16::try_from(buffer[(x, y)].symbol().width().max(1)).unwrap_or(rect.width),
                    )
                    .min(rect.width)
            });
        if start > 0 && buffer[(rect.x + start - 1, y)].symbol().width() > 1 {
            start -= 1;
        }
        if end > 0 && end < rect.width {
            let width = buffer[(rect.x + end - 1, y)].symbol().width();
            end = end
                .saturating_add(u16::try_from(width.saturating_sub(1)).unwrap_or(rect.width))
                .min(rect.width);
        }
        if end.min(used) > start {
            buffer.set_style(
                Rect::new(rect.x + start, y, end.min(used) - start, 1),
                app.theme.selection(),
            );
        }
    }
}
