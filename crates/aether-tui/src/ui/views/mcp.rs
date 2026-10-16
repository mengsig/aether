//! `/mcp`: server health, `r` reconnect. Owner: W7.

use crossterm::event::{KeyCode, KeyEvent};
use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::text::{Line, Span};

use aether_core::mcp::{McpServerHealth, McpServerState};
use aether_protocol::Command;

use super::{BodyScroll, Hint, clean, frame_height, render_body, row, sep, title, unhandled};
use crate::app::{App, Effect, Then};
use crate::ui::bottom::{BottomView, ViewAction, ViewOutcome};
use crate::ui::theme::tokens::Theme;

const HINTS: &[Hint] = &[("↑↓", "scroll"), ("r", "reconnect"), ("esc", "back")];

#[derive(Debug, Default)]
pub struct McpView {
    scroll: BodyScroll,
}

impl BottomView for McpView {
    fn wheel(&mut self, up: bool, app: &App) -> ViewOutcome {
        self.handle_key(super::wheel_key(up), app)
    }

    fn kind(&self) -> Option<super::ViewKind> {
        Some(super::ViewKind::Mcp)
    }

    fn handle_key(&mut self, key: KeyEvent, app: &App) -> ViewOutcome {
        match key.code {
            KeyCode::Esc => ViewOutcome::Close(Vec::new()),
            KeyCode::Char('r') => {
                ViewOutcome::Act(vec![ViewAction::Effect(Box::new(Effect::Request {
                    command: Command::McpServers {
                        refresh: true,
                        server: None,
                    },
                    then: Then::McpServers,
                }))])
            }
            _ if self.scroll.key(key, body(app, &app.theme).len()) => ViewOutcome::Consumed,
            _ => unhandled(key),
        }
    }

    fn desired_height(&self, _width: u16, app: &App) -> u16 {
        frame_height(body(app, &app.theme).len())
    }

    fn render(&self, frame: &mut Frame, area: Rect, app: &App, theme: &Theme) {
        let meta = if app.mcp_refreshing {
            format!("reconnecting{}", theme.glyphs().ellipsis)
        } else {
            String::new()
        };
        let title = title("MCP servers", meta, theme);
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

fn body(app: &App, theme: &Theme) -> Vec<Line<'static>> {
    if app.mcp_servers.is_empty() {
        return vec![row(
            format!(
                "No MCP servers configured{}add [[mcp.servers]] to config.toml",
                sep(theme)
            ),
            theme.secondary(),
        )];
    }
    app.mcp_servers
        .iter()
        .flat_map(|server| server_lines(server, theme))
        .collect()
}

/// The server, its error if any, then one indented row per tool.
fn server_lines(server: &McpServerHealth, theme: &Theme) -> Vec<Line<'static>> {
    let dot = sep(theme);
    let (state, style) = match server.state {
        _ if server.auth_failed() => ("auth_failed", theme.failure()),
        McpServerState::Ready => ("ready", theme.success()),
        McpServerState::Failed => ("failed", theme.failure()),
        McpServerState::Unknown => ("not contacted", theme.secondary()),
    };
    let mut lines = vec![Line::from(vec![
        Span::styled(
            format!("  {}  ", clean(&server.name)),
            theme.text().patch(theme.title()),
        ),
        Span::styled(state, style),
        Span::styled(
            format!("{dot}{}", clean(&server.transport)),
            theme.secondary(),
        ),
    ])];
    if let Some(error) = &server.error {
        lines.push(row(format!("  {}", clean(error)), theme.failure()));
    }
    for tool in &server.tools {
        let origin = if tool.declared {
            "declared"
        } else {
            "inferred"
        };
        lines.push(Line::from(vec![
            Span::styled(format!("    {}", clean(&tool.qualified_name)), theme.text()),
            Span::styled(
                format!(
                    "  {}{dot}{origin}",
                    format!("{:?}", tool.side_effect).to_lowercase()
                ),
                theme.secondary(),
            ),
        ]));
    }
    lines
}
