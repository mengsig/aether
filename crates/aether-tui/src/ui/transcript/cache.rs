//! Layout cache: `(agent, cell, width, detail, theme_epoch, names)` → laid-out rows.
//! Bounded LRU (64 entries / 8 MiB); the active cell is keyed by revision. Owner: W2.

use std::collections::HashMap;
use std::sync::Arc;

use ratatui::text::Span;

use aether_core::id::AgentId;

use crate::app::transcript::{CellId, CellRevision, TranscriptEpoch};
use crate::ui::cells::text::Layout;

pub const MAX_ENTRIES: usize = 64;
pub const MAX_BYTES: usize = 8 * 1024 * 1024;
/// Heights are a few bytes each; past this many the map starts over.
const MAX_HEIGHTS: usize = 200_000;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct LayoutKey {
    pub agent: AgentId,
    pub epoch: TranscriptEpoch,
    pub cell: CellId,
    pub revision: CellRevision,
    pub width: u16,
    pub detail: bool,
    /// Bumped whenever the theme changes, invalidating every entry.
    pub theme_epoch: u32,
    /// `App::names_version`: a rename re-lays every cell that names an id.
    pub names: u64,
    pub presentation: u32,
    pub compact: bool,
    /// How many unseen entries the `N new` divider above this cell counts; 0 for none.
    pub unseen: usize,
}

/// A key without its revision: one height per cell and layout, the latest.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
struct HeightKey {
    agent: AgentId,
    epoch: TranscriptEpoch,
    cell: CellId,
    width: u16,
    detail: bool,
    theme_epoch: u32,
    names: u64,
    presentation: u32,
    compact: bool,
}

impl From<&LayoutKey> for HeightKey {
    fn from(key: &LayoutKey) -> Self {
        Self {
            agent: key.agent,
            epoch: key.epoch,
            cell: key.cell,
            width: key.width,
            detail: key.detail,
            theme_epoch: key.theme_epoch,
            names: key.names,
            presentation: key.presentation,
            compact: key.compact,
        }
    }
}

#[derive(Debug, Default)]
pub struct LayoutCache {
    /// Least recently used first.
    entries: Vec<(LayoutKey, Arc<Layout>, usize)>,
    bytes: usize,
    /// Outlives the lines, so scrolling past evicted cells needs no relayout.
    heights: HashMap<HeightKey, (CellRevision, usize)>,
    inserts: u64,
}

impl LayoutCache {
    pub fn get(&mut self, key: &LayoutKey) -> Option<Arc<Layout>> {
        let at = self.entries.iter().position(|(k, _, _)| k == key)?;
        let entry = self.entries.remove(at);
        let lines = Arc::clone(&entry.1);
        self.entries.push(entry);
        Some(lines)
    }

    pub fn insert(&mut self, key: LayoutKey, lines: Arc<Layout>) {
        self.inserts += 1;
        if self.heights.len() >= MAX_HEIGHTS {
            self.heights.clear();
        }
        let height = lines.len();
        self.heights
            .insert(HeightKey::from(&key), (key.revision, height));
        if let Some(at) = self.entries.iter().position(|(k, _, _)| *k == key) {
            let (_, _, bytes) = self.entries.remove(at);
            self.bytes -= bytes;
        }
        let bytes = size_of_layout(&lines);
        // Keep the height for scroll math, but let the caller own an oversized
        // layout only for this draw. It must not displace useful cached cells.
        if bytes > MAX_BYTES {
            return;
        }
        self.entries.push((key, lines, bytes));
        self.bytes += bytes;
        while self.entries.len() > MAX_ENTRIES || self.bytes > MAX_BYTES {
            let (_, _, bytes) = self.entries.remove(0);
            self.bytes -= bytes;
        }
    }

    /// Cached height, without the lines (scroll math for unseen cells).
    pub fn height(&self, key: &LayoutKey) -> Option<usize> {
        match self.heights.get(&HeightKey::from(key)) {
            Some((revision, height)) if *revision == key.revision => Some(*height),
            _ => None,
        }
    }

    pub fn len(&self) -> usize {
        self.entries.len()
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// Bytes the cached lines hold, by estimate.
    pub fn bytes(&self) -> usize {
        self.bytes
    }

    /// Layout insert attempts since creation: what tests count to prove only
    /// visible cells were laid out.
    pub fn inserts(&self) -> u64 {
        self.inserts
    }
}

fn size_of_layout(layout: &Layout) -> usize {
    // Include the Arc header and allocated capacities; borrowed text holds no
    // allocation in this cache, and spare String/Vec capacity still does.
    let mut bytes = 2 * size_of::<usize>() + size_of::<Layout>() + layout.row_bytes();
    for line in layout.lines() {
        bytes = bytes.saturating_add(line.spans.capacity() * size_of::<Span<'static>>());
        for span in &line.spans {
            if let std::borrow::Cow::Owned(text) = &span.content {
                bytes = bytes.saturating_add(text.capacity());
            }
        }
    }
    bytes
}

#[cfg(test)]
mod tests {
    use super::*;
    use ratatui::text::Line;

    static EPOCH: std::sync::LazyLock<TranscriptEpoch> =
        std::sync::LazyLock::new(TranscriptEpoch::default);

    fn key(agent: AgentId, cell: u64) -> LayoutKey {
        LayoutKey {
            agent,
            epoch: *EPOCH,
            cell: CellId(cell),
            revision: CellRevision(0),
            width: 80,
            detail: false,
            theme_epoch: 0,
            names: 0,
            presentation: 0,
            compact: false,
            unseen: 0,
        }
    }

    fn lines(text: &str, rows: usize) -> Arc<Layout> {
        let lines = (0..rows)
            .map(|_| Line::from(Span::raw(text.to_owned())))
            .collect();
        Arc::new(Layout::unmapped(lines))
    }

    #[test]
    fn evicts_the_least_recently_used_entry_past_the_entry_bound() {
        let agent = AgentId::new();
        let mut cache = LayoutCache::default();
        for cell in 0..MAX_ENTRIES as u64 {
            cache.insert(key(agent, cell), lines("x", 1));
        }
        // Touch the oldest so the second-oldest is the one to go.
        assert!(cache.get(&key(agent, 0)).is_some());
        cache.insert(key(agent, 999), lines("x", 1));
        assert_eq!(cache.len(), MAX_ENTRIES);
        assert!(cache.get(&key(agent, 0)).is_some());
        assert!(cache.get(&key(agent, 1)).is_none());
        assert!(cache.get(&key(agent, 999)).is_some());
        assert_eq!(cache.inserts(), MAX_ENTRIES as u64 + 1);
    }

    #[test]
    fn evicts_past_the_byte_bound_but_keeps_heights() {
        let agent = AgentId::new();
        let mut cache = LayoutCache::default();
        let big = "x".repeat(1024 * 1024);
        for cell in 0..10 {
            cache.insert(key(agent, cell), lines(&big, 1));
        }
        assert!(cache.bytes() <= MAX_BYTES);
        assert!(cache.len() < 10);
        assert!(cache.get(&key(agent, 0)).is_none(), "the oldest went first");
        assert_eq!(cache.height(&key(agent, 0)), Some(1), "its height stays");
    }

    #[test]
    fn a_new_revision_misses_both_lines_and_height() {
        let agent = AgentId::new();
        let mut cache = LayoutCache::default();
        cache.insert(key(agent, 1), lines("x", 3));
        let mut next = key(agent, 1);
        next.revision = CellRevision(1);
        assert!(cache.get(&next).is_none());
        assert_eq!(cache.height(&next), None);
        assert_eq!(cache.height(&key(agent, 1)), Some(3));
        let mut wide = key(agent, 1);
        wide.width = 120;
        assert_eq!(cache.height(&wide), None, "width is part of the key");
    }

    #[test]
    fn oversized_singleton_keeps_its_height_but_no_lines() {
        let agent = AgentId::new();
        let mut cache = LayoutCache::default();
        let key = key(agent, 1);
        cache.insert(key, lines(&"x".repeat(MAX_BYTES + 1), 1));
        assert_eq!(cache.bytes(), 0);
        assert!(cache.is_empty());
        assert!(cache.get(&key).is_none());
        assert_eq!(cache.height(&key), Some(1));
    }

    #[test]
    fn oversized_replacement_removes_old_lines_without_evicting_other_cells() {
        let agent = AgentId::new();
        let mut cache = LayoutCache::default();
        let replaced = key(agent, 1);
        let other = key(agent, 2);
        cache.insert(replaced, lines("old", 1));
        cache.insert(other, lines("keep", 1));
        let retained = cache.get(&other).expect("retained layout");
        let retained_bytes = size_of_layout(&retained);
        cache.insert(replaced, lines(&"x".repeat(MAX_BYTES), 2));
        assert_eq!(cache.bytes(), retained_bytes);
        assert_eq!(cache.len(), 1);
        assert!(cache.get(&replaced).is_none());
        assert_eq!(cache.height(&replaced), Some(2));
        assert!(Arc::ptr_eq(
            &cache.get(&other).expect("retained"),
            &retained
        ));
    }

    #[test]
    fn byte_bound_counts_spare_string_and_span_capacity() {
        let agent = AgentId::new();
        let mut cache = LayoutCache::default();
        let mut text = String::with_capacity(MAX_BYTES);
        text.push('x');
        let one = |line| Arc::new(Layout::unmapped(vec![line]));
        cache.insert(key(agent, 1), one(Line::from(Span::raw(text))));
        assert!(cache.is_empty(), "capacity counts even for short text");

        let mut spans = Vec::with_capacity(MAX_BYTES / size_of::<Span<'static>>() + 1);
        spans.push(Span::raw("x"));
        cache.insert(key(agent, 2), one(Line::from(spans)));
        assert!(cache.is_empty(), "unused span capacity counts too");
        assert_eq!(cache.bytes(), 0);
    }

    #[test]
    fn an_entry_at_the_byte_bound_is_retained_and_shared() {
        let agent = AgentId::new();
        let mut cache = LayoutCache::default();
        let empty = lines("", 1);
        let layout = lines(&"x".repeat(MAX_BYTES - size_of_layout(&empty)), 1);
        assert_eq!(size_of_layout(&layout), MAX_BYTES);
        cache.insert(key(agent, 1), Arc::clone(&layout));
        assert_eq!(cache.bytes(), MAX_BYTES);
        assert!(Arc::ptr_eq(
            &cache.get(&key(agent, 1)).expect("hit"),
            &layout
        ));
        cache.insert(key(agent, 2), lines("next", 1));
        assert!(cache.bytes() <= MAX_BYTES);
        assert!(cache.get(&key(agent, 1)).is_none());
        assert_eq!(cache.height(&key(agent, 1)), Some(1));
    }
}
