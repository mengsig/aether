//! `/usage`: providers, sign-in, health, limit windows, engine load. Owner: W7.

use crossterm::event::{KeyCode, KeyEvent};
use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::text::{Line, Span};

use aether_core::provider::{ProviderHealth, ProviderUsage};
use aether_protocol::{Command, ProviderInfo};

use unicode_width::UnicodeWidthStr;

use super::{BodyScroll, Hint, clean, frame_height, render_body, row, sep, title, unhandled};
use crate::app::{App, Effect, Then};
use crate::ui::bottom::{BottomView, ViewAction, ViewOutcome};
use crate::ui::theme::tokens::Theme;

/// A limit window this full is called out.
const NEARLY_SPENT: u64 = 80;
const HINTS: &[Hint] = &[("↑↓", "scroll"), ("r", "refresh"), ("esc", "back")];

#[derive(Debug, Default)]
pub struct UsageView {
    scroll: BodyScroll,
}

impl BottomView for UsageView {
    fn wheel(&mut self, up: bool, app: &App) -> ViewOutcome {
        self.handle_key(super::wheel_key(up), app)
    }

    fn kind(&self) -> Option<super::ViewKind> {
        Some(super::ViewKind::Usage)
    }

    fn handle_key(&mut self, key: KeyEvent, app: &App) -> ViewOutcome {
        match key.code {
            KeyCode::Esc => ViewOutcome::Close(Vec::new()),
            // One refresh at a time: a second would race the first's reply.
            KeyCode::Char('r') if app.usage_loading => ViewOutcome::Consumed,
            KeyCode::Char('r') => {
                ViewOutcome::Act(vec![ViewAction::Effect(Box::new(Effect::Request {
                    command: Command::Providers,
                    then: Then::Usage,
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
        let meta = if app.usage_loading {
            format!("refreshing{}", theme.glyphs().ellipsis)
        } else {
            String::new()
        };
        let title = title("Usage", meta, theme);
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

/// One row per provider, its limit windows under it; providers that report
/// no limits are named once at the end instead of once each.
pub(crate) fn body(app: &App, theme: &Theme) -> Vec<Line<'static>> {
    let mut lines = Vec::new();
    if let Some(error) = &app.usage_error {
        lines.push(row(
            format!("Refresh failed: {}", clean(error)),
            theme.failure(),
        ));
    }
    if app.providers.is_empty() && !app.usage_loading && app.usage_error.is_none() {
        lines.push(row("No providers configured", theme.secondary()));
    }
    let name_width = app
        .providers
        .iter()
        .map(|p| clean(&p.provider).width())
        .max()
        .unwrap_or(0);
    let mut silent = Vec::new();
    for provider in &app.providers {
        lines.extend(provider_lines(provider, name_width, theme));
        let reports = provider.health.usage.is_some() || provider.health.quota.is_some();
        if provider.enabled && provider.configured && !reports {
            silent.push(clean(&provider.provider));
        }
    }
    if !silent.is_empty() {
        let verb = if silent.len() == 1 { "does" } else { "do" };
        let names = and_list(&silent);
        lines.push(row(
            format!("{names} {verb} not report usage limits"),
            theme.secondary(),
        ));
    }
    if let Some(line) = engine_line(app, theme) {
        lines.push(line);
    }
    lines
}

/// `a`, `a and b`, `a, b and c`.
fn and_list(names: &[String]) -> String {
    match names {
        [] => String::new(),
        [one] => one.clone(),
        [rest @ .., last] => format!("{} and {last}", rest.join(", ")),
    }
}

/// Daemon-wide engine turns, only while any run.
fn engine_line(app: &App, theme: &Theme) -> Option<Line<'static>> {
    let status = app.status.as_ref()?;
    let running = status.engine_turns_running;
    if running == 0 {
        return None;
    }
    let load = match status.max_concurrent_engine_turns {
        Some(max) => format!("{running} of {max} engine turns running"),
        None => format!("{running} engine turns running"),
    };
    Some(row(load, theme.secondary()))
}

/// `openai  ready · OPENAI_API_KEY · 12 models · idle`, then its windows.
fn provider_lines(provider: &ProviderInfo, name_width: usize, theme: &Theme) -> Vec<Line<'static>> {
    let dot = sep(theme);
    let name = clean(&provider.provider);
    let mut head = vec![Span::styled(
        format!("  {name:<name_width$}  "),
        theme.text().patch(theme.title()),
    )];
    if !provider.enabled {
        head.push(Span::styled("off", theme.secondary()));
        return vec![Line::from(head)];
    }
    if !provider.configured {
        head.push(Span::styled(
            format!("not signed in{dot}/login {name}"),
            theme.secondary(),
        ));
        return vec![Line::from(head)];
    }
    head.push(Span::styled("ready", theme.success()));
    if let Some(source) = &provider.auth_source {
        head.push(Span::styled(
            format!("{dot}{}", clean(source)),
            theme.secondary(),
        ));
    }
    head.push(Span::styled(
        format!("{dot}{} models{dot}", provider.model_count()),
        theme.secondary(),
    ));
    head.extend(health_spans(&provider.health, theme));
    let mut lines = vec![Line::from(head)];
    let indent = " ".repeat(name_width + 2);
    let sub = |line: Line<'static>| {
        let mut spans = line.spans;
        spans.insert(0, Span::raw(indent.clone()));
        Line::from(spans)
    };
    match (&provider.health.usage, provider.health.quota) {
        (Some(usage), _) => lines.extend(window_lines(usage, theme).into_iter().map(sub)),
        (None, Some(window)) => lines.push(sub(row(window.headroom_label("quota"), theme.text()))),
        (None, None) => {}
    }
    if let Some(detail) = health_detail(&provider.health, theme) {
        lines.push(sub(row(detail, theme.failure())));
    }
    lines
}

fn window_lines(usage: &ProviderUsage, theme: &Theme) -> Vec<Line<'static>> {
    let mut lines: Vec<_> = usage
        .windows
        .iter()
        .map(|window| {
            let label = match window.name.as_str() {
                "primary" | "secondary" | "five_hour" | "seven_day" | "quota" => window
                    .quota
                    .window_label()
                    .unwrap_or_else(|| window.name.replace('_', " ")),
                _ => window.name.replace('_', " "),
            };
            let style = if window.quota.used_percent >= NEARLY_SPENT {
                theme.accent()
            } else {
                theme.text()
            };
            row(window.quota.headroom_label(&clean(&label)), style)
        })
        .collect();
    lines.push(row(
        format!("as of {} UTC", usage.observed_at.format("%Y-%m-%d %H:%M")),
        theme.secondary(),
    ));
    lines
}

/// Turns in flight against the provider's limit, and any backoff.
fn health_spans(health: &ProviderHealth, theme: &Theme) -> Vec<Span<'static>> {
    let (load, style) = match (health.limit, health.running) {
        (Some(limit), running) if health.at_limit() => (
            format!("{running} of {limit} turns, at the limit"),
            theme.accent(),
        ),
        (Some(_), 0) | (None, 0) => ("idle".to_owned(), theme.secondary()),
        (Some(limit), running) => (format!("{running} of {limit} turns"), theme.text()),
        (None, running) => (format!("{running} turns"), theme.text()),
    };
    let mut spans = vec![Span::styled(load, style)];
    if let Some(backoff) = &health.backoff {
        spans.push(Span::styled(
            format!("{}backing off: {}", sep(theme), clean(&backoff.reason)),
            theme.failure(),
        ));
    }
    spans
}

/// What the provider has been failing with, and where a turn went instead.
fn health_detail(health: &ProviderHealth, theme: &Theme) -> Option<String> {
    let mut parts = Vec::new();
    if let Some(errors) = health.error_summary() {
        parts.push(clean(&errors));
    }
    if let Some(fallback) = &health.last_fallback {
        parts.push(format!(
            "last fallback to {} ({})",
            clean(&fallback.to.to_string()),
            clean(&fallback.reason)
        ));
    }
    (!parts.is_empty()).then(|| parts.join(&sep(theme)))
}
