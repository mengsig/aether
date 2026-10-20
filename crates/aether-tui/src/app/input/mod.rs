//! Key dispatch for the redesign (SPEC §1.3, §4): one order, first taker wins:
//! approval (if shown) → board (if focused) → view → global → composer. Owner: W6.
//! Integration routes `AppEvent::Key` here instead of `App::on_key`.

mod board;
mod composer;
pub(super) mod confirm;
mod global;
mod mouse;
mod slash;
mod view;

#[cfg(test)]
mod board_actions_tests;
#[cfg(test)]
mod rebinding_tests;
#[cfg(test)]
mod safety_tests;
#[cfg(test)]
mod tests;

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

use super::{App, Effect};

/// One dispatch stage's answer.
#[derive(Debug)]
pub(crate) enum Dispatch {
    /// The key was this stage's; stop here.
    Handled(Vec<Effect>),
    /// Not this stage's; try the next.
    Pass,
}

impl App {
    pub fn dispatch_key(&mut self, key: KeyEvent) -> Vec<Effect> {
        #[cfg(unix)]
        if ctrl(key, 'z') {
            return vec![Effect::Suspend];
        }
        if self.terminal_replies.consume(key) {
            return Vec::new();
        }
        self.dirty = true;
        self.track_approvals();
        // The approval on screen when the key was pressed takes it; only then
        // does this key count towards the typing guard.
        let answered = self.approval_stage(key);
        self.bottom.approval.note_key(self.now);
        if let Some(effects) = answered {
            return effects;
        }
        if let Some(effects) = self.viewer_key(key) {
            return effects;
        }
        let stages: [fn(&mut App, KeyEvent) -> Dispatch; 4] = [
            App::board_key,
            App::view_key,
            App::global_key,
            App::composer_key,
        ];
        for stage in stages {
            if let Dispatch::Handled(effects) = stage(self, key) {
                return effects;
            }
        }
        Vec::new()
    }
}

/// `code` with exactly `mods` held (Shift on a char is part of the char).
fn chord(key: KeyEvent, code: KeyCode, mods: KeyModifiers) -> bool {
    key.code == code && key.modifiers == mods
}

fn ctrl(key: KeyEvent, c: char) -> bool {
    chord(key, KeyCode::Char(c), KeyModifiers::CONTROL)
}
