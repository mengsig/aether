//! `/report`: the run report in plain words; Enter opens it in the pager. Owner: W7.

use crossterm::event::{KeyCode, KeyEvent};
use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::text::{Line, Span};

use aether_core::model::Usage;
use aether_core::report::{AgentLine, Ended, OpenLoop, RunReport};

use super::pager::{PagerSource, PagerView, report_refresh};
use super::{BodyScroll, Hint, clean, frame_height, render_body, row, sep, short_id, unhandled};
use crate::app::App;
use crate::format;
use crate::ui::bottom::{BottomView, ViewAction, ViewOutcome};
use crate::ui::theme::tokens::Theme;

const HINTS: &[Hint] = &[
    ("↑↓", "scroll"),
    ("enter", "full screen"),
    ("r", "refresh"),
    ("esc", "back"),
];

/// The head of the report in the bottom pane; Enter reads all of it full
/// screen.
#[derive(Debug, Default)]
pub struct ReportView {
    scroll: BodyScroll,
}

impl BottomView for ReportView {
    fn wheel(&mut self, up: bool, app: &App) -> ViewOutcome {
        self.handle_key(super::wheel_key(up), app)
    }

    fn kind(&self) -> Option<super::ViewKind> {
        Some(super::ViewKind::Report)
    }

    fn handle_key(&mut self, key: KeyEvent, app: &App) -> ViewOutcome {
        match key.code {
            KeyCode::Esc => ViewOutcome::Close(Vec::new()),
            KeyCode::Enter => ViewOutcome::Act(vec![ViewAction::Push(Box::new(PagerView::new(
                PagerSource::Report,
            )))]),
            KeyCode::Char('r') => report_refresh(app),
            _ if self.scroll.key(key, report_lines(app, &app.theme).1.len()) => {
                ViewOutcome::Consumed
            }
            _ => unhandled(key),
        }
    }

    fn desired_height(&self, _width: u16, app: &App) -> u16 {
        frame_height(report_lines(app, &app.theme).1.len())
    }

    fn render(&self, frame: &mut Frame, area: Rect, app: &App, theme: &Theme) {
        let (title, body) = report_lines(app, theme);
        render_body(frame, area, title, body, self.scroll, HINTS, theme);
    }
}

/// The report's title and rows, or why there is none yet. Never blank.
pub(super) fn report_lines(app: &App, theme: &Theme) -> (Vec<Span<'static>>, Vec<Line<'static>>) {
    let ready = app.report.report.as_ref().filter(|_| !app.report.loading);
    let name = if let Some(report) = ready {
        report
            .body
            .agents
            .iter()
            .find(|agent| agent.depth == 0)
            .map_or_else(
                || short_id(theme, &report.run),
                |agent| {
                    if app.tree.nodes.contains_key(&agent.id) {
                        clean(&app.agent_display_name(agent.id))
                    } else {
                        clean(&agent.name)
                    }
                },
            )
    } else {
        app.selected_agent().map_or_else(
            || "no agent".to_owned(),
            |agent| clean(&app.agent_display_name(agent.id)),
        )
    };
    let title = vec![
        Span::styled("Report", theme.title()),
        Span::styled(format!("{}{name}", sep(theme)), theme.secondary()),
    ];
    let waiting = |text: String| vec![row(text, theme.secondary())];
    let body = if app.selected.is_none() {
        waiting("No agent selected; pick one and try again".to_owned())
    } else if let Some(error) = &app.report.error {
        vec![row(
            format!("Could not read the report: {}", clean(error)),
            theme.failure(),
        )]
    } else if let Some(report) = ready {
        body(report, app, theme)
    } else {
        waiting(format!("reading the log{}", theme.glyphs().ellipsis))
    };
    (title, body)
}

fn body(report: &RunReport, app: &App, theme: &Theme) -> Vec<Line<'static>> {
    let b = &report.body;
    let dot = sep(theme);
    let mut lines = Vec::new();
    if report.provenance.is_some() {
        lines.push(row("Imported from another machine", theme.secondary()));
    }
    if let Some(objective) = b.objective.as_deref().map(clean) {
        lines.push(row(objective, theme.text()));
    }
    let mut summary = vec![
        b.outcome.as_str().to_owned(),
        format::elapsed_compact(b.duration_ms.max(0) as u64),
        plural(b.agents.len() as u64, "agent"),
        plural(b.events, "event"),
    ];
    summary.extend(cost(&b.usage, theme));
    let mut head = vec![Span::styled(
        format!("  {}", summary.join(&dot)),
        theme.secondary(),
    )];
    // The run id only where nothing else names the run.
    if b.objective.is_none() {
        head.push(Span::styled(
            format!("  {}", short_id(theme, &report.run)),
            theme.secondary(),
        ));
    }
    lines.push(Line::from(head));
    if !b.agents.is_empty() {
        lines.push(Line::default());
        lines.extend(b.agents.iter().map(|agent| agent_line(agent, app, theme)));
    }
    if !b.failures.is_empty() {
        lines.push(Line::default());
        for failure in &b.failures {
            let text = format!("{}: {}", clean(&failure.kind), clean(&failure.error));
            lines.push(row(text, theme.failure()));
        }
    }
    let open = open_lines(&b.open_loops, theme);
    if !open.is_empty() {
        lines.push(Line::default());
        lines.extend(open);
    }
    lines
}

fn token_facts(usage: &Usage) -> Vec<String> {
    debug_assert!(usage.cache_write_1h_tokens <= usage.cache_write_tokens);
    let mut facts = Vec::new();
    if usage.total_tokens() > 0 {
        facts.push(format!("{} in", format::tokens(usage.input_tokens)));
    }
    if usage.cache_write_tokens > 0 {
        facts.push(format!(
            "{} cache-write",
            format::tokens(usage.cache_write_tokens)
        ));
    }
    if usage.cache_read_tokens > 0 {
        facts.push(format!(
            "{} cached",
            format::tokens(usage.cache_read_tokens)
        ));
    }
    if usage.total_tokens() > 0 {
        facts.push(format!("{} out", format::tokens(usage.output_tokens)));
    }
    facts
}

/// `worker · reviewer · 3 turns · 2 tools · 17.3k in · 644 out · $0.02 · completed`;
/// the role only where it says more than the name, zero counts left out.
fn agent_line(agent: &AgentLine, app: &App, theme: &Theme) -> Line<'static> {
    let dot = sep(theme);
    let name = if app.tree.nodes.contains_key(&agent.id) {
        app.agent_display_name(agent.id)
    } else {
        agent.name.clone()
    };
    let name = clean(&name);
    let role = clean(&agent.role.to_string());
    let mut spans = vec![Span::styled(
        format!("  {}{name}", "  ".repeat(agent.depth as usize)),
        theme.text().patch(theme.title()),
    )];
    let mut facts = Vec::new();
    if !role.is_empty() && role != name {
        facts.push(role);
    }
    facts.push(plural(u64::from(agent.turns), "turn"));
    if agent.tool_calls > 0 {
        facts.push(plural(u64::from(agent.tool_calls), "tool"));
    }
    let usage = &agent.usage;
    facts.extend(token_facts(usage));
    facts.extend(cost(usage, theme));
    spans.push(Span::styled(
        format!("{dot}{}{dot}", facts.join(&dot)),
        theme.secondary(),
    ));
    let style = match agent.outcome {
        Ended::Running => theme.accent(),
        Ended::Completed { .. } => theme.success(),
        Ended::Failed { .. } => theme.failure(),
        Ended::Cancelled { .. } => theme.secondary(),
    };
    spans.push(Span::styled(agent.outcome.as_str(), style));
    // A failure's error is listed under Failures already.
    let said = match &agent.outcome {
        Ended::Completed {
            summary: Some(text),
        }
        | Ended::Cancelled { reason: Some(text) } => Some(text),
        _ => None,
    };
    if let Some(text) = said {
        spans.push(Span::styled(
            format!("{dot}{}", clean(text)),
            theme.secondary(),
        ));
    }
    Line::from(spans)
}

/// What is unfinished: `Still running: assistant, worker`, then open tasks
/// and unanswered approvals one per row.
fn open_lines(open: &[OpenLoop], theme: &Theme) -> Vec<Line<'static>> {
    let running: Vec<String> = open
        .iter()
        .filter_map(|open| match open {
            OpenLoop::Agent { name, .. } => Some(clean(name)),
            _ => None,
        })
        .collect();
    let mut lines = Vec::new();
    if !running.is_empty() {
        lines.push(row(
            format!("Still running: {}", running.join(", ")),
            theme.text(),
        ));
    }
    for loop_ in open {
        let text = match loop_ {
            OpenLoop::Agent { .. } => continue,
            OpenLoop::Task { objective, .. } => format!("Open task: {}", clean(objective)),
            OpenLoop::Approval {
                tool, description, ..
            } => format!(
                "Waiting for approval: {} {}",
                clean(tool),
                clean(description)
            ),
        };
        lines.push(row(text, theme.text()));
    }
    lines
}

/// The cost in the shared phrasing, or nothing when nobody reported one.
fn cost(usage: &Usage, theme: &Theme) -> Option<String> {
    let estimate = format::cost_estimate(usage);
    let billed = (usage.cost_complete || usage.cost_usd > 0.0).then(|| format::cost(usage));
    let text = match (billed, estimate) {
        (None, None) => return None,
        (Some(billed), None) => billed,
        (None, Some(estimate)) => estimate,
        (Some(billed), Some(estimate)) => format!("{billed} ({estimate})"),
    };
    if theme.ascii() {
        return Some(text.replace("≥ ", ">= ").replace(Usage::APPROX, "~"));
    }
    Some(text)
}

/// `1 turn`, `3 turns`.
fn plural(n: u64, noun: &str) -> String {
    if n == 1 {
        format!("1 {noun}")
    } else {
        format!("{n} {noun}s")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn report_agent_shows_cache_writes_only_when_present() {
        let app = crate::testing::populated_app();
        let mut agent = AgentLine {
            id: aether_core::id::AgentId::new(),
            parent: None,
            name: "worker".into(),
            role: aether_core::agent::Role("worker".into()),
            model: aether_core::model::ModelRef::new("mock", "model"),
            depth: 0,
            turns: 1,
            tool_calls: 0,
            usage: Usage {
                input_tokens: 12,
                cache_write_tokens: 34,
                cache_read_tokens: 56,
                output_tokens: 78,
                ..Default::default()
            },
            subtree_usage: Usage::default(),
            outcome: Ended::Running,
            created_at: aether_core::time::now(),
            ended_at: None,
            workspace: None,
        };
        let text = agent_line(&agent, &app, &app.theme).to_string();
        assert!(
            text.contains("12 in · 34 cache-write · 56 cached · 78 out"),
            "{text}"
        );
        agent.usage.cache_write_tokens = 0;
        assert!(
            !agent_line(&agent, &app, &app.theme)
                .to_string()
                .contains("cache-write")
        );
    }
}
