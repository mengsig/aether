//! `/theme`: every built-in and every theme file, each row a swatch of its
//! own status and tool hues on this terminal; Enter paints with it and saves.

use crossterm::event::KeyEvent;
use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::text::{Line, Span};
use unicode_width::UnicodeWidthStr;

use super::picker::{Picker, PickerEvent, PickerRow, RowState, label_column, picker_line};
use super::{Hint, Window, body_room, frame_height, home_path, render_frame, title, unhandled};
use crate::app::App;
use crate::commands::SlashCommand;
use crate::ui::bottom::{BottomView, ViewAction, ViewOutcome};
use crate::ui::theme::choice::Palette;
use crate::ui::theme::tokens::{Theme, ThemeMode, ThemeSpec, ToolKind};
use crate::ui::theme::{Builtin, ThemeChoice, files};

const HINTS: &[Hint] = &[
    ("↑↓", "choose"),
    ("type", "to filter"),
    ("enter", "use"),
    ("esc", "back"),
];

#[derive(Debug)]
pub struct ThemeView {
    picker: Picker<ThemeChoice>,
    /// Each item resolved for this terminal, in `picker.items` order.
    previews: Vec<Theme>,
    /// Why the themes folder could not be listed, shown in the title.
    problem: Option<String>,
    /// `NO_COLOR` picked the theme: swatches stay colourless while it is mono.
    no_color: bool,
}

impl ThemeView {
    /// Lists the themes folder now; a file added later shows on the next `/theme`.
    pub fn new(app: &App) -> ThemeView {
        let dir = app.options.prefs_dir.as_deref();
        let (names, problem) = match dir.map(files::list) {
            None => (Vec::new(), None),
            Some(Ok(names)) => (names, None),
            Some(Err(problem)) => (Vec::new(), Some(problem)),
        };
        let mut items: Vec<(PickerRow, ThemeChoice)> = ThemeChoice::builtins()
            .into_iter()
            .map(|choice| (PickerRow::new(choice.to_string(), about(&choice)), choice))
            .collect();
        for name in names {
            let Ok(choice) = name.parse::<ThemeChoice>() else {
                continue;
            };
            let file = dir.map(|dir| home_path(&files::dir(dir).join(format!("{name}.toml"))));
            let row = match choice.palette {
                Palette::User(_) => PickerRow::new(name, file.unwrap_or_default()),
                // `/theme nord` means the built-in, so the file is unreachable.
                Palette::Builtin(_) => {
                    PickerRow::new(name, "file hidden by the built-in of this name").disabled()
                }
            };
            items.push((row, choice));
        }
        let base = app.theme.spec();
        let previews = items
            .iter()
            .map(|(_, choice)| preview(choice, base, dir))
            .collect();
        let mut picker = Picker::new("Theme", items);
        let visible = picker.visible();
        let current = &app.theme_choice;
        // A pinned `gruvbox-light` is not listed; start on its family instead.
        picker.cursor = visible
            .iter()
            .position(|&i| picker.items[i].1 == *current)
            .or_else(|| {
                visible.iter().position(|&i| {
                    let item = &picker.items[i].1;
                    item.palette == current.palette && item.mode != ThemeMode::Mono
                })
            })
            .unwrap_or(0);
        ThemeView {
            picker,
            previews,
            problem,
            no_color: app.options.no_color_theme,
        }
    }

    fn lines(&self, area: Rect, theme: &Theme) -> Vec<Line<'static>> {
        let visible = self.picker.visible();
        if visible.is_empty() {
            return vec![super::row("no matches", theme.secondary())];
        }
        let rows: Vec<&PickerRow> = visible.iter().map(|&i| &self.picker.items[i].0).collect();
        let numbered = self.picker.items.len() <= 9;
        let lead = 2 + if numbered { 3 } else { 0 };
        let swatch_width = SWATCH * 2 + 1;
        let room = usize::from(area.width).saturating_sub(lead + swatch_width + 2);
        let label_width = label_column(&rows, room);
        let lines = visible
            .iter()
            .enumerate()
            .map(|(offset, &index)| {
                let (row, _) = &self.picker.items[index];
                let state = if offset == self.picker.cursor {
                    RowState::Selected
                } else {
                    RowState::Plain
                };
                let label = PickerRow {
                    detail: String::new(),
                    ..row.clone()
                };
                let mut line = picker_line(
                    theme,
                    &label,
                    numbered.then_some(offset),
                    label_width,
                    state,
                );
                let used: usize = line.spans.iter().map(|s| s.content.width()).sum();
                let pad = (lead + label_width).saturating_sub(used) + 2;
                line.spans.push(Span::raw(" ".repeat(pad)));
                line.spans.extend(swatch(
                    &self.previews[index],
                    row.disabled,
                    theme,
                    self.no_color,
                ));
                line.spans.push(Span::raw("  "));
                line.spans
                    .push(Span::styled(row.detail.clone(), theme.secondary()));
                line
            })
            .collect();
        let room = body_room(area).min(super::picker::MAX_VISIBLE_ROWS);
        Window::following(rows.len(), room, self.picker.cursor).lines(lines, theme)
    }
}

/// Dots in a swatch: accent, running, done, failed, needs-you, then the shell and edit verbs.
const SWATCH: usize = 7;

/// `no_color` keeps the swatches as colourless as the theme in use.
fn swatch(preview: &Theme, disabled: bool, theme: &Theme, no_color: bool) -> Vec<Span<'static>> {
    let preview = if no_color && theme.spec().mode == ThemeMode::Mono {
        theme
    } else {
        preview
    };
    let dot = theme.glyphs().running;
    let styles = [
        preview.accent(),
        preview.running(),
        preview.success(),
        preview.failure(),
        preview.attention(),
        preview.tool(ToolKind::Shell),
        preview.tool(ToolKind::Edit),
    ];
    let mut spans = Vec::with_capacity(SWATCH * 2);
    for (at, style) in styles.into_iter().enumerate() {
        if at > 0 {
            spans.push(Span::raw(" "));
        }
        let style = if disabled { theme.track() } else { style };
        spans.push(Span::styled(dot, style));
    }
    spans
}

/// `choice` as this terminal would show it; a broken file previews its fallback.
fn preview(choice: &ThemeChoice, base: ThemeSpec, dir: Option<&std::path::Path>) -> Theme {
    let scheme = match files::scheme(choice, dir) {
        Ok(scheme) => scheme,
        Err(error) => error.fallback.map(Builtin::scheme).unwrap_or_default(),
    };
    Theme::resolve(ThemeSpec {
        mode: choice.mode,
        scheme,
        ..base
    })
}

fn about(choice: &ThemeChoice) -> String {
    let Palette::Builtin(builtin) = &choice.palette else {
        return String::new();
    };
    let family = match builtin {
        Builtin::Aether => "Aether",
        Builtin::Nord => "Nord",
        Builtin::Catppuccin => "Catppuccin",
        Builtin::Gruvbox => "Gruvbox",
        Builtin::TokyoNight => "Tokyo Night",
    };
    match choice.mode {
        ThemeMode::Auto => format!("{family}, follows the terminal"),
        ThemeMode::Dark => format!("{family}, dark"),
        ThemeMode::Light => format!("{family}, light"),
        ThemeMode::Mono => "no colour: weight and dim only".to_owned(),
    }
}

impl BottomView for ThemeView {
    fn wheel(&mut self, up: bool, app: &App) -> ViewOutcome {
        self.handle_key(super::wheel_key(up), app)
    }

    fn kind(&self) -> Option<super::ViewKind> {
        Some(super::ViewKind::Theme)
    }

    fn handle_key(&mut self, key: KeyEvent, _app: &App) -> ViewOutcome {
        match self.picker.handle_key(key) {
            PickerEvent::Chosen(index) => {
                let choice = self.picker.items[index].1.clone();
                ViewOutcome::Close(vec![ViewAction::Command(SlashCommand::SetTheme(choice))])
            }
            PickerEvent::Moved | PickerEvent::Filtered => ViewOutcome::Consumed,
            PickerEvent::Cancelled => ViewOutcome::Close(Vec::new()),
            PickerEvent::Ignored => unhandled(key),
        }
    }

    fn desired_height(&self, width: u16, _app: &App) -> u16 {
        let _ = width;
        frame_height(self.picker.body_len())
    }

    fn render(&self, frame: &mut Frame, area: Rect, app: &App, theme: &Theme) {
        let meta = match &self.problem {
            Some(problem) => problem.clone(),
            None => format!("now {}", app.theme_choice),
        };
        let mut heading = title("Theme", meta, theme);
        heading.extend(self.picker.filter_spans(theme));
        render_frame(frame, area, heading, self.lines(area, theme), HINTS, theme);
    }
}
