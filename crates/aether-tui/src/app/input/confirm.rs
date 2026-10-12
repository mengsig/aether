//! `/cancel`'s confirm row: ending an agent is final, so it asks first (§6.2).

use aether_core::id::AgentId;
use aether_protocol::Command;
use crossterm::event::{KeyCode, KeyEvent, KeyEventKind, KeyModifiers};
use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::text::Span;

use crate::app::{App, Effect, Then};
use crate::ui::bottom::{BottomView, ViewAction, ViewOutcome};
use crate::ui::theme::tokens::Theme;
use crate::ui::views::picker::{Picker, PickerRow};

#[derive(Debug)]
pub(crate) struct ConfirmCancel {
    name: String,
    details: String,
    picker: Picker<bool>,
    command: Command,
    then: Then,
    remove: bool,
}

impl ConfirmCancel {
    pub(crate) fn action(
        agent: AgentId,
        name: String,
        title: String,
        details: String,
        remove: bool,
        finished: bool,
    ) -> Self {
        let row = |label: &str| PickerRow {
            label: label.to_owned(),
            detail: String::new(),
            disabled: false,
        };
        let mut picker = Picker::new(
            title,
            vec![
                (
                    row(if remove {
                        "Yes, remove it"
                    } else {
                        "Yes, end it"
                    }),
                    true,
                ),
                (row("No"), false),
            ],
        );
        // The safe answer is under the cursor.
        picker.cursor = 1;
        Self {
            name,
            details,
            picker,
            command: if finished {
                Command::Archive {
                    target: aether_protocol::ArchiveTarget::Agent(agent),
                    archived: true,
                    idempotency_key: None,
                }
            } else {
                Command::Cancel {
                    agent,
                    reason: Some("stopped from the tui".into()),
                    idempotency_key: None,
                }
            },
            then: if finished {
                Then::BoardArchived {
                    agent,
                    archived: true,
                    after_stop: false,
                }
            } else {
                Then::BoardStopped { agent, remove }
            },
            remove,
        }
    }
}

impl ConfirmCancel {
    fn detail_lines(&self, width: u16) -> Vec<String> {
        textwrap::wrap(&self.details, usize::from(width.saturating_sub(4)).max(1))
            .into_iter()
            .map(|line| line.into_owned())
            .collect()
    }

    fn end(&self) -> ViewOutcome {
        let end = Effect::Request {
            command: self.command.clone(),
            then: self.then.clone(),
        };
        ViewOutcome::Close(vec![
            ViewAction::Effect(Box::new(end)),
            ViewAction::Flash(format!(
                "requesting {} for {}",
                if self.remove { "removal" } else { "stop" },
                self.name
            )),
        ])
    }
}

impl BottomView for ConfirmCancel {
    /// Not a filtering list: typing must never move the cursor off No.
    fn handle_key(&mut self, key: KeyEvent, _app: &App) -> ViewOutcome {
        if key
            .modifiers
            .intersects(KeyModifiers::CONTROL | KeyModifiers::ALT)
        {
            return ViewOutcome::Ignored;
        }
        // A held key must never answer: only a fresh press confirms or dismisses.
        if key.kind != KeyEventKind::Press {
            return ViewOutcome::Consumed;
        }
        match key.code {
            KeyCode::Char('y' | 'Y' | '1') => self.end(),
            KeyCode::Char('n' | 'N' | '2') | KeyCode::Esc => ViewOutcome::Close(Vec::new()),
            KeyCode::Enter if self.picker.items[self.picker.cursor].1 => self.end(),
            KeyCode::Enter => ViewOutcome::Close(Vec::new()),
            KeyCode::Up | KeyCode::Down => {
                self.picker.handle_key(key);
                ViewOutcome::Consumed
            }
            _ => ViewOutcome::Consumed,
        }
    }

    fn desired_height(&self, width: u16, _app: &App) -> u16 {
        crate::ui::views::frame_height(self.detail_lines(width).len() + 3)
    }

    fn render(&self, frame: &mut Frame, area: Rect, _app: &App, theme: &Theme) {
        let mut body = self
            .detail_lines(area.width)
            .into_iter()
            .map(|line| crate::ui::views::row(line, theme.secondary()))
            .collect::<Vec<_>>();
        body.push(ratatui::text::Line::default());
        body.extend(self.picker.lines(area.width, 2, theme, true, true));
        crate::ui::views::render_frame(
            frame,
            area,
            vec![Span::styled(self.picker.title.clone(), theme.title())],
            body,
            &[],
            theme,
        );
    }

    fn abandon_on_disconnect(&self) -> bool {
        true
    }

    fn hints(&self) -> &'static str {
        if self.remove {
            "y remove it · n keep it · enter confirm · esc back"
        } else {
            "y end it · n keep it · enter confirm · esc back"
        }
    }
}
