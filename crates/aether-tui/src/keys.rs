//! Configurable shortcuts shared by dispatch and on-screen help.

use std::collections::BTreeMap;
use std::fmt;
use std::str::FromStr;

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

macro_rules! actions {
    ($($action:ident, $name:literal, $label:literal, $defaults:literal;)+) => {
        #[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
        pub enum KeyAction { $($action,)+ }
        impl KeyAction {
            pub const ALL: &'static [Self] = &[$(Self::$action,)+];
            pub fn name(self) -> &'static str { match self { $(Self::$action => $name,)+ } }
            pub const fn label(self) -> &'static str { match self { $(Self::$action => $label,)+ } }
            pub fn defaults(self) -> &'static str { match self { $(Self::$action => $defaults,)+ } }
            pub fn parse(name: &str) -> Option<Self> {
                Self::ALL.iter().copied().find(|a| a.name() == name)
            }
        }
    };
}

actions! {
    Agents, "agents", "go to the agents", "ctrl+g";
    Board, "board", "show / hide the board", "ctrl+b";
    Workspace, "workspace", "switch workspace", "ctrl+o";
    Details, "details", "details & reasoning", "ctrl+t";
    History, "history", "search history", "ctrl+r";
    Find, "find", "find in transcript", "ctrl+f";
    Parent, "parent", "view the parent agent", "alt+up";
    LessEffort, "less_effort", "Less reasoning effort", "alt+,";
    MoreEffort, "more_effort", "More reasoning effort", "alt+.";
    PageUp, "page_up", "Scroll a page up", "pageup";
    PageDown, "page_down", "Scroll a page down", "pagedown";
    LineUp, "line_up", "Scroll a line up", "shift+up";
    LineDown, "line_down", "Scroll a line down", "shift+down";
    Top, "top", "Top of transcript", "ctrl+home";
    Latest, "latest", "Follow latest", "ctrl+end";
    Dictate, "dictate", "start / stop dictation", "f4";
    InsertDictation, "insert_dictation", "insert recovered draft", "f5";
    Send, "send", "send · steers if running", "enter";
    Queue, "queue", "queue for after the turn", "tab";
    Newline, "newline", "newline without sending", "shift+enter / alt+enter / ctrl+j";
    Unqueue, "unqueue", "edit last queued", "shift+left";
    Shortcuts, "shortcuts", "shortcuts (empty draft)", "?";
    LineStart, "line_start", "Line start", "ctrl+a / home";
    LineEnd, "line_end", "Line end", "ctrl+e / end";
    EraseWord, "erase_word", "Erase word back", "ctrl+w / alt+backspace";
    EraseStart, "erase_start", "Erase to line start", "ctrl+u";
    EraseEnd, "erase_end", "Erase to line end", "ctrl+k";
    Yank, "yank", "paste last erased", "ctrl+y";
    PasteImage, "paste_image", "paste clipboard", "ctrl+v / alt+v";
    WordLeft, "word_left", "Move a word left", "alt+b / ctrl+left";
    WordRight, "word_right", "Move a word right", "alt+f / ctrl+right";
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct KeyChord {
    code: KeyCode,
    modifiers: KeyModifiers,
}

impl KeyChord {
    pub fn from_event(key: KeyEvent) -> Result<Self, String> {
        let mut chord = Self {
            code: key.code,
            modifiers: key.modifiers,
        };
        if chord
            .modifiers
            .intersects(KeyModifiers::SUPER | KeyModifiers::HYPER | KeyModifiers::META)
        {
            return Err("Only Ctrl, Alt and Shift modifiers are supported".into());
        }
        if let KeyCode::Char(c) = chord.code {
            if c.is_control() || !c.is_ascii() {
                return Err("Use an ASCII key, navigation key or function key".into());
            }
            if c.is_ascii_uppercase() {
                chord.code = KeyCode::Char(c.to_ascii_lowercase());
                chord.modifiers.insert(KeyModifiers::SHIFT);
            } else if !c.is_ascii_alphabetic() {
                chord.modifiers.remove(KeyModifiers::SHIFT);
            }
        }
        if chord.code == KeyCode::BackTab {
            chord.code = KeyCode::Tab;
            chord.modifiers.insert(KeyModifiers::SHIFT);
        }
        if chord.modifiers.contains(KeyModifiers::CONTROL)
            && !chord.modifiers.contains(KeyModifiers::SHIFT)
        {
            let alias = match chord.code {
                KeyCode::Char('i') => Some(KeyCode::Tab),
                KeyCode::Char('m') => Some(KeyCode::Enter),
                KeyCode::Char('[') => Some(KeyCode::Esc),
                _ => None,
            };
            if let Some(code) = alias {
                chord.code = code;
                chord.modifiers.remove(KeyModifiers::CONTROL);
            }
        }
        // Legacy terminals encode Ctrl letters as control bytes, losing Shift and punctuation.
        if chord.modifiers.contains(KeyModifiers::CONTROL)
            && let KeyCode::Char(c) = chord.code
            && (chord.modifiers.contains(KeyModifiers::SHIFT)
                || !c.is_ascii_lowercase()
                || c == 'h')
        {
            return Err("That Ctrl chord is ambiguous in terminals".into());
        }
        match chord.code {
            KeyCode::Char(_)
            | KeyCode::Enter
            | KeyCode::Tab
            | KeyCode::Esc
            | KeyCode::Backspace
            | KeyCode::Delete
            | KeyCode::Insert
            | KeyCode::Up
            | KeyCode::Down
            | KeyCode::Left
            | KeyCode::Right
            | KeyCode::Home
            | KeyCode::End
            | KeyCode::PageUp
            | KeyCode::PageDown
            | KeyCode::F(1..=24) => Ok(chord),
            _ => Err("Unsupported key".into()),
        }
    }

    fn reserved(self) -> bool {
        matches!(self.code, KeyCode::Esc)
            || (self.modifiers.contains(KeyModifiers::CONTROL)
                && matches!(self.code, KeyCode::Char('c' | 'd' | 'z' | 's' | 'q')))
    }
}

impl FromStr for KeyChord {
    type Err = String;

    fn from_str(raw: &str) -> Result<Self, Self::Err> {
        let text = raw.trim().to_ascii_lowercase();
        let mut rest = text.as_str();
        let mut modifiers = KeyModifiers::NONE;
        while let Some((prefix, tail)) = rest.split_once('+') {
            let modifier = match prefix {
                "ctrl" => KeyModifiers::CONTROL,
                "alt" => KeyModifiers::ALT,
                "shift" => KeyModifiers::SHIFT,
                _ => return Err(format!("Unknown key modifier: {prefix:?}")),
            };
            if modifiers.contains(modifier) {
                return Err("Repeated key modifier".into());
            }
            modifiers.insert(modifier);
            rest = tail;
        }
        let code = match rest {
            "enter" => KeyCode::Enter,
            "tab" => KeyCode::Tab,
            "esc" => KeyCode::Esc,
            "backspace" => KeyCode::Backspace,
            "delete" => KeyCode::Delete,
            "insert" => KeyCode::Insert,
            "space" => KeyCode::Char(' '),
            "plus" => KeyCode::Char('+'),
            "up" => KeyCode::Up,
            "down" => KeyCode::Down,
            "left" => KeyCode::Left,
            "right" => KeyCode::Right,
            "home" => KeyCode::Home,
            "end" => KeyCode::End,
            "pageup" => KeyCode::PageUp,
            "pagedown" => KeyCode::PageDown,
            _ if rest.starts_with('f') && rest.len() > 1 => {
                KeyCode::F(rest[1..].parse().map_err(|_| "Invalid function key")?)
            }
            _ if rest.len() == 1 => KeyCode::Char(char::from(rest.as_bytes()[0])),
            _ => return Err(format!("Unknown key: {raw:?}")),
        };
        Self::from_event(KeyEvent::new(code, modifiers))
    }
}

impl fmt::Display for KeyChord {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        for (modifier, label) in [
            (KeyModifiers::CONTROL, "ctrl+"),
            (KeyModifiers::ALT, "alt+"),
            (KeyModifiers::SHIFT, "shift+"),
        ] {
            if self.modifiers.contains(modifier) {
                f.write_str(label)?;
            }
        }
        let name = match self.code {
            KeyCode::Char(' ') => "space",
            KeyCode::Char('+') => "plus",
            KeyCode::Char(c) => return write!(f, "{c}"),
            KeyCode::F(n) => return write!(f, "f{n}"),
            KeyCode::Enter => "enter",
            KeyCode::Tab => "tab",
            KeyCode::Esc => "esc",
            KeyCode::Backspace => "backspace",
            KeyCode::Delete => "delete",
            KeyCode::Insert => "insert",
            KeyCode::Up => "up",
            KeyCode::Down => "down",
            KeyCode::Left => "left",
            KeyCode::Right => "right",
            KeyCode::Home => "home",
            KeyCode::End => "end",
            KeyCode::PageUp => "pageup",
            KeyCode::PageDown => "pagedown",
            _ => return Err(fmt::Error),
        };
        f.write_str(name)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct KeyBindings {
    bindings: BTreeMap<KeyAction, Vec<KeyChord>>,
    paste_image_fallback: bool,
}

impl Default for KeyBindings {
    fn default() -> Self {
        Self {
            bindings: KeyAction::ALL.iter().map(|a| (*a, defaults(*a))).collect(),
            paste_image_fallback: false,
        }
    }
}

#[allow(clippy::expect_used)]
fn defaults(action: KeyAction) -> Vec<KeyChord> {
    action
        .defaults()
        .split(" / ")
        .map(|s| s.parse().expect("valid default chord"))
        .collect()
}

impl KeyBindings {
    pub fn matches(&self, action: KeyAction, key: KeyEvent) -> bool {
        KeyChord::from_event(key).is_ok_and(|chord| self.bindings[&action].contains(&chord))
    }

    pub fn customised(&self, action: KeyAction) -> bool {
        if action == KeyAction::PasteImage && self.paste_image_fallback {
            debug_assert_eq!(self.bindings[&action], defaults(action)[1..]);
            return false;
        }
        self.bindings[&action] != defaults(action)
    }

    pub fn display(&self, action: KeyAction, ascii: bool) -> String {
        let text = self.bindings[&action]
            .iter()
            .map(ToString::to_string)
            .collect::<Vec<_>>()
            .join(" / ");
        if ascii {
            text
        } else {
            text.replace("pageup", "pgup")
                .replace("pagedown", "pgdn")
                .replace("up", "↑")
                .replace("down", "↓")
                .replace("left", "←")
                .replace("right", "→")
        }
    }

    pub fn help_chord(&self, actions: &[KeyAction], ascii: bool) -> String {
        let mut chords = Vec::new();
        for action in actions {
            let all = &self.bindings[action];
            let index = if !self.customised(*action)
                && matches!(action, KeyAction::WordLeft | KeyAction::WordRight)
            {
                1
            } else {
                0
            };
            chords.push(all[index].to_string());
        }
        let prefix = chords
            .first()
            .and_then(|c| c.rsplit_once('+'))
            .map(|(prefix, _)| format!("{prefix}+"));
        if let Some(prefix) = prefix
            && chords.iter().all(|c| c.starts_with(&prefix))
        {
            for chord in chords.iter_mut().skip(1) {
                *chord = chord[prefix.len()..].to_owned();
            }
        }
        let text = chords.join("/");
        let text = text
            .replace("ctrl+", "Ctrl+")
            .replace("alt+", "Alt+")
            .replace("shift+", "Shift+")
            .replace("pageup", "PgUp")
            .replace("pagedown", "PgDn")
            .replace("enter", "Enter")
            .replace("tab", "Tab")
            .replace("home", "Home")
            .replace("end", "End")
            .replace("backspace", "Backspace");
        let mut text = text;
        if ascii {
            text = text
                .replace("left", "Left")
                .replace("right", "Right")
                .replace("up", "Up")
                .replace("down", "Down");
        } else {
            text = text
                .replace("left", "←")
                .replace("right", "→")
                .replace("up", "↑")
                .replace("down", "↓");
        }
        // Letter and function-key names use the same casing as the fixed help rows.
        text.split('/')
            .map(|part| {
                let (prefix, key) = part.rsplit_once('+').map_or(("", part), |(p, k)| (p, k));
                let key =
                    if key.len() == 1 || (key.starts_with('f') && key[1..].parse::<u8>().is_ok()) {
                        key.to_ascii_uppercase()
                    } else {
                        key.to_owned()
                    };
                if prefix.is_empty() {
                    key
                } else {
                    format!("{prefix}+{key}")
                }
            })
            .collect::<Vec<_>>()
            .join("/")
    }

    pub fn assign(&mut self, action: KeyAction, chord: KeyChord) -> Result<(), String> {
        self.validate(action, &[chord])?;
        self.bindings.insert(action, vec![chord]);
        if action == KeyAction::PasteImage {
            self.paste_image_fallback = false;
        }
        Ok(())
    }

    pub fn reset(&mut self, action: KeyAction) -> Result<(), String> {
        let chords = defaults(action);
        self.validate(action, &chords)?;
        self.bindings.insert(action, chords);
        if action == KeyAction::PasteImage {
            self.paste_image_fallback = false;
        }
        Ok(())
    }

    fn validate(&self, action: KeyAction, chords: &[KeyChord]) -> Result<(), String> {
        for chord in chords {
            if chord.code == KeyCode::Char(']') && chord.modifiers == KeyModifiers::ALT {
                return Err(format!("{chord} is reserved for terminal replies"));
            }
            if chord.reserved() {
                return Err(format!("{chord} is reserved for interrupt / quit"));
            }
            if let Some(other) = KeyAction::ALL
                .iter()
                .find(|other| **other != action && self.bindings[other].contains(chord))
            {
                return Err(format!("{chord} is already used by {}", other.label()));
            }
            // Global shortcuts must leave plain typing and fixed list navigation available.
            if !defaults(action).contains(chord)
                && ((chord.modifiers.is_empty() && !matches!(chord.code, KeyCode::F(_)))
                    || (chord.modifiers == KeyModifiers::SHIFT
                        && matches!(chord.code, KeyCode::Char(_))))
            {
                return Err(format!("{chord} is reserved for typing / local navigation"));
            }
        }
        Ok(())
    }

    pub fn overrides(&self) -> BTreeMap<String, toml::Value> {
        self.bindings
            .iter()
            .filter(|(a, _)| self.customised(**a))
            .map(|(a, chords)| {
                (
                    a.name().to_owned(),
                    toml::Value::String(chords[0].to_string()),
                )
            })
            .collect()
    }

    fn preserve_ctrl_v_override(&mut self, candidates: &BTreeMap<KeyAction, KeyChord>) {
        let ctrl_v = KeyChord {
            code: KeyCode::Char('v'),
            modifiers: KeyModifiers::CONTROL,
        };
        if candidates.contains_key(&KeyAction::PasteImage)
            || !candidates.values().any(|chord| *chord == ctrl_v)
        {
            return;
        }
        // Ctrl+V was previously free: keep an existing override that owns it.
        self.bindings
            .entry(KeyAction::PasteImage)
            .and_modify(|chords| {
                debug_assert!(chords.contains(&ctrl_v));
                chords.retain(|chord| *chord != ctrl_v);
                debug_assert!(!chords.is_empty(), "Alt+V remains available");
            });
        self.paste_image_fallback = true;
    }

    pub fn load(raw: &BTreeMap<String, toml::Value>) -> (Self, Vec<String>) {
        let mut keys;
        let mut notices = Vec::new();
        let mut candidates = BTreeMap::new();
        for (name, value) in raw {
            let parsed = KeyAction::parse(name)
                .ok_or_else(|| "unknown action".to_owned())
                .and_then(|a| {
                    value
                        .as_str()
                        .ok_or_else(|| "expected a chord string".to_owned())
                        .and_then(str::parse::<KeyChord>)
                        .map(|chord| (a, chord))
                });
            match parsed {
                Ok((a, chord)) => {
                    candidates.insert(a, chord);
                }
                Err(error) => notices.push(format!(
                    "keys.{}: {error}; using default",
                    name.escape_debug()
                )),
            }
        }
        // Reject conflicts before applying overrides, including conflicts exposed by a fallback.
        loop {
            keys = Self::default();
            for (a, chord) in &candidates {
                keys.bindings.insert(*a, vec![*chord]);
            }
            keys.preserve_ctrl_v_override(&candidates);
            let bad: Vec<_> = candidates
                .iter()
                .filter_map(|(a, chord)| keys.validate(*a, &[*chord]).err().map(|e| (*a, e)))
                .collect();
            if bad.is_empty() {
                break;
            }
            for (a, error) in bad {
                candidates.remove(&a);
                notices.push(format!("keys.{}: {error}; using default", a.name()));
            }
        }
        (keys, notices)
    }
}

#[cfg(test)]
mod tests;
