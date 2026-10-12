//! Row arithmetic over one transcript without laying out all of it: every
//! position is relative to a cell, so only cells the viewport crosses are
//! measured. Owner: W2.

use std::sync::Arc;

use aether_core::id::AgentId;
use aether_core::schedule::Schedule;
use aether_protocol::AgentTree;
use ratatui::text::Line;

use super::Anchor;
use super::cache::{LayoutCache, LayoutKey};
use super::stream::StreamView;
use crate::app::name_of_id;
use crate::app::transcript::CellModel;
use crate::app::transcript::{AgentTranscript, Cell, CellId};
use crate::ui::cells::ChatCell;
use crate::ui::cells::text::{Layout, RowSource};
use crate::ui::theme::tokens::Theme;

/// Anchor id of the streaming reply, which has no cell yet.
pub const STREAM_ID: CellId = CellId(u64::MAX);
/// Id of the session header a root transcript opens with; it is no transcript cell.
pub const HEADER_ID: CellId = CellId(u64::MAX - 1);

/// A row position: `row` rows into item `item`'s block (gap row first).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Pos {
    pub item: usize,
    pub row: usize,
}

/// One item's rows: a cell's cached layout, or the streaming reply's rows
/// lent by its cache. Either way nothing is copied until a row is drawn.
#[derive(Debug, Clone)]
pub enum ItemLines<'a> {
    Cell(Arc<Layout>),
    Stream(StreamView<'a>),
}

impl<'a> ItemLines<'a> {
    pub fn len(&self) -> usize {
        match self {
            ItemLines::Cell(layout) => layout.len(),
            ItemLines::Stream(view) => view.len(),
        }
    }

    pub fn get(&self, row: usize) -> Option<&Line<'static>> {
        match self {
            ItemLines::Cell(layout) => layout.lines().get(row),
            ItemLines::Stream(view) => view.get(row),
        }
    }

    /// Where row `row` sits in its source line; the stream does not track it.
    pub fn source(&self, row: usize) -> RowSource {
        match self {
            ItemLines::Cell(layout) => layout.source(row).unwrap_or_default(),
            ItemLines::Stream(_) => RowSource::default(),
        }
    }
}

/// One transcript as the view sees it: a root's session header, committed
/// cells, the live cell, then the streaming reply. Each non-empty item after the first is
/// preceded by one blank row; empty items (compact reasoning) take none.
#[derive(Debug)]
pub struct Rows<'a> {
    pub visible: Arc<Vec<usize>>,
    pub pending: Arc<Vec<CellId>>,
    pub active: bool,
    pub policy: crate::view_modes::Policy<'a>,
    pub presentation: u32,
    pub agent: AgentId,
    /// Item 0 when set, so the top of a root's history shows the session (§3.1).
    pub header: Option<Cell>,
    pub transcript: &'a AgentTranscript,
    pub stream: Option<StreamView<'a>>,
    pub width: u16,
    pub detail: bool,
    pub theme: &'a Theme,
    pub keys: &'a crate::keys::KeyBindings,
    pub theme_epoch: u32,
    /// Resolves the ids daemon text carries; see `App::names_version`.
    pub tree: &'a AgentTree,
    pub schedules: &'a [Schedule],
    pub names: u64,
}

impl<'a> Rows<'a> {
    pub fn len(&self) -> usize {
        self.lead()
            + self.visible.len()
            + usize::from(self.active)
            + self.pending.len()
            + usize::from(self.stream.is_some())
    }

    /// Items before the transcript's first cell.
    fn lead(&self) -> usize {
        usize::from(self.header.is_some())
    }

    pub fn cell(&self, item: usize) -> Option<&Cell> {
        let t = self.transcript;
        let Some(item) = item.checked_sub(self.lead()) else {
            return self.header.as_ref();
        };
        if let Some(&at) = self.visible.get(item) {
            return t.cells.get(at);
        }
        let item = item - self.visible.len();
        if self.active && item == 0 {
            return t.active.as_ref();
        }
        self.pending
            .get(item.saturating_sub(usize::from(self.active)))
            .and_then(|id| t.cell(*id))
    }

    pub fn id(&self, item: usize) -> CellId {
        self.cell(item).map_or(STREAM_ID, |c| c.id)
    }

    pub fn find(&self, id: CellId) -> Option<usize> {
        if id == STREAM_ID {
            return self.stream.as_ref().map(|_| self.len() - 1);
        }
        if id == HEADER_ID {
            return self.header.as_ref().map(|_| 0);
        }
        if let Some(at) = self.transcript.position(id) {
            if at == self.transcript.cells.len() && self.active {
                return Some(self.lead() + self.visible.len());
            }
            return self
                .visible
                .binary_search(&at)
                .ok()
                .map(|index| self.lead() + index);
        }
        self.pending
            .binary_search(&id)
            .ok()
            .map(|index| self.lead() + self.visible.len() + usize::from(self.active) + index)
    }

    pub fn key(&self, cell: &Cell) -> LayoutKey {
        LayoutKey {
            agent: self.agent,
            epoch: self.transcript.epoch,
            cell: cell.id,
            revision: cell.revision,
            width: self.width,
            detail: self.detail || self.policy.expands(&cell.model),
            theme_epoch: self.theme_epoch,
            names: self.names,
            presentation: self.presentation,
            compact: self.policy.compact(&cell.model),
        }
    }

    /// A failed tool line under another: a run of failures reads as its first line.
    fn folded(&self, item: usize) -> bool {
        let failed = |at: usize| {
            self.cell(at)
                .is_some_and(|cell| self.policy.failed_tool_line(&cell.model))
        };
        item > 0 && failed(item) && failed(item - 1)
    }

    pub fn lines(&self, item: usize, cache: &mut LayoutCache) -> ItemLines<'a> {
        let Some(cell) = self.cell(item) else {
            return match self.stream {
                Some(view) => ItemLines::Stream(view),
                None => ItemLines::Cell(Arc::default()),
            };
        };
        if self.folded(item) {
            return ItemLines::Cell(Arc::default());
        }
        let key = self.key(cell);
        if let Some(lines) = cache.get(&key) {
            return ItemLines::Cell(lines);
        }
        if self.policy.compact(&cell.model)
            && let Some(line) = crate::ui::cells::compact::line(&cell.model, self.width, self.theme)
        {
            let layout = Arc::new(crate::ui::cells::text::Layout::unmapped(vec![line]));
            cache.insert(key, Arc::clone(&layout));
            return ItemLines::Cell(layout);
        }
        let named = cell.model.with_names(|id| name_of_id(self.tree, id));
        let model = named.as_ref().unwrap_or(&cell.model);
        let scheduled = model.with_schedule_name(
            |id| {
                let job = self.schedules.iter().find(|job| job.id == id)?;
                Some(job.name.clone())
            },
            self.detail,
        );
        let model = scheduled.as_ref().unwrap_or(model);
        let mut layout = if matches!(model, CellModel::Header { .. }) {
            Layout::unmapped(crate::ui::cells::header::display_lines_with_keys(
                model, self.width, self.theme, self.keys,
            ))
        } else {
            model.layout(self.width, key.detail, self.theme)
        };
        if self.keys.customised(crate::keys::KeyAction::Details) {
            let hint = format!(
                " ({} to view)",
                self.keys
                    .display(crate::keys::KeyAction::Details, self.theme.ascii())
            );
            for (line, _) in layout.rows_mut() {
                let mut changed = false;
                for span in &mut line.spans {
                    if span.style == self.theme.secondary()
                        && let Some(prefix) = span
                            .content
                            .strip_suffix(crate::ui::cells::text::DETAILS_HINT)
                        && prefix.starts_with(&format!("{} +", self.theme.glyphs().ellipsis))
                    {
                        span.content = format!("{prefix}{hint}").into();
                        changed = true;
                    }
                }
                if changed {
                    *line = Line::from(crate::ui::cells::text::truncate(
                        std::mem::take(&mut line.spans),
                        usize::from(self.width),
                        self.theme,
                    ));
                }
            }
        }
        let layout = Arc::new(layout);
        cache.insert(key, Arc::clone(&layout));
        ItemLines::Cell(layout)
    }

    pub fn height(&self, item: usize, cache: &mut LayoutCache) -> usize {
        if self.folded(item) {
            return 0;
        }
        if let Some(cell) = self.cell(item)
            && let Some(height) = cache.height(&self.key(cell))
        {
            return height;
        }
        self.lines(item, cache).len()
    }

    /// The blank row above an item: none for the first or for an empty item.
    pub fn gap(&self, item: usize, cache: &mut LayoutCache) -> usize {
        usize::from(
            item > 0
                && self.height(item, cache) > 0
                && !self
                    .policy
                    .definition
                    .is_some_and(|d| d.density == crate::view_modes::Density::Compact),
        )
    }

    pub fn block(&self, item: usize, cache: &mut LayoutCache) -> usize {
        self.gap(item, cache) + self.height(item, cache)
    }

    /// The top row when the last row sits at the bottom of `view` rows.
    pub fn bottom_top(&self, view: usize, cache: &mut LayoutCache) -> Pos {
        let mut below = 0;
        for item in (0..self.len()).rev() {
            let block = self.block(item, cache);
            if below + block >= view {
                return Pos {
                    item,
                    row: below + block - view,
                };
            }
            below += block;
        }
        Pos { item: 0, row: 0 }
    }

    pub fn resolve(&self, anchor: Anchor, view: usize, cache: &mut LayoutCache) -> Pos {
        let Anchor::Reading { cell, row } = anchor else {
            return self.bottom_top(view, cache);
        };
        match self.find(cell) {
            Some(item) => {
                let last = self.block(item, cache).saturating_sub(1);
                Pos {
                    item,
                    row: row.min(last),
                }
            }
            None if cell <= self.transcript.id_floor => self.bottom_top(view, cache),
            None => self
                .transcript
                .position(cell)
                .and_then(|at| {
                    let index = self.visible.partition_point(|pos| *pos < at);
                    self.visible
                        .get(index)
                        .or_else(|| self.visible.last())
                        .and_then(|at| self.visible.binary_search(at).ok())
                })
                .map(|index| Pos {
                    item: self.lead() + index,
                    row: 0,
                })
                .or_else(|| {
                    self.transcript
                        .all_cells()
                        .find(|retained| retained.id > cell && self.find(retained.id).is_some())
                        .and_then(|retained| self.find(retained.id))
                        .map(|item| Pos { item, row: 0 })
                })
                .unwrap_or_else(|| self.bottom_top(view, cache)),
        }
    }

    /// Whether everything from `pos` down fits in `view` rows, i.e. `pos` is
    /// at or past the bottom-aligned top. Measures at most a viewport.
    fn fits_below(&self, pos: Pos, view: usize, cache: &mut LayoutCache) -> bool {
        let mut rows = 0;
        for item in pos.item..self.len() {
            let block = self.block(item, cache);
            rows += if item == pos.item {
                block.saturating_sub(pos.row)
            } else {
                block
            };
            if rows > view {
                return false;
            }
        }
        true
    }

    /// Reading at `pos`, or following when `pos` reached the bottom.
    pub fn anchor_at(&self, pos: Pos, view: usize, cache: &mut LayoutCache) -> Anchor {
        if self.len() == 0 || self.fits_below(pos, view, cache) {
            return Anchor::Latest;
        }
        Anchor::Reading {
            cell: self.id(pos.item),
            row: pos.row,
        }
    }

    pub fn up(&self, mut pos: Pos, mut n: usize, cache: &mut LayoutCache) -> Pos {
        loop {
            if n <= pos.row {
                pos.row -= n;
                return pos;
            }
            n -= pos.row;
            // One more row reaches the last row of the previous non-empty item.
            let mut prev = pos.item;
            let block = loop {
                if prev == 0 {
                    return Pos {
                        item: pos.item,
                        row: 0,
                    };
                }
                prev -= 1;
                let block = self.block(prev, cache);
                if block > 0 {
                    break block;
                }
            };
            n -= 1;
            pos = Pos {
                item: prev,
                row: block - 1,
            };
        }
    }

    pub fn down(&self, mut pos: Pos, n: usize, cache: &mut LayoutCache) -> Pos {
        pos.row += n;
        loop {
            let block = self.block(pos.item, cache);
            if pos.row < block || pos.item + 1 >= self.len() {
                pos.row = pos.row.min(block.saturating_sub(1));
                return pos;
            }
            pos.row -= block;
            pos.item += 1;
        }
    }
}
