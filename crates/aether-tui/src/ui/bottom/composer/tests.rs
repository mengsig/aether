#![cfg(test)]

use aether_core::agent::AgentStatus;
use aether_core::id::AgentId;
use crossterm::event::KeyEvent;
use ratatui::Terminal;
use ratatui::backend::TestBackend;
use ratatui::buffer::{Buffer, Cell as BufCell};
use ratatui::style::{Color, Style};

use super::*;
use crate::app::board::BoardFocus;
use crate::ui::bottom::{BottomView, ViewOutcome};
use crate::ui::theme::tokens::ThemeSpec;

const WIDTHS: [u16; 2] = [80, 120];

fn app_with(status: AgentStatus) -> App {
    let mut app = crate::testing::app_with_transcript(&[]);
    let id = app.selected.unwrap();
    app.tree.nodes.get_mut(&id).unwrap().agent.status = status;
    app
}

fn ascii(app: &mut App) {
    app.theme = Theme::resolve(ThemeSpec {
        ascii: true,
        ..Default::default()
    });
}

struct Frame {
    buffer: Buffer,
    cursor: Option<(u16, u16)>,
}

impl Frame {
    fn rows(&self) -> Vec<String> {
        let width = usize::from(self.buffer.area.width);
        self.buffer
            .content()
            .chunks(width)
            .map(|row| row.iter().map(BufCell::symbol).collect::<String>())
            .collect()
    }

    fn row(&self, y: usize) -> String {
        self.rows()[y].trim_end().to_owned()
    }

    fn cell(&self, x: u16, y: u16) -> &BufCell {
        &self.buffer[(x, y)]
    }
}

/// The composer alone, at its desired height for `width`.
fn draw(app: &App, width: u16) -> Frame {
    let mut cache = ComposerCache::default();
    let height = desired_height(app, width, &mut cache);
    let mut terminal = Terminal::new(TestBackend::new(width, height)).unwrap();
    let mut cursor = None;
    terminal
        .draw(|f| cursor = render(f, f.area(), app, &mut cache, &app.theme))
        .unwrap();
    Frame {
        buffer: terminal.backend().buffer().clone(),
        cursor,
    }
}

/// `cell` carries every attribute `style` sets.
fn styled(cell: &BufCell, style: Style) -> bool {
    style.fg.is_none_or(|fg| cell.fg == fg)
        && style.bg.is_none_or(|bg| cell.bg == bg)
        && cell.modifier.contains(style.add_modifier)
}

fn tint_bg(theme: &Theme) -> Color {
    theme.tint().bg.unwrap_or(Color::Reset)
}

#[derive(Debug)]
struct OpenView;

impl BottomView for OpenView {
    fn handle_key(&mut self, _: KeyEvent, _: &App) -> ViewOutcome {
        ViewOutcome::Ignored
    }

    fn desired_height(&self, _: u16, _: &App) -> u16 {
        1
    }

    fn render(&self, _: &mut ratatui::Frame, _: Rect, _: &App, _: &Theme) {}
}

#[test]
fn empty_and_focused_shows_the_message_placeholder_on_a_tinted_block() {
    for width in WIDTHS {
        let app = app_with(AgentStatus::Ready);
        let theme = app.theme;
        let frame = draw(&app, width);
        assert_eq!(frame.rows().len(), 3);
        assert_eq!(frame.row(0), "");
        assert_eq!(frame.row(1), "› Message assistant");
        assert_eq!(frame.row(2), "");
        assert!(styled(frame.cell(0, 1), theme.title()));
        assert!(styled(frame.cell(2, 1), theme.secondary()));
        for y in 0..3 {
            assert_eq!(frame.cell(width - 1, y).bg, tint_bg(&theme));
        }
        assert_eq!(frame.cursor, Some((2, 1)));
    }
}

#[test]
fn a_running_agent_is_steered() {
    for width in WIDTHS {
        let app = app_with(AgentStatus::Running);
        assert_eq!(draw(&app, width).row(1), "› Steer assistant");
    }
}

#[test]
fn after_a_deny_the_placeholder_asks_what_to_do_instead() {
    let mut app = app_with(AgentStatus::Running);
    app.bottom.approval.deny_target = app.selected;
    for width in WIDTHS {
        assert_eq!(
            draw(&app, width).row(1),
            "› Tell assistant what to do instead"
        );
    }
}

#[test]
fn no_agent_yet_invites_a_new_one() {
    let mut app = app_with(AgentStatus::Ready);
    app.selected = Some(AgentId::new());
    assert_eq!(placeholder(&app).as_deref(), Some("Message a new agent"));
}

#[test]
fn an_open_view_dims_the_prompt_drops_the_placeholder_and_the_cursor() {
    for width in WIDTHS {
        let mut app = app_with(AgentStatus::Ready);
        app.bottom.push_view(Box::new(OpenView));
        let frame = draw(&app, width);
        assert_eq!(frame.row(1), "›");
        assert!(styled(frame.cell(0, 1), app.theme.secondary()));
        assert_eq!(frame.cursor, None);
        assert_eq!(placeholder(&app), None);
    }
}

#[test]
fn a_focused_board_dims_the_prompt_but_keeps_the_placeholder() {
    let mut app = app_with(AgentStatus::Ready);
    app.board.open = true;
    app.board.focus = BoardFocus::Board;
    let frame = draw(&app, 80);
    assert_eq!(frame.row(1), "› Message assistant");
    assert!(styled(frame.cell(0, 1), app.theme.secondary()));
    assert_eq!(frame.cursor, None);
}

#[test]
fn a_draft_wraps_by_word_with_an_indent_and_the_cursor_follows() {
    let words = "steer the implementer towards the failing snapshot tests first";
    for width in WIDTHS {
        let mut app = app_with(AgentStatus::Ready);
        let draft = [words; 3].join(" ");
        app.input.set_text(draft.clone());
        let frame = draw(&app, width);
        let text_width = usize::from(width) - 4;
        let rows: Vec<String> = frame.rows()[1..frame.rows().len() - 1]
            .iter()
            .map(|r| {
                r[r.char_indices().nth(2).unwrap().0..]
                    .trim_end()
                    .to_owned()
            })
            .collect();
        assert!(rows.len() > 1, "{rows:?}");
        assert!(rows.iter().all(|r| r.chars().count() <= text_width));
        // Rejoining the rows gives back the words: no row splits one.
        assert_eq!(rows.join(" "), draft);
        assert!(frame.row(2).starts_with("  "));
        assert!(styled(frame.cell(2, 1), app.theme.text()));
        let last = u16::try_from(rows.len()).unwrap();
        let tail = u16::try_from(rows.last().unwrap().chars().count()).unwrap();
        assert_eq!(frame.cursor, Some((2 + tail, last)));
    }
}

#[test]
fn hard_newlines_are_rows_and_height_counts_them() {
    for width in WIDTHS {
        let mut app = app_with(AgentStatus::Ready);
        app.input.set_text("one\ntwo\n\nfour");
        let frame = draw(&app, width);
        assert_eq!(frame.rows().len(), 6);
        assert_eq!(frame.row(1), "› one");
        assert_eq!(frame.row(2), "  two");
        assert_eq!(frame.row(3), "");
        assert_eq!(frame.row(4), "  four");
    }
}

#[test]
fn past_eight_rows_the_block_stops_growing_and_scrolls_to_the_cursor() {
    for width in WIDTHS {
        let mut app = app_with(AgentStatus::Ready);
        let lines: Vec<String> = (1..=12).map(|i| format!("line {i}")).collect();
        app.input.set_text(lines.join("\n"));
        let frame = draw(&app, width);
        assert_eq!(
            frame.rows().len(),
            usize::from(COMPOSER_MAX_INPUT_ROWS + COMPOSER_PADDING_ROWS)
        );
        assert_eq!(frame.row(1), "  line 5");
        assert_eq!(frame.row(8), "  line 12");
        assert_eq!(frame.cursor, Some((9, 8)));

        app.input.cursor = 0;
        let frame = draw(&app, width);
        assert_eq!(frame.row(1), "› line 1");
        assert_eq!(frame.cursor, Some((2, 1)));
    }
}

#[test]
fn the_scroll_window_moves_only_as_far_as_the_cursor_needs() {
    assert_eq!(scroll_to(0, 3, 8, 12), 0);
    assert_eq!(scroll_to(0, 9, 8, 12), 2);
    assert_eq!(scroll_to(4, 5, 8, 12), 4);
    assert_eq!(scroll_to(4, 1, 8, 12), 1);
    assert_eq!(scroll_to(9, 0, 8, 3), 0);
}

#[test]
fn paste_placeholders_paint_in_the_accent() {
    for width in WIDTHS {
        let mut app = app_with(AgentStatus::Ready);
        app.input.set_text("see ");
        app.composer_paste(&"x".repeat(2_652));
        let frame = draw(&app, width);
        assert_eq!(frame.row(1), "› see [Pasted 2,652 chars]");
        let theme = app.theme;
        assert!(styled(frame.cell(2, 1), theme.text()));
        assert!(styled(frame.cell(6, 1), theme.accent()));
        assert!(styled(frame.cell(25, 1), theme.accent()));
    }
}

#[test]
fn ascii_mode_uses_the_ascii_prompt() {
    for width in WIDTHS {
        let mut app = app_with(AgentStatus::Ready);
        ascii(&mut app);
        assert_eq!(draw(&app, width).row(1), "> Message assistant");
        app.input.set_text("tab\there");
        assert_eq!(draw(&app, width).row(1), "> tab    here");
    }
}

#[test]
fn the_wrap_cache_reuses_rows_until_the_draft_or_width_changes() {
    let mut app = app_with(AgentStatus::Ready);
    app.input.set_text("cached");
    let mut cache = ComposerCache::default();
    desired_height(&app, 80, &mut cache);
    let key = cache.key;
    desired_height(&app, 80, &mut cache);
    assert_eq!(cache.key, key);
    desired_height(&app, 100, &mut cache);
    assert_ne!(cache.key, key);
    app.input.insert_char('!');
    desired_height(&app, 100, &mut cache);
    assert_eq!(cache.rows[0].text, "cached!");
}

#[test]
fn paste_placeholders_number_repeats_and_group_thousands() {
    assert_eq!(paste_placeholder(1_001, &[], ""), "[Pasted 1,001 chars]");
    assert_eq!(
        paste_placeholder(1_234_567, &[], ""),
        "[Pasted 1,234,567 chars]"
    );
    let taken = [PastedText {
        placeholder: "[Pasted 1,001 chars]".into(),
        text: String::new(),
        range: 0..0,
    }];
    assert_eq!(
        paste_placeholder(1_001, &taken, ""),
        "[Pasted 1,001 chars #2]"
    );
}

#[test]
fn a_tiny_area_paints_without_padding_and_never_panics() {
    let app = app_with(AgentStatus::Ready);
    let mut cache = ComposerCache::default();
    for (w, h) in [(1, 1), (3, 2), (80, 1), (0, 0)] {
        let mut terminal = Terminal::new(TestBackend::new(w.max(1), h.max(1))).unwrap();
        terminal
            .draw(|f| {
                let area = Rect::new(0, 0, w, h);
                render(f, area, &app, &mut cache, &app.theme);
            })
            .unwrap();
    }
}

fn attached(name: &str) -> ImageAttachment {
    ImageAttachment {
        media_type: "image/png".into(),
        data: "iVBORw0KGgpwaXhlbHM=".into(),
        name: Some(name.into()),
    }
}

#[test]
fn attached_images_are_a_row_of_chips_above_the_draft() {
    for width in WIDTHS {
        let mut app = app_with(AgentStatus::Ready);
        let bare = draw(&app, width);
        app.input.images = vec![attached("shot.png"), attached("second.png")];
        app.input.set_text("what is this?");
        let frame = draw(&app, width);
        assert_eq!(frame.rows().len(), bare.rows().len() + 1);
        assert_eq!(
            frame.row(1),
            "  [image: shot.png, 14 B]  [image: second.png, 14 B]"
        );
        assert_eq!(frame.row(2), "› what is this?");
        assert!(styled(frame.cell(2, 1), app.theme.secondary()));
        // The cursor follows the draft down a row.
        assert_eq!(frame.cursor, Some((15, 2)));
    }
}

#[test]
fn chips_that_do_not_fit_are_counted() {
    let images = [attached("a.png"), attached("b.png"), attached("c.png")];
    let one = "[image: a.png, 14 B]";
    assert_eq!(chips_text(&images[..1], 80), one);
    assert_eq!(
        chips_text(&images, 80),
        "[image: a.png, 14 B]  [image: b.png, 14 B]  [image: c.png, 14 B]"
    );
    assert_eq!(
        chips_text(&images, 52),
        "[image: a.png, 14 B]  [image: b.png, 14 B]  +1 more"
    );
    assert_eq!(chips_text(&images, 30), "[image: a.png, 14 B]  +2 more");
    // Even a row too narrow for one chip names the first image.
    assert_eq!(chips_text(&images, 4), "[image: a.png, 14 B]  +2 more");
    assert_eq!(chips_text(&[], 80), "");
}

#[test]
fn the_draft_keeps_its_row_before_the_chips_get_one() {
    let mut app = app_with(AgentStatus::Ready);
    app.input.images = vec![attached("shot.png")];
    let rows = |height| block_rows(&app, height);
    assert_eq!((rows(4).pad, rows(4).chips, rows(4).draft), (1, 1, 1));
    assert_eq!((rows(3).pad, rows(3).chips, rows(3).draft), (1, 0, 1));
    assert_eq!((rows(2).pad, rows(2).chips, rows(2).draft), (0, 1, 1));
    assert_eq!((rows(1).pad, rows(1).chips, rows(1).draft), (0, 0, 1));
    assert_eq!((rows(0).pad, rows(0).chips, rows(0).draft), (0, 0, 0));
}
