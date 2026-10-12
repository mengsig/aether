//! `/schedules`: pause/resume/cancel (`p`/`c`), history. Owner: W7.

use crossterm::event::{KeyCode, KeyEvent};
use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::text::Line;

use aether_core::id::ScheduleId;
use aether_core::schedule::{
    BusyPolicy, Recurrence, Schedule, ScheduleAction, ScheduleMode, ScheduleOutcome, ScheduleStatus,
};
use aether_protocol::Command;

use super::picker::{Picker, PickerEvent, PickerRow};
use super::{
    Hint, ViewKind, Window, body_room, clean, frame_height, render_frame, row, sep, unhandled,
};
use crate::app::{App, Effect, Then};
use crate::ui::bottom::{BottomView, ViewAction, ViewOutcome};
use crate::ui::theme::tokens::Theme;

/// Detail rows under the list before PgUp/PgDn scroll them.
const DETAIL_ROWS: usize = 8;
const HISTORY_LIMIT: usize = 100;
const HINTS: &[Hint] = &[
    ("↑↓", "choose"),
    ("p", "pause/resume"),
    ("c", "cancel"),
    ("r", "refresh"),
    ("pgup pgdn", "history"),
    ("esc", "back"),
];
const CONFIRM_HINTS: &[Hint] = &[("c", "again to cancel"), ("esc", "keep")];
const EMPTY_HINTS: &[Hint] = &[("r", "refresh"), ("esc", "back")];

#[derive(Debug)]
pub struct SchedulesView {
    picker: Picker<ScheduleId>,
    /// Armed by the first `c`; a second `c` on the same timer cancels it.
    confirm_cancel: Option<ScheduleId>,
    detail_offset: usize,
}

impl Default for SchedulesView {
    fn default() -> Self {
        SchedulesView {
            picker: Picker::new("Schedules", Vec::new()),
            confirm_cancel: None,
            detail_offset: 0,
        }
    }
}

impl SchedulesView {
    fn picker(&self, app: &App) -> Picker<ScheduleId> {
        self.picker.with_items(rows(app))
    }

    /// Read through a fresh picker: the stored one predates any list reply
    /// that arrived since the last key.
    fn selected<'a>(&self, app: &'a App) -> Option<&'a Schedule> {
        let id = *self.picker(app).selected()?;
        app.schedules.jobs.iter().find(|job| job.id == id)
    }

    fn pause_or_resume(&self, app: &App) -> ViewOutcome {
        let Some(job) = self.selected(app) else {
            return ViewOutcome::Consumed;
        };
        let action = match job.status {
            ScheduleStatus::Active { .. } => ScheduleAction::Pause,
            ScheduleStatus::Paused { .. } | ScheduleStatus::Failed { .. } => ScheduleAction::Resume,
            ScheduleStatus::Cancelled { .. } | ScheduleStatus::Exhausted { .. } => {
                return ViewOutcome::Act(vec![ViewAction::Flash(
                    "This timer is finished; create a new schedule to run it again.".into(),
                )]);
            }
        };
        update(job.id, action)
    }

    fn cancel(&mut self, app: &App) -> ViewOutcome {
        let Some(job) = self.selected(app) else {
            return ViewOutcome::Consumed;
        };
        if self.confirm_cancel != Some(job.id) {
            self.confirm_cancel = Some(job.id);
            return ViewOutcome::Consumed;
        }
        self.confirm_cancel = None;
        update(job.id, ScheduleAction::Cancel)
    }

    fn detail(&self, app: &App, theme: &Theme) -> Vec<Line<'static>> {
        if let Some(error) = &app.schedules.error {
            let text = format!(
                "Could not load schedules: {}{}r retry",
                clean(error),
                sep(theme)
            );
            return vec![row(text, theme.failure())];
        }
        if app.schedules.jobs.is_empty() {
            let text = if app.schedules.loading {
                format!("loading schedules{}", theme.glyphs().ellipsis)
            } else {
                format!(
                    "No schedules{}ask an agent for a timer, or run aether schedule create",
                    sep(theme)
                )
            };
            return vec![row(text, theme.secondary())];
        }
        let Some(job) = self.selected(app) else {
            return Vec::new();
        };
        let mut lines = Vec::new();
        if self.confirm_cancel == Some(job.id) {
            lines.push(row(
                "Cancelling stops future deliveries; work already delivered is kept",
                theme.failure(),
            ));
        }
        lines.extend(job_lines(app, job, theme));
        lines.extend(history_lines(app, job, theme));
        lines
    }
}

impl BottomView for SchedulesView {
    fn wheel(&mut self, up: bool, app: &App) -> ViewOutcome {
        self.handle_key(super::wheel_key(up), app)
    }

    fn kind(&self) -> Option<ViewKind> {
        Some(ViewKind::Schedules)
    }

    fn handle_key(&mut self, key: KeyEvent, app: &App) -> ViewOutcome {
        self.picker = self.picker(app);
        // Esc while a cancel is armed keeps the timer and the view.
        if key.code == KeyCode::Esc && self.confirm_cancel.take().is_some() {
            return ViewOutcome::Consumed;
        }
        if key.code != KeyCode::Char('c') {
            self.confirm_cancel = None;
        }
        let before = self.picker.selected().copied();
        match key.code {
            KeyCode::Char('p') => return self.pause_or_resume(app),
            KeyCode::Char('c') => return self.cancel(app),
            KeyCode::Char('r') => {
                return ViewOutcome::Act(vec![ViewAction::Effect(Box::new(Effect::Request {
                    command: Command::ListSchedules { target: None },
                    then: Then::Schedules,
                }))]);
            }
            KeyCode::PageUp | KeyCode::PageDown => {
                let max = self
                    .detail(app, &app.theme)
                    .len()
                    .saturating_sub(DETAIL_ROWS);
                self.detail_offset = if key.code == KeyCode::PageUp {
                    self.detail_offset.saturating_sub(DETAIL_ROWS - 1)
                } else {
                    (self.detail_offset + DETAIL_ROWS - 1).min(max)
                };
                return ViewOutcome::Consumed;
            }
            // Letters are hotkeys here, so they never reach the filter.
            KeyCode::Char(c) if !c.is_ascii_digit() => return unhandled(key),
            _ => {}
        }
        match self.picker.handle_key(key) {
            PickerEvent::Cancelled => ViewOutcome::Close(Vec::new()),
            PickerEvent::Ignored => unhandled(key),
            _ => match self.picker.selected().copied() {
                Some(id) if Some(id) != before => {
                    self.detail_offset = 0;
                    ViewOutcome::Act(vec![history_request(id)])
                }
                _ => ViewOutcome::Consumed,
            },
        }
    }

    fn desired_height(&self, _width: u16, app: &App) -> u16 {
        let picker = self.picker(app);
        let detail = self.detail(app, &app.theme).len().min(DETAIL_ROWS);
        if picker.items.is_empty() {
            return frame_height(detail);
        }
        frame_height(picker.body_len() + 1 + detail)
    }

    fn render(&self, frame: &mut Frame, area: Rect, app: &App, theme: &Theme) {
        let picker = self.picker(app);
        let detail = self.detail(app, theme);
        let hints = match picker.selected() {
            Some(id) if self.confirm_cancel == Some(*id) => CONFIRM_HINTS,
            Some(_) => HINTS,
            None => EMPTY_HINTS,
        };
        let room = body_room(area);
        let mut body = Vec::new();
        if !picker.items.is_empty() {
            let numbered = picker.items.len() <= 9;
            let list_room = picker.body_len().min(room);
            body = picker.lines(area.width, list_room, theme, true, numbered);
            body.push(Line::default());
        }
        let detail_room = room.saturating_sub(body.len()).min(DETAIL_ROWS);
        let window = Window::scrolled(detail.len(), detail_room, self.detail_offset);
        body.extend(window.lines(detail, theme));
        render_frame(frame, area, picker.title_spans(theme), body, hints, theme);
    }
}

fn update(id: ScheduleId, action: ScheduleAction) -> ViewOutcome {
    ViewOutcome::Act(vec![ViewAction::Effect(Box::new(Effect::Request {
        command: Command::UpdateSchedule {
            id,
            action,
            idempotency_key: None,
        },
        then: Then::ScheduleChanged,
    }))])
}

fn history_request(id: ScheduleId) -> ViewAction {
    ViewAction::Effect(Box::new(Effect::Request {
        command: Command::ScheduleHistory {
            id,
            limit: HISTORY_LIMIT,
        },
        then: Then::ScheduleHistory(id),
    }))
}

fn rows(app: &App) -> Vec<(PickerRow, ScheduleId)> {
    app.schedules
        .jobs
        .iter()
        .map(|job| (PickerRow::new(clean(&job.name), summary(app, job)), job.id))
        .collect()
}

fn summary(app: &App, job: &Schedule) -> String {
    let theme = &app.theme;
    let mut parts = vec![status(&job.status).to_owned()];
    if let ScheduleStatus::Active { next_due } = &job.status {
        parts.push(format!("next {}", next_fire(*next_due, app.now)));
    }
    if let Some(last) = &job.last {
        let last = match &last.outcome {
            ScheduleOutcome::Delivered { .. } => "delivered".to_owned(),
            ScheduleOutcome::Spawned { agent, .. } => format!("started {}", name(app, *agent)),
            ScheduleOutcome::Skipped { .. } => "skipped".to_owned(),
            ScheduleOutcome::Failed { .. } => "failed".to_owned(),
        };
        parts.push(format!("last {last}"));
    }
    parts.join(&sep(theme))
}

fn next_fire(due: aether_core::time::Timestamp, now: aether_core::time::Timestamp) -> String {
    use chrono::Datelike as _;
    let due = due.with_timezone(&chrono::Local);
    let today = now.with_timezone(&chrono::Local).date_naive();
    // Dated like the history rows below it; a year only when it differs.
    due.format(if due.date_naive() == today {
        "%H:%M:%S"
    } else if due.year() == today.year() {
        "%b %d %H:%M:%S"
    } else {
        "%b %d %Y %H:%M:%S"
    })
    .to_string()
}

fn status(status: &ScheduleStatus) -> &'static str {
    match status {
        ScheduleStatus::Active { .. } => "active",
        ScheduleStatus::Paused { .. } => "paused",
        ScheduleStatus::Cancelled { .. } => "cancelled",
        ScheduleStatus::Exhausted { .. } => "limit reached",
        ScheduleStatus::Failed { .. } => "failed",
    }
}

/// An agent by name where the tree knows it.
fn name(app: &App, agent: aether_core::id::AgentId) -> String {
    clean(&app.agent_name(agent))
}

/// `every hour`, `every 15 minutes`, `every 90 seconds`.
fn every(seconds: u64) -> String {
    const UNITS: [(u64, &str); 4] = [
        (86_400, "day"),
        (3_600, "hour"),
        (60, "minute"),
        (1, "second"),
    ];
    let (size, unit) = UNITS
        .into_iter()
        .find(|(size, _)| seconds >= *size && seconds.is_multiple_of(*size))
        .unwrap_or((1, "second"));
    match seconds / size {
        1 => format!("every {unit}"),
        n => format!("every {n} {unit}s"),
    }
}

/// What the timer does, how often, and how far it has got.
fn job_lines(app: &App, job: &Schedule, theme: &Theme) -> Vec<Line<'static>> {
    let dot = sep(theme);
    let recurrence = match &job.recurrence {
        Recurrence::Interval { seconds } => every(seconds.get()),
        Recurrence::Cron {
            expression,
            timezone,
        } => format!("{}{dot}{}", clean(expression), clean(timezone)),
    };
    let limit = job
        .max_deliveries
        .map(|limit| format!(" of {limit}"))
        .unwrap_or_default();
    let mut lines = Vec::new();
    match &job.mode {
        ScheduleMode::Deliver { target, .. } => {
            let busy = match job.busy {
                BusyPolicy::Skip => "skips while the target is busy",
                BusyPolicy::QueueOne => "queues one trigger while the target is busy",
            };
            lines.push(row(
                format!("to {}{dot}{recurrence}{dot}{busy}", name(app, *target)),
                theme.text(),
            ));
            lines.push(row(
                format!("{}{limit} delivered", job.deliveries),
                theme.secondary(),
            ));
        }
        ScheduleMode::Spawn {
            template,
            last_spawned,
        } => {
            let model = template
                .model
                .as_ref()
                .map_or_else(|| "daemon default".into(), ToString::to_string);
            lines.push(row(
                format!(
                    "spawns a {} run on {model}{dot}{recurrence}{dot}skips while it is live",
                    clean(&template.role)
                ),
                theme.text(),
            ));
            let mut progress = format!("{}{limit} started", job.deliveries);
            if let Some(agent) = last_spawned {
                progress.push_str(&format!("{dot}last run {}", name(app, *agent)));
            }
            lines.push(row(progress, theme.secondary()));
            lines.push(row(
                format!("task: {}", clean(&template.objective)),
                theme.secondary(),
            ));
        }
    }
    if let ScheduleStatus::Failed { error, .. } = &job.status {
        lines.push(row(clean(error), theme.failure()));
    }
    let payload = match &job.payload {
        serde_json::Value::String(text) => clean(text),
        other => clean(&other.to_string()),
    };
    lines.push(row(format!("Sends: {payload}"), theme.secondary()));
    lines
}

/// Newest first, as the daemon sends them; only shown once the reply for
/// this timer has arrived.
fn history_lines(app: &App, job: &Schedule, theme: &Theme) -> Vec<Line<'static>> {
    let dot = sep(theme);
    let mut lines = vec![Line::default(), row("History", theme.secondary())];
    let history = match &app.schedules.history {
        Some((id, history)) if *id == job.id => history,
        _ => {
            let loading = format!("loading history{}", theme.glyphs().ellipsis);
            lines.push(row(loading, theme.secondary()));
            return lines;
        }
    };
    if history.is_empty() {
        lines.push(row("Nothing delivered yet", theme.secondary()));
    }
    for entry in history {
        let (outcome, style) = match &entry.outcome {
            ScheduleOutcome::Delivered { .. } => ("delivered".to_owned(), theme.text()),
            ScheduleOutcome::Spawned { agent, .. } => {
                (format!("started {}", name(app, *agent)), theme.text())
            }
            ScheduleOutcome::Skipped { reason } => {
                (format!("skipped{dot}{}", clean(reason)), theme.secondary())
            }
            ScheduleOutcome::Failed { error } => {
                (format!("failed{dot}{}", clean(error)), theme.failure())
            }
        };
        let due = entry
            .due
            .with_timezone(&chrono::Local)
            .format("%b %d %H:%M");
        lines.push(row(format!("{due}{dot}{outcome}"), style));
    }
    lines
}
