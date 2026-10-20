//! Stage 4: the composer: editing, Enter send/steer, Tab queue, ⇧← pull back,
//! history ↑/↓, `/` `@` `?` triggers. Owner: W6 routes; W4 owns the editing ops.

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

use super::Dispatch;
use crate::app::composer_ops::EditOp;
use crate::app::{App, InputState};
use crate::keys::{KeyAction, KeyBindings};
use crate::ui::bottom::composer;
use crate::ui::bottom::popup::{MentionTab, PopupKind};

impl App {
    pub(super) fn composer_key(&mut self, key: KeyEvent) -> Dispatch {
        debug_assert!(self.input.cursor <= self.input.text.len());
        debug_assert!(self.input.text.is_char_boundary(self.input.cursor));
        // Checked before the guard below, so a hidden composer explains a paste key.
        if self.keys.matches(KeyAction::PasteImage, key) {
            let image_only = key.modifiers == KeyModifiers::ALT && key.code == KeyCode::Char('v');
            return Dispatch::Handled(self.paste_key(image_only));
        }
        // An open view or search has the keys; what it ignored is not typing.
        if self.board.focused()
            || !self.bottom.view_stack.is_empty()
            || self.search_focused()
            || crate::viewer::hides_composer(self)
        {
            return Dispatch::Handled(Vec::new());
        }
        if let Some(op) = edit_op(&self.keys, key) {
            self.composer_edit(op);
            return Dispatch::Handled(Vec::new());
        }
        let newline = self.keys.matches(KeyAction::Newline, key);
        if newline {
            self.input.insert_char('\n');
            return Dispatch::Handled(Vec::new());
        }
        if self.keys.matches(KeyAction::Unqueue, key) {
            self.composer_unqueue();
            return Dispatch::Handled(Vec::new());
        }
        if self.keys.matches(KeyAction::Shortcuts, key)
            && (self.input.text.is_empty() || self.bottom.shortcuts)
        {
            self.toggle_shortcuts();
            return Dispatch::Handled(Vec::new());
        }
        self.composer_navigation(key)
    }

    fn composer_navigation(&mut self, key: KeyEvent) -> Dispatch {
        debug_assert!(self.input.cursor <= self.input.text.len());
        debug_assert!(self.input.text.is_char_boundary(self.input.cursor));
        let plain = key.modifiers.is_empty();
        let effects = match key.code {
            _ if self.keys.matches(KeyAction::Send, key) => self.composer_submit(),
            _ if self.keys.matches(KeyAction::Queue, key) => self.composer_queue(),
            KeyCode::Up if plain => {
                match composer::vertical_target(self, -1) {
                    Some(cursor) => self.composer_move(|input| input.cursor = cursor),
                    None if self.input.history_prev() => {}
                    None => {}
                }
                Vec::new()
            }
            KeyCode::Down if plain => {
                match composer::vertical_target(self, 1) {
                    Some(cursor) => self.composer_move(|input| input.cursor = cursor),
                    None if self.input.history_next() => {}
                    None => {}
                }
                Vec::new()
            }
            KeyCode::Left if plain => {
                self.composer_move(InputState::left);
                Vec::new()
            }
            KeyCode::Right if plain => {
                self.composer_move(InputState::right);
                Vec::new()
            }
            KeyCode::Backspace if plain => {
                if !self.remove_last_image() {
                    self.composer_edit(EditOp::Backspace);
                }
                Vec::new()
            }
            KeyCode::Delete if plain => {
                self.composer_edit(EditOp::Delete);
                Vec::new()
            }
            KeyCode::Char(c) if plain || key.modifiers == KeyModifiers::SHIFT => {
                self.type_char(c);
                Vec::new()
            }
            _ => return Dispatch::Pass,
        };
        Dispatch::Handled(effects)
    }

    /// A printable character; `?`, `/` and `@` also open their panel or popup.
    /// `q` is just a letter here: nothing in the composer quits.
    fn type_char(&mut self, c: char) {
        let empty = self.input.text.is_empty();
        if self.bottom.shortcuts {
            self.toggle_shortcuts();
        }
        self.input.insert_char(c);
        match c {
            '/' if empty => self.open_popup(PopupKind::Slash),
            '@' => self.open_popup(PopupKind::Mention(MentionTab::Files)),
            _ => {}
        }
    }
}

/// The emacs-style edits W4 implements.
fn edit_op(keys: &KeyBindings, key: KeyEvent) -> Option<EditOp> {
    let actions = [
        (KeyAction::LineStart, EditOp::Home),
        (KeyAction::LineEnd, EditOp::End),
        (KeyAction::EraseWord, EditOp::KillWordBack),
        (KeyAction::EraseStart, EditOp::KillToStart),
        (KeyAction::EraseEnd, EditOp::KillToEnd),
        (KeyAction::Yank, EditOp::Yank),
        (KeyAction::WordLeft, EditOp::WordLeft),
        (KeyAction::WordRight, EditOp::WordRight),
    ];
    actions
        .into_iter()
        .find_map(|(a, op)| keys.matches(a, key).then_some(op))
}
