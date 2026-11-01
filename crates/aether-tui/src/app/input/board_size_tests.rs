//! The viewer's size keys resize a focused board and keep the viewer's meaning elsewhere (#346).

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use ratatui::layout::Rect;

use crate::app::board::BoardFocus;
use crate::app::{App, AppEvent};
use crate::testing::{populated_app, render_with};
use crate::ui::layout::{BOARD_MIN_WIDTH, BoardMode, MAIN_MIN_COLS, board_max_width};
use crate::ui::transcript::ViewCache;

fn focused_board(cols: u16) -> App {
    let mut app = populated_app();
    app.approvals.clear();
    app.size = (cols, 40);
    app.board.open = true;
    app.board.focus = BoardFocus::Board;
    app.board.cursor = app.selected;
    app
}

fn alt(app: &mut App, c: char) {
    app.dispatch_key(KeyEvent::new(KeyCode::Char(c), KeyModifiers::ALT));
}

fn ctrl(app: &mut App, c: char) {
    app.dispatch_key(KeyEvent::new(KeyCode::Char(c), KeyModifiers::CONTROL));
}

#[track_caller]
fn docked_width(app: &App) -> u16 {
    match app.board_mode() {
        BoardMode::Docked { width } => width,
        other => panic!("the board is not docked: {other:?}"),
    }
}

fn open_viewer(app: &mut App) {
    app.viewer.open = true;
    app.viewer.preview = Some(Ok(crate::viewer::Preview::Unsupported(
        std::path::PathBuf::from("preview.txt"),
    )));
}

#[test]
fn alt_minus_and_alt_equals_step_the_focused_board_by_five_percent() {
    let mut app = focused_board(140);
    assert_eq!(docked_width(&app), 42);
    alt(&mut app, '=');
    assert_eq!(docked_width(&app), 49);
    alt(&mut app, '-');
    assert_eq!(docked_width(&app), 42);
    alt(&mut app, '-');
    assert_eq!(docked_width(&app), BOARD_MIN_WIDTH);
    alt(&mut app, '-');
    assert_eq!(docked_width(&app), BOARD_MIN_WIDTH);
    assert!(app.board.focused(), "resizing keeps the board's focus");
    assert_eq!(app.viewer.share, 50, "the viewer is not resized");
}

#[test]
fn widening_stops_where_the_transcript_keeps_its_minimum() {
    for cols in [100, 140, 220] {
        let mut app = focused_board(cols);
        for _ in 0..40 {
            alt(&mut app, '=');
        }
        let width = docked_width(&app);
        assert_eq!(width, board_max_width(cols), "{cols} columns");
        assert_eq!(cols - width - 2, MAIN_MIN_COLS, "{cols} columns");
        assert_eq!(
            crate::ui::bottom_width(cols, app.board_mode()),
            MAIN_MIN_COLS
        );
    }
}

#[test]
fn the_width_survives_hiding_showing_and_leaving_the_board() {
    let mut app = focused_board(140);
    alt(&mut app, '=');
    alt(&mut app, '=');
    assert_eq!(docked_width(&app), 56);
    ctrl(&mut app, 'b');
    assert_eq!(app.board_mode(), BoardMode::Hidden);
    ctrl(&mut app, 'b');
    assert_eq!(docked_width(&app), 56);
    ctrl(&mut app, 'g');
    assert!(app.board.focused());
    app.dispatch_key(KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE));
    assert!(!app.board.focused());
    assert_eq!(docked_width(&app), 56);
}

#[test]
fn a_terminal_resize_clamps_the_width_and_keeps_the_choice() {
    let mut app = focused_board(220);
    for _ in 0..9 {
        alt(&mut app, '=');
    }
    assert_eq!(docked_width(&app), 155);
    app.reduce(AppEvent::Resize(120, 40));
    assert_eq!(docked_width(&app), board_max_width(120));
    app.reduce(AppEvent::Resize(220, 40));
    assert_eq!(docked_width(&app), 155);
    app.reduce(AppEvent::Resize(120, 40));
    alt(&mut app, '-');
    assert_eq!(
        docked_width(&app),
        board_max_width(120) - 6,
        "steps from what shows"
    );
}

#[test]
fn alt_z_fills_the_screen_and_leaving_docks_the_board_again() {
    let mut app = focused_board(140);
    alt(&mut app, '=');
    alt(&mut app, 'z');
    assert_eq!(app.board_mode(), BoardMode::Overlay);
    assert!(app.shown_approval().is_none());
    alt(&mut app, '=');
    assert_eq!(
        app.board_mode(),
        BoardMode::Overlay,
        "already as wide as it gets"
    );
    alt(&mut app, '-');
    assert_eq!(
        docked_width(&app),
        49,
        "narrowing docks it at the chosen width"
    );
    alt(&mut app, 'z');
    alt(&mut app, 'z');
    assert_eq!(docked_width(&app), 49);
    alt(&mut app, 'z');
    app.dispatch_key(KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE));
    assert!(app.board.open && !app.board.focused() && !app.board.full);
    assert_eq!(docked_width(&app), 49);
    ctrl(&mut app, 'g');
    assert_eq!(docked_width(&app), 49, "focus comes back docked");
}

#[test]
fn below_one_hundred_columns_the_keys_only_explain() {
    let mut app = focused_board(80);
    let before = app.board.clone();
    for c in ['=', '-', 'z'] {
        app.flash = None;
        alt(&mut app, c);
        assert_eq!(app.board, before, "Alt+{c}");
        assert_eq!(app.board_mode(), BoardMode::Overlay);
        assert!(
            app.live_flash()
                .is_some_and(|text| text.contains("100 columns"))
        );
    }
    assert!(app.input.text.is_empty(), "nothing reaches the composer");
}

#[test]
fn unfocused_the_keys_still_drive_the_viewer() {
    let mut app = focused_board(140);
    app.board.focus = BoardFocus::BottomPane;
    open_viewer(&mut app);
    alt(&mut app, '=');
    assert_eq!(app.viewer.share, 55);
    alt(&mut app, '-');
    alt(&mut app, '-');
    assert_eq!(app.viewer.share, 45);
    alt(&mut app, 'z');
    assert!(app.viewer.fullscreen);
    assert_eq!(app.board.width, None);
    assert!(!app.board.full);
}

#[test]
fn with_the_viewer_open_the_focused_board_takes_the_keys() {
    let mut app = focused_board(140);
    open_viewer(&mut app);
    alt(&mut app, '=');
    alt(&mut app, 'z');
    assert_eq!(app.board.width, Some(49));
    assert!(app.board.full);
    assert_eq!(app.viewer.share, 50);
    assert!(!app.viewer.fullscreen);
}

fn render(app: &mut App, cols: u16) -> ViewCache {
    app.size = (cols, 40);
    let mut cache = ViewCache::default();
    render_with(app, &mut cache, cols, 40);
    cache
}

#[track_caller]
fn assert_apart(cols: u16, what: &str, a: Rect, b: Rect) {
    assert!(
        a.is_empty() || b.is_empty() || !a.intersects(b),
        "{cols} columns, {what}: {a:?} overlaps {b:?}"
    );
}

#[test]
fn a_resized_board_lays_out_cleanly_at_100_140_and_220_columns() {
    for cols in [100, 140, 220] {
        for width in [None, Some(0), Some(48), Some(u16::MAX)] {
            let mut app = focused_board(cols);
            app.board.width = width;
            let shown = docked_width(&app);
            let cache = render(&mut app, cols);
            let regions = cache.regions;
            assert_eq!(
                regions.board.x + regions.board.width + 2,
                cols,
                "{cols} {width:?}"
            );
            assert_eq!(
                regions.board.width + 4,
                shown,
                "padding inside {shown} columns"
            );
            assert!(
                regions.transcript.width >= MAIN_MIN_COLS,
                "{cols} {width:?}"
            );
            assert_apart(cols, "transcript", regions.transcript, regions.board);
            assert_apart(cols, "composer", regions.composer_block, regions.board);
        }
        let mut app = focused_board(cols);
        alt(&mut app, 'z');
        let regions = render(&mut app, cols).regions;
        assert_eq!(regions.board.width, cols - 4, "full screen at {cols}");
        assert!(regions.composer_block.is_empty() && regions.transcript.is_empty());
    }
}

/// Every width, with the viewer beside an unfocused board or behind a focused one.
#[test]
fn board_and_viewer_never_overlap_or_panic_at_any_width() {
    for cols in 1..=240 {
        for width in [None, Some(u16::MAX)] {
            for focused in [false, true] {
                let mut app = focused_board(cols);
                app.board.width = width;
                open_viewer(&mut app);
                if !focused {
                    app.board.focus = BoardFocus::BottomPane;
                }
                let cache = render(&mut app, cols);
                let regions = cache.regions;
                let what = format!("width {width:?}, focused {focused}");
                assert_apart(cols, &what, regions.board, cache.viewer_area);
                assert_apart(cols, &what, regions.transcript, cache.viewer_area);
                assert_apart(cols, &what, regions.composer_block, cache.viewer_area);
                assert_apart(cols, &what, regions.transcript, regions.board);
                assert_apart(cols, &what, regions.composer_block, regions.board);
            }
        }
    }
}

#[test]
fn a_due_approval_docks_a_full_screen_board_instead_of_closing_it() {
    let mut app = focused_board(140);
    alt(&mut app, 'z');
    assert_eq!(app.board_mode(), BoardMode::Overlay);
    app.approvals = populated_app().approvals;
    assert!(!app.approvals.is_empty(), "the fixture has an approval");
    app.reduce(AppEvent::Resize(140, 40));
    app.now += chrono::Duration::milliseconds(crate::app::approval_flow::TYPING_GUARD_MS);
    app.reduce(AppEvent::Resize(140, 40));
    assert!(app.board.open, "the docked board stays");
    assert!(!app.board.focused() && !app.board.full);
    assert_eq!(docked_width(&app), 42);
    assert!(app.shown_approval().is_some());
}
