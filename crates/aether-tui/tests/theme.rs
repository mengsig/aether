#![allow(clippy::expect_used, clippy::unwrap_used, clippy::panic)]

//! The theme is the only place colour is chosen, and choosing another theme
//! really repaints. Both are properties the UI can lose silently, so they are
//! checked here rather than trusted.

use aether_client::SubscriptionFrame;
use aether_core::id::InvocationId;
use aether_protocol::Delta;
use aether_tui::app::{App, AppEvent};
use aether_tui::prefs::UiPrefs;
use aether_tui::testing::populated_app;
use aether_tui::ui;
use aether_tui::ui::theme::ColorDepth;
use aether_tui::ui::theme::probe::contrast_ratio;
use aether_tui::ui::theme::tokens::{
    ASCII_GLYPHS, Glyphs, MIN_TEXT_CONTRAST, Rgb, TerminalPalette, Theme, ThemeMode, ThemeSpec,
    UNICODE_GLYPHS, shown_rgb,
};
use aether_tui::ui::transcript::ViewCache;
use aether_tui::ui::views::{self, ViewKind};
use ratatui::Terminal;
use ratatui::backend::TestBackend;
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::Line;
use std::collections::HashSet;

const DARK: TerminalPalette = TerminalPalette {
    fg: Rgb(0xe6, 0xed, 0xf3),
    bg: Rgb(0x0d, 0x11, 0x17),
};
const LIGHT: TerminalPalette = TerminalPalette {
    fg: Rgb(0x1f, 0x23, 0x28),
    bg: Rgb(0xff, 0xff, 0xff),
};

/// Every slash view a person can open over the frame.
const VIEWS: [ViewKind; 13] = [
    ViewKind::Model,
    ViewKind::Usage,
    ViewKind::Approvals,
    ViewKind::Memory,
    ViewKind::Schedules,
    ViewKind::Mcp,
    ViewKind::Doctor,
    ViewKind::Report,
    ViewKind::Artifacts,
    ViewKind::Settings,
    ViewKind::Theme,
    ViewKind::Voice,
    ViewKind::Login,
];

/// Every source file except the theme itself, as (relative path, shipping
/// text). Test-only files and a file's `#[cfg(test)]` tail are excluded: the rule is about what
/// the UI paints, and a test naming a style to assert on is not a call site.
fn sources() -> Vec<(String, String)> {
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
    let mut out = Vec::new();
    let mut stack = vec![root.clone()];
    while let Some(dir) = stack.pop() {
        for entry in std::fs::read_dir(&dir).expect("read src") {
            let path = entry.expect("entry").path();
            if path.is_dir() {
                stack.push(path);
                continue;
            }
            if path.extension().is_some_and(|e| e == "rs") {
                let rel = path
                    .strip_prefix(&root)
                    .expect("under src")
                    .display()
                    .to_string();
                if rel == "ui/theme.rs" || rel.starts_with("ui/theme/") {
                    continue;
                }
                let text = std::fs::read_to_string(&path).expect("read");
                if text.starts_with("#![cfg(test)]\n") {
                    continue;
                }
                let shipping = match text.find("\n#[cfg(test)]") {
                    Some(at) => text[..at].to_owned(),
                    None => text,
                };
                out.push((rel, shipping));
            }
        }
    }
    assert!(out.len() > 5, "expected the TUI sources, found {out:?}");
    out
}

/// Mono *is* modifiers: bold, dim and reverse are how it carries every
/// meaning colour carries elsewhere. So a call site naming a colour, a
/// modifier or a fresh `Style` is making the decision the token table owns.
#[test]
fn no_style_is_named_outside_the_theme() {
    let mut offenders = Vec::new();
    for (path, text) in sources() {
        for (n, line) in text.lines().enumerate() {
            for named in ["Color::", "Modifier::", "Style::"] {
                if line.contains(named) {
                    offenders.push(format!("{path}:{}: {}", n + 1, line.trim()));
                }
            }
        }
    }
    assert!(
        offenders.is_empty(),
        "colour and weight must come from ui::theme, not a call site:\n{}",
        offenders.join("\n")
    );
}

/// A screen with something on it: a question, a reply with markdown and a
/// fenced block, a tool row, a pending approval, text still streaming and
/// the board. The session header paints almost nothing, so a theme test
/// against it proves almost nothing.
fn fixture(view: Option<ViewKind>) -> App {
    let mut app = populated_app();
    app.board.open = true;
    app.reduce(AppEvent::Frame(SubscriptionFrame::Delta {
        agent: app.selected.expect("an agent"),
        invocation: InvocationId::new(),
        delta: Delta::Text {
            text: "a reply with **markdown**, `code` and a list:\n\n- one\n- two\n\n```rust\nfn main() {}\n```\n\n> quoted\n"
                .into(),
        },
    }));
    if let Some(kind) = view {
        let opened = views::open(kind, &app);
        app.bottom.push_view(opened);
    }
    app
}

fn spec(mode: ThemeMode, depth: ColorDepth, palette: Option<TerminalPalette>) -> ThemeSpec {
    ThemeSpec {
        mode,
        depth,
        palette,
        ..ThemeSpec::default()
    }
}

/// Paint `app` in `spec`. Nothing reads the wall clock at draw time, so two
/// draws of one app differ only in what the theme decides.
fn draw(app: &mut App, spec: ThemeSpec) -> Terminal<TestBackend> {
    app.theme = Theme::resolve(spec);
    let mut terminal = Terminal::new(TestBackend::new(120, 40)).expect("terminal");
    terminal
        .draw(|frame| ui::render(frame, app, &mut ViewCache::default()))
        .expect("draw");
    terminal
}

fn foregrounds(terminal: &Terminal<TestBackend>) -> Vec<Color> {
    terminal
        .backend()
        .buffer()
        .content()
        .iter()
        .map(|cell| cell.fg)
        .collect()
}

fn backgrounds(terminal: &Terminal<TestBackend>) -> Vec<Color> {
    terminal
        .backend()
        .buffer()
        .content()
        .iter()
        .map(|cell| cell.bg)
        .collect()
}

fn symbols(terminal: &Terminal<TestBackend>) -> Vec<String> {
    terminal
        .backend()
        .buffer()
        .content()
        .iter()
        .map(|c| c.symbol().to_owned())
        .collect()
}

#[test]
fn dark_and_light_paint_the_same_words_in_different_ink() {
    let mut app = fixture(None);
    let dark = draw(
        &mut app,
        spec(ThemeMode::Dark, ColorDepth::Indexed256, Some(DARK)),
    );
    let light = draw(
        &mut app,
        spec(ThemeMode::Light, ColorDepth::Indexed256, Some(LIGHT)),
    );
    assert_eq!(
        symbols(&dark),
        symbols(&light),
        "the theme must not move text"
    );
    assert_ne!(
        foregrounds(&dark),
        foregrounds(&light),
        "dark and light paint the same"
    );
    assert!(
        foregrounds(&dark)
            .iter()
            .filter(|c| !matches!(c, Color::Reset))
            .count()
            > 50,
        "the fixture must actually paint something"
    );
}

/// `auto` follows the probed background: the same app on a light terminal
/// paints what `light` paints.
#[test]
fn auto_follows_the_probed_background() {
    let mut app = fixture(None);
    let auto = draw(
        &mut app,
        spec(ThemeMode::Auto, ColorDepth::Indexed256, Some(LIGHT)),
    );
    let light = draw(
        &mut app,
        spec(ThemeMode::Light, ColorDepth::Indexed256, Some(LIGHT)),
    );
    assert_eq!(foregrounds(&auto), foregrounds(&light));
    assert_eq!(backgrounds(&auto), backgrounds(&light));
}

/// Mono's claim is that it uses no colour anywhere, on every view over a
/// populated screen, whatever the terminal reported. The one exception is
/// `/theme`: unless `NO_COLOR` chose mono, the swatch dots on its theme rows
/// preview each theme in its own colours. Only those dots' ink may be
/// coloured; every other cell, and every background, stays colourless.
#[test]
fn mono_paints_no_colour_on_any_screen() {
    for palette in [None, Some(DARK)] {
        for view in std::iter::once(None).chain(VIEWS.map(Some)) {
            let mut app = fixture(view);
            let mono = draw(
                &mut app,
                spec(ThemeMode::Mono, ColorDepth::Truecolor, palette),
            );
            let swatches = if view == Some(ViewKind::Theme) {
                swatch_dots(&mono, app.theme.glyphs().running)
            } else {
                HashSet::new()
            };
            let coloured: Vec<Color> = foregrounds(&mono)
                .into_iter()
                .enumerate()
                .filter(|(at, _)| !swatches.contains(at))
                .map(|(_, fg)| fg)
                .chain(backgrounds(&mono))
                .filter(|c| !matches!(c, Color::Reset))
                .collect();
            assert!(
                coloured.is_empty(),
                "mono painted {} coloured cells over {view:?} (palette {palette:?}), e.g. {:?}",
                coloured.len(),
                &coloured[..coloured.len().min(5)]
            );
        }
    }
}

/// Buffer indices of `/theme` swatch dots: runs of seven `dot`s one space
/// apart, as `views::theme` lays a swatch out. Nothing else on a screen looks so.
fn swatch_dots(terminal: &Terminal<TestBackend>, dot: &str) -> HashSet<usize> {
    const DOTS: usize = 7;
    let buffer = terminal.backend().buffer();
    let width = usize::from(buffer.area.width);
    let mut found = HashSet::new();
    for (y, row) in buffer.content().chunks(width).enumerate() {
        for x in 0..width.saturating_sub(DOTS * 2 - 2) {
            let is_swatch = row[x..x + DOTS * 2 - 1]
                .iter()
                .enumerate()
                .all(|(i, cell)| cell.symbol() == if i % 2 == 0 { dot } else { " " });
            if is_swatch {
                found.extend((0..DOTS).map(|i| y * width + x + i * 2));
            }
        }
    }
    found
}

/// `NO_COLOR` picks mono unless `--theme` says otherwise, and mono is colourless.
#[test]
fn no_color_paints_mono() {
    let prefs = UiPrefs {
        theme: Some("nord".parse().unwrap()),
        ..UiPrefs::default()
    };
    let chosen = prefs.theme(None, Some("1"));
    assert_eq!(chosen.mode, ThemeMode::Mono);
    let mut app = fixture(None);
    let painted = draw(
        &mut app,
        spec(chosen.mode, ColorDepth::Truecolor, Some(DARK)),
    );
    assert!(
        foregrounds(&painted)
            .into_iter()
            .chain(backgrounds(&painted))
            .all(|c| c == Color::Reset)
    );
    let flagged = prefs.theme(Some("dark".parse().unwrap()), Some("1"));
    assert_eq!(flagged.mode, ThemeMode::Dark);
}

#[test]
fn truecolor_terminals_get_rgb() {
    let mut app = fixture(None);
    let hex = draw(
        &mut app,
        spec(ThemeMode::Dark, ColorDepth::Truecolor, Some(DARK)),
    );
    assert!(
        foregrounds(&hex)
            .iter()
            .any(|c| matches!(c, Color::Rgb(..))),
        "a truecolor terminal should be painted in rgb"
    );
    let indexed = draw(
        &mut app,
        spec(ThemeMode::Dark, ColorDepth::Indexed256, Some(DARK)),
    );
    assert!(
        foregrounds(&indexed)
            .iter()
            .all(|c| !matches!(c, Color::Rgb(..))),
        "a 256-colour terminal was sent rgb"
    );
}

/// SPEC §5.2: no token paints a background except `tint`, so every cell's
/// background is the terminal's own or exactly the tint, over every view.
#[test]
fn only_the_tint_paints_a_background() {
    for (mode, palette) in [
        (ThemeMode::Dark, Some(DARK)),
        (ThemeMode::Light, Some(LIGHT)),
        (ThemeMode::Dark, None),
    ] {
        for view in std::iter::once(None).chain(VIEWS.map(Some)) {
            let mut app = fixture(view);
            let painted = draw(&mut app, spec(mode, ColorDepth::Truecolor, palette));
            let tint = app.theme.tint().bg;
            let stray: Vec<Color> = backgrounds(&painted)
                .into_iter()
                .filter(|bg| *bg != Color::Reset && Some(*bg) != tint)
                .collect();
            assert!(
                stray.is_empty(),
                "{mode} {view:?} palette={palette:?}: {} cells painted a non-tint background, e.g. {:?}",
                stray.len(),
                &stray[..stray.len().min(5)]
            );
            if palette.is_none() {
                assert_eq!(tint, None, "an unknown palette has no tint to blend");
            }
        }
    }
    // The tint is in use somewhere: the user block and the composer sit on it.
    let mut app = fixture(None);
    let painted = draw(
        &mut app,
        spec(ThemeMode::Dark, ColorDepth::Truecolor, Some(DARK)),
    );
    let tint = app.theme.tint().bg.expect("a known palette tints");
    assert!(backgrounds(&painted).contains(&tint));
}

/// SPEC §5.1: the accent stays readable (≥ 4.5:1) on the terminal it lands on,
/// even a background chosen to defeat Iris.
#[test]
fn the_accent_is_readable_on_every_probed_background() {
    let grey = TerminalPalette {
        fg: Rgb(0, 0, 0),
        bg: Rgb(0x80, 0x88, 0xc0),
    };
    for palette in [DARK, LIGHT, grey] {
        for depth in [ColorDepth::Truecolor, ColorDepth::Indexed256] {
            for mode in [ThemeMode::Auto, ThemeMode::Dark, ThemeMode::Light] {
                let theme = Theme::resolve(spec(mode, depth, Some(palette)));
                let fg = theme.accent().fg.expect("the accent names a colour");
                let rgb = shown_rgb(fg).expect("a fixed colour at this depth");
                let ratio = contrast_ratio(rgb, palette.bg);
                assert!(
                    ratio >= MIN_TEXT_CONTRAST,
                    "{mode} {depth:?} on {:?}: {fg:?} is {ratio:.2}:1",
                    palette.bg
                );
            }
        }
    }
    // Sixteen colours have no Iris: magenta stands in.
    let basic = Theme::resolve(spec(ThemeMode::Dark, ColorDepth::Basic16, Some(DARK)));
    assert_eq!(basic.accent().fg, Some(Color::Magenta));
}

/// Without a palette, blends are impossible: the tint drops out, the pulse
/// holds at the running hue and the selection falls back to an underline.
#[test]
fn an_unknown_palette_drops_the_blends() {
    let theme = Theme::resolve(spec(ThemeMode::Dark, ColorDepth::Truecolor, None));
    assert_eq!(theme.tint(), Style::new());
    assert!(theme.selected().add_modifier.contains(Modifier::UNDERLINED));
    assert_eq!(theme.pulse(0.0), theme.running());
    assert_eq!(theme.pulse(1.0), theme.running());
    assert_eq!(theme.shimmer(0.5), theme.secondary());

    let known = Theme::resolve(spec(ThemeMode::Dark, ColorDepth::Truecolor, Some(DARK)));
    assert!(known.tint().bg.is_some());
    assert_eq!(known.selected(), known.tint());
    assert_ne!(known.pulse(0.0), known.pulse(1.0), "the pulse must sweep");

    let still = Theme::resolve(ThemeSpec {
        reduced_motion: true,
        ..spec(ThemeMode::Dark, ColorDepth::Truecolor, Some(DARK))
    });
    assert_eq!(
        still.pulse(0.0),
        still.pulse(1.0),
        "reduced motion holds still"
    );
}

/// Markdown emphasis is weight and slant only, patched over whatever colour
/// is in force, so it reads the same in every mode and depth.
#[test]
fn emphasis_tokens_are_modifiers_only() {
    for mode in ThemeMode::ALL {
        for palette in [None, Some(DARK)] {
            let theme = Theme::resolve(spec(mode, ColorDepth::Truecolor, palette));
            for (style, modifier) in [
                (theme.strong(), Modifier::BOLD),
                (theme.emphasis(), Modifier::ITALIC),
                (theme.underline(), Modifier::UNDERLINED),
                (theme.strike(), Modifier::CROSSED_OUT),
            ] {
                assert_eq!(style, Style::new().add_modifier(modifier), "{mode}");
            }
            assert_eq!(theme.aside(), theme.secondary().patch(theme.emphasis()));
            for style in [
                theme.text(),
                theme.secondary(),
                theme.title(),
                theme.accent(),
                theme.success(),
                theme.failure(),
                theme.track(),
                theme.rule(),
                theme.pulse(0.5),
            ] {
                assert_eq!(style.bg, None, "{mode}: only the tint has a background");
            }
        }
    }
}

/// SPEC §5.2's ASCII column, glyph by glyph.
#[test]
fn the_ascii_glyph_table_matches_the_spec() {
    let pairs = |g: &Glyphs| {
        [
            g.prompt,
            g.bullet,
            g.child,
            g.pipe,
            g.tee,
            g.bar,
            g.running,
            g.waiting,
            g.idle,
            g.needs_you,
            g.done,
            g.failed,
            g.paused,
            g.cancelled,
            g.queued,
            g.caret,
        ]
    };
    assert_eq!(
        pairs(&UNICODE_GLYPHS),
        [
            "›", "•", "└", "│", "├", "━", "●", "◌", "○", "◆", "✓", "✗", "‖", "⊘", "↪", "▍"
        ]
    );
    assert_eq!(
        pairs(&ASCII_GLYPHS),
        [
            ">", "*", "\\", "|", "|-", "=", "*", "~", "o", "!", "+", "x", "=", "-", ">", "|"
        ]
    );
    let all = |g: &Glyphs| {
        [
            g.prompt,
            g.bullet,
            g.child,
            g.pipe,
            g.tee,
            g.bar,
            g.rule,
            g.running,
            g.waiting,
            g.idle,
            g.needs_you,
            g.blocked,
            g.paused,
            g.done,
            g.failed,
            g.cancelled,
            g.queued,
            g.caret,
            g.ellipsis,
            g.dot,
        ]
    };
    for glyph in all(&ASCII_GLYPHS) {
        assert!(!glyph.is_empty() && glyph.is_ascii(), "{glyph:?}");
    }
    for glyph in all(&UNICODE_GLYPHS) {
        assert!(!glyph.is_empty(), "an empty unicode glyph");
    }

    let unicode = Theme::resolve(ThemeSpec::default());
    let ascii = Theme::resolve(ThemeSpec {
        ascii: true,
        ..ThemeSpec::default()
    });
    assert_eq!(*unicode.glyphs(), UNICODE_GLYPHS);
    assert_eq!(*ascii.glyphs(), ASCII_GLYPHS);
    assert!(!unicode.ascii() && ascii.ascii());
    assert_eq!(unicode.decoration("▎ ", "| "), "▎ ");
    assert_eq!(ascii.decoration("▎ ", "| "), "| ");
}

fn rendered(source: &str, theme: &Theme) -> Vec<Line<'static>> {
    ui::markdown::render(source, 60, theme.text(), theme)
}

fn span_style(lines: &[Line<'_>], text: &str) -> Style {
    lines
        .iter()
        .flat_map(|l| l.spans.iter())
        .find(|s| s.content == text)
        .unwrap_or_else(|| panic!("no span {text:?} in {lines:?}"))
        .style
}

fn row(lines: &[Line<'_>], containing: &str) -> String {
    lines
        .iter()
        .map(|l| {
            l.spans
                .iter()
                .map(|s| s.content.as_ref())
                .collect::<String>()
        })
        .find(|r| r.contains(containing))
        .unwrap_or_else(|| panic!("no row with {containing:?} in {lines:?}"))
}

/// SPEC §3.1 markdown: headings in their hue, H1 underlined, H3 italic; code
/// spans and links in their own tokens; dim list markers and quote bars.
#[test]
fn markdown_paints_through_the_tokens() {
    let source = "# One\n\n## Two\n\n### Three\n\nplain *em* **strong** ~~gone~~ `code` [link](https://example.com)\n\n- item\n\n1. first\n\n> quoted\n";
    for spec in [
        spec(ThemeMode::Dark, ColorDepth::Truecolor, Some(DARK)),
        spec(ThemeMode::Light, ColorDepth::Indexed256, Some(LIGHT)),
        spec(ThemeMode::Dark, ColorDepth::Basic16, None),
        spec(ThemeMode::Mono, ColorDepth::Truecolor, None),
    ] {
        for ascii in [false, true] {
            let theme = Theme::resolve(ThemeSpec { ascii, ..spec });
            let lines = rendered(source, &theme);
            let heading = theme.heading();
            assert_eq!(span_style(&lines, "One"), heading.patch(theme.underline()));
            assert_eq!(span_style(&lines, "Two"), heading);
            assert_eq!(span_style(&lines, "Three"), heading.patch(theme.emphasis()));
            assert_eq!(span_style(&lines, "plain "), theme.text());
            assert_eq!(span_style(&lines, "em"), theme.emphasis());
            assert_eq!(span_style(&lines, "strong"), theme.strong());
            assert_eq!(span_style(&lines, "gone"), theme.strike());
            assert_eq!(span_style(&lines, "code"), theme.code());
            assert_eq!(span_style(&lines, "link"), theme.link());
            assert_eq!(span_style(&lines, "- "), theme.secondary());
            assert_eq!(span_style(&lines, "1. "), theme.secondary());
            let bar = theme.decoration("▎ ", "| ");
            assert_eq!(span_style(&lines, bar), theme.secondary());
            assert!(row(&lines, "quoted").starts_with(bar));
            assert!(row(&lines, "item").starts_with("- "));
            assert!(row(&lines, "https://example.com").contains("link"));
        }
    }
}
