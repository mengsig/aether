//! `/login`: provider picker. Enter hands off to the normal login command,
//! which suspends the TUI for the external flow. Owner: W7.

use crossterm::event::KeyEvent;
use ratatui::Frame;
use ratatui::layout::Rect;

use aether_protocol::AuthInfo;

use super::picker::{Picker, PickerEvent, PickerRow};
use super::{Hint, sep, unhandled};
use crate::app::{App, cli_missing, install_command};
use crate::commands::SlashCommand;
use crate::ui::bottom::{BottomView, ViewAction, ViewOutcome};
use crate::ui::theme::tokens::Theme;

const HINTS: &[Hint] = &[("↑↓", "choose"), ("enter", "sign in"), ("esc", "back")];

#[derive(Debug)]
pub struct LoginView {
    picker: Picker<String>,
}

impl Default for LoginView {
    fn default() -> Self {
        LoginView {
            picker: Picker::new("Sign in", Vec::new()),
        }
    }
}

impl LoginView {
    fn picker(&self, app: &App) -> Picker<String> {
        self.picker.with_items(rows(app))
    }
}

impl BottomView for LoginView {
    fn wheel(&mut self, up: bool, app: &App) -> ViewOutcome {
        self.handle_key(super::wheel_key(up), app)
    }

    fn kind(&self) -> Option<super::ViewKind> {
        Some(super::ViewKind::Login)
    }

    fn handle_key(&mut self, key: KeyEvent, app: &App) -> ViewOutcome {
        self.picker = self.picker(app);
        match self.picker.handle_key(key) {
            PickerEvent::Chosen(index) => choose(&app.auth[index], &app.theme),
            PickerEvent::Moved | PickerEvent::Filtered => ViewOutcome::Consumed,
            PickerEvent::Cancelled => ViewOutcome::Close(Vec::new()),
            PickerEvent::Ignored => unhandled(key),
        }
    }

    fn desired_height(&self, width: u16, app: &App) -> u16 {
        self.picker(app).desired_height(width)
    }

    fn render(&self, frame: &mut Frame, area: Rect, app: &App, theme: &Theme) {
        self.picker(app).render_hinted(frame, area, theme, HINTS);
    }
}

/// A missing CLI cannot be signed into; say how to install it instead.
fn choose(info: &AuthInfo, theme: &Theme) -> ViewOutcome {
    if cli_missing(info) {
        let program = info.program.as_deref().unwrap_or(&info.provider);
        let hint = match install_command(&info.provider) {
            Some(cmd) => format!("{program} is not installed{}run: {cmd}", sep(theme)),
            None => format!("{program} is not installed"),
        };
        return ViewOutcome::Act(vec![ViewAction::Flash(hint)]);
    }
    ViewOutcome::Close(vec![ViewAction::Command(SlashCommand::Login {
        provider: Some(info.provider.clone()),
    })])
}

fn rows(app: &App) -> Vec<(PickerRow, String)> {
    let theme = &app.theme;
    app.auth
        .iter()
        .map(|info| {
            (
                PickerRow::new(info.provider_name.clone(), status(info, theme)),
                info.provider.clone(),
            )
        })
        .collect()
}

/// Where the provider stands and what Enter would do about it.
pub(crate) fn status(info: &AuthInfo, theme: &Theme) -> String {
    let dot = sep(theme);
    if !info.configured
        && let Some(detail) = &info.detail
    {
        return format!("not signed in{dot}{detail}");
    }
    if info.methods.iter().all(|m| m == "none") {
        return "no sign-in needed".into();
    }
    if info.configured {
        let via = info.source.as_deref().unwrap_or("ready");
        return format!("signed in{dot}{via}");
    }
    if cli_missing(info) {
        let how = install_command(&info.provider).unwrap_or("install the CLI first");
        return format!("not installed{dot}{how}");
    }
    let how = if info.program.is_some() {
        "subscription sign-in in the browser"
    } else if info.methods.iter().any(|m| m == "oauth") {
        "sign in in the browser"
    } else if info.methods.iter().any(|m| m == "api_key") {
        "paste an API key"
    } else {
        "nothing to do"
    };
    format!("not signed in{dot}{how}")
}
