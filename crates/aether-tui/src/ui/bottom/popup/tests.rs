#![cfg(test)]

use ratatui::Terminal;
use ratatui::backend::TestBackend;
use ratatui::buffer::{Buffer, Cell as BufCell};
use ratatui::style::Style;
use ratatui::text::Text;

use super::*;
use crate::ui::theme::tokens::ThemeSpec;

const WIDTHS: [u16; 2] = [80, 120];
const HEIGHT: u16 = 24;
/// The composer's rows at the bottom of the screen.
const COMPOSER: u16 = 3;

fn app() -> App {
    crate::testing::app_with_transcript(&[])
}

fn ascii_theme() -> Theme {
    Theme::resolve(ThemeSpec {
        ascii: true,
        ..Default::default()
    })
}

/// The screen filled with `#`, then the popup painted over it.
fn draw(app: &App, popup: &Popup, theme: &Theme, width: u16) -> Buffer {
    let mut terminal = Terminal::new(TestBackend::new(width, HEIGHT)).unwrap();
    let fill = "#".repeat(usize::from(width));
    let composer = Rect::new(0, HEIGHT - COMPOSER, width, COMPOSER);
    terminal
        .draw(|f| {
            let lines = vec![Line::raw(fill.clone()); usize::from(HEIGHT)];
            f.render_widget(Paragraph::new(Text::from(lines)), f.area());
            render(f, composer, popup, app, theme);
        })
        .unwrap();
    terminal.backend().buffer().clone()
}

fn rows(buffer: &Buffer) -> Vec<String> {
    let width = usize::from(buffer.area.width);
    buffer
        .content()
        .chunks(width)
        .map(|row| row.iter().map(BufCell::symbol).collect::<String>())
        .collect()
}

fn styled(cell: &BufCell, style: Style) -> bool {
    style.fg.is_none_or(|fg| cell.fg == fg)
        && style.bg.is_none_or(|bg| cell.bg == bg)
        && cell.modifier.contains(style.add_modifier)
}

fn slash(query: &str) -> Popup {
    Popup {
        query: query.into(),
        ..Popup::new(PopupKind::Slash)
    }
}

/// The popup's first row on screen.
fn top(popup: &Popup, app: &App) -> usize {
    usize::from(HEIGHT - COMPOSER - desired_height(popup, app))
}

#[test]
fn the_slash_popup_sits_above_the_composer_and_leaves_every_other_row_alone() {
    let app = app();
    let popup = slash("");
    for width in WIDTHS {
        let buffer = draw(&app, &popup, &app.theme, width);
        let rows = rows(&buffer);
        let top = top(&popup, &app);
        assert_eq!(desired_height(&popup, &app), POPUP_MAX_ROWS + 4);
        let fill = "#".repeat(usize::from(width));
        for (y, row) in rows.iter().enumerate() {
            let inside = (top..usize::from(HEIGHT - COMPOSER)).contains(&y);
            assert_eq!(*row == fill, !inside, "row {y}: {row:?}");
        }
        assert_eq!(rows[top].trim_end(), "");
        let first = format!("/{}", COMMANDS[0].name);
        assert!(
            rows[top + 1].starts_with(&format!("  {first} ")),
            "{rows:?}"
        );
        assert_eq!(
            rows[top + 10].trim_end(),
            "  enter run · tab complete · ↑↓ select · esc close"
        );
    }
}

#[test]
fn the_selected_row_is_accented_and_highlighted_across_the_popup() {
    let app = app();
    let theme = app.theme;
    let mut popup = slash("");
    popup.cursor = 1;
    for width in WIDTHS {
        let buffer = draw(&app, &popup, &theme, width);
        let y = u16::try_from(top(&popup, &app)).unwrap() + 2;
        assert!(styled(&buffer[(2, y)], theme.accent()));
        assert!(styled(&buffer[(2, y)], theme.selected()));
        assert!(styled(&buffer[(width - 1, y)], theme.selected()));
        assert!(styled(&buffer[(2, y - 1)], theme.text()));
        assert!(!styled(&buffer[(width - 1, y - 1)], theme.selected()));
        let hints = y + 8;
        assert!(styled(&buffer[(2, hints)], theme.title()));
        assert!(styled(&buffer[(7, hints)], theme.secondary()));
    }
}

#[test]
fn descriptions_are_never_cut_mid_word() {
    let app = app();
    let theme = app.theme;
    for width in [40, 80, 120] {
        for (cursor, spec) in COMMANDS.iter().enumerate() {
            let popup = Popup {
                cursor,
                ..slash("")
            };
            let rows = rows(&draw(&app, &popup, &theme, width));
            let label = format!("  /{} ", spec.name);
            let Some(row) = rows.iter().find(|row| row.starts_with(&label)) else {
                panic!("/{} not shown at {width}: {rows:?}", spec.name);
            };
            let shown = row[label.len()..].trim();
            if shown == spec.about {
                continue;
            }
            let head = shown.strip_suffix(theme.glyphs().ellipsis).unwrap_or("");
            let rest = spec.about.strip_prefix(head).unwrap_or("x");
            assert!(
                head.is_empty() && shown.is_empty() || rest.starts_with(' '),
                "{width}: {shown:?} cuts {:?}",
                spec.about
            );
        }
    }
}

#[test]
fn the_list_scrolls_to_keep_the_cursor_visible() {
    let app = app();
    let last = COMMANDS.len() - 1;
    let popup = Popup {
        cursor: last,
        ..slash("")
    };
    let rows = rows(&draw(&app, &popup, &app.theme, 80));
    let top = top(&popup, &app);
    let name = format!("  /{} ", COMMANDS[last].name);
    assert!(rows[top + usize::from(POPUP_MAX_ROWS)].starts_with(&name));
}

/// The palette says how much it clips, and typing reaches every command.
#[test]
fn a_clipped_list_counts_what_is_below_and_typing_finds_it() {
    let app = app();
    let popup = slash("");
    let total = items(&popup, &app).len();
    let hidden = total - usize::from(POPUP_MAX_ROWS);
    assert!(hidden > 0, "the registry outgrows the popup");
    let buffer = draw(&app, &popup, &app.theme, 80);
    let rows = rows(&buffer);
    let at = top(&popup, &app) + usize::from(POPUP_MAX_ROWS) + 1;
    assert_eq!(
        rows[at].trim_end(),
        format!("  ↓ {hidden} more · type to filter")
    );
    assert!(styled(
        &buffer[(2, u16::try_from(at).unwrap())],
        app.theme.secondary()
    ));
    let ascii = rows_of_ascii(&app, &popup);
    assert_eq!(
        ascii[at].trim_end(),
        format!("  v {hidden} more | type to filter")
    );

    let last = items(&popup, &app).pop().expect("a command");
    let name = match &last {
        PopupItem::Command { name, .. } => *name,
        PopupItem::Mode { name, .. } => name.as_str(),
        other => panic!("{other:?}"),
    };
    let found = items(&slash(name), &app);
    assert!(found.contains(&last), "{name} is reachable by typing");
    let narrowed = rows_of(&app, &slash(name));
    assert!(
        !narrowed.iter().any(|r| r.contains("more · type to filter")),
        "{narrowed:?}"
    );
}

fn rows_of(app: &App, popup: &Popup) -> Vec<String> {
    rows(&draw(app, popup, &app.theme, 80))
}

fn rows_of_ascii(app: &App, popup: &Popup) -> Vec<String> {
    rows(&draw(app, popup, &ascii_theme(), 80))
}

#[test]
fn a_query_without_matches_says_so_in_one_row() {
    let app = app();
    let popup = slash("zzzzqqq");
    assert_eq!(desired_height(&popup, &app), 5);
    let buffer = draw(&app, &popup, &app.theme, 80);
    let rows = rows(&buffer);
    let top = top(&popup, &app);
    assert_eq!(rows[top + 1].trim_end(), "  no matches");
    let y = u16::try_from(top + 1).unwrap();
    assert!(styled(&buffer[(2, y)], app.theme.secondary()));
}

#[test]
fn mention_tabs_mark_the_active_one() {
    let mut app = app();
    let parent = app.selected.unwrap();
    let mut child = app.tree.nodes[&parent].clone();
    child.agent.id = AgentId::new();
    child.agent.name = "reviewer".into();
    child.children.clear();
    app.tree.nodes.insert(child.agent.id, child);
    let theme = app.theme;
    for width in WIDTHS {
        let mut popup = Popup::new(PopupKind::Mention(MentionTab::Agents));
        popup.files = vec!["src/main.rs".into()];
        let buffer = draw(&app, &popup, &theme, width);
        let lines = rows(&buffer);
        let top = top(&popup, &app);
        assert_eq!(lines[top].trim_end(), "  Files · Agents");
        assert!(lines[top + 1].starts_with("  @reviewer  "), "{lines:?}");
        let y = u16::try_from(top).unwrap();
        assert!(styled(&buffer[(2, y)], theme.secondary()));
        assert!(styled(
            &buffer[(10, y)],
            theme.accent().patch(theme.title())
        ));

        popup.kind = PopupKind::Mention(MentionTab::Files);
        let buffer = draw(&app, &popup, &theme, width);
        assert!(styled(&buffer[(2, y)], theme.accent().patch(theme.title())));
        assert!(rows(&buffer)[top + 1].starts_with("  src/main.rs "));
    }
}

#[test]
fn history_search_shows_the_query_and_multi_line_entries_on_one_row() {
    let mut app = app();
    app.input.set_text("first line\nsecond line");
    app.input.take();
    let theme = app.theme;
    let popup = Popup {
        query: "first".into(),
        ..Popup::new(PopupKind::History)
    };
    for width in WIDTHS {
        let rows = rows(&draw(&app, &popup, &theme, width));
        let top = top(&popup, &app);
        assert_eq!(rows[top].trim_end(), "  search history first▍");
        assert_eq!(rows[top + 1].trim_end(), "  first line …");
        assert_eq!(
            rows[top + 3].trim_end(),
            "  enter accept · ctrl+r older · esc cancel"
        );
    }
}

#[test]
fn ascii_mode_paints_only_ascii() {
    let mut app = app();
    app.input.set_text("first line\nsecond line");
    app.input.take();
    let theme = ascii_theme();
    let popups = [
        slash(""),
        Popup::new(PopupKind::Mention(MentionTab::Files)),
        Popup::new(PopupKind::History),
    ];
    for popup in popups {
        for width in WIDTHS {
            for row in rows(&draw(&app, &popup, &theme, width)) {
                assert!(row.is_ascii(), "{row:?}");
            }
        }
    }
    let rows = rows(&draw(&app, &slash(""), &theme, 80));
    let top = top(&slash(""), &app);
    assert_eq!(
        rows[top + 10].trim_end(),
        "  enter run | tab complete | ^v select | esc close"
    );
}

#[test]
fn a_popup_taller_than_the_space_above_is_clipped_not_panicking() {
    let app = app();
    let popup = slash("");
    let mut terminal = Terminal::new(TestBackend::new(80, 6)).unwrap();
    terminal
        .draw(|f| {
            render(f, Rect::new(0, 3, 80, 3), &popup, &app, &app.theme);
        })
        .unwrap();
    let area = area_above(Rect::new(0, 3, 80, 3), desired_height(&popup, &app));
    assert_eq!(area, Rect::new(0, 0, 80, 3));
    assert_eq!(area_above(Rect::new(0, 0, 80, 3), 5).height, 0);
}

#[test]
fn fuzzy_classes_rank_exact_prefix_substring_then_gapped() {
    assert_eq!(fuzzy_match("Model", "model"), Some(Match::Exact));
    assert_eq!(fuzzy_match("models", "mod"), Some(Match::Prefix));
    assert_eq!(fuzzy_match("permissions", "miss"), Some(Match::Substring));
    assert_eq!(fuzzy_match("compact", "cpt"), Some(Match::Fuzzy));
    assert_eq!(fuzzy_match("compact", "tpc"), None);
    assert_eq!(fuzzy_match("anything", ""), Some(Match::Prefix));
    let ranked = ranked([
        ("c", Some(Match::Fuzzy)),
        ("a", None),
        ("b", Some(Match::Exact)),
        ("d", Some(Match::Fuzzy)),
    ]);
    assert_eq!(ranked, ["b", "c", "d"]);
}

#[test]
fn files_rank_a_name_match_above_a_directory_match() {
    let app = app();
    let popup = Popup {
        query: "app".into(),
        files: vec![
            "app/zeta.rs".into(),
            "src/app.rs".into(),
            "readme.md".into(),
        ],
        ..Popup::new(PopupKind::Mention(MentionTab::Files))
    };
    let paths: Vec<String> = items(&popup, &app)
        .into_iter()
        .map(|item| match item {
            PopupItem::File { path } => path,
            other => panic!("{other:?}"),
        })
        .collect();
    assert_eq!(paths, ["src/app.rs", "app/zeta.rs"]);
}

#[test]
fn history_matches_are_newest_first_and_unique() {
    let mut app = app();
    for text in ["run tests", "build", "Run Tests", "run tests"] {
        app.input.set_text(text);
        app.input.take();
    }
    assert_eq!(history_matches(&app, "run"), ["run tests", "Run Tests"]);
    assert_eq!(history_matches(&app, "").len(), 3);
}

#[test]
fn fit_words_cuts_only_between_words() {
    assert_eq!(fit_words("short", 10, "…"), "short");
    assert_eq!(fit_words("switch the model now", 13, "…"), "switch the…");
    assert_eq!(fit_words("switch the model now", 12, "…"), "switch the…");
    assert_eq!(fit_words("switch the model now", 10, "…"), "switch…");
    assert_eq!(fit_words("unbreakable words", 5, "…"), "unbr…");
    assert_eq!(fit_words("unbreakable words", 1, "…"), "");
    assert_eq!(fit_words("a b", 0, "~"), "");
}

#[test]
fn a_host_command_row_shows_its_args_about_and_host() {
    let mut app = app();
    app.options.host_commands = crate::commands::HostCommands::parse(
        r#"{"version":1,"host":"archon","commands":[
            {"name":"answer","args":"<n> <text>","about":"answer question n"}
        ]}"#,
    )
    .unwrap();
    let dot = app.theme.glyphs().dot;
    let rows = rows_of(&app, &slash("answer"));
    let row = rows
        .iter()
        .find(|r| r.contains("/answer"))
        .expect("host row");
    assert_eq!(
        row.trim_end(),
        format!("  /answer <n> <text>  answer question n {dot} archon")
    );
}
