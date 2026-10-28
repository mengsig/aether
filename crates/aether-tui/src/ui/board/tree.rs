//! Agent tree: `Agents` header, two rows per agent, dim guides, glyphs,
//! needs-you marker, fold. Owner: W3.

use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::text::{Line, Span};
use unicode_width::UnicodeWidthStr;

use aether_core::id::AgentId;
use aether_core::time::Timestamp;

use super::{
    BoardScroll, elapsed_text, fit_spans, left_room, split_line, status_glyph, status_style,
    truncate,
};
use crate::app::board::{BoardModel, BoardRow, BoardStatus, Guide};
use crate::ui::theme::tokens::{Theme, ThemeMode};

/// What the tree paints besides the model: cursor (only while focused), viewed agent, clock.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TreeContext {
    pub cursor: Option<AgentId>,
    pub viewed: Option<AgentId>,
    pub now: Timestamp,
    /// `BoardState::revision`; with the clock's second it keys the label cache.
    pub revision: u64,
}

/// Header and the blank row under it.
const HEADER_ROWS: u16 = 2;

/// Elapsed and cost text per row, rebuilt at most once a second or when the
/// model changes; every frame in between (the pulse) reuses it.
#[derive(Debug, Clone, PartialEq, Default)]
pub(super) struct Labels {
    key: Option<LabelKey>,
    elapsed: Vec<String>,
    cost: Vec<String>,
    cost_width: usize,
    summary: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct LabelKey {
    revision: u64,
    second: i64,
    ascii: bool,
}

impl Labels {
    fn refresh(&mut self, model: &BoardModel, ctx: TreeContext, theme: &Theme) {
        let key = LabelKey {
            revision: ctx.revision,
            second: ctx.now.timestamp(),
            ascii: theme.ascii(),
        };
        if self.key == Some(key) && self.elapsed.len() == model.rows.len() {
            return;
        }
        self.key = Some(key);
        self.elapsed = model
            .rows
            .iter()
            .map(|r| elapsed_text(r.status, r.elapsed_ms(ctx.now)))
            .collect();
        self.cost = model
            .rows
            .iter()
            .map(|r| {
                let cost = cost_text(r.cost);
                if r.folded && !cost.is_empty() {
                    format!("{cost} total")
                } else {
                    cost
                }
            })
            .collect();
        self.cost_width = self.cost.iter().map(|c| c.width()).max().unwrap_or(0);
        self.summary = summary(model, theme);
    }
}

/// Cost provenance uses the same vocabulary as the inspector.
fn cost_text(cost: Option<crate::app::transcript::Cost>) -> String {
    let Some(cost) = cost else {
        return String::new();
    };
    debug_assert!(cost.usd.is_finite());
    debug_assert!(cost.usd >= 0.0);
    let prefix = if cost.estimated { "~" } else { "" };
    let basis = cost
        .basis
        .label()
        .map(|label| format!(" {label}"))
        .unwrap_or_default();
    format!("{prefix}${:.2}{basis}", cost.usd)
}

/// `5 agents · 2 running · $1.84`; the running count only when something
/// runs, the total only when something was priced.
fn summary(model: &BoardModel, theme: &Theme) -> String {
    let dot = theme.glyphs().dot;
    let s = &model.summary;
    let noun = if s.agents == 1 { "agent" } else { "agents" };
    let mut parts = vec![format!("{} {noun}", s.agents)];
    if s.running > 0 {
        parts.push(format!("{} running", s.running));
    }
    if s.total_cost.is_some() {
        parts.push(cost_text(s.total_cost));
    }
    parts.join(&format!(" {dot} "))
}

/// One painted row of the tree body.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum TreeLine {
    /// Blank row between roots.
    Gap,
    Title(usize),
    Subtitle(usize),
}

fn tree_lines(model: &BoardModel) -> Vec<TreeLine> {
    let mut lines = Vec::with_capacity(model.rows.len() * 2);
    for (i, row) in model.rows.iter().enumerate() {
        if i > 0 && row.depth == 0 {
            lines.push(TreeLine::Gap);
        }
        lines.push(TreeLine::Title(i));
        lines.push(TreeLine::Subtitle(i));
    }
    lines
}

pub fn agent_at(model: &BoardModel, offset: usize, row: usize) -> Option<AgentId> {
    match tree_lines(model).get(offset.saturating_add(row))? {
        TreeLine::Gap => None,
        TreeLine::Title(i) | TreeLine::Subtitle(i) => Some(model.rows[*i].id),
    }
}

pub fn render(
    frame: &mut Frame,
    area: Rect,
    model: &BoardModel,
    ctx: TreeContext,
    scroll: &mut BoardScroll,
    theme: &Theme,
) {
    if area.height == 0 || area.width == 0 {
        return;
    }
    scroll.labels.refresh(model, ctx, theme);
    let width = usize::from(area.width);
    let labels = &scroll.labels;
    let header = split_line(
        vec![Span::styled("Agents", theme.title())],
        fit_spans(
            vec![Span::styled(labels.summary.clone(), theme.secondary())],
            width.saturating_sub("Agents".width() + 1),
            theme,
        ),
        width,
        theme,
    );
    let buf = frame.buffer_mut();
    buf.set_line(area.x, area.y, &header, area.width);
    if area.height <= HEADER_ROWS {
        return;
    }
    if model.rows.is_empty() {
        let empty = Span::styled("No agents yet", theme.secondary());
        buf.set_span(area.x, area.y + HEADER_ROWS, &empty, area.width);
        return;
    }

    let lines = tree_lines(model);
    let body = usize::from(area.height - HEADER_ROWS);
    let anchor = ctx.cursor.or(ctx.viewed).and_then(|id| model.index_of(id));
    if ctx.cursor.or(ctx.viewed) != scroll.last_anchor {
        scroll.manual_scroll = false;
    }
    scroll.last_anchor = ctx.cursor.or(ctx.viewed);
    if scroll.wheel_delta != 0 {
        scroll.manual_scroll = true;
    }
    if !scroll.manual_scroll {
        scroll.tree_offset = scrolled(scroll.tree_offset, &lines, anchor, body);
    } else {
        scroll.tree_offset = scroll
            .tree_offset
            .saturating_add_signed(scroll.wheel_delta)
            .min(lines.len().saturating_sub(body));
        scroll.wheel_delta = 0;
    }
    scroll.tree_area = Rect {
        y: area.y + HEADER_ROWS,
        height: area.height - HEADER_ROWS,
        ..area
    };
    let labels = &scroll.labels;
    let mut y = area.y + HEADER_ROWS;
    for line in lines.iter().skip(scroll.tree_offset).take(body) {
        let (index, painted) = match *line {
            TreeLine::Gap => {
                y += 1;
                continue;
            }
            TreeLine::Title(i) => (i, title_line(model, i, labels, ctx, width, theme)),
            TreeLine::Subtitle(i) => (i, subtitle_line(&model.rows[i], width, theme)),
        };
        if ctx.cursor == Some(model.rows[index].id) {
            if theme.spec().mode == ThemeMode::Mono {
                if matches!(line, TreeLine::Title(_)) {
                    buf.set_stringn(
                        area.x.saturating_sub(1),
                        y,
                        theme.glyphs().prompt,
                        1,
                        theme.title(),
                    );
                }
            } else {
                buf.set_style(cursor_band(area, y, buf.area), theme.selected());
            }
        }
        buf.set_line(area.x, y, &painted, area.width);
        y += 1;
    }
}

/// The cursor tint bleeds one column into the board padding on each side.
fn cursor_band(area: Rect, y: u16, screen: Rect) -> Rect {
    let x = area.x.saturating_sub(1).max(screen.x);
    let right = area.right().saturating_add(1).min(screen.right());
    Rect {
        x,
        y,
        width: right.saturating_sub(x),
        height: 1,
    }
}

/// Keep the anchor's two rows in view, moving as little as possible.
fn scrolled(offset: usize, lines: &[TreeLine], anchor: Option<usize>, body: usize) -> usize {
    let max = lines.len().saturating_sub(body);
    let mut offset = offset.min(max);
    let Some(top) = anchor.and_then(|i| lines.iter().position(|l| *l == TreeLine::Title(i))) else {
        return offset;
    };
    let bottom = (top + 2).min(lines.len());
    if top < offset {
        offset = top;
    } else if bottom > offset + body {
        offset = bottom.saturating_sub(body).min(top);
    }
    offset.min(max)
}

/// `├ ● name  role            2m 40s  $0.38`
fn title_line(
    model: &BoardModel,
    index: usize,
    labels: &Labels,
    ctx: TreeContext,
    width: usize,
    theme: &Theme,
) -> Line<'static> {
    let row = &model.rows[index];
    // Running and needs-you agents carry their hue in the name too.
    let hue = match row.status {
        BoardStatus::Running | BoardStatus::NeedsYou => status_style(row.status, theme),
        _ => theme.text(),
    };
    let name_style = if ctx.viewed == Some(row.id) {
        hue.patch(theme.title())
    } else {
        hue
    };
    let right = right_columns(
        &labels.elapsed[index],
        &labels.cost[index],
        labels.cost_width,
        theme,
    );
    let mut left = vec![
        Span::styled(guides(row.guides.iter().copied(), theme), theme.rule()),
        status_glyph(row.status, theme, ctx.now),
        Span::raw(" "),
    ];
    // The role goes first, then the name's end; its start is what reads.
    let lead: usize = left.iter().map(|s| s.content.width()).sum();
    let name = truncate(
        &row.name,
        left_room(&right, width).saturating_sub(lead),
        theme,
    );
    left.push(Span::styled(name, name_style));
    if !row.role.is_empty() && row.group.is_none() {
        left.push(Span::raw("  "));
        left.push(Span::styled(row.role.clone(), theme.secondary()));
    }
    split_line(left, right, width, theme)
}

/// `2m 40s  $0.38`, the cost right-aligned in a column `cost_width` wide; a
/// row without a price leaves that column blank, and an empty row is empty.
pub(super) fn right_columns(
    elapsed: &str,
    cost: &str,
    cost_width: usize,
    theme: &Theme,
) -> Vec<Span<'static>> {
    if elapsed.is_empty() && cost.is_empty() {
        return Vec::new();
    }
    let mut spans = vec![Span::styled(elapsed.to_owned(), theme.secondary())];
    if cost_width > 0 {
        let pad = cost_width.saturating_sub(cost.width());
        spans.push(Span::raw(" ".repeat(2 + pad)));
        spans.push(Span::styled(cost.to_owned(), theme.text()));
    }
    spans
}

/// `│   sonnet-5.5 · medium · aether              needs you`
fn subtitle_line(row: &BoardRow, width: usize, theme: &Theme) -> Line<'static> {
    let dot = theme.glyphs().dot;
    let own = if row.has_children && !row.folded {
        Guide::Pipe
    } else {
        Guide::Blank
    };
    let columns = row.guides.iter().map(|g| g.continued()).chain([own]);
    let mut detail = match &row.group {
        Some(pipeline) => {
            let marker = theme.glyphs().ellipsis.chars().next().unwrap_or('~');
            let id = crate::format::short_id_with_marker(&row.id, marker);
            format!("{pipeline} {dot} {id}")
        }
        None => row.model.clone(),
    };
    if let (Some(effort), None) = (row.effort, &row.group) {
        detail.push_str(&format!(" {dot} {effort}"));
    }
    // Before the fold count and objective, so a narrow row cuts those first.
    if let Some(workspace) = &row.workspace {
        detail.push_str(&format!(" {dot} {workspace}"));
    }
    if row.folded_agents > 0 {
        detail.push_str(&format!(" {dot} {} folded", row.folded_agents));
    }
    if let Some(objective) = &row.root_objective {
        detail.push_str(&format!(" {dot} {objective}"));
    }
    let left = vec![
        Span::styled(guides(columns, theme), theme.rule()),
        Span::styled(detail, theme.secondary()),
    ];
    let right = match row.status {
        _ if row.needs_you || row.folded_needs_you => {
            vec![Span::styled("needs you", theme.attention())]
        }
        BoardStatus::Blocked { quota: true } => {
            vec![Span::styled(
                format!("blocked {dot} quota"),
                theme.failure(),
            )]
        }
        BoardStatus::Blocked { quota: false } => {
            vec![Span::styled("blocked", theme.blocked())]
        }
        _ => Vec::new(),
    };
    if right.is_empty() {
        return Line::from(fit_spans(left, width, theme));
    }
    split_line(left, right, width, theme)
}

/// Two columns per guide, so children hang under their parent's glyph.
fn guides(columns: impl Iterator<Item = Guide>, theme: &Theme) -> String {
    let g = theme.glyphs();
    columns
        .map(|guide| {
            let glyph = match guide {
                Guide::Pipe => g.pipe,
                Guide::Blank => "",
                Guide::Tee => g.tee,
                Guide::Corner => g.child,
            };
            format!("{glyph:<2}")
        })
        .collect()
}

/// Rows the tree wants (header + 2 per agent + root gaps).
pub fn desired_height(model: &BoardModel) -> u16 {
    let body = if model.rows.is_empty() {
        1
    } else {
        tree_lines(model).len()
    };
    u16::try_from(body)
        .unwrap_or(u16::MAX)
        .saturating_add(HEADER_ROWS)
}
