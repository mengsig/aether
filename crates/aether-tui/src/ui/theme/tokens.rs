//! The redesign's token table (SPEC §5.2) and glyph table (§5.2 ASCII mode).
//!
//! Every new surface paints through [`Theme`]; no call site names a colour.
//! Owner: W1.

use std::fmt;
use std::str::FromStr;

use ratatui::style::{Color, Modifier, Style};

use super::ColorDepth;
use super::colors::basic16;
use super::probe::{background_of, blend, contrast_ratio};
use super::scheme::{HUES, Hue, HuePatch, Hues, Scheme};

/// `/settings` theme choice. `dusk`/`dawn` parse as aliases of dark/light.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ThemeMode {
    /// Probe-driven: light/dark from the terminal background.
    #[default]
    Auto,
    Dark,
    Light,
    /// Weight, dim and reverse only; also what `NO_COLOR` selects.
    Mono,
}

impl ThemeMode {
    pub const ALL: [ThemeMode; 4] = [
        ThemeMode::Auto,
        ThemeMode::Dark,
        ThemeMode::Light,
        ThemeMode::Mono,
    ];

    pub fn as_str(self) -> &'static str {
        match self {
            ThemeMode::Auto => "auto",
            ThemeMode::Dark => "dark",
            ThemeMode::Light => "light",
            ThemeMode::Mono => "mono",
        }
    }
}

impl fmt::Display for ThemeMode {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

impl FromStr for ThemeMode {
    type Err = ();

    fn from_str(s: &str) -> Result<Self, ()> {
        match s.trim().to_ascii_lowercase().as_str() {
            "auto" => Ok(ThemeMode::Auto),
            "dark" | "dusk" => Ok(ThemeMode::Dark),
            "light" | "dawn" => Ok(ThemeMode::Light),
            "mono" | "monochrome" => Ok(ThemeMode::Mono),
            _ => Err(()),
        }
    }
}

/// Which way the terminal leans, after the probe or its fallbacks.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Background {
    #[default]
    Dark,
    Light,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Rgb(pub u8, pub u8, pub u8);

/// What OSC 10/11 reported. `None` in [`ThemeSpec`] means the probe failed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TerminalPalette {
    pub fg: Rgb,
    pub bg: Rgb,
}

/// Everything [`Theme::resolve`] needs; built once at startup and on `/theme`.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct ThemeSpec {
    pub mode: ThemeMode,
    pub depth: ColorDepth,
    pub palette: Option<TerminalPalette>,
    pub ascii: bool,
    pub reduced_motion: bool,
    /// The chosen theme's hues, a user theme file already applied.
    pub scheme: Scheme,
    /// `[theme]` colours from the overrides file; they win over any theme.
    pub overrides: HuePatch,
    /// `COLORFGBG`'s guess (`probe::background_of(None, ..)`); `auto` uses it
    /// only when the probe failed.
    pub background_hint: Background,
}

/// §5.2 tokens, resolved to styles. Only `tint` and `selection` carry a background.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Tokens {
    pub text: Style,
    pub secondary: Style,
    pub title: Style,
    /// One style per [`Hue`], foreground only.
    pub hues: [Style; HUES],
    /// Commands and paths: the foreground leaning toward the code hue.
    pub command: Style,
    /// Background-only style; `None` when the palette is unknown.
    pub tint: Option<Color>,
    pub selection: Style,
    pub track: Style,
    pub rule: Style,
    /// Ends of the running pulse (dim running → running); `None` holds it still.
    pub pulse: Option<(Rgb, Rgb)>,
    /// Ends of the status-row shimmer (quiet → running).
    pub shimmer: Option<(Rgb, Rgb)>,
}

/// Which family a tool call belongs to; each paints its verb in its own hue.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ToolKind {
    Shell,
    Read,
    Edit,
    Web,
    Call,
    Agent,
}

/// One table for every authored glyph, so `--ascii` is a table swap.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Glyphs {
    pub prompt: &'static str,
    pub bullet: &'static str,
    pub child: &'static str,
    pub pipe: &'static str,
    pub tee: &'static str,
    pub bar: &'static str,
    pub rule: &'static str,
    pub running: &'static str,
    pub waiting: &'static str,
    pub idle: &'static str,
    pub needs_you: &'static str,
    pub blocked: &'static str,
    pub paused: &'static str,
    pub done: &'static str,
    pub failed: &'static str,
    pub cancelled: &'static str,
    pub queued: &'static str,
    pub caret: &'static str,
    pub ellipsis: &'static str,
    pub dot: &'static str,
}

pub const UNICODE_GLYPHS: Glyphs = Glyphs {
    prompt: "›",
    bullet: "•",
    child: "└",
    pipe: "│",
    tee: "├",
    bar: "━",
    rule: "─",
    running: "●",
    waiting: "◌",
    idle: "○",
    needs_you: "◆",
    blocked: "◌",
    paused: "‖",
    done: "✓",
    failed: "✗",
    cancelled: "⊘",
    queued: "↪",
    caret: "▍",
    ellipsis: "…",
    dot: "·",
};

pub const ASCII_GLYPHS: Glyphs = Glyphs {
    prompt: ">",
    bullet: "*",
    child: "\\",
    pipe: "|",
    tee: "|-",
    bar: "=",
    rule: "-",
    running: "*",
    waiting: "~",
    idle: "o",
    needs_you: "!",
    blocked: "?",
    paused: "=",
    done: "+",
    failed: "x",
    cancelled: "-",
    queued: ">",
    caret: "|",
    ellipsis: "~",
    dot: "|",
};

/// Iris (SPEC §5.1).
pub const IRIS_DARK: Rgb = Rgb(0xA5, 0xB4, 0xFC);
pub const IRIS_LIGHT: Rgb = Rgb(0x4F, 0x46, 0xE5);
/// WCAG AA for body text.
pub const MIN_TEXT_CONTRAST: f64 = 4.5;

const BLACK: Rgb = Rgb(0, 0, 0);
const WHITE: Rgb = Rgb(255, 255, 255);
/// How finely a hue walks toward black/white to reach contrast.
const READABLE_STEPS: u16 = 64;
/// How much of the running hue is left at the dim end of its pulse.
const PULSE_DIM: f32 = 0.45;
/// How far commands and paths lean from the foreground toward the code hue.
const COMMAND_TINT: f32 = 0.3;
/// How much accent shows through a selection's background.
const SELECTION_ALPHA: f32 = 0.3;

/// A resolved theme: tokens plus glyphs. Cheap to copy; rendering never does
/// colour arithmetic beyond [`Theme::pulse`].
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Theme {
    spec: ThemeSpec,
    background: Background,
    tokens: Tokens,
    glyphs: &'static Glyphs,
}

impl Default for Theme {
    fn default() -> Self {
        Theme::resolve(ThemeSpec::default())
    }
}

impl Theme {
    /// The §5.2 table for `spec`. Blends need the probed palette and a depth
    /// above ANSI-16; without them tint and pulse drop out.
    pub fn resolve(spec: ThemeSpec) -> Theme {
        let background = match spec.mode {
            ThemeMode::Dark => Background::Dark,
            ThemeMode::Light => Background::Light,
            ThemeMode::Auto | ThemeMode::Mono => match spec.palette {
                Some(palette) => background_of(Some(palette), None),
                None => spec.background_hint,
            },
        };
        let tokens = match spec.mode {
            ThemeMode::Mono => mono_tokens(),
            _ => color_tokens(spec, background),
        };
        Theme {
            spec,
            background,
            tokens,
            glyphs: if spec.ascii {
                &ASCII_GLYPHS
            } else {
                &UNICODE_GLYPHS
            },
        }
    }

    pub fn spec(&self) -> ThemeSpec {
        self.spec
    }

    pub fn background(&self) -> Background {
        self.background
    }

    pub fn glyphs(&self) -> &'static Glyphs {
        self.glyphs
    }

    /// `unicode`, or `ascii` when decoration is ASCII-only.
    pub fn decoration<'a>(&self, unicode: &'a str, ascii: &'a str) -> &'a str {
        if self.ascii() { ascii } else { unicode }
    }

    pub fn ascii(&self) -> bool {
        self.spec.ascii
    }

    pub fn reduced_motion(&self) -> bool {
        self.spec.reduced_motion
    }

    pub fn text(&self) -> Style {
        self.tokens.text
    }

    pub fn secondary(&self) -> Style {
        self.tokens.secondary
    }

    pub fn title(&self) -> Style {
        self.tokens.title
    }

    /// The resolved style of one [`Hue`], foreground only.
    pub fn hue(&self, hue: Hue) -> Style {
        self.tokens.hues[hue as usize]
    }

    pub fn accent(&self) -> Style {
        self.hue(Hue::Accent)
    }

    pub fn success(&self) -> Style {
        self.hue(Hue::Success)
    }

    pub fn failure(&self) -> Style {
        self.hue(Hue::Failure)
    }

    /// Something running, held still; [`Theme::pulse`] is its animated form.
    pub fn running(&self) -> Style {
        self.hue(Hue::Running)
    }

    pub fn waiting(&self) -> Style {
        self.hue(Hue::Waiting)
    }

    /// Needs you: an approval, a question, an agent waiting on you.
    pub fn attention(&self) -> Style {
        self.hue(Hue::Attention)
    }

    pub fn blocked(&self) -> Style {
        self.hue(Hue::Blocked)
    }

    /// Paused or cancelled: out of play, so quiet.
    pub fn paused(&self) -> Style {
        self.tokens.secondary
    }

    /// A tool call's verb ("Ran", "Read", "Edited", "Called", …).
    pub fn tool(&self, kind: ToolKind) -> Style {
        self.hue(match kind {
            ToolKind::Shell => Hue::Shell,
            ToolKind::Read => Hue::Read,
            ToolKind::Edit => Hue::Edit,
            ToolKind::Web => Hue::Web,
            ToolKind::Call => Hue::Call,
            ToolKind::Agent => Hue::Agent,
        })
    }

    /// The user's prompt marker.
    pub fn user(&self) -> Style {
        self.hue(Hue::User)
    }

    /// Inline code and code blocks.
    pub fn code(&self) -> Style {
        self.hue(Hue::Code)
    }

    /// Commands and paths a tool call names: text with a hint of the code hue.
    pub fn command(&self) -> Style {
        self.tokens.command
    }

    pub fn link(&self) -> Style {
        self.hue(Hue::Link).add_modifier(Modifier::UNDERLINED)
    }

    /// File targets retain error, search and code colours from their content.
    pub fn file_reference(&self) -> Style {
        Style::default().add_modifier(Modifier::UNDERLINED)
    }

    pub fn heading(&self) -> Style {
        self.hue(Hue::Heading).add_modifier(Modifier::BOLD)
    }

    pub fn diff_add(&self) -> Style {
        self.hue(Hue::DiffAdd)
    }

    pub fn diff_remove(&self) -> Style {
        self.hue(Hue::DiffRemove)
    }

    /// The model's reasoning: an aside.
    pub fn reasoning(&self) -> Style {
        self.aside()
    }

    /// Mouse selection: an accent-washed background, or reversed without one.
    pub fn selection(&self) -> Style {
        self.tokens.selection
    }

    pub fn track(&self) -> Style {
        self.tokens.track
    }

    pub fn rule(&self) -> Style {
        self.tokens.rule
    }

    /// Markdown emphasis: modifiers only, patched over whatever colour is in
    /// force, so they read the same in every mode and depth.
    pub fn strong(&self) -> Style {
        Style::new().add_modifier(Modifier::BOLD)
    }

    pub fn emphasis(&self) -> Style {
        Style::new().add_modifier(Modifier::ITALIC)
    }

    pub fn underline(&self) -> Style {
        Style::new().add_modifier(Modifier::UNDERLINED)
    }

    pub fn strike(&self) -> Style {
        Style::new().add_modifier(Modifier::CROSSED_OUT)
    }

    /// Asides (reasoning, a code fence's language): `secondary`, in italic.
    pub fn aside(&self) -> Style {
        self.tokens.secondary.patch(self.emphasis())
    }

    /// Background for user/composer/approval blocks; plain when the palette is unknown.
    pub fn tint(&self) -> Style {
        match self.tokens.tint {
            Some(bg) => Style::new().bg(bg),
            None => Style::new(),
        }
    }

    /// Cursor/selected row: the tint, or an underline when there is no tint;
    /// a reversed block would outweigh everything else in mono.
    pub fn selected(&self) -> Style {
        match self.tokens.tint {
            Some(bg) => Style::new().bg(bg),
            None => Style::new().add_modifier(Modifier::UNDERLINED),
        }
    }

    /// Find: the current match is accented, bold, underlined and reversed; other hits
    /// lie on the tint, or are underlined when there is none.
    pub fn find_hit(&self, current: bool) -> Style {
        match (current, self.tokens.tint) {
            (true, _) => self
                .accent()
                .add_modifier(Modifier::BOLD | Modifier::UNDERLINED | Modifier::REVERSED),
            (false, Some(bg)) => Style::new().bg(bg),
            (false, None) => Style::new().add_modifier(Modifier::UNDERLINED),
        }
    }

    /// Whether [`Theme::pulse`] and [`Theme::shimmer`] move at all: frames
    /// scheduled for a still pulse would only repaint the same colours.
    pub fn animates(&self) -> bool {
        self.tokens.pulse.is_some() && !self.spec.reduced_motion
    }

    /// The running hue at `phase` (0.0 = its dim end, 1.0 = full); held at
    /// [`Theme::running`] without a palette, at ANSI-16 or with reduced motion.
    pub fn pulse(&self, phase: f32) -> Style {
        match self.tokens.pulse {
            Some((dim, bright)) if !self.spec.reduced_motion => {
                Style::new().fg(best_color(blend(bright, dim, phase), self.spec.depth))
            }
            _ => self.running(),
        }
    }

    /// The status-row shimmer at `phase` (0.0 = quiet, 1.0 = the running hue);
    /// static `secondary` whenever [`Theme::pulse`] holds still.
    pub fn shimmer(&self, phase: f32) -> Style {
        match self.tokens.shimmer {
            Some((quiet, bright)) if !self.spec.reduced_motion => {
                Style::new().fg(best_color(blend(bright, quiet, phase), self.spec.depth))
            }
            _ => self.tokens.secondary,
        }
    }
}

fn mono_tokens() -> Tokens {
    let dim = Style::new().add_modifier(Modifier::DIM);
    let bold = Style::new().add_modifier(Modifier::BOLD);
    let hues = Hue::ALL.map(|hue| match hue {
        Hue::Accent | Hue::Running | Hue::Attention | Hue::Failure | Hue::Code => bold,
        _ => Style::new(),
    });
    Tokens {
        text: Style::new(),
        secondary: dim,
        title: bold,
        hues,
        command: Style::new(),
        tint: None,
        selection: Style::new().add_modifier(Modifier::REVERSED),
        track: dim,
        rule: dim,
        pulse: None,
        shimmer: None,
    }
}

fn color_tokens(spec: ThemeSpec, background: Background) -> Tokens {
    let dim = Style::new().add_modifier(Modifier::DIM);
    let hues = spec.scheme.hues(background);
    let rgbs = Hue::ALL.map(|hue| hue_rgb(spec, &hues, hue));
    let colors = Hue::ALL.map(|hue| match (spec.depth, spec.overrides.get(hue)) {
        // ANSI-16 shows the terminal's own palette, which is its theme.
        (ColorDepth::Basic16, None) => hue.ansi16(),
        _ => Some(best_color(rgbs[hue as usize], spec.depth)),
    });
    let hue_styles = colors.map(|color| match color {
        Some(color) => Style::new().fg(color),
        None => Style::new(),
    });
    let blendable = spec.palette.filter(|_| spec.depth != ColorDepth::Basic16);
    let fg = |rgb| Style::new().fg(best_color(rgb, spec.depth));
    let base = Tokens {
        text: Style::new(),
        secondary: dim,
        title: Style::new().add_modifier(Modifier::BOLD),
        hues: hue_styles,
        command: Style::new(),
        tint: None,
        selection: Style::new().add_modifier(Modifier::REVERSED),
        track: dim,
        rule: dim,
        pulse: None,
        shimmer: None,
    };
    let Some(p) = blendable else {
        return base;
    };
    let quiet = blend(p.fg, p.bg, 0.55);
    let running = rgbs[Hue::Running as usize];
    let command = readable_rgb(
        blend(rgbs[Hue::Code as usize], p.fg, COMMAND_TINT),
        p.bg,
        spec.depth,
    );
    Tokens {
        secondary: fg(quiet),
        command: fg(command),
        tint: Some(band_color(p.bg, spec.depth, background)),
        selection: Style::new().bg(best_color(
            blend(rgbs[Hue::Accent as usize], p.bg, SELECTION_ALPHA),
            spec.depth,
        )),
        track: fg(blend(p.fg, p.bg, 0.18)),
        pulse: Some((blend(running, p.bg, PULSE_DIM), running)),
        shimmer: Some((quiet, running)),
        ..base
    }
}

const MIN_BAND_CONTRAST: f64 = 1.15;

fn band_color(bg: Rgb, depth: ColorDepth, background: Background) -> Color {
    debug_assert!(depth != ColorDepth::Basic16, "ANSI-16 shows no band");
    let tint = match background {
        Background::Dark => blend(WHITE, bg, 0.07),
        Background::Light => blend(BLACK, bg, 0.06),
    };
    if depth == ColorDepth::Truecolor {
        return best_color(tint, depth);
    }
    let bg_luminance = super::probe::luminance(bg);
    let index = (16..=255u8)
        .filter(|&index| {
            let rgb = xterm_rgb(index);
            let brighter = super::probe::luminance(rgb) > bg_luminance;
            brighter == (background == Background::Dark)
                && contrast_ratio(rgb, bg) >= MIN_BAND_CONTRAST
        })
        .min_by_key(|&index| {
            let Rgb(r, g, b) = xterm_rgb(index);
            u32::from(r.abs_diff(tint.0)).pow(2)
                + u32::from(g.abs_diff(tint.1)).pow(2)
                + u32::from(b.abs_diff(tint.2)).pow(2)
        })
        .unwrap_or_else(|| nearest_index(tint));
    debug_assert!(index >= 16);
    debug_assert!((0.0..=1.0).contains(&bg_luminance));
    Color::Indexed(index)
}

/// `hue` from the overrides or the scheme, made readable on the probed bg.
fn hue_rgb(spec: ThemeSpec, hues: &Hues, hue: Hue) -> Rgb {
    let preferred = spec.overrides.get(hue).unwrap_or(hues.get(hue));
    match spec.palette {
        Some(palette) => readable_rgb(preferred, palette.bg, spec.depth),
        None => preferred,
    }
}

/// `preferred` as `depth` can show it, walked toward black or white until
/// it reaches [`MIN_TEXT_CONTRAST`] on `bg`.
pub fn readable_color_on(preferred: Rgb, bg: Rgb, depth: ColorDepth) -> Color {
    best_color(readable_rgb(preferred, bg, depth), depth)
}

/// The step of the walk in [`readable_color_on`], before quantising; ANSI-16
/// cannot be walked, so it keeps `preferred`.
fn readable_rgb(preferred: Rgb, bg: Rgb, depth: ColorDepth) -> Rgb {
    if depth == ColorDepth::Basic16 {
        return preferred;
    }
    let endpoint = if contrast_ratio(BLACK, bg) >= contrast_ratio(WHITE, bg) {
        BLACK
    } else {
        WHITE
    };
    (0..=READABLE_STEPS)
        .map(|step| {
            blend(
                endpoint,
                preferred,
                f32::from(step) / f32::from(READABLE_STEPS),
            )
        })
        .find(|rgb| {
            shown_rgb(best_color(*rgb, depth))
                .is_some_and(|shown| contrast_ratio(shown, bg) >= MIN_TEXT_CONTRAST)
        })
        .unwrap_or(endpoint)
}

/// The closest colour `depth` can display.
pub fn best_color(rgb: Rgb, depth: ColorDepth) -> Color {
    match depth {
        ColorDepth::Truecolor => Color::Rgb(rgb.0, rgb.1, rgb.2),
        ColorDepth::Indexed256 => Color::Indexed(nearest_index(rgb)),
        ColorDepth::Basic16 => basic16(nearest_index(rgb)),
    }
}

/// What a fixed colour looks like; `None` for the terminal-defined ANSI 0–15.
pub fn shown_rgb(color: Color) -> Option<Rgb> {
    match color {
        Color::Rgb(r, g, b) => Some(Rgb(r, g, b)),
        Color::Indexed(index) if index >= 16 => Some(xterm_rgb(index)),
        _ => None,
    }
}

const CUBE_LEVELS: [u8; 6] = [0, 95, 135, 175, 215, 255];

/// Nearest xterm-256 index in the fixed range 16–255 (cube or grey ramp).
fn nearest_index(rgb: Rgb) -> u8 {
    let level = |v: u8| {
        (0u8..6)
            .min_by_key(|&i| CUBE_LEVELS[usize::from(i)].abs_diff(v))
            .unwrap_or(0)
    };
    let cube = 16 + 36 * level(rgb.0) + 6 * level(rgb.1) + level(rgb.2);
    let mean = (u16::from(rgb.0) + u16::from(rgb.1) + u16::from(rgb.2)) / 3;
    let grey = 232
        + u8::try_from(mean.saturating_sub(3) / 10)
            .unwrap_or(23)
            .min(23);
    let distance = |index: u8| {
        let shown = xterm_rgb(index);
        let d = |a: u8, b: u8| u32::from(a.abs_diff(b)).pow(2);
        d(shown.0, rgb.0) + d(shown.1, rgb.1) + d(shown.2, rgb.2)
    };
    if distance(grey) < distance(cube) {
        grey
    } else {
        cube
    }
}

fn xterm_rgb(index: u8) -> Rgb {
    debug_assert!(index >= 16, "ANSI 0-15 are terminal-defined");
    if index >= 232 {
        let v = 8 + (index - 232) * 10;
        return Rgb(v, v, v);
    }
    let n = index - 16;
    let level = |i: u8| CUBE_LEVELS[usize::from(i)];
    Rgb(level(n / 36), level(n / 6 % 6), level(n % 6))
}

#[cfg(test)]
mod tests {
    use super::*;

    const GITHUB_DARK: TerminalPalette = TerminalPalette {
        fg: Rgb(0xe6, 0xed, 0xf3),
        bg: Rgb(0x0d, 0x11, 0x17),
    };
    const PAPER: TerminalPalette = TerminalPalette {
        fg: Rgb(0x1f, 0x23, 0x28),
        bg: Rgb(0xff, 0xff, 0xff),
    };

    fn spec(mode: ThemeMode, depth: ColorDepth, palette: Option<TerminalPalette>) -> ThemeSpec {
        ThemeSpec {
            mode,
            depth,
            palette,
            ..ThemeSpec::default()
        }
    }

    fn every_style(theme: &Theme) -> Vec<(&'static str, Style)> {
        let mut styles = vec![
            ("text", theme.text()),
            ("secondary", theme.secondary()),
            ("title", theme.title()),
            ("command", theme.command()),
            ("link", theme.link()),
            ("heading", theme.heading()),
            ("track", theme.track()),
            ("rule", theme.rule()),
            ("pulse(0)", theme.pulse(0.0)),
            ("pulse(1)", theme.pulse(1.0)),
            ("shimmer(0)", theme.shimmer(0.0)),
            ("shimmer(1)", theme.shimmer(1.0)),
        ];
        styles.extend(Hue::ALL.map(|hue| (hue.key(), theme.hue(hue))));
        styles
    }

    fn matrix() -> Vec<ThemeSpec> {
        let palettes = [
            None,
            Some(GITHUB_DARK),
            Some(PAPER),
            Some(TerminalPalette {
                fg: Rgb(0x83, 0x94, 0x96),
                bg: Rgb(0x00, 0x2b, 0x36),
            }),
            Some(TerminalPalette {
                fg: Rgb(0x65, 0x7b, 0x83),
                bg: Rgb(0xfd, 0xf6, 0xe3),
            }),
            Some(TerminalPalette {
                fg: Rgb(0, 0, 0),
                bg: Rgb(0x77, 0x77, 0x77),
            }),
        ];
        let mut specs = Vec::new();
        for mode in ThemeMode::ALL {
            for depth in [
                ColorDepth::Truecolor,
                ColorDepth::Indexed256,
                ColorDepth::Basic16,
            ] {
                for palette in palettes {
                    specs.push(spec(mode, depth, palette));
                }
            }
        }
        specs
    }

    #[test]
    fn quantized_bands_remain_distinct_from_probed_backgrounds() {
        for bg in [
            Rgb(0x30, 0x0a, 0x24),
            BLACK,
            Rgb(0xf6, 0xf8, 0xfa),
            Rgb(0, 0x2b, 0x36),
        ] {
            let palette = Some(TerminalPalette { fg: WHITE, bg });
            let theme = Theme::resolve(spec(ThemeMode::Auto, ColorDepth::Indexed256, palette));
            let Color::Indexed(index) = theme.tint().bg.unwrap() else {
                panic!("indexed tint")
            };
            let rgb = xterm_rgb(index);
            assert_ne!(index, nearest_index(bg));
            assert!(contrast_ratio(rgb, bg) >= MIN_BAND_CONTRAST);
            assert_eq!(
                super::super::probe::luminance(rgb) > super::super::probe::luminance(bg),
                theme.background() == Background::Dark
            );
            let basic = Theme::resolve(spec(ThemeMode::Auto, ColorDepth::Basic16, palette));
            assert_eq!(basic.tint().bg, None, "ANSI-16 cannot show index {index}");
            let theme = Theme::resolve(spec(ThemeMode::Auto, ColorDepth::Truecolor, palette));
            let tint = match theme.background() {
                Background::Dark => blend(WHITE, bg, 0.07),
                Background::Light => blend(BLACK, bg, 0.06),
            };
            assert_eq!(theme.tint().bg, Some(Color::Rgb(tint.0, tint.1, tint.2)));
        }
    }

    #[test]
    fn the_accent_reads_at_aa_contrast_on_every_probed_background() {
        for s in matrix() {
            let Some(palette) = s.palette else { continue };
            if s.mode == ThemeMode::Mono || s.depth == ColorDepth::Basic16 {
                continue;
            }
            let theme = Theme::resolve(s);
            let accent = theme.accent().fg.expect("accent has a colour");
            let shown = shown_rgb(accent).expect("a fixed colour");
            let ratio = contrast_ratio(shown, palette.bg);
            assert!(
                ratio >= MIN_TEXT_CONTRAST,
                "{s:?}: {accent:?} at {ratio:.2}"
            );
        }
    }

    #[test]
    fn iris_is_the_spec_colour_at_every_depth_and_passes_aa_unadjusted() {
        let resolve = |mode, depth| Theme::resolve(spec(mode, depth, None)).accent().fg;
        use ColorDepth::*;
        assert_eq!(
            resolve(ThemeMode::Dark, Truecolor),
            Some(Color::Rgb(0xA5, 0xB4, 0xFC))
        );
        assert_eq!(
            resolve(ThemeMode::Light, Truecolor),
            Some(Color::Rgb(0x4F, 0x46, 0xE5))
        );
        assert_eq!(
            resolve(ThemeMode::Dark, Indexed256),
            Some(Color::Indexed(147))
        );
        assert_eq!(
            resolve(ThemeMode::Light, Indexed256),
            Some(Color::Indexed(62))
        );
        assert_eq!(resolve(ThemeMode::Dark, Basic16), Some(Color::Magenta));
        assert_eq!(resolve(ThemeMode::Light, Basic16), Some(Color::Magenta));

        // §5.1's reference grounds: no adjustment needed on either.
        assert!(contrast_ratio(IRIS_DARK, GITHUB_DARK.bg) >= MIN_TEXT_CONTRAST);
        assert!(contrast_ratio(IRIS_LIGHT, PAPER.bg) >= MIN_TEXT_CONTRAST);
        assert_eq!(Scheme::default().dark.get(Hue::Accent), IRIS_DARK);
        assert_eq!(Scheme::default().light.get(Hue::Accent), IRIS_LIGHT);
        assert!(contrast_ratio(xterm_rgb(147), GITHUB_DARK.bg) >= MIN_TEXT_CONTRAST);
        assert!(contrast_ratio(xterm_rgb(62), PAPER.bg) >= MIN_TEXT_CONTRAST);
        let probed = |palette| Theme::resolve(spec(ThemeMode::Auto, Truecolor, Some(palette)));
        assert_eq!(
            probed(GITHUB_DARK).accent().fg,
            Some(Color::Rgb(0xA5, 0xB4, 0xFC))
        );
        assert_eq!(
            probed(PAPER).accent().fg,
            Some(Color::Rgb(0x4F, 0x46, 0xE5))
        );
    }

    #[test]
    fn no_token_but_tint_and_selection_paints_a_background() {
        for s in matrix() {
            let theme = Theme::resolve(s);
            for (name, style) in every_style(&theme) {
                assert_eq!(style.bg, None, "{name} in {s:?}");
            }
            let tint = theme.tint();
            assert_eq!(tint.fg, None, "{s:?}");
            assert!(tint.add_modifier.is_empty(), "{s:?}");
            assert_eq!(theme.selection().fg, None, "{s:?}");
        }
    }

    #[test]
    fn every_hue_reads_at_aa_contrast_in_every_builtin_on_every_probed_background() {
        for builtin in crate::ui::theme::Builtin::ALL {
            for s in matrix() {
                let Some(palette) = s.palette else { continue };
                if s.mode == ThemeMode::Mono || s.depth == ColorDepth::Basic16 {
                    continue;
                }
                let theme = Theme::resolve(ThemeSpec {
                    scheme: builtin.scheme(),
                    ..s
                });
                for (name, style) in every_style(&theme) {
                    let Some(shown) = style.fg.and_then(shown_rgb) else {
                        continue;
                    };
                    if name.starts_with("pulse(0") || name.starts_with("shimmer(0") {
                        continue;
                    }
                    let ratio = contrast_ratio(shown, palette.bg);
                    assert!(
                        ratio >= MIN_TEXT_CONTRAST || name == "track" || name == "secondary",
                        "{} {name} {s:?}: {ratio:.2}",
                        builtin.name()
                    );
                }
            }
        }
    }

    #[test]
    fn tool_kinds_and_statuses_each_get_their_own_hue() {
        let theme = Theme::resolve(spec(
            ThemeMode::Dark,
            ColorDepth::Truecolor,
            Some(GITHUB_DARK),
        ));
        let kinds = [
            ToolKind::Shell,
            ToolKind::Read,
            ToolKind::Edit,
            ToolKind::Web,
            ToolKind::Call,
            ToolKind::Agent,
        ]
        .map(|kind| theme.tool(kind).fg);
        for (i, a) in kinds.iter().enumerate() {
            assert!(a.is_some());
            assert!(!kinds[i + 1..].contains(a), "{i}");
        }
        let status = [
            theme.running(),
            theme.waiting(),
            theme.attention(),
            theme.success(),
            theme.failure(),
            theme.blocked(),
            theme.paused(),
        ]
        .map(|style| style.fg);
        for (i, a) in status.iter().enumerate() {
            assert!(!status[i + 1..].contains(a), "{i}");
        }
        // Commands are text leaning toward code: neither of the two.
        assert_ne!(theme.command().fg, theme.code().fg);
        assert_ne!(theme.command().fg, theme.text().fg);
    }

    #[test]
    fn ansi16_paints_each_hue_in_a_basic_colour() {
        let theme = Theme::resolve(spec(
            ThemeMode::Dark,
            ColorDepth::Basic16,
            Some(GITHUB_DARK),
        ));
        assert_eq!(theme.running().fg, Some(Color::Yellow));
        assert_eq!(theme.success().fg, Some(Color::Green));
        assert_eq!(theme.failure().fg, Some(Color::Red));
        assert_eq!(theme.tool(ToolKind::Shell).fg, Some(Color::Cyan));
        assert_eq!(theme.heading().fg, None);
        // The palette is the terminal's, so every built-in looks the same.
        let nord = Theme::resolve(ThemeSpec {
            scheme: crate::ui::theme::Builtin::Nord.scheme(),
            ..theme.spec()
        });
        assert_eq!(nord.running(), theme.running());
        assert_eq!(
            theme.selection(),
            Style::new().add_modifier(Modifier::REVERSED)
        );
    }

    #[test]
    fn mono_has_no_colours_at_all() {
        for s in matrix().into_iter().filter(|s| s.mode == ThemeMode::Mono) {
            let theme = Theme::resolve(s);
            for (name, style) in every_style(&theme) {
                assert_eq!((style.fg, style.bg), (None, None), "{name} in {s:?}");
            }
            assert_eq!(theme.tint(), Style::new());
            assert_eq!(
                theme.selected(),
                Style::new().add_modifier(Modifier::UNDERLINED)
            );
            assert_eq!(theme.accent(), Style::new().add_modifier(Modifier::BOLD));
            assert_eq!(
                theme.selection(),
                Style::new().add_modifier(Modifier::REVERSED)
            );
            assert_eq!(theme.pulse(0.3), theme.running());
        }
    }

    #[test]
    fn a_failed_probe_drops_tint_and_holds_the_pulse_still() {
        for depth in [
            ColorDepth::Truecolor,
            ColorDepth::Indexed256,
            ColorDepth::Basic16,
        ] {
            let theme = Theme::resolve(spec(ThemeMode::Auto, depth, None));
            let dim = Style::new().add_modifier(Modifier::DIM);
            assert_eq!(theme.tint(), Style::new());
            assert_eq!(
                theme.selected(),
                Style::new().add_modifier(Modifier::UNDERLINED)
            );
            assert_eq!(theme.secondary(), dim);
            assert_eq!(theme.pulse(0.0), theme.running());
            assert_eq!(theme.pulse(0.7), theme.running());
            assert_eq!(theme.shimmer(0.7), dim);
            assert_eq!(theme.command(), Style::new());
        }
        // ANSI-16 cannot show a blend even when the palette is known.
        let basic = Theme::resolve(spec(
            ThemeMode::Auto,
            ColorDepth::Basic16,
            Some(GITHUB_DARK),
        ));
        assert_eq!(
            basic.selected(),
            Style::new().add_modifier(Modifier::UNDERLINED)
        );
        assert_eq!(basic.tint().bg, None);
        assert_eq!(basic.pulse(1.0), basic.running());
        assert_eq!(basic.shimmer(1.0), basic.secondary());
    }

    #[test]
    fn probed_tokens_follow_the_blend_table() {
        let dark = Theme::resolve(spec(
            ThemeMode::Auto,
            ColorDepth::Truecolor,
            Some(GITHUB_DARK),
        ));
        let rgb = |c: Rgb| Some(Color::Rgb(c.0, c.1, c.2));
        assert_eq!(dark.background(), Background::Dark);
        assert_eq!(dark.tint().bg, rgb(blend(WHITE, GITHUB_DARK.bg, 0.07)));
        assert_eq!(dark.selected().bg, dark.tint().bg);
        assert_eq!(
            dark.track().fg,
            rgb(blend(GITHUB_DARK.fg, GITHUB_DARK.bg, 0.18))
        );
        assert_eq!(
            dark.secondary().fg,
            rgb(blend(GITHUB_DARK.fg, GITHUB_DARK.bg, 0.55))
        );
        let running = dark.spec().scheme.dark.get(Hue::Running);
        assert_eq!(dark.pulse(1.0), dark.running());
        assert_eq!(dark.pulse(1.0).fg, rgb(running));
        assert_eq!(
            dark.pulse(0.0).fg,
            rgb(blend(running, GITHUB_DARK.bg, PULSE_DIM))
        );
        let middle = dark.pulse(0.5).fg;
        assert_ne!(middle, dark.pulse(0.0).fg);
        assert_ne!(middle, dark.pulse(1.0).fg);
        assert_eq!(dark.shimmer(0.0).fg, dark.secondary().fg);
        assert_eq!(dark.shimmer(1.0).fg, rgb(running));
        let accent = dark.spec().scheme.dark.get(Hue::Accent);
        assert_eq!(
            dark.selection().bg,
            rgb(blend(accent, GITHUB_DARK.bg, SELECTION_ALPHA))
        );

        let light = Theme::resolve(spec(ThemeMode::Auto, ColorDepth::Truecolor, Some(PAPER)));
        assert_eq!(light.background(), Background::Light);
        assert_eq!(light.tint().bg, rgb(blend(BLACK, PAPER.bg, 0.06)));

        // 256 colours quantise every blend to a fixed index.
        let indexed = Theme::resolve(spec(
            ThemeMode::Auto,
            ColorDepth::Indexed256,
            Some(GITHUB_DARK),
        ));
        for style in [
            indexed.secondary(),
            indexed.track(),
            indexed.tint(),
            indexed.pulse(0.3),
        ] {
            let color = style.fg.or(style.bg);
            assert!(
                matches!(color, Some(Color::Indexed(i)) if i >= 16),
                "{color:?}"
            );
        }
    }

    #[test]
    fn reduced_motion_holds_the_pulse_still() {
        let theme = Theme::resolve(ThemeSpec {
            reduced_motion: true,
            ..spec(ThemeMode::Auto, ColorDepth::Truecolor, Some(GITHUB_DARK))
        });
        assert_eq!(theme.pulse(0.0), theme.running());
        assert_eq!(theme.pulse(1.0), theme.running());
        assert_eq!(theme.shimmer(0.5), theme.secondary());
    }

    #[test]
    fn an_override_wins_over_any_theme_and_still_reads() {
        let orange = Rgb(0xff, 0x88, 0x00);
        let mut overrides = HuePatch::default();
        overrides.set(Hue::Accent, orange);
        let with = |palette, depth| {
            Theme::resolve(ThemeSpec {
                overrides,
                scheme: crate::ui::theme::Builtin::Gruvbox.scheme(),
                ..spec(ThemeMode::Auto, depth, palette)
            })
            .accent()
            .fg
        };
        assert_eq!(
            with(Some(GITHUB_DARK), ColorDepth::Truecolor),
            Some(Color::Rgb(0xff, 0x88, 0x00))
        );
        assert_eq!(
            with(None, ColorDepth::Truecolor),
            Some(Color::Rgb(0xff, 0x88, 0x00))
        );
        // Orange on white is below AA, so it deepens.
        let on_paper = with(Some(PAPER), ColorDepth::Truecolor)
            .and_then(shown_rgb)
            .unwrap();
        assert_ne!(on_paper, orange);
        assert!(contrast_ratio(on_paper, PAPER.bg) >= MIN_TEXT_CONTRAST);
        assert_ne!(with(None, ColorDepth::Basic16), Some(Color::Magenta));
    }

    #[test]
    fn modes_choose_the_background() {
        let bg = |mode, palette, hint| {
            Theme::resolve(ThemeSpec {
                background_hint: hint,
                ..spec(mode, ColorDepth::Truecolor, palette)
            })
            .background()
        };
        use Background::*;
        assert_eq!(bg(ThemeMode::Auto, Some(PAPER), Dark), Light);
        assert_eq!(bg(ThemeMode::Auto, Some(GITHUB_DARK), Light), Dark);
        assert_eq!(bg(ThemeMode::Auto, None, Light), Light);
        assert_eq!(bg(ThemeMode::Auto, None, Dark), Dark);
        assert_eq!(bg(ThemeMode::Dark, Some(PAPER), Light), Dark);
        assert_eq!(bg(ThemeMode::Light, Some(GITHUB_DARK), Dark), Light);
    }

    #[test]
    fn theme_names_parse_with_their_aliases() {
        for mode in ThemeMode::ALL {
            assert_eq!(mode.as_str().parse(), Ok(mode));
        }
        assert_eq!("dusk".parse(), Ok(ThemeMode::Dark));
        assert_eq!(" Dawn ".parse(), Ok(ThemeMode::Light));
        assert_eq!("neon".parse::<ThemeMode>(), Err(()));
    }

    #[test]
    fn ascii_swaps_the_whole_glyph_table() {
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
        let ascii = Theme::resolve(ThemeSpec {
            ascii: true,
            ..ThemeSpec::default()
        });
        assert_eq!(ascii.glyphs(), &ASCII_GLYPHS);
        assert!(all(ascii.glyphs()).iter().all(|g| g.is_ascii()));
        assert_eq!(Theme::default().glyphs(), &UNICODE_GLYPHS);
        // Single-column glyphs keep alignment identical across the two tables.
        for g in all(&UNICODE_GLYPHS) {
            assert_eq!(unicode_width::UnicodeWidthStr::width(g), 1, "{g}");
        }
    }

    #[test]
    fn nearest_index_round_trips_the_fixed_palette() {
        for index in 16..=255u8 {
            let back = nearest_index(xterm_rgb(index));
            assert_eq!(xterm_rgb(back), xterm_rgb(index), "{index}");
        }
    }
}
