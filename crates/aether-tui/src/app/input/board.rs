//! Stage 2: board keys while it has focus (↑↓ ←→ Home End Enter `.` Esc, and the
//! viewer's Alt+- / Alt+= / Alt+Z sizing it; Ctrl+G, Ctrl+B and other keys pass to
//! global dispatch). Owner: W6 (board model helpers: W3).

use crossterm::event::{KeyCode, KeyEvent, KeyEventKind, KeyEventState, KeyModifiers};

use super::Dispatch;
use crate::app::{App, Effect};
use crate::keys::KeyAction;
use crate::ui::layout::{self, BOARD_DOCK_MIN_COLS};

impl App {
    pub(super) fn board_key(&mut self, key: KeyEvent) -> Dispatch {
        if !self.board.focused() {
            return Dispatch::Pass;
        }
        // Legacy terminals send Shift+d as a bare 'D', same as Caps Lock; only a
        // reported CAPS_LOCK state tells them apart.
        if let KeyCode::Char(c) = key.code
            && (key.modifiers == KeyModifiers::SHIFT
                || (key.modifiers.is_empty()
                    && c.is_ascii_uppercase()
                    && !key.state.contains(KeyEventState::CAPS_LOCK)))
            && matches!(c.to_ascii_uppercase(), 'X' | 'D' | 'P' | 'I')
        {
            return Dispatch::Handled(if key.kind == KeyEventKind::Press {
                self.board_action(c.to_ascii_uppercase())
            } else {
                Vec::new()
            });
        }
        if [KeyAction::History, KeyAction::Find, KeyAction::Workspace]
            .into_iter()
            .any(|a| self.keys.matches(a, key))
        {
            self.leave_board();
            return Dispatch::Pass;
        }
        if let Some(effects) = self.board_size_key(key) {
            return Dispatch::Handled(effects);
        }
        if !key.modifiers.is_empty() {
            return Dispatch::Pass;
        }
        let effects = match key.code {
            KeyCode::Up => {
                self.board_move(-1);
                Vec::new()
            }
            KeyCode::Down => {
                self.board_move(1);
                Vec::new()
            }
            KeyCode::Left => {
                self.board_fold();
                Vec::new()
            }
            KeyCode::Right => {
                self.board_unfold();
                Vec::new()
            }
            KeyCode::Home => {
                self.board_home();
                Vec::new()
            }
            KeyCode::End => {
                self.board_end();
                Vec::new()
            }
            KeyCode::Enter => self.view_board_cursor(),
            KeyCode::Char('.') => self.board_toggle_archived(),
            KeyCode::Esc => {
                self.leave_board();
                Vec::new()
            }
            _ => return Dispatch::Pass,
        };
        Dispatch::Handled(effects)
    }

    /// The viewer's size keys narrow, widen or fill the screen with the focused board.
    fn board_size_key(&mut self, key: KeyEvent) -> Option<Vec<Effect>> {
        let less = self.keys.matches(KeyAction::ViewerLess, key);
        let more = self.keys.matches(KeyAction::ViewerMore, key);
        if !(less || more || self.keys.matches(KeyAction::ViewerFull, key)) {
            return None;
        }
        let cols = self.size.0;
        if cols < BOARD_DOCK_MIN_COLS {
            self.flash(format!(
                "the board already fills the screen; it resizes at {BOARD_DOCK_MIN_COLS} columns or wider"
            ));
            return Some(Vec::new());
        }
        if !(less || more) {
            self.board.full = !self.board.full;
        } else if self.board.full {
            // Narrowing a full screen board docks it again; it cannot get wider.
            self.board.full = !less;
        } else {
            let current = layout::chosen_board_width(cols, self.board.width);
            self.board.width = Some(layout::step_board_width(cols, current, more));
        }
        debug_assert!(self.board.focused());
        debug_assert!(self.board.full == self.board_fullscreen_at(cols));
        Some(Vec::new())
    }

    /// Enter: view the cursor's agent; docked hands focus back, the overlay closes.
    /// An agent waiting on approvals gets them shown again.
    fn view_board_cursor(&mut self) -> Vec<Effect> {
        let Some(agent) = self.board.cursor else {
            return Vec::new();
        };
        self.leave_board();
        if self
            .approvals
            .iter()
            .any(|request| request.agent_id == agent)
        {
            self.reopen_approvals(agent);
        }
        self.select(agent)
    }
}
