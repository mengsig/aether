//! Markdown for replies (SPEC §3.1): H1 bold+underline, H2 bold, H3 bold
//! italic, code spans and links in the accent, dim list markers and quote
//! bars, fenced code unboxed. The words stay plain text underneath, so
//! selecting and copying from the terminal still gives the reply itself.

use pulldown_cmark::{Event, HeadingLevel, Options, Parser, Tag, TagEnd};
use ratatui::style::Style;
use ratatui::text::{Line, Span};
use unicode_width::UnicodeWidthStr;

use crate::ui::cells::text::{Layout, RowSource, clean};
use crate::ui::theme::Theme;

/// Render `text` with body style `base`, wrapping at `width`. Terminal
/// controls in the text are dropped first, so a reply cannot drive the screen.
/// Oversized graphemes remain intact on their own row and may be clipped by the terminal.
pub fn render(text: &str, width: usize, base: Style, theme: &Theme) -> Vec<Line<'static>> {
    render_layout(text, width, base, theme).into_lines()
}

/// [`render`], with where each row sits in its source line.
pub fn render_layout(text: &str, width: usize, base: Style, theme: &Theme) -> Layout {
    layout_clean(&clean(text), width, base, theme)
}

/// [`render`] of text already passed through `clean`.
fn render_clean(text: &str, width: usize, base: Style, theme: &Theme) -> Vec<Line<'static>> {
    layout_clean(text, width, base, theme).into_lines()
}

fn layout_clean(text: &str, width: usize, base: Style, theme: &Theme) -> Layout {
    let mut r = Renderer::new(width, base, theme);
    for event in Parser::new_ext(text, options()) {
        r.event(event);
    }
    let mut out = r.into_layout();
    while out.lines().last().is_some_and(|l| l.spans.is_empty()) {
        out.pop();
    }
    out
}

/// Until code is highlighted, this indent alone sets a block apart from prose.
const CODE_INDENT: &str = "  ";

fn options() -> Options {
    let mut options = Options::empty();
    options.insert(Options::ENABLE_STRIKETHROUGH);
    options.insert(Options::ENABLE_TABLES);
    options.insert(Options::ENABLE_TASKLISTS);
    options
}

fn trim_blank_end(lines: &mut Vec<Line<'static>>) {
    while lines.last().is_some_and(|l| l.spans.is_empty()) {
        lines.pop();
    }
}

/// One styled run inside a paragraph. `"\n"` forces a line break.
#[derive(Debug, Clone)]
struct Run {
    text: String,
    style: Style,
}

#[derive(Debug)]
struct ListState {
    /// Next number for an ordered list; `None` for bullets.
    next: Option<u64>,
    /// Width of the marker, so continuation lines align under the text.
    marker_width: usize,
    /// The current item has not printed its marker yet.
    fresh: bool,
}

#[derive(Debug, Default)]
struct TableState {
    rows: Vec<Vec<String>>,
    row: Vec<String>,
    cell: String,
    header_rows: usize,
}

struct Renderer<'a> {
    width: usize,
    base: Style,
    theme: &'a Theme,
    out: Layout,
    runs: Vec<Run>,
    /// Inline style stack: emphasis, strong, links, code.
    styles: Vec<Style>,
    lists: Vec<ListState>,
    quote: usize,
    /// Text of the code block being collected.
    code: Option<String>,
    table: Option<TableState>,
    link: Option<String>,
}

impl<'a> Renderer<'a> {
    fn new(width: usize, base: Style, theme: &'a Theme) -> Renderer<'a> {
        Renderer {
            width: width.max(8),
            base,
            theme,
            out: Layout::default(),
            runs: Vec::new(),
            styles: Vec::new(),
            lists: Vec::new(),
            quote: 0,
            code: None,
            table: None,
            link: None,
        }
    }

    /// Every line rendered, trailing blank lines included.
    fn into_lines(self) -> Vec<Line<'static>> {
        self.into_layout().into_lines()
    }

    fn into_layout(mut self) -> Layout {
        self.flush_runs();
        self.out
    }

    /// Emit `prefix` then `body` as one row, `continues` set when a wrap split it from the row above.
    fn emit(
        &mut self,
        mut prefix: Vec<Span<'static>>,
        body: Vec<Span<'static>>,
        continues: Option<usize>,
    ) {
        let source = RowSource {
            prefix: prefix.iter().map(|s| s.content.width()).sum(),
            continues,
        };
        prefix.extend(body);
        self.out.push(Line::from(prefix), source);
    }

    fn event(&mut self, event: Event<'_>) {
        if let Some(buf) = self.code.as_mut() {
            match event {
                Event::Text(t) => buf.push_str(&t),
                Event::End(TagEnd::CodeBlock) => self.flush_code(),
                _ => {}
            }
            return;
        }
        if let Some(table) = self.table.as_mut() {
            match event {
                Event::Text(t) | Event::Code(t) => table.cell.push_str(&t),
                Event::SoftBreak | Event::HardBreak => table.cell.push(' '),
                Event::End(TagEnd::TableHead) => {
                    let row = std::mem::take(&mut table.row);
                    table.rows.push(row);
                    table.header_rows = table.rows.len();
                }
                Event::End(TagEnd::TableCell) => {
                    let cell = std::mem::take(&mut table.cell);
                    table.row.push(cell.trim().to_owned());
                }
                Event::End(TagEnd::TableRow) => {
                    let row = std::mem::take(&mut table.row);
                    table.rows.push(row);
                }
                Event::End(TagEnd::Table) => self.flush_table(),
                _ => {}
            }
            return;
        }
        match event {
            Event::Start(tag) => self.start(tag),
            Event::End(tag) => self.end(tag),
            Event::Text(t) => self.push_text(&t),
            Event::Code(t) => self.push_run(&t, self.current_style().patch(self.theme.code())),
            Event::InlineMath(t) | Event::DisplayMath(t) => {
                self.push_run(&t, self.current_style().patch(self.theme.code()))
            }
            Event::Html(t) | Event::InlineHtml(t) => self.push_text(&t),
            Event::FootnoteReference(name) => self.push_text(&format!("[{name}]")),
            Event::SoftBreak => self.push_text(" "),
            Event::HardBreak => self.runs.push(Run {
                text: "\n".into(),
                style: self.base,
            }),
            Event::Rule => {
                self.flush_runs();
                let prefix = self.prefix(false);
                let rule = Span::styled(
                    self.theme.glyphs().rule.repeat(self.width.min(40)),
                    self.theme.secondary(),
                );
                self.emit(prefix, vec![rule], None);
                self.blank();
            }
            Event::TaskListMarker(done) => self.push_run(
                if done {
                    self.theme.decoration("☑ ", "x ")
                } else {
                    self.theme.decoration("☐ ", "o ")
                },
                self.theme.secondary(),
            ),
        }
    }

    fn start(&mut self, tag: Tag<'_>) {
        match tag {
            Tag::Paragraph => {}
            Tag::Heading { level, .. } => {
                self.flush_runs();
                let t = self.theme;
                let style = match level {
                    HeadingLevel::H1 => t.heading().patch(t.underline()),
                    HeadingLevel::H2 => t.heading(),
                    _ => t.heading().patch(t.emphasis()),
                };
                self.push_style(style);
            }
            Tag::BlockQuote(_) => {
                self.flush_runs();
                self.quote += 1;
            }
            // The fence's language waits for syntax highlighting (SPEC §5.2).
            Tag::CodeBlock(_) => {
                self.flush_runs();
                self.code = Some(String::new());
            }
            Tag::List(start) => {
                self.flush_runs();
                let marker_width = match start {
                    Some(n) => n.max(1).ilog10() as usize + 3,
                    None => 2,
                };
                self.lists.push(ListState {
                    next: start,
                    marker_width,
                    fresh: false,
                });
            }
            Tag::Item => {
                self.flush_runs();
                if let Some(list) = self.lists.last_mut() {
                    list.fresh = true;
                    if let Some(n) = list.next {
                        list.marker_width = n.max(1).ilog10() as usize + 3;
                    }
                }
            }
            Tag::Table(_) => {
                self.flush_runs();
                self.table = Some(TableState::default());
            }
            Tag::Emphasis => self.push_style(self.theme.emphasis()),
            Tag::Strong => self.push_style(self.theme.strong()),
            Tag::Strikethrough => self.push_style(self.theme.strike()),
            Tag::Link { dest_url, .. } => {
                self.link = Some(dest_url.to_string());
                self.push_style(self.theme.link());
            }
            Tag::Image { dest_url, .. } => {
                self.push_run(&format!("[image {dest_url}]"), self.theme.secondary());
                self.push_style(self.theme.secondary());
            }
            Tag::TableHead
            | Tag::TableRow
            | Tag::TableCell
            | Tag::HtmlBlock
            | Tag::FootnoteDefinition(_)
            | Tag::DefinitionList
            | Tag::DefinitionListTitle
            | Tag::DefinitionListDefinition
            | Tag::Superscript
            | Tag::Subscript
            | Tag::MetadataBlock(_) => {}
        }
    }

    fn end(&mut self, tag: TagEnd) {
        match tag {
            TagEnd::Paragraph => {
                self.flush_runs();
                if self.lists.is_empty() {
                    self.blank();
                }
            }
            TagEnd::Heading(_) => {
                self.flush_runs();
                self.styles.pop();
                self.blank();
            }
            TagEnd::BlockQuote(_) => {
                self.flush_runs();
                self.quote = self.quote.saturating_sub(1);
                self.blank();
            }
            TagEnd::List(_) => {
                self.flush_runs();
                self.lists.pop();
                if self.lists.is_empty() {
                    self.blank();
                }
            }
            TagEnd::Item => self.flush_runs(),
            TagEnd::Emphasis | TagEnd::Strong | TagEnd::Strikethrough | TagEnd::Image => {
                self.styles.pop();
            }
            TagEnd::Link => {
                self.styles.pop();
                if let Some(url) = self.link.take() {
                    let shown = self.runs.last().map(|r| r.text.trim() == url.trim());
                    if shown != Some(true) {
                        self.push_run(&format!(" ({url})"), self.theme.secondary());
                    }
                }
            }
            TagEnd::CodeBlock | TagEnd::Table => {}
            _ => {}
        }
    }

    fn push_style(&mut self, style: Style) {
        let current = self.current_style();
        self.styles.push(current.patch(style));
    }

    fn current_style(&self) -> Style {
        self.styles.last().copied().unwrap_or(self.base)
    }

    fn push_text(&mut self, text: &str) {
        let style = self.current_style();
        self.push_run(text, style);
    }

    fn push_run(&mut self, text: &str, style: Style) {
        if text.is_empty() {
            return;
        }
        if let Some(last) = self.runs.last_mut()
            && last.style == style
            && last.text != "\n"
        {
            last.text.push_str(text);
            return;
        }
        self.runs.push(Run {
            text: text.to_owned(),
            style,
        });
    }

    /// Prefix spans for a line: quote bars, list indentation and, for the
    /// first line of a fresh item, its marker.
    fn prefix(&mut self, first: bool) -> Vec<Span<'static>> {
        let mut spans = Vec::new();
        if self.quote > 0 {
            spans.push(Span::styled(
                self.theme.decoration("▎ ", "| ").repeat(self.quote),
                self.theme.secondary(),
            ));
        }
        let depth = self.lists.len();
        if depth > 0 {
            let indent = self
                .lists
                .iter()
                .take(depth - 1)
                .map(|l| l.marker_width)
                .sum::<usize>();
            if indent > 0 {
                spans.push(Span::raw(" ".repeat(indent)));
            }
            let Some(list) = self.lists.last_mut() else {
                return spans;
            };
            if first && list.fresh {
                list.fresh = false;
                let marker = match list.next.as_mut() {
                    Some(n) => {
                        let m = format!("{n}. ");
                        *n += 1;
                        m
                    }
                    None => "- ".into(),
                };
                let pad = list.marker_width.saturating_sub(marker.width());
                spans.push(Span::styled(marker, self.theme.secondary()));
                if pad > 0 {
                    spans.push(Span::raw(" ".repeat(pad)));
                }
            } else {
                spans.push(Span::raw(" ".repeat(list.marker_width)));
            }
        }
        spans
    }

    fn prefix_width(&self) -> usize {
        self.quote * 2 + self.lists.iter().map(|l| l.marker_width).sum::<usize>()
    }

    /// Wrap the pending runs into lines and emit them.
    fn flush_runs(&mut self) {
        let runs = std::mem::take(&mut self.runs);
        if runs.iter().all(|r| r.text.trim().is_empty()) {
            return;
        }
        let inner = self.width.saturating_sub(self.prefix_width()).max(4);
        let lines = wrap_runs(&runs, inner);
        for (i, (spans, continues)) in lines.into_iter().enumerate() {
            let prefix = self.prefix(i == 0);
            self.emit(prefix, spans, continues);
        }
    }

    /// Unboxed, one line per code line, hard-split at the width, indented
    /// [`CODE_INDENT`] past the prose so a block parts from paragraphs.
    fn flush_code(&mut self) {
        let Some(text) = self.code.take() else {
            return;
        };
        let inner = self
            .width
            .saturating_sub(self.prefix_width() + CODE_INDENT.len())
            .max(4);
        for raw in text.trim_end_matches('\n').split('\n') {
            let raw = raw.replace('\t', "    ");
            for (i, piece) in split_width(&raw, inner).into_iter().enumerate() {
                let mut prefix = self.prefix(false);
                let mut body = Vec::new();
                if !piece.is_empty() {
                    prefix.push(Span::raw(CODE_INDENT));
                    body.push(Span::styled(piece, self.base.patch(self.theme.command())));
                }
                self.emit(prefix, body, (i > 0).then_some(0));
            }
        }
        self.blank();
    }

    /// Columns padded to their widest cell, long cells wrapped within their
    /// column, a bold header, a rule under it.
    fn flush_table(&mut self) {
        let Some(table) = self.table.take() else {
            return;
        };
        let columns = table.rows.iter().map(Vec::len).max().unwrap_or(0);
        if columns == 0 {
            return;
        }
        let inner = self.width.saturating_sub(self.prefix_width()).max(4);
        let mut widths = vec![0usize; columns];
        for row in &table.rows {
            for (i, cell) in row.iter().enumerate() {
                widths[i] = widths[i].max(display_width(cell));
            }
        }
        // Keep the table inside the pane: shrink the widest columns first.
        // Keep both content and separators inside the pane. A fixed two-cell
        // gap can consume the whole row in a wide table, so narrow it as the
        // number of columns grows; content columns may shrink to zero when
        // there are more columns than available cells.
        let gap = if columns > 1 {
            2.min(inner.saturating_sub(1) / (columns - 1))
        } else {
            0
        };
        let content_width = inner.saturating_sub(gap * (columns - 1));
        let floor = usize::from(content_width >= columns);
        while widths.iter().sum::<usize>() > content_width {
            let Some(widest) = widths.iter_mut().max() else {
                break;
            };
            if *widest <= floor {
                break;
            }
            *widest -= 1;
        }
        for (r, row) in table.rows.iter().enumerate() {
            let header = r < table.header_rows;
            let style = if header {
                self.base.patch(self.theme.strong())
            } else {
                self.base
            };
            let cells: Vec<Vec<String>> = widths
                .iter()
                .enumerate()
                .map(|(i, width)| wrap_text(row.get(i).map_or("", String::as_str), *width))
                .collect();
            let height = cells.iter().map(Vec::len).max().unwrap_or(1);
            for line in 0..height {
                let prefix = self.prefix(false);
                let mut spans = Vec::new();
                for (i, width) in widths.iter().enumerate() {
                    let text = cells[i].get(line).map_or("", String::as_str);
                    spans.push(Span::styled(fit(text, *width), style));
                    if i + 1 < columns {
                        spans.push(Span::raw(" ".repeat(gap)));
                    }
                }
                self.emit(prefix, spans, None);
            }
            if header && r + 1 == table.header_rows {
                let prefix = self.prefix(false);
                let total = (widths.iter().sum::<usize>() + gap * (columns - 1)).min(inner);
                let rule = Span::styled(
                    self.theme.glyphs().rule.repeat(total),
                    self.theme.secondary(),
                );
                self.emit(prefix, vec![rule], None);
            }
        }
        self.blank();
    }

    fn blank(&mut self) {
        if self.out.lines().last().is_some_and(|l| !l.spans.is_empty()) {
            self.out.push(Line::default(), RowSource::default());
        }
    }
}

mod stream;
pub use stream::{MarkdownStream, StreamLines};

mod wrapping;
use crate::ui::text_width::display_width;
use wrapping::{fit, split_width, wrap_runs, wrap_text};

#[cfg(test)]
mod tests;
