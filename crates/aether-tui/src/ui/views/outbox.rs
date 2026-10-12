//! `/outbox`: remote operations the host could not account for, and the three
//! things a person may decide about one. Each choice is confirmed first.

use crossterm::event::{KeyCode, KeyEvent};
use ratatui::Frame;
use ratatui::layout::Rect;

use aether_client::{Settlement, Unreadable, Unresolved};

use super::picker::{Picker, PickerEvent, PickerRow};
use super::{Hint, clean, render_empty, sep, title, unhandled};
use crate::app::{App, Effect};
use crate::ui::bottom::{BottomView, ViewAction, ViewOutcome};
use crate::ui::theme::tokens::Theme;

const HINTS: &[Hint] = &[
    ("↑↓", "choose"),
    ("d", "it was delivered"),
    ("r", "send again"),
    ("x", "discard"),
    ("esc", "back"),
];
const CONFIRM_DELIVERED: &[Hint] = &[
    ("enter", "mark delivered; nothing is sent"),
    ("esc", "keep it"),
];
const CONFIRM_RESEND: &[Hint] = &[
    ("enter", "send again; it may run a second time"),
    ("esc", "keep it"),
];
const CONFIRM_DISCARD: &[Hint] = &[
    ("enter", "discard; it may already have run"),
    ("esc", "keep it"),
];

#[derive(Debug)]
pub struct OutboxView {
    picker: Picker<String>,
    /// The decision waiting on Enter, with the key it is about.
    armed: Option<(String, Settlement)>,
}

impl Default for OutboxView {
    fn default() -> Self {
        OutboxView {
            picker: Picker::new("Unconfirmed deliveries", Vec::new()),
            armed: None,
        }
    }
}

impl OutboxView {
    fn picker(&self, app: &App) -> Picker<String> {
        self.picker.with_items(rows(app))
    }

    fn arm(&mut self, choice: Settlement, app: &App) -> ViewOutcome {
        let Some(key) = self.picker.selected() else {
            return ViewOutcome::Act(vec![ViewAction::Flash("no operation highlighted".into())]);
        };
        let unreadable = app.unreadable.iter().any(|bad| &bad.key == key);
        if unreadable && choice != Settlement::Discard {
            return ViewOutcome::Act(vec![ViewAction::Flash(
                "an unreadable file can only be discarded".into(),
            )]);
        }
        self.armed = Some((key.clone(), choice));
        ViewOutcome::Consumed
    }

    /// Enter carries the armed decision out; any other key lets it go.
    fn confirm(&mut self, key: KeyEvent, app: &App) -> ViewOutcome {
        let Some((entry, choice)) = self.armed.take() else {
            return ViewOutcome::Ignored;
        };
        let still_there = app.undelivered.iter().any(|item| item.key() == entry)
            || app.unreadable.iter().any(|bad| bad.key == entry);
        if key.code != KeyCode::Enter || !still_there {
            return ViewOutcome::Consumed;
        }
        ViewOutcome::Act(vec![ViewAction::Effect(Box::new(Effect::Outbox {
            settle: Some((entry, choice)),
        }))])
    }
}

impl BottomView for OutboxView {
    fn wheel(&mut self, up: bool, app: &App) -> ViewOutcome {
        self.handle_key(super::wheel_key(up), app)
    }

    fn kind(&self) -> Option<super::ViewKind> {
        Some(super::ViewKind::Outbox)
    }

    fn handle_key(&mut self, key: KeyEvent, app: &App) -> ViewOutcome {
        if self.armed.is_some() {
            return self.confirm(key, app);
        }
        self.picker = self.picker(app);
        match key.code {
            KeyCode::Char('d') => return self.arm(Settlement::MarkDelivered, app),
            KeyCode::Char('r') => return self.arm(Settlement::SendAgain, app),
            KeyCode::Char('x') | KeyCode::Delete => return self.arm(Settlement::Discard, app),
            // Letters are hotkeys here, so they never reach the filter.
            KeyCode::Char(_) => return unhandled(key),
            _ => {}
        }
        match self.picker.handle_key(key) {
            PickerEvent::Cancelled => ViewOutcome::Close(Vec::new()),
            PickerEvent::Ignored => unhandled(key),
            _ => ViewOutcome::Consumed,
        }
    }

    fn desired_height(&self, width: u16, app: &App) -> u16 {
        self.picker(app).desired_height(width)
    }

    fn render(&self, frame: &mut Frame, area: Rect, app: &App, theme: &Theme) {
        let picker = self.picker(app);
        let heading = title(
            "Unconfirmed deliveries",
            "the host cannot say whether these ran",
            theme,
        );
        if picker.items.is_empty() {
            let text = format!(
                "Nothing is waiting on a decision{}every remote operation was answered",
                sep(theme)
            );
            return render_empty(frame, area, heading, &text, theme);
        }
        let hints = match self.armed.as_ref().map(|(_, choice)| choice) {
            None => HINTS,
            Some(Settlement::MarkDelivered) => CONFIRM_DELIVERED,
            Some(Settlement::SendAgain) => CONFIRM_RESEND,
            Some(Settlement::Discard) => CONFIRM_DISCARD,
        };
        picker.render_titled(frame, area, heading, theme, hints);
    }
}

fn rows(app: &App) -> Vec<(PickerRow, String)> {
    let operations = app
        .undelivered
        .iter()
        .map(|entry| (entry_row(entry, &app.theme), entry.key().to_owned()));
    let files = app
        .unreadable
        .iter()
        .map(|bad| (unreadable_row(bad, &app.theme), bad.key.clone()));
    operations.chain(files).collect()
}

fn unreadable_row(bad: &Unreadable, theme: &Theme) -> PickerRow {
    PickerRow::new(
        format!("unreadable file {}", clean(&bad.key)),
        format!("{}{}discard only", clean(&bad.reason), sep(theme)),
    )
}

fn entry_row(entry: &Unresolved, theme: &Theme) -> PickerRow {
    let dot = sep(theme);
    let reason = entry
        .unconfirmed
        .as_deref()
        .unwrap_or("delivery unconfirmed");
    PickerRow::new(
        format!(
            "{} {}",
            entry.command.name(),
            entry.sent_at.format("%m-%d %H:%M")
        ),
        format!("{}{dot}{}", clean(&summary(entry)), clean(reason)),
    )
}

/// What the operation would do, in the few words a row has room for.
fn summary(entry: &Unresolved) -> String {
    use aether_protocol::Command;
    match &entry.command {
        Command::SendMessage { content, .. } | Command::ReplyToQuestion { content, .. } => {
            content.clone()
        }
        Command::CreateTask(task) => task.objective.clone(),
        _ => entry.key().to_owned(),
    }
}
