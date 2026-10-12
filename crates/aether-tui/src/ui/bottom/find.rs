//! Ctrl+F find in the viewed transcript (footer bar). Owner: W4.

use std::collections::HashMap;

use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::text::{Line, Span};
use ratatui::widgets::Paragraph;

use aether_core::id::AgentId;

use crate::app::App;
use crate::app::transcript::{CellId, CellRevision, TranscriptEpoch};
use crate::ui::cells::{ChatCell, plain_theme};
use crate::ui::theme::tokens::Theme;

/// Layout width for indexing; source lines rejoin soft wraps before matching.
const FIND_WIDTH: u16 = 1_000;

#[derive(Debug, Clone, Default)]
pub struct FindBar {
    pub query: String,
    /// One entry per occurrence, oldest first; repeated cell ids identify successive hits.
    pub matches: Vec<CellId>,
    pub current: Option<usize>,
    /// Plain lines per cell revision: rendering every cell on each key misses the frame.
    pub plain: PlainIndex,
}

#[derive(Debug, Clone, Default)]
pub struct PlainIndex {
    agent: Option<AgentId>,
    /// The lowercased query every cached `hit` answers.
    needle: String,
    cells: HashMap<CellId, Searchable>,
    /// The transcript `matches` were last computed from; unchanged means nothing to rescan.
    scanned: Option<(TranscriptEpoch, u64)>,
    presentation: Option<(u32, u64, bool, u16)>,
    #[cfg(test)]
    scans: usize,
}

/// A cell's text in both views: details hold rows the compact preview omits.
#[derive(Debug, Clone)]
struct Searchable {
    revision: CellRevision,
    /// Source lines joined by `\n`: one contiguous scan, and a one-line query never spans two.
    compact: String,
    /// `None` where details show nothing the compact text does not.
    detail: Option<String>,
    // One-line modes prefer shown occurrences before revealing the full cell.
    one_line: bool,
    /// Occurrence count for `PlainIndex::needle`: a transcript change rescans changed cells only.
    hit: Option<usize>,
}

impl Searchable {
    /// `theme` is `plain_theme()`, resolved once per refresh rather than per cell.
    fn of(cell: &impl ChatCell, revision: CellRevision, theme: &Theme) -> Self {
        let compact = cell.layout(FIND_WIDTH, false, theme).rejoined().join("\n");
        let detail = cell.layout(FIND_WIDTH, true, theme).rejoined().join("\n");
        Searchable {
            revision,
            detail: (detail != compact).then_some(detail),
            compact,
            one_line: false,
            hit: None,
        }
    }

    fn matches(&self, needle: &Needle) -> usize {
        let compact = needle.count(&self.compact);
        match &self.detail {
            Some(detail) => compact.max(needle.count(detail)),
            None => compact,
        }
    }
}

fn occurrence_at<'a>(
    text: &'a str,
    needle: &Needle,
    mut index: usize,
) -> Option<(&'a str, usize, usize)> {
    for line in text.split('\n') {
        let hits = needle.ranges(line);
        if let Some(&(start, end)) = hits.get(index) {
            return Some((line, start, end));
        }
        index -= hits.len();
    }
    None
}

impl FindBar {
    pub(crate) fn occurrence_in(&self, cell: CellId) -> Option<usize> {
        let current = self.current?;
        if self.matches.get(current) != Some(&cell) {
            return None;
        }
        Some(
            self.matches[..current]
                .iter()
                .rev()
                .take_while(|id| **id == cell)
                .count(),
        )
    }
}

/// Recompute `matches` for the viewed agent; Enter/↓ then push `ScrollIntent::Reveal`.
/// Runs after most events while open; an unchanged transcript skips the walk.
pub fn refresh(bar: &mut FindBar, app: &App) {
    let transcript = app.selected.and_then(|id| app.transcripts.get(&id));
    let cells = transcript.map(|t| t.cells.as_slice()).unwrap_or_default();
    let source = transcript.map(|t| (t.epoch, t.committed_revision()));
    let policy = app.policy(app.selected);
    let width = if policy
        .definition
        .is_some_and(|d| d.density == crate::view_modes::Density::Compact)
    {
        crate::ui::bottom_width(app.size.0, app.board_mode())
    } else {
        FIND_WIDTH
    };
    let index = &mut bar.plain;
    let presentation = (
        app.presentation_revision,
        app.names_version,
        app.details,
        width,
    );
    if index.agent != app.selected || index.presentation != Some(presentation) {
        index.presentation = Some(presentation);
        index.agent = app.selected;
        index.cells.clear();
        index.scanned = None;
    }
    if bar.query.is_empty() {
        index.scanned = None;
        set_matches(bar, Vec::new());
        return;
    }
    let needle = fold(&bar.query);
    if index.needle != needle {
        index.cells.values_mut().for_each(|entry| entry.hit = None);
        index.needle.clone_from(&needle);
        index.scanned = None;
    }
    if source.is_some() && index.scanned == source {
        return;
    }
    index.scanned = source;
    #[cfg(test)]
    {
        index.scans += 1;
    }
    let theme = plain_theme();
    let searcher = Needle::new(&bar.query);
    let mut matches = Vec::new();
    let named = policy.definition.is_some() || app.details && app.view_mode != "standard";
    let mut after_failed = false;
    for cell in cells.iter().chain(
        transcript
            .into_iter()
            .filter(|_| named)
            .flat_map(|t| t.pending_messages.values()),
    ) {
        if !policy.shows(&cell.model)
            || !named && transcript.is_some_and(|t| t.named_only.contains(&cell.id))
        {
            continue;
        }
        // Only the first of a run of failed tool lines is drawn.
        let failed = policy.failed_tool_line(&cell.model);
        let folded = failed && after_failed;
        after_failed = failed;
        if folded {
            continue;
        }
        let entry = index
            .cells
            .entry(cell.id)
            .or_insert_with(|| Searchable::of(&cell.model, cell.revision, &theme));
        if entry.revision != cell.revision {
            *entry = Searchable::of(&cell.model, cell.revision, &theme);
        }
        // These rows are already expanded; finding them must not reveal hidden categories.
        if policy.expands(&cell.model)
            && let Some(detail) = entry.detail.take()
        {
            entry.compact = detail;
            entry.hit = None;
        }
        if policy.compact(&cell.model)
            && let Some(line) = crate::ui::cells::compact::line(&cell.model, width, &theme)
        {
            entry.one_line = true;
            let compact = line.to_string();
            if entry.compact != compact {
                entry.detail = Some(
                    cell.model
                        .layout(FIND_WIDTH, true, &theme)
                        .rejoined()
                        .join("\n"),
                );
                entry.compact = compact;
                entry.hit = None;
            }
        }
        let hit = match entry.hit {
            Some(hit) => hit,
            None => *entry.hit.insert(entry.matches(&searcher)),
        };
        matches.extend(std::iter::repeat_n(cell.id, hit));
    }
    set_matches(bar, matches);
}

/// Full walks of the viewed transcript so far.
#[cfg(test)]
pub(crate) fn scans(bar: &FindBar) -> usize {
    bar.plain.scans
}

/// The current match shows only in details: its compact preview leaves it out.
pub fn current_needs_details(bar: &FindBar) -> bool {
    let Some(cell) = bar.current.and_then(|i| bar.matches.get(i)) else {
        return false;
    };
    let needle = Needle::new(&bar.query);
    let occurrence = bar.occurrence_in(*cell).unwrap_or(0);
    bar.plain.cells.get(cell).is_some_and(|entry| {
        if entry.one_line {
            return occurrence >= needle.count(&entry.compact);
        }
        let Some(detail) = &entry.detail else {
            return false;
        };
        let detail = occurrence_at(detail, &needle, occurrence);
        detail.is_some() && detail != occurrence_at(&entry.compact, &needle, occurrence)
    })
}

/// Cells whose compact or detail text contains `query` on one line, ignoring case.
#[cfg(test)]
pub(crate) fn matching_cells<'a, C: ChatCell + 'a>(
    cells: impl Iterator<Item = (CellId, &'a C)>,
    query: &str,
) -> Vec<CellId> {
    if query.is_empty() {
        return Vec::new();
    }
    let needle = Needle::new(query);
    let theme = plain_theme();
    cells
        .filter(|(_, cell)| {
            Searchable::of(*cell, CellRevision::default(), &theme).matches(&needle) > 0
        })
        .map(|(id, _)| id)
        .collect()
}

/// New matches start at the first one, or keep the current cell when it still matches.
pub(crate) fn set_matches(bar: &mut FindBar, matches: Vec<CellId>) {
    let current_cell = bar.current.and_then(|i| bar.matches.get(i).copied());
    let occurrence = current_cell
        .and_then(|cell| bar.occurrence_in(cell))
        .unwrap_or(0);
    bar.current = current_cell
        .and_then(|cell| {
            matches
                .iter()
                .enumerate()
                .filter(|(_, m)| **m == cell)
                .nth(occurrence)
                .map(|(i, _)| i)
        })
        .or((!matches.is_empty()).then_some(0));
    bar.matches = matches;
}

/// Next (`1`) or previous (`-1`) match, wrapping. `false` when there is none.
pub fn step(bar: &mut FindBar, delta: isize) -> bool {
    let len = bar.matches.len();
    let Some(current) = bar.current else {
        return false;
    };
    let len = isize::try_from(len).unwrap_or(isize::MAX);
    let current = isize::try_from(current).unwrap_or(0);
    let next = (current + delta).rem_euclid(len);
    bar.current = usize::try_from(next).ok();
    true
}

/// Replaces the hint row while open, so a live `flash` takes the place of its hints.
pub fn render(frame: &mut Frame, area: Rect, bar: &FindBar, flash: Option<&str>, theme: &Theme) {
    let mut spans = vec![
        Span::raw("  "),
        Span::styled("find ", theme.secondary()),
        Span::styled(bar.query.clone(), theme.text()),
        Span::styled(theme.glyphs().caret, theme.accent()),
        Span::raw("  "),
    ];
    let status = match bar.current {
        Some(current) => Some(format!("{} of {}", current + 1, bar.matches.len())),
        None if !bar.query.is_empty() => Some("no matches".to_owned()),
        None => None,
    };
    let dot = theme.glyphs().dot;
    if let Some(status) = status {
        spans.push(Span::styled(format!("{status} {dot} "), theme.secondary()));
    }
    if let Some(flash) = flash {
        spans.push(Span::styled(flash.to_owned(), theme.text()));
        frame.render_widget(Paragraph::new(Line::from(spans)), area);
        return;
    }
    let up = if theme.ascii() { "^" } else { "↑" };
    let hints = [("enter", "next"), (up, "previous"), ("esc", "close")];
    for (i, (key, desc)) in hints.into_iter().enumerate() {
        if i > 0 {
            spans.push(Span::styled(format!(" {dot} "), theme.secondary()));
        }
        spans.push(Span::styled(key, theme.title()));
        spans.push(Span::styled(format!(" {desc}"), theme.secondary()));
    }
    frame.render_widget(Paragraph::new(Line::from(spans)), area);
}

/// Byte ranges of every case-insensitive match, including overlaps.
pub(crate) fn occurrences(text: &str, query: &str) -> Vec<(usize, usize)> {
    Needle::new(query).ranges(text)
}

/// Lowercase char by char, as the text is: `str::to_lowercase` turns a final `Σ` into `ς`.
fn fold(text: &str) -> String {
    text.chars()
        .flat_map(char::to_lowercase)
        .map(fold_sigma)
        .collect()
}

fn fold_sigma(ch: char) -> char {
    if ch == 'ς' { 'σ' } else { ch }
}

/// A folded query and its KMP table, built once per search rather than once per line.
/// KMP keeps repetitive queries linear; folded Unicode ranges map back to source characters.
struct Needle {
    bytes: Vec<u8>,
    /// border[i]: the longest proper prefix of bytes[..=i] that is also its suffix.
    border: Vec<usize>,
}

impl Needle {
    fn new(query: &str) -> Self {
        debug_assert!(!query.contains('\n'), "find queries are one line");
        let bytes = fold(query).into_bytes();
        let mut border = vec![0; bytes.len()];
        let mut k = 0;
        for i in 1..bytes.len() {
            while k > 0 && bytes[i] != bytes[k] {
                k = border[k - 1];
            }
            if bytes[i] == bytes[k] {
                k += 1;
            }
            border[i] = k;
        }
        Needle { bytes, border }
    }

    fn count(&self, text: &str) -> usize {
        let mut count = 0;
        self.scan(text, |_, _| count += 1);
        count
    }

    fn ranges(&self, text: &str) -> Vec<(usize, usize)> {
        let mut hits = Vec::new();
        self.scan(text, |start, end| hits.push((start, end)));
        hits
    }

    /// Calls `hit` with the source byte range of every match, overlaps included.
    fn scan(&self, text: &str, mut hit: impl FnMut(usize, usize)) {
        let len = self.bytes.len();
        if len == 0 {
            return;
        }
        let mut k = 0;
        if text.is_ascii() {
            for (i, byte) in text.bytes().enumerate() {
                if self.step(&mut k, byte.to_ascii_lowercase()) {
                    hit(i + 1 - len, i + 1);
                }
            }
            return;
        }
        // Folding is streamed, not copied: a ring holds the source start of the last `len` folded bytes.
        let mut starts = vec![0; len];
        // The next ring slot to write, which is also the oldest: where a match ending now began.
        let mut slot = 0;
        let mut last = None;
        let mut feed = |byte: u8, start: usize, end: usize| {
            starts[slot] = start;
            slot = if slot + 1 == len { 0 } else { slot + 1 };
            if !self.step(&mut k, byte) {
                return;
            }
            // One source character can fold to several matches of the same range.
            let range = (starts[slot], end);
            if last != Some(range) {
                hit(range.0, range.1);
                last = Some(range);
            }
        };
        for (start, ch) in text.char_indices() {
            let end = start + ch.len_utf8();
            if ch.is_ascii() {
                feed((ch as u8).to_ascii_lowercase(), start, end);
                continue;
            }
            for lower in ch.to_lowercase() {
                let mut buf = [0; 4];
                for &byte in fold_sigma(lower).encode_utf8(&mut buf).as_bytes() {
                    feed(byte, start, end);
                }
            }
        }
    }

    /// One KMP step over a folded byte; `true` when it completes a match.
    fn step(&self, k: &mut usize, byte: u8) -> bool {
        while *k > 0 && byte != self.bytes[*k] {
            *k = self.border[*k - 1];
        }
        if byte == self.bytes[*k] {
            *k += 1;
        }
        if *k < self.bytes.len() {
            return false;
        }
        *k = self.border[*k - 1];
        true
    }
}

#[cfg(test)]
mod tests {
    use ratatui::Terminal;
    use ratatui::backend::TestBackend;
    use ratatui::buffer::Buffer;

    use super::*;
    use crate::ui::theme::tokens::ThemeSpec;

    struct Plain(&'static str);

    impl ChatCell for Plain {
        fn display_lines(&self, _: u16, _: &Theme) -> Vec<Line<'static>> {
            self.0.lines().map(Line::from).collect()
        }

        fn plain_lines(&self, _: u16) -> Vec<String> {
            self.0.lines().map(str::to_owned).collect()
        }
    }

    fn row(bar: &FindBar, theme: &Theme, width: u16) -> Buffer {
        let mut terminal = Terminal::new(TestBackend::new(width, 1)).unwrap();
        terminal
            .draw(|f| render(f, f.area(), bar, None, theme))
            .unwrap();
        terminal.backend().buffer().clone()
    }

    fn text(buffer: &Buffer) -> String {
        let row: String = buffer.content().iter().map(|c| c.symbol()).collect();
        row.trim_end().to_owned()
    }

    #[test]
    fn final_sigma_matches_both_sigma_forms_with_source_byte_ranges() {
        for query in ["ΟΣ", "οσ", "ος"] {
            assert_eq!(
                occurrences("xος ΟΣ οσ!", query),
                [(1, 5), (6, 10), (11, 15)]
            );
        }
    }

    #[test]
    fn matches_are_case_insensitive_and_never_span_lines() {
        let cells = [
            (CellId(1), Plain("Running the Tests")),
            (CellId(2), Plain("the\ntests")),
            (CellId(3), Plain("all tests pass")),
        ];
        let iter = || cells.iter().map(|(id, cell)| (*id, cell));
        assert_eq!(
            matching_cells(iter(), "TESTS"),
            [CellId(1), CellId(2), CellId(3)]
        );
        assert_eq!(matching_cells(iter(), "the tests"), [CellId(1)]);
        assert!(matching_cells(iter(), "").is_empty());
    }

    #[test]
    fn tool_output_the_compact_preview_omits_still_matches() {
        use crate::app::transcript::{CallStatus, CellModel};
        let output = (1..=20)
            .map(|i| format!("ux-file-{i:02}.txt"))
            .collect::<Vec<_>>()
            .join("\n");
        let ls = CellModel::Exec {
            call: None,
            command: "ls".into(),
            status: CallStatus::Succeeded,
            output,
            exit: Some(0),
            duration_ms: Some(10),
            is_search: false,
            artifacts: Vec::new(),
        };
        let cells = [(CellId(1), ls)];
        let iter = || cells.iter().map(|(id, cell)| (*id, cell));
        for query in ["UX-FILE-03", "ux-file-11", "ux-file-20"] {
            assert_eq!(matching_cells(iter(), query), [CellId(1)], "{query}");
        }
        assert!(matching_cells(iter(), "ux-file-21").is_empty());
    }

    #[test]
    fn new_matches_keep_the_current_cell_when_it_still_matches() {
        let mut bar = FindBar::default();
        set_matches(&mut bar, vec![CellId(1), CellId(4), CellId(7)]);
        assert_eq!(bar.current, Some(0));
        bar.current = Some(1);
        set_matches(&mut bar, vec![CellId(4), CellId(7)]);
        assert_eq!(bar.current, Some(0));
        set_matches(&mut bar, vec![CellId(1), CellId(7)]);
        assert_eq!(bar.current, Some(0));
        set_matches(&mut bar, Vec::new());
        assert_eq!(bar.current, None);
    }

    #[test]
    fn steps_wrap_both_ways_and_report_nothing_to_step_to() {
        let mut bar = FindBar::default();
        assert!(!step(&mut bar, 1));
        set_matches(&mut bar, vec![CellId(1), CellId(2), CellId(3)]);
        assert!(step(&mut bar, -1));
        assert_eq!(bar.current, Some(2));
        assert!(step(&mut bar, 1));
        assert_eq!(bar.current, Some(0));
    }

    #[test]
    fn the_bar_shows_query_count_and_hints() {
        let theme = Theme::resolve(ThemeSpec::default());
        let mut bar = FindBar {
            query: "tests".into(),
            ..Default::default()
        };
        for width in [80, 120] {
            let buffer = row(&bar, &theme, width);
            assert_eq!(
                text(&buffer),
                "  find tests▍  no matches · enter next · ↑ previous · esc close"
            );
            assert_eq!(buffer[(2, 0)].fg, theme.secondary().fg.unwrap_or_default());
            assert_eq!(buffer[(7, 0)].fg, theme.text().fg.unwrap_or_default());
            assert_eq!(buffer[(12, 0)].fg, theme.accent().fg.unwrap_or_default());
        }
        set_matches(&mut bar, vec![CellId(1), CellId(2)]);
        bar.current = Some(1);
        assert_eq!(
            text(&row(&bar, &theme, 80)),
            "  find tests▍  2 of 2 · enter next · ↑ previous · esc close"
        );
        let ascii = Theme::resolve(ThemeSpec {
            ascii: true,
            ..Default::default()
        });
        assert_eq!(
            text(&row(&bar, &ascii, 120)),
            "  find tests|  2 of 2 | enter next | ^ previous | esc close"
        );
    }

    /// The bar covers the footer, so a flash raised while it is open shows here.
    #[test]
    fn a_flash_takes_the_place_of_the_hints() {
        let theme = Theme::resolve(ThemeSpec::default());
        let mut bar = FindBar {
            query: "ux-file-03".into(),
            ..Default::default()
        };
        set_matches(&mut bar, vec![CellId(4)]);
        let mut terminal = Terminal::new(TestBackend::new(90, 1)).unwrap();
        let flash = Some("details on to show the match · ctrl+t hides them");
        terminal
            .draw(|f| render(f, f.area(), &bar, flash, &theme))
            .unwrap();
        assert_eq!(
            text(terminal.backend().buffer()),
            "  find ux-file-03▍  1 of 1 · details on to show the match · ctrl+t hides them"
        );
    }

    /// Folds a copy, maps it back and dedups: the streamed scan must agree byte for byte.
    fn reference(text: &str, query: &str) -> Vec<(usize, usize)> {
        let needle = fold(query);
        let mut folded = String::new();
        let mut source = Vec::new();
        for (start, ch) in text.char_indices() {
            for lower in ch.to_lowercase() {
                folded.push(lower);
                source.extend(std::iter::repeat_n(
                    (start, start + ch.len_utf8()),
                    lower.len_utf8(),
                ));
            }
        }
        let mut hits: Vec<_> = (0..folded.len())
            .filter(|&at| {
                !needle.is_empty() && folded.as_bytes()[at..].starts_with(needle.as_bytes())
            })
            .map(|at| (source[at].0, source[at + needle.len() - 1].1))
            .collect();
        hits.dedup();
        hits
    }

    #[test]
    fn streamed_folding_matches_a_folded_copy() {
        let texts = [
            "",
            "plain ascii, Plain ASCII",
            "› about the build · └ about THE build…",
            "İİ i̇i̇ İi",
            "ΟΔΟΣ οδοσ ΟΔΟΣ",
            "\u{212A}elvin KELVIN kelvin",
            "aaaa ÅÅÅÅ åå",
            "ß SS ss ẞ",
            "日本語のテキスト about",
        ];
        let queries = [
            "a",
            "aa",
            "about the",
            "i",
            "i̇",
            "İ",
            "οδοσ",
            "kelvin",
            "å",
            "ß",
            "ss",
            "本語",
            "x",
        ];
        for text in texts {
            for query in queries {
                assert_eq!(
                    Needle::new(query).ranges(text),
                    reference(text, query),
                    "{text:?} / {query:?}"
                );
                assert_eq!(Needle::new(query).count(text), reference(text, query).len());
            }
        }
    }
}
