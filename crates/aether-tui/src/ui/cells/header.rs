//! Cell family: session header: `aether` bold + version, cwd, one dim hint line. Owner: W1.
//! Handles `CellModel::Header`.

use std::path::Path;

use ratatui::style::Style;
use ratatui::text::{Line, Span};
use unicode_width::UnicodeWidthStr;

use crate::app::transcript::CellModel;
use crate::format;
use crate::ui::theme::tokens::Theme;

/// Aligns the header with the text column of bulleted cells.
const INDENT: &str = "  ";

pub fn display_lines(cell: &CellModel, width: u16, theme: &Theme) -> Vec<Line<'static>> {
    display_lines_with_keys(cell, width, theme, &crate::keys::KeyBindings::default())
}

pub fn display_lines_with_keys(
    cell: &CellModel,
    width: u16,
    theme: &Theme,
    keys: &crate::keys::KeyBindings,
) -> Vec<Line<'static>> {
    let CellModel::Header {
        version,
        cwd,
        needs_sign_in,
        sign_in_details,
    } = cell
    else {
        unreachable!("header::display_lines called for {cell:?}");
    };
    let glyphs = theme.glyphs();
    let marker = glyphs.ellipsis.chars().next().unwrap_or('~');
    let dot = glyphs.dot;

    let mut rows = vec![
        vec![
            ("aether".to_owned(), theme.title()),
            (format!(" v{version}"), theme.secondary()),
        ],
        vec![(
            format::short_path_with_marker(Path::new(cwd), marker),
            theme.secondary(),
        )],
    ];
    if *needs_sign_in {
        rows.push(vec![
            ("/login".to_owned(), theme.accent()),
            (" to connect a provider".to_owned(), theme.text()),
        ]);
        for detail in sign_in_details {
            rows.push(vec![(detail.clone(), theme.secondary())]);
        }
    }
    rows.push(vec![(
        format!(
            "Ask anything {dot} / commands {dot} {} agents {dot} {} shortcuts",
            keys.display(crate::keys::KeyAction::Agents, theme.ascii()),
            keys.display(crate::keys::KeyAction::Shortcuts, theme.ascii())
        ),
        theme.secondary(),
    )]);

    let room = usize::from(width).saturating_sub(INDENT.width());
    rows.into_iter()
        .map(|parts| fit(parts, room, marker))
        .collect()
}

pub fn detail_lines(cell: &CellModel, width: u16, theme: &Theme) -> Vec<Line<'static>> {
    display_lines(cell, width, theme)
}

pub fn plain_lines(cell: &CellModel, width: u16) -> Vec<String> {
    display_lines(cell, width, &Theme::default())
        .into_iter()
        .map(|line| {
            line.spans
                .iter()
                .map(|span| span.content.as_ref())
                .collect()
        })
        .collect()
}

/// One indented line of `parts`, cut with `marker` where it passes `room`.
fn fit(parts: Vec<(String, Style)>, room: usize, marker: char) -> Line<'static> {
    let mut spans = vec![Span::raw(INDENT)];
    let mut left = room;
    for (text, style) in parts {
        if left == 0 {
            break;
        }
        let shown = format::truncate_with_marker(&text, left, marker);
        left = left.saturating_sub(shown.width());
        // A cut part ends the line; later parts would read as part of it.
        let cut = shown != text;
        spans.push(Span::styled(shown, style));
        if cut {
            break;
        }
    }
    Line::from(spans)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ui::bottom::status_line::tests::{dark_theme, shows};

    fn header(needs_sign_in: bool) -> CellModel {
        CellModel::Header {
            version: "0.9.0".into(),
            cwd: "/work/aether".into(),
            needs_sign_in,
            sign_in_details: Vec::new(),
        }
    }

    fn text(lines: &[Line<'_>]) -> Vec<String> {
        lines
            .iter()
            .map(|line| {
                line.spans
                    .iter()
                    .map(|span| span.content.as_ref())
                    .collect()
            })
            .collect()
    }

    #[test]
    fn wide_terminals_show_name_cwd_and_one_hint_line() {
        let theme = dark_theme(false);
        let expected = [
            "  aether v0.9.0",
            "  /work/aether",
            "  Ask anything · / commands · ctrl+g agents · ? shortcuts",
        ];
        for width in [80, 120] {
            assert_eq!(
                text(&display_lines(&header(false), width, &theme)),
                expected
            );
            assert_eq!(plain_lines(&header(false), width), expected);
        }
    }

    #[test]
    fn ascii_and_sign_in() {
        let theme = dark_theme(true);
        assert_eq!(
            text(&display_lines(&header(true), 80, &theme)),
            [
                "  aether v0.9.0",
                "  /work/aether",
                "  /login to connect a provider",
                "  Ask anything | / commands | ctrl+g agents | ? shortcuts",
            ]
        );
    }

    #[test]
    fn styles_come_from_the_theme() {
        for theme in [dark_theme(false), Theme::default()] {
            let lines = display_lines(&header(true), 120, &theme);
            let style = |row: usize, span: usize| lines[row].spans[span].style;
            assert_eq!(style(0, 1), theme.title());
            assert_eq!(style(0, 2), theme.secondary());
            assert_eq!(style(1, 1), theme.secondary());
            assert_eq!(style(2, 1), theme.accent());
            assert_eq!(style(2, 2), theme.text());
            assert_eq!(style(3, 1), theme.secondary());
            let backgrounds = lines
                .iter()
                .flat_map(|l| &l.spans)
                .any(|s| s.style.bg.is_some());
            assert!(!backgrounds);
        }
    }

    #[test]
    fn narrow_lines_are_cut_with_the_ellipsis() {
        let lines = text(&display_lines(&header(false), 24, &dark_theme(false)));
        assert_eq!(
            lines,
            [
                "  aether v0.9.0",
                "  /work/aether",
                "  Ask anything · / comm…"
            ]
        );
        assert!(lines.iter().all(|line| line.width() <= 24));
        let ascii = text(&display_lines(&header(false), 10, &dark_theme(true)));
        assert_eq!(ascii, ["  aether ~", "  /work/a~", "  Ask any~"]);
    }

    #[test]
    fn a_rendered_header_matches_its_styles() {
        use ratatui::Terminal;
        use ratatui::backend::TestBackend;
        use ratatui::widgets::Paragraph;

        let theme = dark_theme(false);
        let lines = display_lines(&header(false), 80, &theme);
        let mut terminal = Terminal::new(TestBackend::new(80, 3)).expect("terminal");
        terminal
            .draw(|frame| frame.render_widget(Paragraph::new(lines), frame.area()))
            .expect("draw");
        let buffer = terminal.backend().buffer();
        assert!(shows(&buffer[(2, 0)], theme.title()));
        assert!(shows(&buffer[(9, 0)], theme.secondary()));
        assert!(shows(&buffer[(2, 2)], theme.secondary()));
    }
    #[test]
    fn sign_in_hint_displays_daemon_details() {
        let mut cell = header(true);
        let CellModel::Header {
            sign_in_details, ..
        } = &mut cell
        else {
            unreachable!()
        };
        sign_in_details.push("claude-code: /second/claude: status timed out".into());
        let lines = plain_lines(&cell, 120);
        assert!(
            lines
                .iter()
                .any(|line| line.contains("/second/claude: status timed out"))
        );
        assert!(lines.iter().any(|line| line.contains("/login")));
    }
}
