//! `/doctor`: checks list. Owner: W7.

use crossterm::event::{KeyCode, KeyEvent};
use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::text::{Line, Span};

use aether_protocol::AuthInfo;

use super::{
    BodyScroll, Hint, clean, frame_height, home_path, render_body, row, sep, title, unhandled,
};
use crate::app::{App, cli_missing};
use crate::ui::bottom::{BottomView, ViewOutcome};
use crate::ui::text_width::display_width;
use crate::ui::theme::tokens::Theme;

/// Width of the check-name column.
const CHECK_WIDTH: usize = 13;

const HINTS: &[Hint] = &[("↑↓", "scroll"), ("esc", "back")];

#[derive(Debug, Default)]
pub struct DoctorView {
    scroll: BodyScroll,
}

impl BottomView for DoctorView {
    fn wheel(&mut self, up: bool, app: &App) -> ViewOutcome {
        self.handle_key(super::wheel_key(up), app)
    }

    fn kind(&self) -> Option<super::ViewKind> {
        Some(super::ViewKind::Doctor)
    }

    fn handle_key(&mut self, key: KeyEvent, app: &App) -> ViewOutcome {
        match key.code {
            KeyCode::Esc => ViewOutcome::Close(Vec::new()),
            _ if self.scroll.key(key, body(app, &app.theme).len()) => ViewOutcome::Consumed,
            _ => unhandled(key),
        }
    }

    fn desired_height(&self, _width: u16, app: &App) -> u16 {
        frame_height(body(app, &app.theme).len())
    }

    fn render(&self, frame: &mut Frame, area: Rect, app: &App, theme: &Theme) {
        let title = title("Doctor", "", theme);
        render_body(
            frame,
            area,
            title,
            body(app, theme),
            self.scroll,
            HINTS,
            theme,
        );
    }
}

/// One line per check; a failing check says what to do next.
fn check(theme: &Theme, what: &str, state: String, ok: bool, next: &str) -> Line<'static> {
    let (mark, style) = if ok {
        (theme.glyphs().done, theme.success())
    } else {
        (theme.glyphs().failed, theme.failure())
    };
    let mut spans = vec![
        Span::styled(format!("  {mark} "), style),
        Span::styled(format!("{what:<CHECK_WIDTH$}"), theme.secondary()),
        Span::styled(state, theme.text()),
    ];
    if !next.is_empty() {
        spans.push(Span::styled(
            format!("{}{next}", sep(theme)),
            theme.accent(),
        ));
    }
    Line::from(spans)
}

fn body(app: &App, theme: &Theme) -> Vec<Line<'static>> {
    let dot = sep(theme);
    let mut lines = Vec::new();
    lines.push(match (&app.status, app.connected) {
        (Some(status), true) => check(
            theme,
            "daemon",
            format!(
                "aetherd {}{dot}pid {}{dot}{} events",
                clean(&status.version),
                status.pid,
                status.last_seq
            ),
            status.stale.is_none(),
            &status.stale.as_ref().map_or_else(String::new, |stale| {
                format!(
                    "{}; its store reads are slow, a fresh one is running",
                    crate::format::stale_status(stale)
                )
            }),
        ),
        (_, true) => check(theme, "daemon", "connected".into(), true, ""),
        _ => check(
            theme,
            "daemon",
            "not connected".into(),
            false,
            &format!(
                "reconnecting; `{} daemon` starts one",
                app.options.command_hint()
            ),
        ),
    });
    let socket = app
        .options
        .socket_path
        .as_ref()
        .map_or_else(|| "default".into(), |p| home_path(p));
    lines.push(check(theme, "socket", socket, true, ""));
    lines.push(match &app.options.default_model {
        Some(model) => check(theme, "model", model.to_string(), true, ""),
        None => check(
            theme,
            "model",
            "none configured".into(),
            false,
            "/login, or set [model] default in config.toml",
        ),
    });
    let signed_in = app.auth.iter().filter(|a| a.configured).count();
    lines.push(check(
        theme,
        "providers",
        format!("{signed_in} of {} ready", app.auth.len()),
        signed_in > 0,
        if signed_in > 0 {
            ""
        } else {
            "/login to sign in"
        },
    ));
    lines.extend(provider_lines(app, theme));
    lines.push(check(
        theme,
        "appearance",
        format!(
            "{:?} theme{dot}{} motion",
            theme.spec().mode,
            app.motion.label()
        )
        .to_lowercase(),
        true,
        "",
    ));
    let prefs = app.options.prefs_dir.as_ref().map_or_else(
        || "not saved this run".into(),
        |p| home_path(&p.join(crate::prefs::FILE)),
    );
    lines.push(check(theme, "preferences", prefs, true, ""));
    lines
}

/// Signed-in providers and ones needing a CLI or browser sign-in; API-key
/// providers nobody uses are only counted.
pub(super) fn provider_lines(app: &App, theme: &Theme) -> Vec<Line<'static>> {
    let notable: Vec<&AuthInfo> = app
        .auth
        .iter()
        .filter(|a| a.configured || cli_missing(a) || !a.methods.iter().all(|m| m == "api_key"))
        .collect();
    let column = notable
        .iter()
        .map(|a| display_width(&clean(&a.provider_name)))
        .max()
        .unwrap_or(12)
        + 2;
    let mut lines: Vec<_> = notable
        .iter()
        .map(|a| {
            let name = clean(&a.provider_name);
            let padding = " ".repeat(column - display_width(&name));
            Line::from(vec![
                Span::styled(format!("      {name}{padding}"), theme.secondary()),
                Span::styled(super::login::status(a, theme), theme.text()),
            ])
        })
        .collect();
    let rest = app.auth.len() - notable.len();
    if rest > 0 {
        let dot = sep(theme);
        lines.push(row(
            format!("    {rest} more{dot}/login <provider> for an API key{dot}/model lists them"),
            theme.secondary(),
        ));
    }
    lines
}
