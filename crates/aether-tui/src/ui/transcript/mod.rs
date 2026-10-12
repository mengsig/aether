//! The transcript of the viewed agent (SPEC §1.1, §3, §7.4): scrolling by
//! semantic intents, the pinned prompt header, find highlights. Owner: W2.

pub mod cache;
mod projection;
mod selection;
pub mod stream;
mod window;
pub(crate) use selection::{
    selected_text, selection_order, selection_point, selection_resolves, selection_rows,
};

use std::collections::HashMap;

use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::style::Style;
use ratatui::text::{Line, Span};

use aether_core::id::AgentId;
use aether_core::time::Timestamp;

use crate::app::App;
use crate::app::transcript::{
    AgentTranscript, Cell, CellId, CellModel, CellRevision, ScrollIntent,
};
use crate::ui::board::BoardScroll;
use crate::ui::bottom::composer::ComposerCache;
use crate::ui::bottom::find::occurrences;
use crate::ui::cells::{self, user};
use crate::ui::theme::tokens::Theme;

pub use cache::LayoutCache;
use cache::LayoutKey;
use stream::StreamCache;
use window::{ItemLines, Pos, Rows};

/// Where a transcript is scrolled to, per agent. Lives here, not in `App`,
/// because resolving `PageUp` needs row heights only the layout knows.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Anchor {
    /// Follow the newest row.
    #[default]
    Latest,
    /// Keep `row` rows into `cell` at the top of the viewport.
    Reading { cell: CellId, row: usize },
}

/// Scroll state of every transcript, and how far into `App::scroll_log` it has read.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct TranscriptView {
    pub anchors: HashMap<AgentId, Anchor>,
    pub applied_scroll_seq: u64,
    pub(crate) older_requests: Vec<AgentId>,
    upward_at_top: Vec<AgentId>,
}

/// Render caches owned by the runner and passed `&mut` into `ui::render`.
/// Geometry, selection and wrapping stay here; `App` holds no `Rect`s or `Line`s.
#[derive(Debug, Default)]
pub struct ViewCache {
    presentation: Option<(u32, u64, bool)>,
    /// W2.
    pub layout: LayoutCache,
    pub regions: crate::ui::selection::FrameRegions,
    pub mouse: crate::ui::selection::MouseState,
    /// W2.
    pub transcript: TranscriptView,
    /// W3: tree scroll offset inside the board.
    pub board: BoardScroll,
    /// W4: composer wrap.
    pub composer: ComposerCache,
    /// W2: the streaming reply's last rendering.
    pub stream: StreamCache,
    pub find_hit: FindHitCache,
}

/// The current find hit's rows: mapping it walks its whole cell, too slow per frame.
#[derive(Debug, Default)]
pub struct FindHitCache {
    /// The layout, query and occurrence `rows` were mapped for.
    key: Option<(LayoutKey, String, usize)>,
    rows: Vec<(usize, usize, usize)>,
    #[cfg(test)]
    maps: usize,
}

impl FindHitCache {
    fn rows(
        &mut self,
        key: LayoutKey,
        lines: &ItemLines<'_>,
        query: &str,
        occurrence: usize,
    ) -> Vec<(usize, usize, usize)> {
        let fresh = self
            .key
            .as_ref()
            .is_some_and(|(k, q, o)| (*k, q.as_str(), *o) == (key, query, occurrence));
        if !fresh {
            #[cfg(test)]
            {
                self.maps += 1;
            }
            self.rows = selected_hit(lines, query, occurrence);
            self.key = Some((key, query.to_owned(), occurrence));
        }
        self.rows.clone()
    }
}

/// Paint the viewed agent's transcript into `area`, applying pending scroll
/// intents first. Lays out only the visible window.
pub fn render(frame: &mut Frame, area: Rect, app: &App, cache: &mut ViewCache, theme: &Theme) {
    cache.mouse.pinned = None;
    let view = usize::from(area.height);
    if view == 0 || area.width == 0 {
        return;
    }
    apply_scroll(app, area, cache, theme);
    let viewed = app
        .selected
        .and_then(|agent| Some((agent, app.transcripts.get(&agent)?)));
    let Some((agent, transcript)) = viewed else {
        return render_header(frame, area, app, theme);
    };
    let rows = rows_for(app, agent, transcript, area.width, &mut cache.stream, theme);
    if rows.len() == 0 {
        return render_header(frame, area, app, theme);
    }
    let anchor = cache.transcript.anchor(agent);
    let layout = &mut cache.layout;
    let top = rows.resolve(anchor, view, layout);
    cache.mouse.displayed_top = Anchor::Reading {
        cell: rows.id(top.item),
        row: top.row,
    };
    if let Anchor::Reading { cell, .. } = anchor
        && rows.find(cell).is_none()
        && rows.id(top.item) > cell
    {
        cache
            .transcript
            .anchors
            .insert(agent, rows.anchor_at(top, view, layout));
    }
    let phase = pulse_phase(app.now);
    let mut y = 0;
    let mut pos = top;
    while y < view && pos.item < rows.len() {
        let gap = rows.gap(pos.item, layout);
        let lines = rows.lines(pos.item, layout);
        let cell = rows.cell(pos.item);
        let current_hit = cell
            .and_then(|cell| {
                let find = app.bottom.find.as_ref()?;
                let occurrence = find.occurrence_in(cell.id)?;
                let key = rows.key(cell);
                Some(cache.find_hit.rows(key, &lines, &find.query, occurrence))
            })
            .unwrap_or_default();
        for row in pos.row..gap + lines.len() {
            if y >= view {
                break;
            }
            // Only drawn rows are copied: a long reply costs its visible rows.
            if let Some(line) = row.checked_sub(gap).and_then(|i| lines.get(i)) {
                let mut line = line.clone();
                if let Some(cell) = cell {
                    let at = (row - gap, lines.len());
                    if !rows.policy.compact(&cell.model) {
                        cells::decorate_live(&mut line, at, &cell.model, theme, phase, app.now);
                    }
                    if let Some(find) = &app.bottom.find {
                        highlight_matches(&mut line, (&lines, row - gap), &find.query, theme);
                        for &(_, start, end) in
                            current_hit.iter().filter(|(at, _, _)| *at == row - gap)
                        {
                            highlight_line(&mut line, &[(start, end)], theme.find_hit(true));
                        }
                    }
                }
                let at = Rect::new(area.x, area.y + y as u16, area.width, 1);
                paint_line(frame, at, &line);
            }
            y += 1;
        }
        pos = Pos {
            item: pos.item + 1,
            row: 0,
        };
    }
    if let Some((pinned, point)) = pinned_prompt(&rows, top, layout) {
        cache.mouse.pinned = Some(point);
        // Header row, then an untinted blank so the next cell reads apart from it.
        let at = Rect::new(area.x, area.y, area.width, PINNED_ROWS.min(area.height));
        frame.render_widget(ratatui::widgets::Clear, at);
        paint_line(frame, Rect { height: 1, ..at }, &pinned);
    }
}

/// A row with its line style across the full width. ratatui skips the style
/// of an empty `Line`, which left a user cell's tinted padding rows bare.
fn paint_line(frame: &mut Frame, at: Rect, line: &Line<'_>) {
    frame.buffer_mut().set_style(at, line.style);
    frame.render_widget(line, at);
}

/// §3.1 session header, while the viewed transcript has nothing to show.
fn render_header(frame: &mut Frame, area: Rect, app: &App, theme: &Theme) {
    let header = session_header(app);
    let lines =
        crate::ui::cells::header::display_lines_with_keys(&header, area.width, theme, &app.keys);
    let shown = area
        .height
        .min(u16::try_from(lines.len()).unwrap_or(u16::MAX));
    frame.render_widget(
        ratatui::widgets::Paragraph::new(lines),
        Rect {
            height: shown,
            ..area
        },
    );
}

/// §3.1 session header: version, cwd, and sign-in steps when nothing is configured.
fn session_header(app: &App) -> CellModel {
    CellModel::Header {
        version: env!("CARGO_PKG_VERSION").to_owned(),
        cwd: app
            .active_workspace()
            .map(|cwd| cwd.display().to_string())
            .unwrap_or_default(),
        needs_sign_in: !app.auth.is_empty() && !app.auth.iter().any(|a| a.configured),
        sign_in_details: app
            .auth
            .iter()
            .filter(|a| !a.configured)
            .filter_map(|a| {
                a.detail
                    .as_ref()
                    .map(|detail| format!("{}: {detail}", a.provider))
            })
            .collect(),
    }
}

impl TranscriptView {
    pub fn anchor(&self, agent: AgentId) -> Anchor {
        self.anchors.get(&agent).copied().unwrap_or_default()
    }
}

fn rows_for<'a>(
    app: &'a App,
    agent: AgentId,
    transcript: &'a AgentTranscript,
    width: u16,
    stream: &'a mut StreamCache,
    theme: &'a Theme,
) -> Rows<'a> {
    let policy = app.policy(Some(agent));
    stream.set_presentation((
        app.presentation_revision,
        policy.definition.is_some(),
        app.details,
    ));
    let projection = stream.visible.project(app, agent, transcript);
    let active = transcript
        .active
        .as_ref()
        .is_some_and(|c| policy.shows(&c.model));
    let stream = if policy.shows_category(crate::view_modes::Category::AssistantText) {
        stream::stream_lines(
            agent,
            &transcript.stream,
            width,
            theme,
            app.theme_epoch,
            stream,
        )
    } else {
        None
    };
    let is_root = app
        .tree
        .nodes
        .get(&agent)
        .is_some_and(|node| node.agent.parent.is_none());
    let header = is_root.then(|| {
        let model = session_header(app);
        use std::hash::{Hash, Hasher};
        let CellModel::Header {
            needs_sign_in,
            sign_in_details,
            ..
        } = &model
        else {
            unreachable!("session_header must return a header");
        };
        let mut revision = std::collections::hash_map::DefaultHasher::new();
        (needs_sign_in, sign_in_details).hash(&mut revision);
        Cell {
            id: window::HEADER_ID,
            revision: CellRevision(revision.finish() as u32),
            at: app.now,
            model,
        }
    });
    Rows {
        visible: projection.committed,
        pending: projection.pending,
        active,
        policy,
        presentation: app.presentation_revision,
        agent,
        header,
        transcript,
        stream,
        width,
        detail: app.details,
        theme,
        keys: &app.keys,
        theme_epoch: app.theme_epoch,
        tree: &app.tree,
        schedules: &app.schedules.jobs,
        names: app.names_version,
    }
}

/// Resolve every intent logged since the last frame against row heights.
fn apply_scroll(app: &App, area: Rect, cache: &mut ViewCache, theme: &Theme) {
    let view = usize::from(area.height);
    let pending: Vec<_> = app
        .scroll_log
        .since(cache.transcript.applied_scroll_seq)
        .copied()
        .collect();
    for request in pending {
        cache.transcript.applied_scroll_seq = request.seq;
        let Some(transcript) = app.transcripts.get(&request.agent) else {
            continue;
        };
        let rows = rows_for(
            app,
            request.agent,
            transcript,
            area.width,
            &mut cache.stream,
            theme,
        );
        let layout = &mut cache.layout;
        let here = rows.resolve(cache.transcript.anchor(request.agent), view, layout);
        let page = view.saturating_sub(1).max(1);
        let pos = match request.intent {
            ScrollIntent::LineUp(n) => rows.up(here, usize::from(n), layout),
            ScrollIntent::LineDown(n) => rows.down(here, usize::from(n), layout),
            ScrollIntent::PageUp => rows.up(here, page, layout),
            ScrollIntent::PageDown => rows.down(here, page, layout),
            ScrollIntent::Top => Pos { item: 0, row: 0 },
            ScrollIntent::Latest => rows.bottom_top(view, layout),
            // Below the rows a pinned prompt may cover.
            ScrollIntent::Reveal(cell) => match rows.find(cell) {
                Some(item) => {
                    let lines = rows.lines(item, layout);
                    let find = app
                        .bottom
                        .find
                        .as_ref()
                        .and_then(|bar| Some((bar.query.as_str(), bar.occurrence_in(cell)?)));
                    let hit = Pos {
                        item,
                        row: rows.gap(item, layout) + hit_row(&lines, find, view),
                    };
                    rows.up(hit, usize::from(PINNED_ROWS), layout)
                }
                None => here,
            },
        };
        let upward_at_top = pos.item == 0
            && pos.row == 0
            && matches!(
                request.intent,
                ScrollIntent::Top | ScrollIntent::PageUp | ScrollIntent::LineUp(_)
            );
        if upward_at_top
            && here.item == 0
            && here.row == 0
            && cache.transcript.upward_at_top.contains(&request.agent)
            && !cache.transcript.older_requests.contains(&request.agent)
        {
            cache.transcript.older_requests.push(request.agent);
        } else if pos.item != 0 || pos.row != 0 || request.intent == ScrollIntent::Latest {
            cache
                .transcript
                .older_requests
                .retain(|agent| *agent != request.agent);
        }
        cache
            .transcript
            .upward_at_top
            .retain(|agent| *agent != request.agent);
        if upward_at_top {
            cache.transcript.upward_at_top.push(request.agent);
        }
        let anchor = match request.intent {
            ScrollIntent::Latest => Anchor::Latest,
            _ => rows.anchor_at(pos, view, layout),
        };
        cache.transcript.anchors.insert(request.agent, anchor);
    }
}

/// Rows the pinned prompt covers: itself and a blank under it.
const PINNED_ROWS: u16 = 2;

/// Reveal the selected occurrence, allowing space for the pinned prompt.
fn hit_row(lines: &ItemLines<'_>, find: Option<(&str, usize)>, view: usize) -> usize {
    let Some((query, occurrence)) = find else {
        return 0;
    };
    let hit = selected_hit(lines, query, occurrence);
    match hit.first() {
        Some(&(row, _, _)) if row + usize::from(PINNED_ROWS) >= view => row,
        _ => 0,
    }
}

/// Map one source-text occurrence back to rows, including both halves of a soft wrap.
fn selected_hit(
    lines: &ItemLines<'_>,
    query: &str,
    occurrence: usize,
) -> Vec<(usize, usize, usize)> {
    let mut text = String::new();
    let mut ranges = Vec::with_capacity(lines.len());
    for row in 0..lines.len() {
        let Some((part, offset)) = row_text(lines, row) else {
            continue;
        };
        match lines.source(row).continues {
            Some(dropped) => text.extend(std::iter::repeat_n(' ', dropped)),
            None if !text.is_empty() => text.push('\n'),
            None => {}
        }
        let start = text.len();
        text.push_str(&part[offset..]);
        ranges.push((row, start, text.len(), offset));
    }
    let Some((start, end)) = occurrences(&text, query).get(occurrence).copied() else {
        return Vec::new();
    };
    ranges
        .into_iter()
        .filter(|(_, from, to, _)| start < *to && end > *from)
        .map(|(row, from, to, offset)| {
            (
                row,
                offset + start.max(from) - from,
                offset + end.min(to) - from,
            )
        })
        .collect()
}

/// Row `row`'s text, and the byte its part of the source line starts at.
fn row_text(lines: &ItemLines<'_>, row: usize) -> Option<(String, usize)> {
    Some(lines.source(row).text_of(lines.get(row)?))
}

/// Byte ranges of row `at`'s line inside a match of `query`. The row is
/// searched in its source line, joined over soft wraps as far as a match
/// overlapping it can reach, so each row costs its width plus the query's.
fn row_hits(lines: &ItemLines<'_>, at: usize, query: &str) -> Vec<(usize, usize)> {
    let Some((text, offset)) = row_text(lines, at) else {
        return Vec::new();
    };
    let reach = query.len().saturating_sub(1);
    let mut head = Vec::new();
    let (mut row, mut before) = (at, 0);
    while before < reach
        && let Some(dropped) = lines.source(row).continues
        && let Some(above) = row.checked_sub(1)
        && let Some((text, start)) = row_text(lines, above)
    {
        // No match overlapping row `at` crosses more than `reach` spaces, however many wrapped away.
        let dropped = dropped.min(reach);
        before += text.len() - start + dropped;
        head.push((text, start, dropped));
        row = above;
    }
    let mut joined = String::new();
    for (text, start, dropped) in head.iter().rev() {
        joined.push_str(&text[*start..]);
        joined.extend(std::iter::repeat_n(' ', *dropped));
    }
    let (from, to) = (joined.len(), joined.len() + text.len() - offset);
    joined.push_str(&text[offset..]);
    let mut row = at + 1;
    while joined.len() - to < reach
        && let Some(dropped) = lines.source(row).continues
        && let Some((text, start)) = row_text(lines, row)
    {
        joined.extend(std::iter::repeat_n(' ', dropped.min(reach)));
        joined.push_str(&text[start..]);
        row += 1;
    }
    occurrences(&joined, query)
        .into_iter()
        .filter(|&(a, b)| a < to && b > from)
        .map(|(a, b)| (offset + a.max(from) - from, offset + b.min(to) - from))
        .collect()
}

/// The prompt the visible rows answer, once its own first row scrolled away.
fn pinned_prompt(
    rows: &Rows<'_>,
    top: Pos,
    layout: &mut LayoutCache,
) -> Option<(Line<'static>, crate::ui::selection::TextPoint)> {
    let item = (0..=top.item.min(rows.len().saturating_sub(1)))
        .rev()
        .find(|&i| matches!(rows.cell(i).map(|c| &c.model), Some(CellModel::User { .. })))?;
    let cell = rows.cell(item)?;
    // Full user blocks start with padding; compact coordination cells start with text.
    let text_row = rows.gap(item, layout) + usize::from(!rows.policy.compact(&cell.model));
    if item == top.item && top.row <= text_row {
        return None;
    }
    Some((
        user::pinned_line(&cell.model, rows.width, rows.theme)?,
        crate::ui::selection::TextPoint {
            stream_generation: None,
            cell: rows.id(item),
            row: text_row,
            column: 0,
        },
    ))
}

/// Triangle wave over 1.6 s for the pulsing bullets.
fn pulse_phase(now: Timestamp) -> f32 {
    const PERIOD_MS: i64 = 1_600;
    let t = now.timestamp_millis().rem_euclid(PERIOD_MS) as f32 / PERIOD_MS as f32;
    1.0 - (2.0 * t - 1.0).abs()
}

/// Tint find hits on row `at` of `lines`; the caller marks the current one.
/// Costs the row, not the match count: `matches` can hold one entry per letter.
fn highlight_matches(
    line: &mut Line<'static>,
    (lines, at): (&ItemLines<'_>, usize),
    query: &str,
    theme: &Theme,
) {
    if query.is_empty() {
        return;
    }
    highlight_line(line, &row_hits(lines, at, query), theme.find_hit(false));
}

/// Patch `style` over the byte ranges `hits` of `line`.
fn highlight_line(line: &mut Line<'static>, hits: &[(usize, usize)], style: Style) {
    if hits.is_empty() {
        return;
    }
    let mut out = Vec::with_capacity(line.spans.len() + hits.len() * 2);
    let mut start = 0;
    for span in line.spans.drain(..) {
        let end = start + span.content.len();
        let mut cuts = vec![start, end];
        for &(a, b) in hits {
            for at in [a, b] {
                if at > start && at < end {
                    cuts.push(at);
                }
            }
        }
        cuts.sort_unstable();
        cuts.dedup();
        for pair in cuts.windows(2) {
            let (a, b) = (pair[0], pair[1]);
            let piece = span.content[a - start..b - start].to_owned();
            let hit = hits.iter().any(|&(ha, hb)| a >= ha && b <= hb);
            let piece_style = if hit {
                span.style.patch(style)
            } else {
                span.style
            };
            out.push(Span::styled(piece, piece_style));
        }
        start = end;
    }
    line.spans = out;
}

#[cfg(test)]
mod tests;

impl ViewCache {
    pub fn update_presentation(&mut self, app: &App) {
        let key = (app.presentation_revision, app.names_version, app.details);
        if self.presentation.is_some_and(|old| old != key) {
            self.mouse.clear_selection();
        }
        self.presentation = Some(key);
    }
}
