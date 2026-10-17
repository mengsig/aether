//! The `?` shortcuts panel above the composer: `commands::KEYS` under its
//! group headings, in up to three columns. Owner: W4.

use std::borrow::Cow;

use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::text::{Line, Span};
use ratatui::widgets::{Clear, Paragraph};
use unicode_width::UnicodeWidthStr;

use crate::commands::{HostCommands, KEYS, KeyGroup};
use crate::keys::KeyBindings;
use crate::ui::bottom::popup::{area_above, fit_words};
use crate::ui::theme::tokens::Theme;

const INDENT: usize = 2;
/// Blank columns between two panel columns.
const COLUMN_GAP: usize = 2;
/// Blank columns between a group's key column and its labels.
const KEY_GAP: usize = 2;

/// One heading and the keys under it.
struct Block {
    #[cfg(test)]
    group: KeyGroup,
    heading: String,
    /// `(chord, label)` as painted.
    rows: Vec<(Cow<'static, str>, Cow<'static, str>)>,
    /// The group's longest chord plus [`KEY_GAP`].
    key_width: usize,
    /// Widest of the heading and every `key_width + label`.
    width: usize,
}

fn blocks(ascii: bool, keys: &KeyBindings) -> Vec<Block> {
    KeyGroup::ALL
        .into_iter()
        .filter_map(|group| {
            let rows: Vec<_> = KEYS
                .iter()
                .filter(|spec| spec.group == group)
                .map(|spec| (spec.live_chord(keys, ascii), spec.live_label(keys, ascii)))
                .collect();
            let heading =
                if group == KeyGroup::Board && keys.customised(crate::keys::KeyAction::Agents) {
                    format!(
                        "Agents board ({})",
                        keys.display(crate::keys::KeyAction::Agents, ascii)
                    )
                } else {
                    group.heading().to_owned()
                };
            let key_width = rows.iter().map(|(chord, _)| chord.width()).max()? + KEY_GAP;
            let width = rows
                .iter()
                .map(|(_, label)| key_width + label.width())
                .chain([heading.width()])
                .max()
                .unwrap_or(key_width);
            Some(Block {
                #[cfg(test)]
                group,
                heading,
                rows,
                key_width,
                width,
            })
        })
        .collect()
}

#[derive(Default)]
struct Column {
    /// Indices into the blocks, top to bottom.
    blocks: Vec<usize>,
    height: usize,
    width: usize,
}

/// Masonry: each block goes to the shortest column so far, so no group is
/// split and the columns end up close in height.
fn pack(blocks: &[Block], count: usize) -> Vec<Column> {
    let mut columns: Vec<Column> = (0..count).map(|_| Column::default()).collect();
    for (index, block) in blocks.iter().enumerate() {
        let Some(column) = columns.iter_mut().min_by_key(|c| c.height) else {
            break;
        };
        let separator = usize::from(!column.blocks.is_empty());
        column.height += separator + 1 + block.rows.len();
        column.width = column.width.max(block.width);
        column.blocks.push(index);
    }
    columns.retain(|c| !c.blocks.is_empty());
    columns
}

/// 3 columns from 120, 2 from 80, else 1; fewer if a row would not fit whole.
fn layout(blocks: &[Block], width: u16) -> Vec<Column> {
    let target = match width {
        120.. => 3,
        80.. => 2,
        _ => 1,
    };
    let width = usize::from(width);
    (2..=target)
        .rev()
        .map(|count| pack(blocks, count))
        .find(|columns| {
            let used: usize = columns.iter().map(|c| c.width).sum();
            INDENT + used + COLUMN_GAP * columns.len().saturating_sub(1) <= width
        })
        .unwrap_or_else(|| pack(blocks, 1))
}

/// A column's cells top to bottom, each padded to `width`. Labels and
/// headings are cut only when one column is wider than the screen.
fn cells(
    blocks: &[Block],
    column: &Column,
    width: usize,
    theme: &Theme,
) -> Vec<Vec<Span<'static>>> {
    let ellipsis = theme.glyphs().ellipsis;
    let mut cells = Vec::new();
    for &index in &column.blocks {
        let block = &blocks[index];
        if !cells.is_empty() {
            cells.push(vec![Span::raw(" ".repeat(width))]);
        }
        let heading = fit_words(&block.heading, width, ellipsis);
        let pad = width.saturating_sub(heading.width());
        cells.push(vec![
            Span::styled(heading, theme.title()),
            Span::raw(" ".repeat(pad)),
        ]);
        for (chord, label) in &block.rows {
            let key = format!("{chord}{}", " ".repeat(block.key_width - chord.width()));
            let label = fit_words(label, width.saturating_sub(block.key_width), ellipsis);
            let pad = width.saturating_sub(block.key_width + label.width());
            cells.push(vec![
                Span::styled(key, theme.text()),
                Span::styled(label, theme.secondary()),
                Span::raw(" ".repeat(pad)),
            ]);
        }
    }
    cells
}

/// Title, the columns side by side, and a closing blank row.
fn lines(width: u16, theme: &Theme, keys: &KeyBindings) -> Vec<Line<'static>> {
    let blocks = blocks(theme.ascii(), keys);
    let columns = layout(&blocks, width);
    let room = usize::from(width).saturating_sub(INDENT);
    let column_cells: Vec<_> = columns
        .iter()
        .map(|column| cells(&blocks, column, column.width.min(room), theme))
        .collect();
    let height = column_cells.iter().map(Vec::len).max().unwrap_or(0);
    let indent = " ".repeat(INDENT);
    let mut lines = vec![
        Line::default(),
        Line::from(vec![
            Span::raw(indent.clone()),
            Span::styled("Keyboard shortcuts", theme.title()),
        ]),
        Line::default(),
    ];
    for row in 0..height {
        let mut spans = vec![Span::raw(indent.clone())];
        for (index, (column, cells)) in columns.iter().zip(&column_cells).enumerate() {
            if index > 0 {
                spans.push(Span::raw(" ".repeat(COLUMN_GAP)));
            }
            match cells.get(row) {
                Some(cell) => spans.extend(cell.iter().cloned()),
                None => spans.push(Span::raw(" ".repeat(column.width))),
            }
        }
        lines.push(Line::from(spans));
    }
    for hint in [
        "Drag: select & copy within one pane",
        "Shift+drag: terminal selection",
        "/mouse off: disable capture",
    ] {
        lines.push(Line::styled(format!("{indent}{hint}"), theme.secondary()));
    }
    lines.push(Line::default());
    lines.push(Line::from(vec![
        Span::raw(" ".repeat(INDENT)),
        Span::styled(
            format!(
                "{} or esc to close",
                keys.display(crate::keys::KeyAction::Shortcuts, theme.ascii())
            ),
            theme.secondary(),
        ),
    ]));
    lines
}

fn lines_with_modes(
    width: u16,
    theme: &Theme,
    keys: &KeyBindings,
    catalog: Option<&crate::view_modes::Catalog>,
    host: &HostCommands,
) -> Vec<Line<'static>> {
    let mut lines = lines(width, theme, keys);
    if !host.commands.is_empty() {
        let mut rows = vec![
            Line::default(),
            Line::styled(format!("  {}", host.heading()), theme.title()),
        ];
        rows.extend(host.help_rows().map(|(usage, about)| {
            Line::styled(
                fit_words(
                    &format!("  {usage}  {about}"),
                    usize::from(width),
                    theme.glyphs().ellipsis,
                ),
                theme.text(),
            )
        }));
        lines.splice(lines.len() - FOOT_ROWS..lines.len() - FOOT_ROWS, rows);
    }
    if let Some(catalog) = catalog {
        let mut modes = vec![Line::default(), Line::styled("  View modes", theme.title())];
        modes.extend(catalog.ordered().map(|mode| {
            let text = format!(
                "  /{}  {}",
                mode.name,
                mode.definition
                    .description
                    .as_deref()
                    .unwrap_or("view mode")
            );
            Line::styled(
                fit_words(&text, usize::from(width), theme.glyphs().ellipsis),
                theme.text(),
            )
        }));
        lines.splice(lines.len() - FOOT_ROWS..lines.len() - FOOT_ROWS, modes);
    }
    lines
}

/// Rows of [`lines`] above the keys (blank, title, blank) and below them (blank, close).
const HEAD_ROWS: usize = 3;
const FOOT_ROWS: usize = 2;

/// How a panel of `total` rows fits in `shown` rows once it overflows.
struct Fit {
    /// Blank, title, blank; just the title when rows are scarce.
    full_head: bool,
    /// Key rows shown at once.
    room: usize,
    max_scroll: usize,
}

fn fit(total: usize, shown: usize) -> Fit {
    let keys = total - HEAD_ROWS - FOOT_ROWS;
    let full_head = shown >= HEAD_ROWS + 2;
    let head = if full_head { HEAD_ROWS } else { 1 };
    // One row below the keys is the cue.
    let room = shown.saturating_sub(head + 1);
    Fit {
        full_head,
        room,
        max_scroll: keys.saturating_sub(room),
    }
}

/// `(page, max)` scroll for the panel above `composer`; `max` is 0 when it fits.
pub fn scroll_range(
    composer: Rect,
    theme: &Theme,
    keys: &KeyBindings,
    catalog: Option<&crate::view_modes::Catalog>,
    host: &HostCommands,
) -> (usize, usize) {
    let total = lines_with_modes(composer.width, theme, keys, catalog, host).len();
    let shown = usize::from(composer.y);
    if total <= shown {
        return (0, 0);
    }
    let fit = fit(total, shown);
    (fit.room.saturating_sub(1).max(1), fit.max_scroll)
}

/// Painted above the composer like a popup (`popup::area_above`). A panel
/// taller than that shows its keys from `scroll` and says how to see the rest.
pub fn render(
    frame: &mut Frame,
    composer: Rect,
    scroll: usize,
    theme: &Theme,
    keys: &KeyBindings,
    catalog: Option<&crate::view_modes::Catalog>,
    host: &HostCommands,
) -> Rect {
    let mut lines = lines_with_modes(composer.width, theme, keys, catalog, host);
    let area = area_above(composer, u16::try_from(lines.len()).unwrap_or(u16::MAX));
    if area.is_empty() {
        return area;
    }
    let shown = usize::from(area.height);
    if lines.len() > shown {
        lines = scrolled(lines, shown, scroll, theme, keys);
    }
    frame.render_widget(Clear, area);
    frame.render_widget(Paragraph::new(lines), area);
    area
}

/// The title, key rows from `scroll`, and a cue naming what is hidden and the keys that reach it.
fn scrolled(
    mut lines: Vec<Line<'static>>,
    shown: usize,
    scroll: usize,
    theme: &Theme,
    keys: &KeyBindings,
) -> Vec<Line<'static>> {
    let fit = fit(lines.len(), shown);
    let scroll = scroll.min(fit.max_scroll);
    let keys_end = lines.len() - FOOT_ROWS;
    let key_rows: Vec<_> = lines.drain(HEAD_ROWS..keys_end).collect();
    let below = key_rows.len().saturating_sub(scroll + fit.room);
    let mut out = if fit.full_head {
        lines[..HEAD_ROWS].to_vec()
    } else {
        vec![lines[1].clone()]
    };
    out.extend(key_rows.into_iter().skip(scroll).take(fit.room));
    let ascii = theme.ascii();
    let mut cue = Vec::new();
    if below > 0 {
        cue.push(format!("{} {below} more", if ascii { "v" } else { "↓" }));
    }
    if scroll > 0 {
        cue.push(format!("{} {scroll} above", if ascii { "^" } else { "↑" }));
    }
    if fit.max_scroll > 0 {
        let keys = theme.decoration("↑↓ pgup pgdn scroll", "up/down pgup/pgdn scroll");
        cue.push(keys.to_owned());
    }
    cue.push(format!(
        "{} or esc to close",
        keys.display(crate::keys::KeyAction::Shortcuts, ascii)
    ));
    out.push(Line::from(vec![
        Span::raw(" ".repeat(INDENT)),
        Span::styled(
            cue.join(&format!(" {} ", theme.glyphs().dot)),
            theme.secondary(),
        ),
    ]));
    out.truncate(shown);
    out
}

#[cfg(test)]
mod tests {
    use std::collections::HashSet;

    use ratatui::Terminal;
    use ratatui::backend::TestBackend;
    use ratatui::buffer::Buffer;

    use super::*;
    use crate::ui::theme::tokens::ThemeSpec;

    const HEIGHT: u16 = 60;
    /// Panel widths the review measured: narrow, the 2- and 3-column
    /// thresholds, and the left pane beside a board docked at 200 columns.
    const WIDTHS: [(u16, usize); 7] = [
        (60, 1),
        (79, 1),
        (80, 2),
        (100, 2),
        (120, 3),
        (143, 3),
        (200, 3),
    ];

    fn draw(theme: &Theme, width: u16, height: u16) -> (Buffer, Vec<String>) {
        draw_scrolled(theme, width, height, 0)
    }

    fn draw_scrolled(
        theme: &Theme,
        width: u16,
        height: u16,
        scroll: usize,
    ) -> (Buffer, Vec<String>) {
        let mut terminal = Terminal::new(TestBackend::new(width, height)).unwrap();
        let composer = Rect::new(0, height - 3, width, 3);
        terminal
            .draw(|f| {
                render(
                    f,
                    composer,
                    scroll,
                    theme,
                    &KeyBindings::default(),
                    None,
                    &HostCommands::default(),
                );
            })
            .unwrap();
        let buffer = terminal.backend().buffer().clone();
        let rows = buffer
            .content()
            .chunks(usize::from(width))
            .map(|row| row.iter().map(|c| c.symbol()).collect::<String>())
            .collect();
        (buffer, rows)
    }

    /// Where each heading sits, as `(row, column)` in characters.
    fn headings(rows: &[String]) -> Vec<(KeyGroup, usize, usize)> {
        let mut found = Vec::new();
        for group in KeyGroup::ALL {
            for (y, row) in rows.iter().enumerate() {
                let chars: Vec<char> = row.chars().collect();
                let needle: Vec<char> = group.heading().chars().collect();
                if let Some(x) = chars.windows(needle.len()).position(|w| w == needle) {
                    found.push((group, y, x));
                }
            }
        }
        found
    }

    fn assert_whole_and_grouped(theme: &Theme, width: u16, columns: usize) {
        let ascii = theme.ascii();
        let (buffer, rows) = draw(theme, width, HEIGHT);
        let ellipsis = theme.glyphs().ellipsis;
        assert!(
            rows.iter().all(|r| !r.contains(ellipsis)),
            "{width} columns cut something:\n{}",
            rows.join("\n")
        );
        let headings = headings(&rows);
        for group in KeyGroup::ALL {
            let count = headings.iter().filter(|(g, ..)| *g == group).count();
            assert_eq!(count, 1, "{:?} at {width} columns", group.heading());
        }
        let starts: HashSet<usize> = headings.iter().map(|(_, _, x)| *x).collect();
        assert_eq!(
            starts.len(),
            columns,
            "columns at {width}:\n{}",
            rows.join("\n")
        );
        let blocks = blocks(ascii, &KeyBindings::default());
        for (group, y, x) in headings {
            let cell = &buffer[(u16::try_from(x).unwrap(), u16::try_from(y).unwrap())];
            assert!(
                cell.modifier.contains(theme.title().add_modifier),
                "{group:?} is bold"
            );
            let block = blocks.iter().find(|b| b.group == group).unwrap();
            for (offset, (chord, label)) in block.rows.iter().enumerate() {
                let row: String = rows[y + 1 + offset].chars().skip(x).collect();
                let expected = format!(
                    "{chord}{}{label}",
                    " ".repeat(block.key_width - chord.width())
                );
                assert!(
                    row.starts_with(&expected),
                    "{width} columns, {group:?}: {row:?} should start {expected:?}"
                );
            }
        }
    }

    #[test]
    fn every_width_shows_every_key_whole_under_its_heading() {
        let theme = Theme::resolve(ThemeSpec::default());
        for (width, columns) in WIDTHS {
            assert_whole_and_grouped(&theme, width, columns);
        }
    }

    #[test]
    fn ascii_mode_paints_only_ascii_and_cuts_nothing() {
        let theme = Theme::resolve(ThemeSpec {
            ascii: true,
            ..Default::default()
        });
        for (width, _) in WIDTHS {
            let (_, rows) = draw(&theme, width, HEIGHT);
            for row in &rows {
                assert!(row.is_ascii(), "{row:?}");
            }
            // ASCII arrows are wider, so the column count may drop; nothing is cut.
            let columns = layout(&blocks(true, &KeyBindings::default()), width).len();
            assert_whole_and_grouped(&theme, width, columns);
        }
    }

    #[test]
    fn the_key_column_is_the_longest_key_plus_two() {
        for block in blocks(false, &KeyBindings::default()) {
            let longest = block.rows.iter().map(|(c, _)| c.width()).max().unwrap();
            assert_eq!(block.key_width, longest + 2, "{:?}", block.group);
        }
    }

    #[test]
    fn a_short_screen_says_there_is_more_and_how_to_reach_it() {
        let theme = Theme::resolve(ThemeSpec::default());
        let height = 16;
        let (_, rows) = draw(&theme, 80, height);
        let panel = &rows[..usize::from(height - 3)];
        assert_eq!(panel[1].trim_end(), "  Keyboard shortcuts");
        let full = lines(80, &theme, &KeyBindings::default()).len();
        let hidden = full - HEAD_ROWS - FOOT_ROWS - (panel.len() - HEAD_ROWS - 1);
        assert_eq!(
            panel.last().unwrap().trim_end(),
            format!("  ↓ {hidden} more · ↑↓ pgup pgdn scroll · ? or esc to close")
        );
    }

    #[test]
    fn shortcuts_at_80_by_24_match_the_snapshot() {
        let mut snapshot = String::new();
        for ascii in [false, true] {
            let theme = Theme::resolve(ThemeSpec {
                ascii,
                ..Default::default()
            });
            let (_, rows) = draw(&theme, 80, 24);
            snapshot.push_str(&format!("ascii={ascii}\n"));
            for row in rows {
                snapshot.push_str(row.trim_end());
                snapshot.push('\n');
            }
            snapshot.push_str("end of frame\n");
        }
        let path = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("src/ui/bottom/shortcuts_80x24.txt");
        if std::env::var_os("AETHER_UPDATE_SNAPSHOTS").is_some() {
            std::fs::write(&path, &snapshot).unwrap();
        }
        assert_eq!(snapshot, std::fs::read_to_string(path).unwrap());
    }

    #[test]
    fn scrolling_reaches_the_last_key_and_stops_there() {
        for theme in [
            Theme::resolve(ThemeSpec::default()),
            Theme::resolve(ThemeSpec {
                ascii: true,
                ..Default::default()
            }),
        ] {
            let height = 16;
            let composer = Rect::new(0, height - 3, 80, 3);
            let (page, max) = scroll_range(
                composer,
                &theme,
                &KeyBindings::default(),
                None,
                &HostCommands::default(),
            );
            assert!(max > 0 && page > 0, "the fixture must not fit");
            let full = lines(80, &theme, &KeyBindings::default());
            let last_key: String = full[full.len() - FOOT_ROWS - 1]
                .spans
                .iter()
                .map(|s| s.content.as_ref())
                .collect();
            // Past the end clamps to the end.
            for scroll in [max, max + 50] {
                let (_, rows) = draw_scrolled(&theme, 80, height, scroll);
                let panel = &rows[..usize::from(height - 3)];
                assert_eq!(panel[1].trim_end(), "  Keyboard shortcuts");
                assert_eq!(panel[panel.len() - 2].trim_end(), last_key.trim_end());
                let cue = panel.last().unwrap();
                assert!(cue.contains(&format!("{max} above")), "{cue}");
                assert!(!cue.contains("more"), "{cue}");
            }
        }
    }

    #[test]
    fn a_cue_offers_scrolling_only_when_keys_are_hidden() {
        let theme = Theme::resolve(ThemeSpec::default());
        let total = u16::try_from(lines(80, &theme, &KeyBindings::default()).len()).unwrap();
        // One row short: the cue takes the footer's two rows and every key fits.
        let height = total + 3 - 1;
        let (_, rows) = draw(&theme, 80, height);
        assert_eq!(rows[1].trim_end(), "  Keyboard shortcuts");
        assert_eq!(
            rows[usize::from(height) - 4].trim_end(),
            "  ? or esc to close"
        );
        assert_eq!(
            scroll_range(
                Rect::new(0, height - 3, 80, 3),
                &theme,
                &KeyBindings::default(),
                None,
                &HostCommands::default(),
            )
            .1,
            0
        );
        let height = total + 3 - 2;
        let (_, rows) = draw(&theme, 80, height);
        assert!(
            rows[usize::from(height) - 4].starts_with("  ↓ 1 more · ↑↓"),
            "{rows:#?}"
        );
    }

    #[test]
    fn help_lists_host_commands_under_their_host() {
        let theme = Theme::resolve(ThemeSpec::default());
        let host = HostCommands::parse(
            r#"{"version":1,"host":"archon","commands":[{"name":"answer","args":"<n> <text>","about":"answer question n"}]}"#,
        )
        .unwrap();
        let text = |host: &HostCommands| -> Vec<String> {
            lines_with_modes(80, &theme, &KeyBindings::default(), None, host)
                .iter()
                .map(|line| line.to_string())
                .collect()
        };
        let rows = text(&host);
        let heading = rows.iter().position(|r| r == "  Host commands (archon)");
        let heading = heading.expect("host heading");
        assert_eq!(rows[heading + 1], "  /answer <n> <text>  answer question n");
        assert_eq!(rows.len(), text(&HostCommands::default()).len() + 3);
    }

    #[test]
    fn a_tall_screen_has_no_more_row() {
        let theme = Theme::resolve(ThemeSpec::default());
        let (_, rows) = draw(&theme, 80, HEIGHT);
        let cue = |r: &String| r.contains(" more ·") || r.contains("pgdn scroll");
        assert!(!rows.iter().any(cue), "{rows:#?}");
        assert!(rows.iter().any(|r| r.contains("? or esc to close")));
    }
}
