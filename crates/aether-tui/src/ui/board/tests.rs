//! TestBackend snapshots of the board: row text against the design mocks,
//! cell styles against the theme's token methods (never literal colours).

use std::path::PathBuf;

use chrono::Duration;
use ratatui::Terminal;
use ratatui::backend::TestBackend;
use ratatui::buffer::{Buffer, Cell};
use ratatui::style::Style;

use aether_core::agent::AgentStatus;
use aether_core::id::AgentId;
use aether_core::model::ModelRef;
use aether_protocol::ModelInfo;

use super::*;
use crate::app::board::BoardFocus;
use crate::app::board::fixtures::{Fleet, TreeBuilder, app_with, fleet, now, push_cell_at};
use crate::app::transcript::{CallStatus, CellModel};
use crate::ui::layout::{board_inner, board_width};
use crate::ui::theme::tokens::{Rgb, TerminalPalette, ThemeSpec};

const WORKSPACE: &str = "/srv/wt/implementer";

/// The mock fleet with the inspector details the mocks show, the board
/// focused and its cursor on `cursor`.
fn mock_app(cursor: fn(&Fleet) -> AgentId) -> Fleet {
    let mut f = fleet();
    let app = &mut f.app;
    app.models = ["sonnet-5.5", "haiku-4.5", "opus-5.5"]
        .into_iter()
        .map(|model| ModelInfo {
            tier: None,
            aliases: Vec::new(),
            model: ModelRef::new("anthropic", model),
            name: model.into(),
            provider_name: "Anthropic".into(),
            capabilities: aether_core::model::CapabilityProfile {
                context_window: 1_000_000,
                compaction_tokens: Some(256_000),
                ..Default::default()
            },
            configured: true,
            auth_source: None,
            is_default: false,
        })
        .collect();
    app.context_tokens.insert(f.implementer, 82_000);
    app.context_tokens.insert(f.tester, 36_000);
    for (id, input, cached, output) in [
        (f.implementer, 32_000, 380_000, 18_200),
        (f.tester, 25_000, 71_000, 4_100),
    ] {
        let agent = &mut app.tree.nodes.get_mut(&id).expect("a fleet agent").agent;
        agent.stats.usage.input_tokens = input;
        agent.stats.usage.cache_read_tokens = cached;
        agent.stats.usage.output_tokens = output;
        agent.scope.workspace_root = Some(PathBuf::from(WORKSPACE));
    }
    let tester_node = app.tree.nodes.get_mut(&f.tester).expect("tester");
    tester_node.task = Some(aether_core::task::Task::new(
        "Integration tests for runs list --json: empty, two runs, imported run; pin the table bytes.",
        aether_core::event::Actor::System,
        aether_core::id::RunId::new(),
        now(),
    ));
    push_cell_at(
        app,
        f.implementer,
        now() - Duration::seconds(12),
        CellModel::Exec {
            call: None,
            command: "cargo test -p aether runs_list".into(),
            status: CallStatus::Succeeded,
            output: String::new(),
            exit: Some(0),
            duration_ms: Some(6_400),
            is_search: false,
            artifacts: Vec::new(),
        },
    );
    let cursor = cursor(&f);
    f.app.board.focus = BoardFocus::Board;
    f.app.board.cursor = Some(cursor);
    f.app.rebuild_board();
    f
}

fn dark() -> Theme {
    Theme::default()
}

fn light() -> Theme {
    Theme::resolve(ThemeSpec {
        palette: Some(TerminalPalette {
            fg: Rgb(36, 41, 47),
            bg: Rgb(250, 250, 250),
        }),
        ..ThemeSpec::default()
    })
}

fn ascii() -> Theme {
    Theme::resolve(ThemeSpec {
        ascii: true,
        ..ThemeSpec::default()
    })
}

/// One docked board column `width` wide, drawn the way `ui::render` will.
fn draw_docked(app: &App, width: u16, height: u16, theme: &Theme) -> Buffer {
    draw_with(app, width, height, theme, &mut BoardScroll::default())
}

fn draw_with(
    app: &App,
    width: u16,
    height: u16,
    theme: &Theme,
    scroll: &mut BoardScroll,
) -> Buffer {
    let mut terminal = Terminal::new(TestBackend::new(width, height)).expect("a test terminal");
    terminal
        .draw(|frame| {
            let area = board_inner(frame.area());
            render(frame, area, app, scroll, theme);
        })
        .expect("draw");
    terminal.backend().buffer().clone()
}

fn rows(buf: &Buffer) -> Vec<String> {
    let width = usize::from(buf.area.width);
    buf.content()
        .chunks(width)
        .map(|row| {
            let text: String = row.iter().map(Cell::symbol).collect();
            text.trim_end().to_owned()
        })
        .collect()
}

/// Rows inside the board's two-column padding.
fn inner_rows(buf: &Buffer) -> Vec<String> {
    rows(buf)
        .into_iter()
        .map(|row| row.strip_prefix("  ").unwrap_or(&row).to_owned())
        .collect()
}

/// Every attribute `style` sets is on the cell; unset ones are not checked.
fn painted(cell: &Cell, style: Style) -> bool {
    style.fg.is_none_or(|fg| cell.fg == fg)
        && style.bg.is_none_or(|bg| cell.bg == bg)
        && cell.modifier.contains(style.add_modifier)
}

/// `(x, y)` of `needle`'s first cell on the first row containing `row`.
fn locate(buf: &Buffer, row: &str, needle: &str) -> (u16, u16) {
    let y = rows(buf)
        .iter()
        .position(|r| r.contains(row))
        .unwrap_or_else(|| panic!("no row contains {row:?}"));
    let text = &rows(buf)[y];
    let at = text
        .find(needle)
        .unwrap_or_else(|| panic!("{needle:?} not in {text:?}"));
    let x = text[..at].chars().count();
    (
        u16::try_from(x).expect("a column"),
        u16::try_from(y).expect("a row"),
    )
}

/// The selection's own marker (its background or modifier) is on the cell.
fn looks_selected(cell: &Cell, theme: &Theme) -> bool {
    let selected = theme.selected();
    let marked_bg = selected.bg.is_some_and(|bg| cell.bg == bg);
    let marked_modifier =
        !selected.add_modifier.is_empty() && cell.modifier.contains(selected.add_modifier);
    marked_bg || marked_modifier
}

fn assert_span(buf: &Buffer, row: &str, needle: &str, style: Style) {
    let (x, y) = locate(buf, row, needle);
    for dx in 0..u16::try_from(needle.chars().count()).expect("a width") {
        let c = &buf[(x + dx, y)];
        assert!(
            painted(c, style),
            "{needle:?} in {row:?} at column {}: {c:?} lacks {style:?}",
            x + dx
        );
    }
}

fn bar_line(width: usize, percent: usize, theme: &Theme) -> String {
    let g = theme.glyphs();
    let bar = width - 11 - 6;
    let filled = bar * percent / 100;
    format!(
        "Context    {}{}{percent:>5}%",
        g.bar.repeat(filled),
        g.bar.repeat(bar - filled)
    )
}

#[test]
fn docked_board_at_48_columns_matches_the_mock() {
    let f = mock_app(|f| f.implementer);
    assert_eq!(board_width(160), 48);
    let buf = draw_docked(&f.app, 48, 40, &dark());
    let expected = [
        "",
        "Agents          5 agents · 2 running · $1.84",
        "",
        "● aether  orchestrator         6m 02s  $1.12",
        "│ opus-5.5 · high",
        "├ ✓ researcher  explorer       1m 03s  $0.21",
        "│   sonnet-5.5 · medium",
        "└ ● implementer  worker        2m 40s  $0.38",
        "  │ sonnet-5.5 · medium · implementer",
        "  ├ ◆ tester  verifier            41s  $0.09",
        "  │   haiku-4.5 · low              needs you",
        "  └ ○ docs  writer                     $0.04",
        "      haiku-4.5 · low",
        "",
        "────────────────────────────────────────────",
        "",
        "implementer  worker                ● Running",
        "child of aether · turn 6 · 2m 40s",
        "",
        "Add --json to aether runs list (one object",
        "per line), keep the table byte-identical,",
        "delegate tests, update docs/cli.md.",
        "",
        &bar_line(44, 32, &dark()),
        "           82k of 256k until compaction",
        "Model      sonnet-5.5 · medium",
        "Tokens     32k in · 380k cached · 18.2k out",
        "Cost       $0.38 · $0.51 with children",
        &format!("Workspace  {WORKSPACE}"),
        "",
        "Children",
        "◆ tester  verifier                41s  $0.09",
        "○ docs  writer                         $0.04",
        "",
        "Last activity                        12s ago",
        "Ran cargo test -p aether runs_list",
        "",
        "",
        "",
        "",
    ];
    assert_eq!(inner_rows(&buf), expected);
}

#[test]
fn docked_board_paints_through_theme_tokens() {
    for theme in [dark(), light()] {
        let f = mock_app(|f| f.implementer);
        let buf = draw_docked(&f.app, 48, 40, &theme);
        assert_span(&buf, "Agents", "Agents", theme.title());
        assert_span(
            &buf,
            "Agents",
            "5 agents · 2 running · $1.84",
            theme.secondary(),
        );
        // The viewed agent's name is bold; others are plain.
        assert_span(&buf, "● aether", "aether", theme.title());
        assert_span(&buf, "● aether", "orchestrator", theme.secondary());
        assert_span(&buf, "● aether", "6m 02s", theme.secondary());
        assert_span(&buf, "● aether", "$1.12", theme.text());
        assert_span(&buf, "│ opus-5.5", "│", theme.rule());
        assert_span(&buf, "│ opus-5.5", "opus-5.5 · high", theme.secondary());
        assert_span(&buf, "researcher  explorer", "├", theme.rule());
        assert_span(&buf, "researcher  explorer", "✓", theme.success());
        assert_span(&buf, "tester  verifier   ", "◆", theme.attention());
        assert_span(&buf, "tester  verifier   ", "◆", theme.title());
        assert_span(&buf, "needs you", "needs you", theme.attention());
        assert_span(&buf, "docs  writer   ", "○", theme.secondary());
        assert_span(&buf, "────", "────", theme.rule());
        // Inspector.
        assert_span(&buf, "● Running", "implementer", theme.title());
        assert_span(&buf, "● Running", "Running", theme.running());
        assert_span(
            &buf,
            "child of aether",
            "child of aether",
            theme.secondary(),
        );
        assert_span(&buf, "Context ", "Context", theme.secondary());
        let (x, y) = locate(&buf, "Context ", theme.glyphs().bar);
        assert!(painted(&buf[(x, y)], theme.accent()));
        assert!(painted(&buf[(x + 11, y)], theme.track()));
        assert_span(&buf, "Model ", "Model", theme.secondary());
        assert_span(&buf, "Children", "Children", theme.secondary());
        assert_span(&buf, "Ran cargo", "Ran", theme.title());
    }
}

#[test]
fn only_the_cursor_rows_carry_selection_and_it_bleeds_into_the_padding() {
    let theme = dark();
    let f = mock_app(|f| f.implementer);
    let buf = draw_docked(&f.app, 48, 40, &theme);
    let (_, title) = locate(&buf, "└ ● implementer", "implementer");
    let cursor_rows = [title, title + 1];
    let plain_bg = Cell::default().bg;
    for y in 0..buf.area.height {
        for x in 0..buf.area.width {
            let c = &buf[(x, y)];
            let in_band = cursor_rows.contains(&y) && (1..47).contains(&x);
            if in_band {
                assert!(painted(c, theme.selected()), "({x}, {y}) unselected");
            } else {
                // Only tint()/selected() may paint a background.
                assert_eq!(c.bg, plain_bg, "({x}, {y}) has a background");
                assert!(!looks_selected(c, &theme), "({x}, {y}) selected");
            }
        }
    }
}

#[test]
fn an_unfocused_board_shows_no_cursor_and_inspects_the_viewed_agent() {
    let theme = dark();
    let mut f = mock_app(|f| f.implementer);
    f.app.board.focus = BoardFocus::BottomPane;
    let buf = draw_docked(&f.app, 48, 40, &theme);
    assert!(!buf.content().iter().any(|c| looks_selected(c, &theme)));
    let head = rows(&buf)
        .into_iter()
        .find(|r| r.ends_with("● Running"))
        .expect("an inspector head");
    assert!(head.contains("aether  orchestrator"), "{head:?}");
}

#[test]
fn mono_cursor_uses_a_title_marker_without_underlining_the_band() {
    use crate::ui::theme::tokens::ThemeMode;

    let theme = Theme::resolve(ThemeSpec {
        mode: ThemeMode::Mono,
        ..Default::default()
    });
    let f = mock_app(|f| f.implementer);
    let buf = draw_docked(&f.app, 48, 40, &theme);
    let (_, y) = locate(&buf, "└ ● implementer", "implementer");
    assert_eq!(buf[(1, y)].symbol(), theme.glyphs().prompt);
    assert!(painted(&buf[(1, y)], theme.title()));
    assert_ne!(buf[(1, y + 1)].symbol(), theme.glyphs().prompt);
    assert!(
        buf.content()
            .iter()
            .all(|cell| !looks_selected(cell, &theme))
    );
}

#[test]
fn narrow_and_wide_docks_keep_names_and_right_columns() {
    for cols in [100, 200] {
        let width = board_width(cols);
        let inner = usize::from(width) - 4;
        let f = mock_app(|f| f.implementer);
        for theme in [dark(), light(), ascii()] {
            let buf = draw_docked(&f.app, width, 40, &theme);
            let text = inner_rows(&buf);
            assert!(text.iter().all(|r| r.chars().count() <= inner), "{text:#?}");
            assert!(text[1].starts_with("Agents") && text[1].ends_with("$1.84"));
            for (name, cost) in [
                ("aether", "$1.12"),
                ("researcher", "$0.21"),
                ("implementer", "$0.38"),
                ("tester", "$0.09"),
                ("docs", "$0.04"),
            ] {
                let row = text[3..13]
                    .iter()
                    .find(|r| r.contains(name))
                    .unwrap_or_else(|| panic!("{name} at {width}: {text:#?}"));
                assert!(row.ends_with(cost), "{row:?} at {width}");
            }
            assert!(text.iter().any(|r| r.ends_with("needs you")), "{text:#?}");
        }
    }
    assert_eq!(board_width(100), 40);
    assert_eq!(board_width(200), 56);
}

#[test]
fn ascii_board_draws_only_ascii() {
    let theme = ascii();
    let f = mock_app(|f| f.implementer);
    let buf = draw_docked(&f.app, 48, 40, &theme);
    let text = inner_rows(&buf);
    for row in &text {
        assert!(row.is_ascii(), "{row:?}");
    }
    assert_eq!(text[1], "Agents          5 agents | 2 running | $1.84");
    assert!(
        text[5].starts_with("|-+ researcher  explorer"),
        "{:?}",
        text[5]
    );
    assert!(
        text[9].starts_with("  |-! tester  verifier"),
        "{:?}",
        text[9]
    );
    let bar = text
        .iter()
        .find(|r| r.starts_with("Context"))
        .expect("a context bar");
    assert!(bar.contains("===") && bar.contains("---"), "{bar:?}");
}

#[test]
fn overlay_at_80_columns_covers_the_screen_and_matches_the_mock() {
    let theme = dark();
    let f = mock_app(|f| f.tester);
    let mut terminal = Terminal::new(TestBackend::new(80, 34)).expect("a test terminal");
    let mut scroll = BoardScroll::default();
    terminal
        .draw(|frame| {
            let area = frame.area();
            for y in area.top()..area.bottom() {
                frame
                    .buffer_mut()
                    .set_string(0, y, "#".repeat(80), theme.failure());
            }
            render_overlay(frame, area, &f.app, &mut scroll, &theme);
        })
        .expect("draw");
    let buf = terminal.backend().buffer().clone();
    assert!(
        buf.content().iter().all(|c| c.symbol() != "#"),
        "the chat shows through"
    );
    let expected = [
        "",
        "Agents                                          5 agents · 2 running · $1.84",
        "",
        "● aether  orchestrator                                         6m 02s  $1.12",
        "│ opus-5.5 · high",
        "├ ✓ researcher  explorer                                       1m 03s  $0.21",
        "│   sonnet-5.5 · medium",
        "└ ● implementer  worker                                        2m 40s  $0.38",
        "  │ sonnet-5.5 · medium · implementer",
        "  ├ ◆ tester  verifier                                            41s  $0.09",
        "  │   haiku-4.5 · low                                              needs you",
        "  └ ○ docs  writer                                                     $0.04",
        "      haiku-4.5 · low",
        "",
        "────────────────────────────────────────────────────────────────────────────",
        "",
        "tester  verifier                                                 ◆ Needs you",
        "child of implementer · turn 3 · 41s",
        "",
        "Integration tests for runs list --json: empty, two runs, imported run; pin",
        "the table bytes.",
        "",
        "◆ Asks: should --json include imported runs, or only live ones?",
        "  enter view and reply",
        "",
        &bar_line(76, 14, &theme),
        "           36k of 256k until compaction · window 1M",
        "Model      haiku-4.5 · low",
        "Tokens     25k in · 71k cached · 4.1k out",
        "Cost       $0.09",
        &format!("Workspace  {WORKSPACE}"),
        "",
        "",
        "",
    ];
    assert_eq!(inner_rows(&buf), expected);
    assert_span(&buf, "◆ Needs you", "Needs you", theme.attention());
    assert_span(&buf, "◆ Asks", "◆", theme.attention());
    assert_span(&buf, "enter view", "enter", theme.title());
    assert_span(&buf, "enter view", "view and reply", theme.secondary());
}

/// R3: at the 40-column dock every figure and path survives, wrapped under the value column.
#[test]
fn the_narrowest_dock_wraps_detail_rows_instead_of_cutting_them() {
    let mut f = mock_app(|f| f.implementer);
    let workspace = PathBuf::from("/tmp/ae2e/retest-real/workspace");
    let node = f
        .app
        .tree
        .nodes
        .get_mut(&f.implementer)
        .expect("implementer");
    node.agent.scope.workspace_root = Some(workspace.clone());
    f.app
        .branches
        .insert(workspace, Some("fix/e2e-round2-tui-detail-rows".into()));
    f.app.rebuild_board();
    for theme in [dark(), light(), ascii()] {
        let dot = theme.glyphs().dot;
        let text = inner_rows(&draw_docked(&f.app, 40, 44, &theme));
        let from = text
            .iter()
            .position(|r| r.starts_with("Model"))
            .expect("a model row");
        assert_eq!(
            text[from..from + 9],
            [
                format!("Model      sonnet-5.5 {dot} medium"),
                format!("Tokens     32k in {dot} 380k cached"),
                "           18.2k out".to_owned(),
                "Cost       $0.38".to_owned(),
                "           $0.51 with children".to_owned(),
                "Workspace  /tmp/ae2e/retest-real/".to_owned(),
                "           workspace".to_owned(),
                "Branch     fix/e2e-round2-tui-detail".to_owned(),
                "           -rows".to_owned(),
            ]
        );
    }
}

#[test]
fn a_short_board_drops_inspector_rows_in_order() {
    let theme = dark();
    let f = mock_app(|f| f.implementer);
    let has = |height: u16, label: &str| {
        inner_rows(&draw_docked(&f.app, 48, height, &theme))
            .iter()
            .any(|r| r.starts_with(label))
    };
    // All 20 inspector rows fit at 36; shorter boards give up parts in order.
    assert!(has(36, "Last activity"));
    assert!(!has(35, "Last activity") && has(35, "Workspace"));
    assert!(!has(32, "Workspace") && has(32, "Tokens"));
    assert!(!has(31, "Tokens") && has(31, "Cost") && has(31, "Children"));
}

#[test]
fn the_cursor_stays_visible_in_a_40_agent_tree() {
    let theme = dark();
    let mut t = TreeBuilder::default();
    let root = t.add(None, "root", "orchestrator", AgentStatus::Running);
    for i in 1..40 {
        t.add(
            Some(root),
            &format!("agent-{i:02}"),
            "worker",
            AgentStatus::Running,
        );
    }
    let mut app = app_with(t.build());
    app.board.focus = BoardFocus::Board;
    let mut scroll = BoardScroll::default();
    let visible = |app: &App, scroll: &mut BoardScroll| {
        let cursor = app.board.cursor.expect("a cursor");
        let name = app.tree.nodes[&cursor].agent.name.clone();
        let buf = draw_with(app, 48, 40, &theme, scroll);
        let (x, y) = locate(&buf, &format!(" {name} "), &name);
        assert!(
            painted(&buf[(x, y)], theme.selected()),
            "{name} not selected"
        );
        assert!(
            painted(&buf[(x, y + 1)], theme.selected()),
            "{name} subtitle cut"
        );
    };
    app.board_end();
    visible(&app, &mut scroll);
    for _ in 0..25 {
        app.board_move(-1);
        visible(&app, &mut scroll);
    }
    app.board_home();
    visible(&app, &mut scroll);
    app.board_move(30);
    visible(&app, &mut scroll);
}

#[test]
fn several_roots_are_separated_and_carry_their_objectives() {
    let mut t = TreeBuilder::default();
    let first = t.add(None, "first", "assistant", AgentStatus::Running);
    let second = t.add(None, "second", "assistant", AgentStatus::Running);
    t.objective(first, "Fix the flaky test");
    t.objective(second, "Write the changelog");
    let app = app_with(t.build());
    let text = inner_rows(&draw_docked(&app, 48, 40, &dark()));
    assert!(text[3].starts_with("● first  assistant"), "{text:#?}");
    assert_eq!(text[4], "  sonnet-5.5 · Fix the flaky test");
    assert_eq!(text[5], "");
    assert!(text[6].starts_with("● second  assistant"), "{text:#?}");
    assert_eq!(text[7], "  sonnet-5.5 · Write the changelog");
}

#[test]
fn an_empty_board_says_so() {
    let app = app_with(Default::default());
    let text = inner_rows(&draw_docked(&app, 48, 20, &dark()));
    assert!(text[1].starts_with("Agents"));
    assert_eq!(text[3], "No agents yet");
}

#[test]
fn labels_are_reformatted_at_most_once_a_second() {
    let theme = dark();
    let mut f = mock_app(|f| f.implementer);
    let mut scroll = BoardScroll::default();
    let aether_row = |buf: &Buffer| {
        rows(buf)
            .into_iter()
            .find(|r| r.contains("● aether"))
            .expect("the root row")
    };
    let first = draw_with(&f.app, 48, 40, &theme, &mut scroll);
    assert!(aether_row(&first).ends_with("6m 02s  $1.12"));
    // Same second, model edited behind the revision's back: the cache holds.
    f.app.now = now() + Duration::milliseconds(500);
    f.app.board.model.rows[0].cost = None;
    let same = draw_with(&f.app, 48, 40, &theme, &mut scroll);
    assert!(aether_row(&same).ends_with("6m 02s  $1.12"));
    f.app.now = now() + Duration::seconds(1);
    let next = draw_with(&f.app, 48, 40, &theme, &mut scroll);
    assert!(
        aether_row(&next).ends_with("6m 03s"),
        "an unknown cost leaves its column blank: {:?}",
        aether_row(&next)
    );
}

#[test]
fn the_running_glyph_pulses_unless_motion_is_reduced() {
    // A pulse needs a probed palette to blend in.
    let theme = light();
    let calm = Theme::resolve(ThemeSpec {
        reduced_motion: true,
        ..ThemeSpec::default()
    });
    let f = mock_app(|f| f.implementer);
    for at in [0, 500, 1_000, 1_500] {
        let t = now() + Duration::milliseconds(at);
        let glyph = status_glyph(BoardStatus::Running, &theme, t);
        assert_eq!(glyph.style, running_style(&theme, t));
        assert_eq!(
            status_glyph(BoardStatus::Running, &calm, t).style,
            calm.running()
        );
    }
    assert_ne!(
        running_style(&theme, now()),
        running_style(&theme, now() + Duration::milliseconds(500))
    );
    assert_eq!(next_frame_in_ms(&f.app, &theme), Some(100));
    assert_eq!(next_frame_in_ms(&f.app, &calm), Some(1_000));
    assert_eq!(next_frame_in_ms(&f.app, &dark()), Some(1_000));
}

#[test]
fn a_settled_or_closed_board_needs_no_frames() {
    let theme = dark();
    let mut t = TreeBuilder::default();
    t.add(
        None,
        "done",
        "assistant",
        AgentStatus::Completed { summary: None },
    );
    let mut app = app_with(t.build());
    assert_eq!(next_frame_in_ms(&app, &theme), None);
    let mut f = fleet();
    f.app.board.open = false;
    assert_eq!(next_frame_in_ms(&f.app, &theme), None);
    app.board.open = true;
    assert_eq!(next_frame_in_ms(&app, &theme), None);
}

#[test]
fn a_failed_agent_shows_its_error_and_recovery_hint() {
    let theme = dark();
    let mut t = TreeBuilder::default();
    let root = t.add(
        None,
        "root",
        "orchestrator",
        AgentStatus::Failed {
            error: "provider refused the request".into(),
        },
    );
    t.took(root, 90);
    let app = app_with(t.build());
    let buf = draw_docked(&app, 48, 30, &theme);
    assert_span(&buf, "✗ Failed", "✗", theme.failure());
    assert_span(&buf, "✗ Failed", "Failed", theme.failure());
    assert_span(
        &buf,
        "provider refused",
        "provider refused the request",
        theme.failure(),
    );
    assert!(rows(&buf).iter().any(|r| r.trim() == "/wake · /restart"));
}

#[test]
fn tree_height_respects_its_share_and_the_inspector_minimum() {
    assert_eq!(tree_height(12, 39), 12);
    assert_eq!(tree_height(80, 39), 21);
    assert_eq!(tree_height(80, 29), 12);
    assert_eq!(tree_height(80, 10), 4);
    assert_eq!(tree_height(80, 3), 3);
}

/// A root waiting on a running child, neither priced.
fn unpriced_pair() -> (App, AgentId, AgentId) {
    let mut t = TreeBuilder::default();
    let root = t.add(None, "assistant", "assistant", AgentStatus::Running);
    let child = t.add(Some(root), "worker", "worker", AgentStatus::Running);
    t.unpriced(root);
    t.unpriced(child);
    t.since(root, 30);
    t.since(child, 20);
    (app_with(t.build()), root, child)
}

#[test]
fn an_unpriced_board_prints_no_cost_and_no_bare_dash() {
    let (app, _, _) = unpriced_pair();
    for theme in [dark(), ascii()] {
        let text = inner_rows(&draw_docked(&app, 48, 40, &theme));
        let dot = theme.glyphs().dot;
        assert!(
            text[1].starts_with("Agents ")
                && text[1].ends_with(&format!(" 2 agents {dot} 2 running")),
            "{:?}",
            text[1]
        );
        assert!(text[3].ends_with(" 30s"), "{:?}", text[3]);
        assert!(text[5].ends_with(" 20s"), "{:?}", text[5]);
        assert!(!text.iter().any(|r| r.starts_with("Cost")), "{text:#?}");
        let children = text
            .iter()
            .position(|r| r == "Children")
            .expect("a children list");
        assert!(text[children + 1].ends_with(" 20s"), "{text:#?}");
        for row in &text {
            assert!(!row.ends_with('—') && !row.ends_with(" -"), "{row:?}");
        }
    }
}

#[test]
fn the_header_says_one_agent_and_appends_a_known_price() {
    let mut t = TreeBuilder::default();
    let root = t.add(None, "assistant", "assistant", AgentStatus::Ready);
    t.cost(root, 0.5);
    let app = app_with(t.build());
    let text = inner_rows(&draw_docked(&app, 48, 30, &dark()));
    assert!(text[1].ends_with("  1 agent · $0.50"), "{:?}", text[1]);
}

#[test]
fn a_priced_child_with_unpriced_parent_shows_only_the_subtree_total() {
    let (mut app, root, child) = unpriced_pair();
    let usage = |usd: f64| aether_core::model::Usage {
        cost_usd: usd,
        ..Default::default()
    };
    app.tree
        .nodes
        .get_mut(&child)
        .expect("child")
        .agent
        .stats
        .usage = usage(0.25);
    app.tree.nodes.get_mut(&root).expect("root").subtree_usage = usage(0.25);
    app.rebuild_board();
    let text = inner_rows(&draw_docked(&app, 48, 40, &dark()));
    assert!(
        text.iter().any(|r| r == "Cost       $0.25 with children"),
        "{text:#?}"
    );
    let root_row = text
        .iter()
        .find(|r| r.contains("● assistant"))
        .expect("the root row");
    assert!(root_row.ends_with("30s"), "{root_row:?}");
}

#[test]
fn an_idle_or_untimed_waiting_board_schedules_no_tick() {
    use aether_core::agent::WaitCondition;
    use aether_core::id::TaskId;
    let theme = dark();
    let mut t = TreeBuilder::default();
    let root = t.add(
        None,
        "assistant",
        "assistant",
        AgentStatus::Waiting {
            condition: WaitCondition::Inbox,
        },
    );
    let waiting = t.add(
        Some(root),
        "worker",
        "worker",
        AgentStatus::Waiting {
            condition: WaitCondition::Task { id: TaskId::new() },
        },
    );
    t.add(Some(root), "queued", "worker", AgentStatus::Ready);
    t.agent(waiting).status_since = None;
    let mut app = app_with(t.build());
    assert_eq!(app.board.model.rows[0].status, BoardStatus::Idle);
    assert_eq!(next_frame_in_ms(&app, &theme), None);
    let text = inner_rows(&draw_docked(&app, 48, 40, &theme));
    assert!(
        text[3].starts_with("○ assistant") && text[3].ends_with("  $0.00"),
        "an idle row shows no clock: {:?}",
        text[3]
    );
    assert!(!text[3].contains("s  $"), "{:?}", text[3]);

    // A waiting clock that is on screen does tick.
    app.tree
        .nodes
        .get_mut(&waiting)
        .expect("worker")
        .agent
        .status_since = Some(now());
    app.rebuild_board();
    assert_eq!(next_frame_in_ms(&app, &theme), Some(1_000));
}

#[test]
fn last_activity_ages_coarsely_and_wakes_only_when_its_text_changes() {
    let theme = dark();
    let mut t = TreeBuilder::default();
    t.add(None, "assistant", "assistant", AgentStatus::Ready);
    let mut app = app_with(t.build());
    let root = app.board.model.rows[0].id;
    push_cell_at(
        &mut app,
        root,
        now() - Duration::seconds(90),
        CellModel::Assistant {
            text: "done".into(),
        },
    );
    assert_eq!(ago_text(12_000), "12s ago");
    assert_eq!(ago_text(90_000), "1m ago");
    assert_eq!(ago_text(7_200_000), "2h ago");
    // 1m 30s in: the text next changes at 2m.
    assert_eq!(next_frame_in_ms(&app, &theme), Some(30_000));
    let text = inner_rows(&draw_docked(&app, 48, 30, &theme));
    assert!(
        text.iter()
            .any(|r| r.starts_with("Last activity") && r.ends_with("1m ago")),
        "{text:#?}"
    );
}

#[test]
fn a_failed_last_activity_is_red_and_a_pending_one_is_not_past_tense() {
    let theme = dark();
    let mut f = mock_app(|f| f.implementer);
    push_cell_at(
        &mut f.app,
        f.implementer,
        now(),
        CellModel::Called {
            target: crate::app::transcript::CallTarget::WebFetch {
                url: "https://example.com/changelog".into(),
            },
            args: String::new(),
            result: String::new(),
            status: CallStatus::Running,
            error: None,
            artifacts: Vec::new(),
        },
    );
    let text = inner_rows(&draw_docked(&f.app, 48, 40, &theme));
    assert!(
        text.iter()
            .any(|r| r == "Fetching https://example.com/changelog"),
        "{text:#?}"
    );
    push_cell_at(
        &mut f.app,
        f.implementer,
        now(),
        CellModel::Exec {
            call: None,
            command: "cargo test".into(),
            status: CallStatus::Failed,
            output: String::new(),
            exit: Some(101),
            duration_ms: None,
            is_search: false,
            artifacts: Vec::new(),
        },
    );
    let buf = draw_docked(&f.app, 48, 40, &theme);
    assert_span(&buf, "Ran cargo test (exit 101)", "Ran", theme.failure());
}

#[test]
fn the_needs_you_line_says_what_the_approval_would_do() {
    let theme = dark();
    let mut f = mock_app(|f| f.docs);
    crate::app::board::fixtures::approval(&mut f.app, f.docs, "cargo test");
    let text = inner_rows(&draw_docked(&f.app, 48, 40, &theme));
    assert!(
        text.iter().any(|r| r == "◆ Wants to run cargo test"),
        "{text:#?}"
    );
    assert!(!text.iter().any(|r| r.contains('{')), "{text:#?}");
}

#[test]
fn same_named_siblings_are_told_apart_on_the_board() {
    let mut t = TreeBuilder::default();
    let root = t.add(None, "aether", "orchestrator", AgentStatus::Running);
    t.add(Some(root), "worker", "worker", AgentStatus::Running);
    t.add(Some(root), "worker", "worker", AgentStatus::Running);
    let app = app_with(t.build());
    let text = inner_rows(&draw_docked(&app, 48, 40, &dark()));
    assert!(text[5].starts_with("├ ● worker  "), "{:?}", text[5]);
    assert!(!text[5].contains("worker  worker"), "{:?}", text[5]);
    assert!(text[7].starts_with("└ ● worker 2  "), "{:?}", text[7]);
}

#[test]
fn a_finished_agent_under_a_second_reads_less_than_one_and_a_live_clock_starts_at_zero() {
    assert_eq!(elapsed_text(BoardStatus::Completed, Some(400)), "<1s");
    assert_eq!(elapsed_text(BoardStatus::Failed, Some(41_000)), "41s");
    assert_eq!(elapsed_text(BoardStatus::Running, Some(400)), "0s");
    assert_eq!(elapsed_text(BoardStatus::Idle, None), "");
}

#[test]
fn long_sibling_names_keep_their_ends_and_the_inspector_shows_them_whole() {
    let name = |n: &str| format!("backend-migration-compatibility-review-of-runs-list-{n}");
    let mut f = mock_app(|f| f.tester);
    for (id, n) in [(f.tester, "07"), (f.docs, "08")] {
        let agent = &mut f.app.tree.nodes.get_mut(&id).expect("a fleet agent").agent;
        agent.name = name(n);
    }
    f.app.rebuild_board();
    for theme in [dark(), ascii()] {
        let text = inner_rows(&draw_docked(&f.app, 48, 40, &theme));
        // Tree rows: the shared middle goes, the number stays.
        for n in ["07", "08"] {
            let row = text[3..13]
                .iter()
                .find(|row| row.contains(&format!("list-{n} ")))
                .unwrap_or_else(|| panic!("-{n} lost in the tree:\n{text:#?}"));
            assert!(row.contains("backend-"), "{row:?}");
        }
        // Inspector head, below the rule and a gap: the whole name wrapped,
        // then role and status.
        let rule = text
            .iter()
            .position(|row| row.starts_with("──") || row.starts_with("--"));
        let head = &text[rule.expect("the inspector rule") + 2..];
        let role = head.iter().position(|row| row.starts_with("verifier"));
        let role = role.unwrap_or_else(|| panic!("no role row:\n{text:#?}"));
        assert!(role > 1, "the name should wrap:\n{text:#?}");
        assert_eq!(head[..role].concat(), name("07"), "{text:#?}");
        assert!(
            head[role].to_lowercase().ends_with("needs you"),
            "{text:#?}"
        );
    }
}

#[test]
fn input_excludes_cache_reads_and_context_stays_unchanged() {
    let mut f = mock_app(|f| f.implementer);
    f.app
        .tree
        .nodes
        .get_mut(&f.implementer)
        .unwrap()
        .agent
        .stats
        .usage
        .cache_write_tokens = 700;
    let text = inner_rows(&draw_docked(&f.app, 56, 44, &dark())).join("\n");
    assert!(text.contains("32k in · 380k cached · 18.2k out"), "{text}");
    assert!(text.contains("82k of 256k until compaction"), "{text}");
}

/// The inspector rows of the implementer on a model with the given profile.
fn context_rows(window: u32, compaction_tokens: Option<u32>, used: u64) -> Vec<String> {
    let mut f = mock_app(|f| f.implementer);
    for info in &mut f.app.models {
        info.capabilities.context_window = window;
        info.capabilities.compaction_tokens = compaction_tokens;
    }
    f.app.context_tokens.insert(f.implementer, used);
    inner_rows(&draw_docked(&f.app, 76, 44, &dark()))
}

fn context_detail_row(rows: &[String]) -> String {
    let at = rows
        .iter()
        .position(|row| row.starts_with("Context"))
        .expect("a context row");
    format!("{} | {}", rows[at].trim_end(), rows[at + 1].trim())
}

#[test]
fn a_claude_context_bar_counts_to_the_256k_threshold_not_the_1m_window() {
    let row = context_detail_row(&context_rows(1_000_000, Some(256_000), 41_300));
    assert!(
        row.ends_with("16% | 41.3k of 256k until compaction · window 1M"),
        "{row}"
    );
}

#[test]
fn a_codex_context_bar_counts_to_the_272k_window() {
    let row = context_detail_row(&context_rows(272_000, Some(272_000), 136_000));
    assert!(
        row.ends_with("50% | 136k of 272k until compaction"),
        "{row}"
    );
}

#[test]
fn a_configured_threshold_sets_the_context_bar() {
    let row = context_detail_row(&context_rows(1_000_000, Some(100_000), 25_000));
    assert!(
        row.ends_with("25% | 25k of 100k until compaction · window 1M"),
        "{row}"
    );
}

#[test]
fn a_model_without_a_threshold_counts_to_75_percent_of_its_window() {
    let row = context_detail_row(&context_rows(400_000, None, 150_000));
    assert!(
        row.ends_with("50% | 150k of 300k until compaction · window 400k"),
        "{row}"
    );
}

#[test]
fn a_context_at_the_threshold_reads_as_compacting() {
    let row = context_detail_row(&context_rows(1_000_000, Some(256_000), 300_000));
    assert!(row.ends_with("100% | at threshold · compacting"), "{row}");
}

#[test]
fn folded_cost_is_the_subtree_total_and_unfolded_cost_is_own() {
    let mut f = mock_app(|f| f.implementer);
    f.app.board_fold();
    let text = inner_rows(&draw_docked(&f.app, 56, 44, &dark()));
    let row = text
        .iter()
        .find(|line| line.contains("implementer") && line.contains("total"))
        .expect("folded total");
    assert!(row.ends_with("$0.51 total"), "{row}");
    f.app.board_unfold();
    let text = inner_rows(&draw_docked(&f.app, 56, 44, &dark()));
    let row = text
        .iter()
        .find(|line| line.contains("implementer") && line.contains("$0.38"))
        .expect("own cost");
    assert!(!row.contains("total"), "{row}");
}

/// Two projects named `aether`, each with children working in the same
/// directory, a sub-directory and an isolated worktree.
fn two_workspaces() -> (App, [AgentId; 5]) {
    let mut t = TreeBuilder::default();
    let main = t.add(None, "main", "assistant", AgentStatus::Running);
    t.workspace(main, "/home/me/src/aether");
    t.objective(main, "Ship workspaces");
    let same = t.add(Some(main), "same", "worker", AgentStatus::Running);
    t.workspace(same, "/home/me/src/aether");
    let sub = t.add(Some(main), "sub", "worker", AgentStatus::Running);
    t.workspace(sub, "/home/me/src/aether/crates/tui");
    let iso = t.add(Some(main), "iso", "worker", AgentStatus::Running);
    t.workspace(iso, "/home/me/.aether/worktrees/agt_1");
    let fork = t.add(None, "fork", "assistant", AgentStatus::Running);
    t.workspace(fork, "/home/me/wt/aether");
    let mut app = app_with(t.build());
    app.options.home = Some("/home/me".into());
    app.rebuild_board();
    (app, [main, same, sub, iso, fork])
}

#[test]
fn workspace_labels_name_roots_and_only_children_that_moved() {
    let (app, _) = two_workspaces();
    let text = inner_rows(&draw_docked(&app, 60, 40, &dark()));
    let detail = |name: &str| {
        let at = text
            .iter()
            .position(|r| r.contains(&format!(" {name}  ")))
            .unwrap_or_else(|| panic!("{name}: {text:#?}"));
        text[at + 1].clone()
    };
    assert_eq!(
        detail("main"),
        "│ sonnet-5.5 · src/aether · Ship workspaces"
    );
    assert_eq!(detail("same"), "│   sonnet-5.5");
    assert_eq!(detail("sub"), "│   sonnet-5.5 · tui");
    assert_eq!(detail("iso"), "    sonnet-5.5 · isolated");
    assert_eq!(detail("fork"), "  sonnet-5.5 · wt/aether");
}

#[test]
fn workspace_labels_give_way_before_the_model_and_right_columns_hold() {
    let (mut app, [main, ..]) = two_workspaces();
    app.tree
        .nodes
        .get_mut(&main)
        .expect("main")
        .agent
        .stats
        .usage
        .cost_usd = 1.5;
    app.rebuild_board();
    for theme in [dark(), ascii()] {
        for width in [board_width(100), 24] {
            let inner = usize::from(width) - 4;
            let text = inner_rows(&draw_docked(&app, width, 40, &theme));
            assert!(text.iter().all(|r| r.chars().count() <= inner), "{text:#?}");
            let at = text.iter().position(|r| r.contains("main")).expect("main");
            assert!(text[at].ends_with("$1.50"), "{:?} at {width}", text[at]);
            assert!(
                text[at + 1].contains("sonnet"),
                "{:?} at {width}",
                text[at + 1]
            );
            if theme.ascii() {
                assert!(text.iter().all(|r| r.is_ascii()), "{text:#?}");
            }
        }
    }
    let wide = inner_rows(&draw_docked(&app, 60, 40, &ascii()));
    assert!(
        wide.iter()
            .any(|r| r.ends_with("sonnet-5.5 | src/aether | Ship workspaces")),
        "{wide:#?}"
    );
}

#[test]
fn priced_tree_preserves_header_and_name_at_docked_width() {
    use aether_core::model::CostBasis;
    let mut f = mock_app(|f| f.implementer);
    let cost = Cost {
        usd: 1.84,
        estimated: true,
        basis: CostBasis::ListPrice,
    };
    f.app.board.model.summary.total_cost = Some(cost);
    for row in &mut f.app.board.model.rows {
        row.cost = Some(cost);
    }
    let text = inner_rows(&draw_docked(&f.app, board_width(160), 44, &dark()));
    assert!(text[1].starts_with("Agents "), "{text:#?}");
    assert!(text[7].contains("implementer"), "{text:#?}");
    assert!(text[7].ends_with("~$1.84 list"), "{text:#?}");
}
