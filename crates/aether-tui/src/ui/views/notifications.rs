//! `/notifications` and Alt+N: the daemon's notification center. A list
//! grouped by state, and a detail that answers, marks read or resolves.

use crossterm::event::{KeyCode, KeyEvent};
use ratatui::Frame;
use ratatui::layout::Rect;

use aether_core::id::{AgentId, NotificationId};
use aether_core::notification::{Notification, NotificationState};

use super::unhandled;
use crate::app::{App, Effect, notifications};
use crate::ui::bottom::{BottomView, ViewAction, ViewOutcome};
use crate::ui::theme::tokens::Theme;

mod render;
#[cfg(test)]
mod tests;

#[derive(Debug, Default)]
pub struct NotificationsView {
    /// The highlighted notification; follows it as live changes regroup the list.
    cursor: Option<NotificationId>,
    /// Where the cursor was, for when its notification leaves the list.
    index: usize,
    detail: Option<Detail>,
}

/// One notification opened.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Detail {
    id: NotificationId,
    /// Over the actions, then the write-an-answer row when it takes text.
    choice: usize,
    /// The operator's own words, while they type them.
    draft: Option<String>,
    scroll: usize,
}

impl NotificationsView {
    /// The highlighted notification's position in the grouped list.
    fn position(&self, rows: &[&Notification]) -> Option<usize> {
        if rows.is_empty() {
            return None;
        }
        let held = self
            .cursor
            .and_then(|id| rows.iter().position(|n| n.id == id));
        Some(held.unwrap_or(self.index.min(rows.len() - 1)))
    }

    fn highlighted<'a>(&self, app: &'a App) -> Option<&'a Notification> {
        let rows = app.notifications.grouped();
        let at = self.position(&rows)?;
        Some(rows[at])
    }

    fn move_cursor(&mut self, app: &App, step: isize) -> ViewOutcome {
        let rows = app.notifications.grouped();
        let Some(at) = self.position(&rows) else {
            return ViewOutcome::Consumed;
        };
        let next = at.saturating_add_signed(step).min(rows.len() - 1);
        self.index = next;
        self.cursor = Some(rows[next].id);
        ViewOutcome::Consumed
    }

    fn list_key(&mut self, key: KeyEvent, app: &App) -> ViewOutcome {
        match key.code {
            KeyCode::Esc => ViewOutcome::Close(Vec::new()),
            KeyCode::Up => self.move_cursor(app, -1),
            KeyCode::Down => self.move_cursor(app, 1),
            KeyCode::PageUp => self.move_cursor(app, -(super::BODY_ROWS as isize)),
            KeyCode::PageDown => self.move_cursor(app, super::BODY_ROWS as isize),
            KeyCode::Home => self.move_cursor(app, isize::MIN),
            KeyCode::End => self.move_cursor(app, isize::MAX),
            KeyCode::Enter => self.open(app),
            KeyCode::Char(c) => match self.highlighted(app) {
                Some(n) => letter(c, n, app),
                None => ViewOutcome::Consumed,
            },
            _ => unhandled(key),
        }
    }

    /// Enter on the list: show the highlighted one, marking it seen.
    fn open(&mut self, app: &App) -> ViewOutcome {
        let Some(n) = self.highlighted(app) else {
            return ViewOutcome::Consumed;
        };
        self.cursor = Some(n.id);
        self.detail = Some(Detail {
            id: n.id,
            choice: 0,
            draft: None,
            scroll: 0,
        });
        if n.state != NotificationState::Unread {
            return ViewOutcome::Consumed;
        }
        ViewOutcome::Act(vec![effect(notifications::mark_read(n.id))])
    }

    fn detail_key(&mut self, key: KeyEvent, app: &App) -> ViewOutcome {
        let Some(detail) = self.detail.as_mut() else {
            return ViewOutcome::Ignored;
        };
        let Some(n) = app.notifications.get(detail.id) else {
            self.detail = None;
            return ViewOutcome::Consumed;
        };
        if detail.draft.is_some() {
            return detail.draft_key(key, n);
        }
        let choices = choices(n);
        match key.code {
            KeyCode::Esc => {
                self.detail = None;
                ViewOutcome::Consumed
            }
            KeyCode::Up => detail.step(-1, choices),
            KeyCode::Down => detail.step(1, choices),
            KeyCode::PageUp => detail.scroll_by(-(super::BODY_ROWS as isize), n, app),
            KeyCode::PageDown => detail.scroll_by(super::BODY_ROWS as isize, n, app),
            KeyCode::Enter => detail.choose(detail.choice, n),
            KeyCode::Char(c @ '1'..='9') => detail.choose(usize::from(c as u8 - b'1'), n),
            KeyCode::Char('a') if n.is_answerable() && n.free_text => detail.start_draft(n),
            KeyCode::Char(c) => letter(c, n, app),
            _ => unhandled(key),
        }
    }
}

impl Detail {
    fn step(&mut self, step: isize, choices: usize) -> ViewOutcome {
        self.choice = self
            .choice
            .saturating_add_signed(step)
            .min(choices.saturating_sub(1));
        ViewOutcome::Consumed
    }

    /// Scroll the text, never past its last row at the terminal's width.
    fn scroll_by(&mut self, step: isize, n: &Notification, app: &App) -> ViewOutcome {
        let last = render::text_len(n, app.size.0, app).saturating_sub(1);
        self.scroll = self.scroll.saturating_add_signed(step).min(last);
        ViewOutcome::Consumed
    }

    /// Row `choice`: an action answers; the last row of a free-text one starts typing.
    fn choose(&mut self, choice: usize, n: &Notification) -> ViewOutcome {
        if !n.is_answerable() || choice >= choices(n) {
            return ViewOutcome::Consumed;
        }
        self.choice = choice;
        match n.actions.get(choice) {
            Some(action) => ViewOutcome::Act(vec![effect(notifications::answer(
                n.id,
                Some(action.id.clone()),
                None,
            ))]),
            None => self.start_draft(n),
        }
    }

    fn start_draft(&mut self, n: &Notification) -> ViewOutcome {
        debug_assert!(n.free_text, "only a free-text notification takes words");
        self.choice = n.actions.len();
        self.draft = Some(String::new());
        ViewOutcome::Consumed
    }

    fn draft_key(&mut self, key: KeyEvent, n: &Notification) -> ViewOutcome {
        let Some(draft) = self.draft.as_mut() else {
            return ViewOutcome::Ignored;
        };
        match key.code {
            KeyCode::Esc => self.draft = None,
            KeyCode::Backspace => {
                draft.pop();
            }
            KeyCode::Char(c) => draft.push(c),
            KeyCode::Enter if draft.trim().is_empty() => {
                return flash("type an answer first, or esc to leave it");
            }
            KeyCode::Enter => {
                let text = draft.trim().to_owned();
                self.draft = None;
                return ViewOutcome::Act(vec![effect(notifications::answer(
                    n.id,
                    None,
                    Some(text),
                ))]);
            }
            _ => {}
        }
        ViewOutcome::Consumed
    }
}

/// The letters both the list and the detail answer to.
fn letter(c: char, n: &Notification, app: &App) -> ViewOutcome {
    match c {
        'r' if n.state == NotificationState::Unread => {
            ViewOutcome::Act(vec![effect(notifications::mark_read(n.id))])
        }
        'r' => flash("already read"),
        'x' if n.state == NotificationState::Resolved => flash("already resolved"),
        'x' => ViewOutcome::Act(vec![effect(notifications::resolve(n.id))]),
        'o' => jump(n, app),
        _ => ViewOutcome::Consumed,
    }
}

/// The agent to jump to: the poster, else an agent the link names.
pub(crate) fn agent_of(n: &Notification) -> Option<AgentId> {
    n.poster_agent()
        .or_else(|| n.link.as_deref().and_then(|link| link.parse().ok()))
}

/// `o`: close the center and view the agent behind the notification.
fn jump(n: &Notification, app: &App) -> ViewOutcome {
    let Some(agent) = agent_of(n) else {
        return flash("posted by a service, not an agent: no chat to open");
    };
    if !app.tree.nodes.contains_key(&agent) {
        return flash("that agent is not in the tree (archived or gone)");
    }
    ViewOutcome::Close(vec![ViewAction::ViewAgent(agent)])
}

/// Rows the detail's cursor moves over: the actions, then writing an answer.
fn choices(n: &Notification) -> usize {
    if !n.is_answerable() {
        return 0;
    }
    n.actions.len() + usize::from(n.free_text)
}

fn effect(effect: Effect) -> ViewAction {
    ViewAction::Effect(Box::new(effect))
}

fn flash(text: impl Into<String>) -> ViewOutcome {
    ViewOutcome::Act(vec![ViewAction::Flash(text.into())])
}

impl BottomView for NotificationsView {
    fn kind(&self) -> Option<super::ViewKind> {
        Some(super::ViewKind::Notifications)
    }

    fn handle_key(&mut self, key: KeyEvent, app: &App) -> ViewOutcome {
        if self.detail.is_some() {
            return self.detail_key(key, app);
        }
        self.list_key(key, app)
    }

    fn wheel(&mut self, up: bool, app: &App) -> ViewOutcome {
        if let Some(detail) = self.detail.as_mut() {
            let Some(n) = app.notifications.get(detail.id) else {
                return ViewOutcome::Consumed;
            };
            return detail.scroll_by(if up { -1 } else { 1 }, n, app);
        }
        self.move_cursor(app, if up { -1 } else { 1 })
    }

    fn handle_paste(&mut self, text: &str) -> ViewOutcome {
        match self.detail.as_mut().and_then(|d| d.draft.as_mut()) {
            Some(draft) => {
                draft.push_str(text);
                ViewOutcome::Consumed
            }
            None => ViewOutcome::Ignored,
        }
    }

    fn on_ctrl_c(&mut self) -> crate::ui::bottom::CancelOutcome {
        use crate::ui::bottom::CancelOutcome;
        match self.detail.as_mut() {
            Some(detail) if detail.draft.is_some() => {
                detail.draft = None;
                CancelOutcome::Consumed
            }
            _ => CancelOutcome::Close,
        }
    }

    fn desired_height(&self, width: u16, app: &App) -> u16 {
        match &self.detail {
            Some(detail) => render::detail_height(detail, width, app),
            None => render::list_height(app),
        }
    }

    fn render(&self, frame: &mut Frame, area: Rect, app: &App, theme: &Theme) {
        match &self.detail {
            Some(detail) => render::detail(frame, area, detail, app, theme),
            None => render::list(frame, area, self, app, theme),
        }
    }
}
