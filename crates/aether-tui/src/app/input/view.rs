//! The approval stage, then (after a focused board) the top slash view and popup/find keys.
//! Also applies `ViewAction`s. Owner: W6 (approval answers: W5's API).

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

use super::{Dispatch, ctrl};
use crate::app::composer_ops::PopupKeyOutcome;
use crate::app::{App, Effect};
use crate::keys::KeyAction;
use crate::ui::bottom::{ViewAction, ViewOutcome};
use crate::ui::layout::BOARD_DOCK_MIN_COLS;
use crate::ui::views::ViewKind;

impl App {
    /// A shown approval wins every key until answered; Ctrl+C defers it.
    pub(super) fn approval_stage(&mut self, key: KeyEvent) -> Option<Vec<Effect>> {
        // Cheap exit before the FIFO walk: no request, nothing can be shown.
        if self.approval_too_short() && !self.bottom_covered() {
            return Some(match self.global_key(key) {
                Dispatch::Handled(effects) => effects,
                Dispatch::Pass => Vec::new(),
            });
        }
        if self.approvals.is_empty() || self.shown_approval().is_none() {
            return None;
        }
        if ctrl(key, 'c') {
            return Some(self.ctrl_c_ladder());
        }
        if self.keys.matches(KeyAction::Board, key) {
            // The approval keeps input, so Ctrl+B only shows or hides a docked board.
            if self.board.open {
                self.close_board();
            } else if self.size.0 >= BOARD_DOCK_MIN_COLS {
                self.board.open = true;
                self.save_board_open();
            }
            return Some(Vec::new());
        }
        if self.keys.matches(KeyAction::Agents, key) {
            self.flash(format!(
                "ctrl+c defers the approval, then {} goes to the agents",
                self.keys.display(KeyAction::Agents, self.theme.ascii())
            ));
            return Some(Vec::new());
        }
        // Only plain keys answer or navigate the card; modified and function keys
        // must never become an approval answer when rebound.
        if key.modifiers.is_empty()
            && !matches!(key.code, KeyCode::F(_) | KeyCode::BackTab)
            && let Some(effects) = self.approval_key(key)
        {
            return Some(effects);
        }
        // Global chords (Ctrl+T, Ctrl+F…) still work; nothing reaches the composer.
        match self.global_key(key) {
            Dispatch::Handled(effects) => Some(effects),
            Dispatch::Pass => Some(Vec::new()),
        }
    }

    /// Pop the top view, let it handle `key` against `&self`, re-push unless closed.
    /// Chords reach only the view's own chords: Ctrl+C must reach the ladder, never act as `c`.
    pub(super) fn view_key(&mut self, key: KeyEvent) -> Dispatch {
        // BackTab encodes Shift+Tab even with an empty modifier field.
        let configured_chord = (!key.modifiers.is_empty()
            || matches!(key.code, KeyCode::F(_) | KeyCode::BackTab))
            && !(key.modifiers == KeyModifiers::SHIFT && matches!(key.code, KeyCode::Char(_)))
            && KeyAction::ALL.iter().any(|a| self.keys.matches(*a, key));
        // Capture owns every key; elsewhere a configured chord must not activate
        // a local row or a view-specific shortcut.
        if configured_chord
            && self
                .bottom
                .top_view()
                .is_some_and(|view| view.kind() != Some(ViewKind::Keys))
            && let Dispatch::Handled(effects) = self.global_key(key)
        {
            return Dispatch::Handled(effects);
        }
        let chord = key
            .modifiers
            .intersects(KeyModifiers::CONTROL | KeyModifiers::ALT | KeyModifiers::SUPER)
            || configured_chord;
        if let Some(mut view) = self.bottom.view_stack.pop() {
            let outcome = if chord {
                view.handle_chord(key, self)
            } else {
                view.handle_key(key, self)
            };
            match outcome {
                ViewOutcome::Ignored => self.bottom.push_view(view),
                ViewOutcome::Consumed => {
                    self.bottom.push_view(view);
                    return Dispatch::Handled(Vec::new());
                }
                // Re-pushed first, so a `Push` lands above its parent.
                ViewOutcome::Act(actions) => {
                    self.bottom.push_view(view);
                    return Dispatch::Handled(self.apply_view_actions(actions));
                }
                ViewOutcome::Close(actions) => {
                    return Dispatch::Handled(self.apply_view_actions(actions));
                }
            }
        }
        // The `?` panel covers the transcript: its arrows and pages scroll it, not what is beneath.
        if self.bottom.shortcuts && self.bottom.view_stack.is_empty() && self.shortcuts_key(key) {
            return Dispatch::Handled(Vec::new());
        }
        let popup_open = self.bottom.popup.is_some() || self.bottom.find.is_some();
        if popup_open {
            match self.popup_key(key) {
                PopupKeyOutcome::Consumed => return Dispatch::Handled(Vec::new()),
                PopupKeyOutcome::SubmitCommand => {
                    return Dispatch::Handled(self.composer_submit());
                }
                PopupKeyOutcome::Pass => {}
            }
        }
        Dispatch::Pass
    }

    /// Bracketed paste: an open view takes it whole, then the find bar or the Ctrl+R
    /// search, else the composer. Nothing lands in a composer the approval or board hides.
    pub(crate) fn dispatch_paste(&mut self, text: &str) -> Vec<Effect> {
        self.dirty = true;
        self.track_approvals();
        let approval_shown = self.shown_approval().is_some();
        self.bottom.approval.note_key(self.now);
        if approval_shown {
            self.flash("paste ignored: answer the approval first");
            return Vec::new();
        }
        if self.board.focused() {
            self.flash("paste ignored: the board has focus (esc leaves it)");
            return Vec::new();
        }
        if crate::viewer::hides_composer(self) {
            self.flash("paste ignored: the file viewer is full screen (Tab returns to chat)");
            return Vec::new();
        }
        let Some(mut view) = self.bottom.view_stack.pop() else {
            if self.bottom.find.is_some() {
                self.find_paste(text);
            } else if self.search_focused() {
                // Only Ctrl+R is left: the find bar was checked first.
                self.history_paste(text);
            } else {
                self.composer_paste(text);
            }
            return Vec::new();
        };
        match view.handle_paste(text) {
            ViewOutcome::Ignored | ViewOutcome::Consumed => {
                self.bottom.push_view(view);
                Vec::new()
            }
            ViewOutcome::Act(actions) => {
                self.bottom.push_view(view);
                self.apply_view_actions(actions)
            }
            ViewOutcome::Close(actions) => self.apply_view_actions(actions),
        }
    }

    pub(crate) fn apply_view_actions(&mut self, actions: Vec<ViewAction>) -> Vec<Effect> {
        let mut effects = Vec::new();
        for action in actions {
            match action {
                ViewAction::Command(command) => effects.extend(self.run_slash(command)),
                ViewAction::Effect(effect) => {
                    self.arm_view_request(&effect);
                    effects.push(*effect);
                }
                ViewAction::InsertText(text) => self.input.insert_str(&text),
                ViewAction::ViewAgent(agent) => effects.extend(self.select(agent)),
                ViewAction::Push(view) => self.bottom.push_view(view),
                ViewAction::Flash(text) => self.flash(text),
                ViewAction::Answer(id, answer) => {
                    effects.extend(self.answer_approval(id, answer));
                }
                ViewAction::Open(kind) => effects.extend(self.open_view(kind)),
                ViewAction::PinWorkspace { path, pinned } => self.pin_workspace(&path, pinned),
                ViewAction::SetKey { action, chord } => self.set_key(action, chord),
                ViewAction::ResetKeys => self.reset_keys(),
            }
        }
        effects
    }
}
