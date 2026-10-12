//! The composer block (SPEC §3.2): tinted, `›` prompt, word wrap cached per
//! width, kill ring, paste placeholder. Owner: W4.

use std::ops::Range;

use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::text::{Line, Span};
use ratatui::widgets::Paragraph;

use aether_core::agent::AgentStatusKind;
use aether_core::attachment::ImageAttachment;
use unicode_width::UnicodeWidthStr;

use crate::app::App;
use crate::input_layout::{InputLayout, InputRow, cursor_position};
use crate::ui::layout::{COMPOSER_MAX_INPUT_ROWS, COMPOSER_PADDING_ROWS};
use crate::ui::theme::tokens::Theme;

/// A paste over this many chars becomes `[Pasted 2,652 chars]`.
pub const PASTE_PLACEHOLDER_CHARS: usize = 1_000;

/// Columns before the text (`› `) and kept free after it.
const PROMPT_COLS: u16 = 2;
const RIGHT_MARGIN_COLS: u16 = 2;

pub use crate::app::PastedText;

/// One Ctrl+K/U/W kill, with the pastes whose placeholders its text holds.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Killed {
    pub text: String,
    pub pastes: Vec<PastedText>,
}

/// Composer state beyond the draft text (`App::input`). Survives submit.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct ComposerState {
    /// Ctrl+K/U/W kills, newest last; Ctrl+Y yanks the newest.
    pub kill_ring: Vec<Killed>,
}

/// Per-width wrap cache, owned by the runner via `ViewCache`.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct ComposerCache {
    /// `(draft revision, text width, ascii)` the cached wrap belongs to.
    pub key: Option<(u64, u16, bool)>,
    /// Visual rows of the draft, each with its byte range.
    pub(crate) rows: Vec<InputRow>,
    /// First input row shown once the draft outgrows the block.
    pub scroll: usize,
}

impl ComposerCache {
    fn rows(&mut self, app: &App, width: u16) -> &[InputRow] {
        let text_width = text_width(width);
        let key = (app.input.revision, text_width, app.theme.ascii());
        if self.key != Some(key) {
            // +1: InputLayout keeps one cell for the cursor at a full row's end.
            let layout = InputLayout::with_ascii(
                &app.input.text,
                0,
                usize::from(text_width) + 1,
                app.theme.ascii(),
            );
            self.rows = layout.rows;
            self.key = Some(key);
        }
        &self.rows
    }
}

/// Where ↑ (`-1`) or ↓ (`1`) puts the cursor among the draft's painted rows;
/// `None` at the first or last row, where the arrows walk history instead.
pub fn vertical_target(app: &App, direction: isize) -> Option<usize> {
    let width = crate::ui::bottom_width(app.size.0, app.board_mode());
    let layout = InputLayout::with_ascii(
        &app.input.text,
        app.input.cursor,
        usize::from(text_width(width)) + 1,
        app.theme.ascii(),
    );
    layout.move_vertical(direction)
}

fn text_width(width: u16) -> u16 {
    width.saturating_sub(PROMPT_COLS + RIGHT_MARGIN_COLS).max(1)
}

/// Block height at `width`: padding rows, the chips row when images are
/// attached, plus 1..=8 input rows.
pub fn desired_height(app: &App, width: u16, cache: &mut ComposerCache) -> u16 {
    let rows = u16::try_from(cache.rows(app, width).len()).unwrap_or(u16::MAX);
    let chips = u16::from(!app.input.images.is_empty());
    rows.clamp(1, COMPOSER_MAX_INPUT_ROWS) + COMPOSER_PADDING_ROWS + chips
}

/// How a block `height` rows tall is shared out, top to bottom: padding,
/// chips, draft, padding.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct BlockRows {
    /// Rows of padding on each side.
    pub pad: u16,
    pub chips: u16,
    pub draft: u16,
}

pub(crate) fn block_rows(app: &App, height: u16) -> BlockRows {
    let pad = u16::from(height > COMPOSER_PADDING_ROWS);
    let inner = height - pad * 2;
    // The draft keeps a row before the chips get one.
    let chips = u16::from(!app.input.images.is_empty() && inner > 1);
    let rows = BlockRows {
        pad,
        chips,
        draft: inner - chips,
    };
    debug_assert_eq!(rows.pad * 2 + rows.chips + rows.draft, height);
    rows
}

/// The attached images on one row: as many chips as fit, then how many more.
pub(crate) fn chips_text(images: &[ImageAttachment], width: usize) -> String {
    let mut text = String::new();
    for (shown, image) in images.iter().enumerate() {
        let chip = crate::app::image_label(image);
        let rest = images.len() - shown - 1;
        let more = if rest == 0 {
            String::new()
        } else {
            format!("  +{rest} more")
        };
        let gap = if shown == 0 { "" } else { "  " };
        // The first chip always shows, cut by the row if it must be.
        if shown > 0 && text.width() + gap.width() + chip.width() + more.width() > width {
            return format!("{text}  +{} more", rest + 1);
        }
        text.push_str(gap);
        text.push_str(&chip);
    }
    text
}

fn chips_line(app: &App, width: u16, theme: &Theme) -> Line<'static> {
    let room = usize::from(text_width(width));
    Line::from(vec![
        Span::raw(" ".repeat(usize::from(PROMPT_COLS))),
        Span::styled(chips_text(&app.input.images, room), theme.secondary()),
    ])
}

/// The bottom pane has focus: the board is not focused and no view is open.
pub fn focused(app: &App) -> bool {
    !app.board.focused() && app.bottom.top_view().is_none()
}

/// Paints the block and returns the terminal cursor position, if focused.
pub fn render(
    frame: &mut Frame,
    area: Rect,
    app: &App,
    cache: &mut ComposerCache,
    theme: &Theme,
) -> Option<(u16, u16)> {
    if area.is_empty() {
        return None;
    }
    let focused = focused(app);
    let BlockRows { pad, chips, draft } = block_rows(app, area.height);
    let visible = usize::from(draft);
    let prompt_style = if focused {
        theme.title()
    } else {
        theme.secondary()
    };
    let prompt = Span::styled(format!("{} ", theme.glyphs().prompt), prompt_style);
    let mut lines = vec![Line::default(); usize::from(pad)];
    if chips > 0 {
        lines.push(chips_line(app, area.width, theme));
    }

    let cursor = if app.input.text.is_empty() {
        let hint = placeholder(app).unwrap_or_default();
        lines.push(Line::from(vec![
            prompt,
            Span::styled(hint, theme.secondary()),
        ]));
        (0, 0)
    } else {
        let pastes = paste_ranges(app);
        let rows = cache.rows(app, area.width);
        let (row, column) = cursor_position(rows, app.input.cursor);
        cache.scroll = scroll_to(cache.scroll, row, visible, cache.rows.len());
        let indent = Span::raw(" ".repeat(usize::from(PROMPT_COLS)));
        for (i, input_row) in cache
            .rows
            .iter()
            .enumerate()
            .skip(cache.scroll)
            .take(visible)
        {
            let lead = if i == 0 {
                prompt.clone()
            } else {
                indent.clone()
            };
            lines.push(row_line(lead, input_row, &pastes, theme));
        }
        (row - cache.scroll, column)
    };

    let tint = theme.tint();
    frame.render_widget(Paragraph::new(lines).style(tint), area);
    if !focused {
        return None;
    }
    let (row, column) = cursor;
    let x = area.x + PROMPT_COLS + u16::try_from(column).unwrap_or(u16::MAX);
    let y = area.y + pad + chips + u16::try_from(row).unwrap_or(u16::MAX);
    (x < area.right() && y < area.bottom()).then_some((x, y))
}

/// Keep the cursor row inside a `visible`-row window starting near `scroll`.
fn scroll_to(scroll: usize, cursor_row: usize, visible: usize, rows: usize) -> usize {
    let visible = visible.max(1);
    let max = rows.saturating_sub(visible);
    let scroll = scroll.min(max);
    if cursor_row < scroll {
        cursor_row
    } else if cursor_row >= scroll + visible {
        cursor_row + 1 - visible
    } else {
        scroll
    }
}

/// One input row as spans: draft text, with paste placeholders in the accent.
fn row_line(
    lead: Span<'static>,
    row: &InputRow,
    pastes: &[Range<usize>],
    theme: &Theme,
) -> Line<'static> {
    let mut spans = vec![lead];
    let mut run = String::new();
    let mut run_style = theme.text();
    for (offset, piece) in row.pieces() {
        let style = if pastes.iter().any(|r| r.contains(&offset)) {
            theme.accent()
        } else {
            theme.text()
        };
        if style != run_style && !run.is_empty() {
            spans.push(Span::styled(std::mem::take(&mut run), run_style));
        }
        run_style = style;
        run.push_str(piece);
    }
    if !run.is_empty() {
        spans.push(Span::styled(run, run_style));
    }
    Line::from(spans)
}

/// Byte ranges of every paste placeholder still present in the draft.
fn paste_ranges(app: &App) -> Vec<Range<usize>> {
    paste_spans(&app.input.text, &app.input.pastes)
        .into_iter()
        .map(|(range, _)| range)
        .collect()
}

/// Intact tracked placeholders in text order.
pub fn paste_spans<'a>(
    text: &str,
    pastes: &'a [PastedText],
) -> Vec<(Range<usize>, &'a PastedText)> {
    let mut spans: Vec<_> = pastes
        .iter()
        .filter(|paste| text.get(paste.range.clone()) == Some(paste.placeholder.as_str()))
        .map(|paste| (paste.range.clone(), paste))
        .collect();
    spans.sort_by_key(|(range, _)| range.start);
    spans
}

/// `Message <name>` / `Steer <name>` / `Tell <name> what to do instead`; none while a view has focus.
pub fn placeholder(app: &App) -> Option<String> {
    if app.bottom.top_view().is_some() {
        return None;
    }
    let Some(node) = app.selected.and_then(|id| app.tree.nodes.get(&id)) else {
        return Some("Message a new agent".into());
    };
    let name = app.agent_display_name(node.agent.id);
    if app.bottom.approval.deny_target == app.selected {
        return Some(format!("Tell {name} what to do instead"));
    }
    if node.agent.status.kind() == AgentStatusKind::Running {
        return Some(format!("Steer {name}"));
    }
    Some(format!("Message {name}"))
}

/// `[Pasted 2,652 chars]`, with `#2`, `#3` … when that text is already taken.
pub fn paste_placeholder(chars: usize, taken: &[PastedText], draft: &str) -> String {
    let base = format!("[Pasted {} chars", group_thousands(chars));
    for n in 1.. {
        let placeholder = match n {
            1 => format!("{base}]"),
            n => format!("{base} #{n}]"),
        };
        if !draft.contains(&placeholder) && taken.iter().all(|t| t.placeholder != placeholder) {
            return placeholder;
        }
    }
    unreachable!("finite draft cannot occupy every numbered placeholder")
}

fn group_thousands(n: usize) -> String {
    let digits = n.to_string();
    let mut out = String::with_capacity(digits.len() + digits.len() / 3);
    for (i, c) in digits.chars().enumerate() {
        if i > 0 && (digits.len() - i).is_multiple_of(3) {
            out.push(',');
        }
        out.push(c);
    }
    out
}

#[cfg(test)]
mod tests;
