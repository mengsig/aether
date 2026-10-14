//! Stage 3: global chords and the Esc / Ctrl+C ladders (§4.2): Ctrl+G, Ctrl+B, Ctrl+O, Ctrl+T,
//! Ctrl+R, Ctrl+F, Alt+↑, Alt+,/., Ctrl+D, scrolling, F4/F5. Owner: W6.

use aether_core::agent::AgentStatusKind;
use aether_core::id::AgentId;
use aether_core::model::Effort;
use crossterm::event::{KeyCode, KeyEvent};

use super::{Dispatch, ctrl};
use crate::app::approval_flow::ApprovalAnswer;
use crate::app::board::BoardFocus;
use crate::app::transcript::ScrollIntent;
use crate::app::{App, EFFORTS, Effect};
use crate::keys::KeyAction;
use crate::ui::bottom::CancelOutcome;
use crate::ui::bottom::popup::PopupKind;
use crate::ui::layout::{BOARD_DOCK_MIN_COLS, BoardMode};
use crate::ui::views::ViewKind;

impl App {
    pub(super) fn global_key(&mut self, key: KeyEvent) -> Dispatch {
        let effects = match key.code {
            KeyCode::Esc if key.modifiers.is_empty() => self.esc_ladder(),
            _ if ctrl(key, 'c') => self.ctrl_c_ladder(),
            _ if ctrl(key, 'd')
                && self.input.text.is_empty()
                && self.bottom.view_stack.is_empty() =>
            {
                self.detach()
            }
            _ if self.keys.matches(KeyAction::Agents, key) => {
                self.go_to_agents();
                Vec::new()
            }
            _ if self.keys.matches(KeyAction::Board, key) => {
                self.show_or_hide_board();
                Vec::new()
            }
            _ if self.keys.matches(KeyAction::Workspace, key) => self.toggle_workspace_picker(),
            _ if self.keys.matches(KeyAction::Details, key) => {
                self.details = !self.details;
                self.presentation_changed();
                Vec::new()
            }
            _ if self.keys.matches(KeyAction::History, key) => {
                self.open_popup(PopupKind::History);
                Vec::new()
            }
            _ if self.keys.matches(KeyAction::Find, key) => {
                self.open_find();
                Vec::new()
            }
            _ if self.keys.matches(KeyAction::Parent, key) => self.view_parent(),
            _ if self.keys.matches(KeyAction::LessEffort, key) => self.step_effort(-1),
            _ if self.keys.matches(KeyAction::MoreEffort, key) => self.step_effort(1),
            _ if self.keys.matches(KeyAction::PageUp, key) => self.scroll(ScrollIntent::PageUp),
            _ if self.keys.matches(KeyAction::PageDown, key) => self.scroll(ScrollIntent::PageDown),
            _ if self.keys.matches(KeyAction::LineUp, key) => self.scroll(ScrollIntent::LineUp(1)),
            _ if self.keys.matches(KeyAction::LineDown, key) => {
                self.scroll(ScrollIntent::LineDown(1))
            }
            _ if self.keys.matches(KeyAction::Top, key) => self.scroll(ScrollIntent::Top),
            _ if self.keys.matches(KeyAction::Latest, key) => self.scroll(ScrollIntent::Latest),
            // Views and popups own their keys; voice waits until they close.
            _ if self.bottom_covered()
                && (self.keys.matches(KeyAction::Dictate, key)
                    || self.keys.matches(KeyAction::InsertDictation, key)) =>
            {
                Vec::new()
            }
            _ if self.keys.matches(KeyAction::Dictate, key) => {
                self.voice_action(if self.voice.recording() {
                    crate::voice::Action::Stop
                } else {
                    crate::voice::Action::Start
                })
            }
            _ if self.keys.matches(KeyAction::InsertDictation, key) => {
                self.voice_action(crate::voice::Action::Insert)
            }
            _ => return Dispatch::Pass,
        };
        Dispatch::Handled(effects)
    }

    /// Ctrl+O: open the workspace picker, or close it when it is on top.
    fn toggle_workspace_picker(&mut self) -> Vec<Effect> {
        if self.bottom.top_view().and_then(|v| v.kind()) == Some(ViewKind::Workspace) {
            self.bottom.view_stack.pop();
            self.dirty = true;
            return Vec::new();
        }
        self.open_view(ViewKind::Workspace)
    }

    /// Esc ladder, first match wins.
    pub(crate) fn esc_ladder(&mut self) -> Vec<Effect> {
        if let Some(id) = self.shown_approval_id() {
            return self.answer_shown_approval(id, ApprovalAnswer::Deny);
        }
        if self.close_transient() || self.bottom.view_stack.pop().is_some() {
            return Vec::new();
        }
        if self.board.focused() {
            self.leave_board();
            return Vec::new();
        }
        match self.running_viewed_agent() {
            Some(agent) => self.interrupt_agent(agent),
            None => Vec::new(),
        }
    }

    /// Ctrl+C ladder, first match wins; the last rung quits and sets `exit_message`.
    pub(crate) fn ctrl_c_ladder(&mut self) -> Vec<Effect> {
        if let Some(id) = self.shown_approval_id() {
            self.defer_approval(id);
            return Vec::new();
        }
        if let Some(mut view) = self.bottom.view_stack.pop() {
            if view.on_ctrl_c() == CancelOutcome::Consumed {
                self.bottom.push_view(view);
            }
            return Vec::new();
        }
        if self.close_transient() {
            return Vec::new();
        }
        if !self.input.text.is_empty() {
            self.clear_draft_to_history();
            return Vec::new();
        }
        if let Some(agent) = self.running_viewed_agent() {
            return self.interrupt_agent(agent);
        }
        self.detach()
    }

    /// Quit the client only; the daemon keeps every agent running.
    pub(crate) fn detach(&mut self) -> Vec<Effect> {
        let (running, needs_you) = self.fleet_counts();
        let counts: Vec<String> = [(running, "running"), (needs_you, "needs you")]
            .into_iter()
            .filter(|(n, _)| *n > 0)
            .map(|(n, what)| format!("{n} {what}"))
            .collect();
        let counts = if counts.is_empty() {
            String::new()
        } else {
            format!(
                " ({})",
                counts.join(&format!(" {} ", self.theme.glyphs().dot))
            )
        };
        let command = self.options.command_hint();
        self.exit_message = Some(format!(
            "Agents keep running in the daemon{counts}. Reattach with: {command}"
        ));
        self.quit_requested = true;
        vec![Effect::Quit]
    }

    fn shown_approval_id(&self) -> Option<aether_core::id::ApprovalId> {
        // Skips W5's FIFO walk in the common case of no requests at all.
        if self.approvals.is_empty() {
            return None;
        }
        self.shown_approval().map(|request| request.id)
    }

    /// Ctrl+G: focus the board, opening it if hidden; from the board, back to
    /// the composer. A view or popup owns the keys, so it closes first.
    fn go_to_agents(&mut self) {
        if self.board.focused() {
            self.leave_board();
        } else if self.bottom_covered() {
            self.flash_close_first(&format!(
                "{} goes to the agents",
                self.keys.display(KeyAction::Agents, self.theme.ascii())
            ));
        } else {
            self.open_and_focus_board();
        }
    }

    /// Ctrl+B: show or hide the board; showing a docked board leaves focus where it is.
    /// The overlay covers the composer, so it takes focus and waits for a view or popup.
    /// A fullscreen view hides any board, so showing one waits for it too.
    fn show_or_hide_board(&mut self) {
        let fullscreen = self.bottom.top_view().is_some_and(|view| view.fullscreen());
        if self.board.open {
            self.close_board();
        } else if fullscreen {
            self.flash_close_first(&format!(
                "{} shows the board",
                self.keys.display(KeyAction::Board, self.theme.ascii())
            ));
        } else if self.size.0 >= BOARD_DOCK_MIN_COLS {
            self.board.open = true;
            self.save_board_open();
        } else if self.bottom_covered() {
            self.flash_close_first(&format!(
                "{} shows the board",
                self.keys.display(KeyAction::Board, self.theme.ascii())
            ));
        } else {
            self.open_and_focus_board();
        }
    }

    pub(super) fn flash_close_first(&mut self, then: &str) {
        let open = if self.bottom.view_stack.is_empty() {
            "the popup"
        } else {
            "this view"
        };
        self.flash(format!("esc closes {open} first, then {then}"));
    }

    /// Close the popup, the `?` panel or the find bar; `true` when one was open.
    /// Leaving Ctrl+R puts back the draft it replaced.
    fn close_transient(&mut self) -> bool {
        if let Some(popup) = self.bottom.popup.take() {
            if let Some(draft) = popup.saved_draft {
                self.input.set_text(draft);
                self.input.pastes = popup.saved_pastes;
            }
            return true;
        }
        if self.bottom.shortcuts {
            self.bottom.shortcuts = false;
            return true;
        }
        self.bottom.find.take().is_some()
    }

    fn running_viewed_agent(&self) -> Option<AgentId> {
        let agent = self.selected_agent()?;
        (agent.status.kind() == AgentStatusKind::Running).then_some(agent.id)
    }

    /// Ctrl+G and `/agent`; Ctrl+B only for the overlay, which must hold focus.
    pub(super) fn open_and_focus_board(&mut self) {
        if !self.board.open {
            self.board.open = true;
            self.save_board_open();
        }
        self.board.focus = BoardFocus::Board;
        if self.board.cursor.is_none() {
            self.board.cursor = self.selected;
        }
    }

    /// Esc in the board: docked hands focus back and stays; the overlay closes.
    pub(in crate::app) fn leave_board(&mut self) {
        if self.board_is_overlay() {
            self.close_board();
        } else {
            self.board.focus = BoardFocus::BottomPane;
        }
    }

    pub(super) fn close_board(&mut self) {
        self.board.open = false;
        self.board.focus = BoardFocus::BottomPane;
        self.save_board_open();
    }

    pub(super) fn save_board_open(&mut self) {
        let open = self.board.open;
        if let Err(reason) = self.save_prefs(|prefs| prefs.board_open = Some(open)) {
            self.flash_error(format!("board state not saved: {reason}"));
        }
    }

    pub(super) fn board_is_overlay(&self) -> bool {
        self.board_mode() == BoardMode::Overlay
    }

    fn view_parent(&mut self) -> Vec<Effect> {
        match self.selected_agent().and_then(|agent| agent.parent) {
            Some(parent) => self.select(parent),
            None => Vec::new(),
        }
    }

    /// Alt+, / Alt+.: one step along `EFFORTS`, stopping at either end.
    fn step_effort(&mut self, delta: isize) -> Vec<Effect> {
        let current = self
            .selected_agent()
            .and_then(|agent| agent.config.effort)
            .or(self.default_effort)
            .unwrap_or(Effort::High);
        let index = EFFORTS
            .iter()
            .position(|(effort, _)| *effort == current)
            .unwrap_or(0);
        let last = EFFORTS.len() - 1;
        let next = EFFORTS[index.saturating_add_signed(delta).min(last)].0;
        if next == current {
            return Vec::new();
        }
        self.flash(format!("effort {next}"));
        self.use_effort(next)
    }

    pub(in crate::app) fn scroll(&mut self, intent: ScrollIntent) -> Vec<Effect> {
        if let Some(agent) = self.selected {
            self.scroll_log.push(agent, intent);
            if intent == ScrollIntent::Latest {
                return self.latest_history_request(agent).into_iter().collect();
            }
        }
        Vec::new()
    }
}
