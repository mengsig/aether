use crate::keys::{KeyAction, KeyBindings};
use std::borrow::Cow;

/// Where a binding applies, so the help can group it the way a person meets
/// it and a test can drive it in the right state.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum KeyScope {
    /// Anywhere, whatever has focus.
    Global,
    /// While typing in the composer.
    Input,
    /// While the board has focus.
    Panel,
    /// While an approval or a list view replaces the composer.
    Overlay,
}

impl KeyScope {
    pub fn title(self) -> &'static str {
        match self {
            KeyScope::Global => "anywhere",
            KeyScope::Input => "in the composer",
            KeyScope::Panel => "in the board (Ctrl+G)",
            KeyScope::Overlay => "in an approval or a list",
        }
    }
}

/// A heading of the `?` panel. Each has a default [`KeyScope`]; essential
/// composer help appears in Session with its input scope preserved. Voice is
/// split out of `Global` because the TUI cannot tell whether it is set up.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum KeyGroup {
    Compose,
    Session,
    Board,
    Lists,
    Voice,
}

impl KeyGroup {
    /// The panel's reading order.
    pub const ALL: [KeyGroup; 5] = [
        KeyGroup::Session,
        KeyGroup::Compose,
        KeyGroup::Board,
        KeyGroup::Lists,
        KeyGroup::Voice,
    ];

    pub fn heading(self) -> &'static str {
        match self {
            KeyGroup::Compose => "Compose",
            KeyGroup::Session => "Session",
            KeyGroup::Board => "Agents board (Ctrl+G)",
            KeyGroup::Lists => "Approvals & lists",
            KeyGroup::Voice => "Voice (/voice to set up)",
        }
    }

    const fn scope(self) -> KeyScope {
        match self {
            KeyGroup::Compose => KeyScope::Input,
            KeyGroup::Session | KeyGroup::Voice => KeyScope::Global,
            KeyGroup::Board => KeyScope::Panel,
            KeyGroup::Lists => KeyScope::Overlay,
        }
    }
}

/// Widest `label` the `?` panel is laid out for; labels are rewritten to
/// fit, never cut.
pub const MAX_LABEL_WIDTH: usize = 24;

/// Help grouping and default documentation. Configurable rows resolve their
/// chords and labels through `keys::KeyAction`; local navigation stays fixed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct KeySpec {
    pub keys: &'static str,
    pub bindings: &'static [KeyAction],
    pub action: &'static str,
    pub scope: KeyScope,
    pub group: KeyGroup,
    /// `keys` as the `?` panel shows them: `/` between alternatives, a shared
    /// modifier written once.
    pub chord: &'static str,
    /// `action` for the `?` panel, at most [`MAX_LABEL_WIDTH`] columns.
    pub label: &'static str,
}

const fn key(
    group: KeyGroup,
    keys: &'static str,
    action: &'static str,
    label: &'static str,
) -> KeySpec {
    KeySpec {
        keys,
        bindings: &[],
        action,
        scope: group.scope(),
        group,
        chord: keys,
        label,
    }
}

use KeyGroup::{Board, Compose, Lists, Session, Voice};

/// The binding set (SPEC §4), in the order a person meets them.
pub const KEYS: &[KeySpec] = &[
    key(
        Compose,
        "Enter",
        "send; while the agent runs, steer it",
        "send · steers if running",
    )
    .bound(&[KeyAction::Send]),
    key(
        Compose,
        "Tab",
        "queue for after this turn; when idle, send",
        "queue for after the turn",
    )
    .bound(&[KeyAction::Queue]),
    // The panel names the usual chord; the docs list all three.
    key(
        Compose,
        "Shift+Enter / Alt+Enter / Ctrl+J",
        "newline without sending",
        "newline · or Ctrl+J",
    )
    .bound(&[KeyAction::Newline])
    .shown_as("Shift+Enter"),
    key(
        Compose,
        "↑ / ↓",
        "previous / next message from history",
        "message history",
    )
    .shown_as("↑/↓"),
    key(
        Compose,
        "Shift+←",
        "pull the last queued message back to edit",
        "edit last queued",
    )
    .bound(&[KeyAction::Unqueue]),
    key(
        Session,
        "?",
        "shortcuts, on an empty draft",
        "shortcuts (empty draft)",
    )
    .bound(&[KeyAction::Shortcuts])
    .in_scope(KeyScope::Input),
    key(
        Compose,
        "/",
        "commands, at the start of a draft",
        "commands (draft start)",
    ),
    key(
        Compose,
        "@",
        "mention a file or an agent",
        "mention a file or agent",
    ),
    key(
        Compose,
        "Ctrl+A / Ctrl+E",
        "start / end of the line",
        "line start / end",
    )
    .bound(&[KeyAction::LineStart, KeyAction::LineEnd])
    .shown_as("Ctrl+A/E"),
    key(
        Compose,
        "Ctrl+W / Ctrl+U / Ctrl+K",
        "delete word back / to line start / to line end",
        "erase word / start / end",
    )
    .bound(&[
        KeyAction::EraseWord,
        KeyAction::EraseStart,
        KeyAction::EraseEnd,
    ])
    .shown_as("Ctrl+W/U/K"),
    key(
        Compose,
        "Ctrl+Y",
        "paste what was last deleted",
        "paste last erased",
    )
    .bound(&[KeyAction::Yank]),
    key(
        Compose,
        "Ctrl+V / Alt+V",
        "paste the clipboard image, else its text (Ctrl+Shift+V: text) / only an image",
        "paste image, else text",
    )
    .bound(&[KeyAction::PasteImage])
    .shown_as("Ctrl+V"),
    key(
        Compose,
        "Alt+B / Alt+F / Ctrl+← / Ctrl+→",
        "move a word back / forward",
        "move by word · Alt+B/F",
    )
    .bound(&[KeyAction::WordLeft, KeyAction::WordRight])
    .shown_as("Ctrl+←/→"),
    key(
        Session,
        "Esc",
        "close what is open, leave the board, else interrupt the turn",
        "close, else interrupt",
    ),
    key(
        Session,
        "Ctrl+C",
        "close, clear the draft, interrupt, else quit (agents keep running)",
        "clear, interrupt, quit",
    ),
    key(
        Session,
        "Ctrl+D",
        "quit on an empty draft (agents keep running)",
        "quit on empty draft",
    ),
    key(
        Session,
        "Ctrl+G",
        "go to the agents board, opening it if hidden",
        "go to the agents",
    )
    .bound(&[KeyAction::Agents]),
    key(
        Session,
        "Ctrl+B",
        "show / hide the agents board",
        "show / hide the board",
    )
    .bound(&[KeyAction::Board]),
    key(
        Session,
        "Ctrl+O",
        "switch workspace: view its agent, or start one there",
        "switch workspace",
    )
    .bound(&[KeyAction::Workspace]),
    key(
        Session,
        "Alt+O",
        "files in the focused or nearest message with references",
        "files in message",
    )
    .bound(&[KeyAction::Files]),
    key(
        Session,
        "Alt+E",
        "open the viewed file externally",
        "open file externally",
    )
    .bound(&[KeyAction::ViewerExternal]),
    key(
        Session,
        "Alt+W",
        "close the file viewer",
        "close file viewer",
    )
    .bound(&[KeyAction::ViewerClose]),
    key(
        Session,
        "Alt+Z",
        "toggle full screen file viewer",
        "full screen viewer",
    )
    .bound(&[KeyAction::ViewerFull]),
    key(Session, "Alt+-", "narrow the file viewer", "narrow viewer")
        .bound(&[KeyAction::ViewerLess]),
    key(Session, "Alt+=", "widen the file viewer", "widen viewer").bound(&[KeyAction::ViewerMore]),
    key(
        Session,
        "Alt+T",
        "switch a PDF to text or an HTML file to a screenshot",
        "PDF/HTML text or image",
    )
    .bound(&[KeyAction::ViewerMode]),
    key(
        Session,
        "Alt+N",
        "the notification center: answer, mark read, resolve, go to the chat",
        "notification center",
    )
    .bound(&[KeyAction::Notifications]),
    key(
        Session,
        "Alt+↑",
        "view the parent agent",
        "view the parent agent",
    )
    .bound(&[KeyAction::Parent]),
    key(
        Session,
        "Ctrl+T",
        "details: full outputs and reasoning",
        "details & reasoning",
    )
    .bound(&[KeyAction::Details]),
    key(
        Session,
        "Ctrl+R",
        "search message history",
        "search history",
    )
    .bound(&[KeyAction::History]),
    key(
        Session,
        "Ctrl+F",
        "find in the transcript",
        "find in transcript",
    )
    .bound(&[KeyAction::Find]),
    key(
        Session,
        "Alt+, / Alt+.",
        "less / more reasoning effort",
        "less / more effort",
    )
    .bound(&[KeyAction::LessEffort, KeyAction::MoreEffort])
    .shown_as("Alt+,/."),
    key(
        Session,
        "PgUp / PgDn",
        "scroll the transcript",
        "scroll a page",
    )
    .bound(&[KeyAction::PageUp, KeyAction::PageDown])
    .shown_as("PgUp/PgDn"),
    key(
        Session,
        "Shift+↑ / Shift+↓",
        "scroll the transcript a line",
        "scroll a line",
    )
    .bound(&[KeyAction::LineUp, KeyAction::LineDown])
    .shown_as("Shift+↑/↓"),
    key(
        Session,
        "Ctrl+Home / Ctrl+End",
        "top of the transcript / latest, following again",
        "top / follow latest",
    )
    .bound(&[KeyAction::Top, KeyAction::Latest])
    .shown_as("Ctrl+Home/End"),
    key(
        Voice,
        "F4",
        "start/stop local dictation; review the draft before sending",
        "start / stop dictation",
    )
    .bound(&[KeyAction::Dictate]),
    key(
        Voice,
        "F5",
        "insert recovered dictation at the cursor without replacing the draft",
        "insert recovered draft",
    )
    .bound(&[KeyAction::InsertDictation]),
    key(Board, "↑ / ↓", "move between agents", "move between agents").shown_as("↑/↓"),
    key(
        Board,
        "← / →",
        "fold / unfold, or go to the parent / first child",
        "fold / unfold",
    )
    .shown_as("←/→"),
    key(
        Board,
        "Home / End",
        "first / last agent",
        "first / last agent",
    )
    .shown_as("Home/End"),
    key(Board, "Enter", "view the agent", "view the agent"),
    key(
        Board,
        "Shift+X",
        "confirm ending the agent and all live descendants",
        "stop agent and workers",
    ),
    key(
        Board,
        "Shift+D",
        "archive this agent; confirm stop first if live; restore an archived row",
        "remove / restore agent",
    ),
    key(
        Board,
        "Shift+P",
        "pause or resume the agent subtree",
        "pause / resume subtree",
    ),
    key(
        Board,
        "Shift+I",
        "interrupt only the current turn; keep the agent",
        "interrupt current turn",
    ),
    key(
        Board,
        ".",
        "show or hide archived agents",
        "show / hide archived",
    ),
    key(
        Board,
        "Esc / Ctrl+G",
        "back to the composer",
        "back to the composer",
    )
    .bound(&[KeyAction::Agents])
    .shown_as("Esc/Ctrl+G"),
    key(
        Lists,
        "y / p / n",
        "allow once / always, or deny",
        "once/always/deny",
    )
    .shown_as("y/p/n"),
    key(
        Lists,
        "a",
        "save an allow rule for this agent and its subagents",
        "save for agent+subagents",
    ),
    key(
        Lists,
        "o",
        "open the agent that asks",
        "open the asking agent",
    ),
    key(Lists, "↑ / ↓ / 1-9", "choose a row", "choose a row").shown_as("↑/↓/1-9"),
    key(
        Lists,
        "Enter",
        "use the highlighted row",
        "use the highlighted row",
    ),
];

impl KeySpec {
    const fn bound(mut self, actions: &'static [KeyAction]) -> Self {
        self.bindings = actions;
        if actions.len() == 1 && !matches!(self.scope, KeyScope::Panel) {
            self.label = actions[0].label();
        }
        self
    }

    pub fn live_chord(&self, keys: &KeyBindings, ascii: bool) -> Cow<'static, str> {
        if self.bindings.is_empty() {
            return self.display_chord(ascii);
        }
        let mut text = keys.help_chord(self.bindings, ascii);
        if self.keys == "Esc / Ctrl+G" {
            text = format!("Esc/{text}");
        }
        Cow::Owned(text)
    }

    const fn in_scope(mut self, scope: KeyScope) -> Self {
        self.scope = scope;
        self
    }
    /// The panel shows `chord` in place of `keys`.
    const fn shown_as(mut self, chord: &'static str) -> Self {
        self.chord = chord;
        self
    }

    pub fn live_label(&self, keys: &KeyBindings, ascii: bool) -> Cow<'static, str> {
        if self.bindings.len() == 1 && self.scope != KeyScope::Panel {
            let label = self.bindings[0].label();
            return if ascii {
                Cow::Owned(label.replace(" · ", ", "))
            } else {
                Cow::Borrowed(label)
            };
        }
        if self.bindings == [KeyAction::WordLeft, KeyAction::WordRight]
            && self.bindings.iter().any(|a| keys.customised(*a))
        {
            return Cow::Borrowed("move by word");
        }
        self.display_label(ascii)
    }

    pub fn display_keys(&self, ascii: bool) -> &'static str {
        if !ascii {
            return self.keys;
        }
        match self.keys {
            "↑ / ↓" => "Up / Down",
            "Shift+←" => "Shift+Left",
            "Alt+B / Alt+F / Ctrl+← / Ctrl+→" => "Alt+B / Alt+F / Ctrl+Left / Ctrl+Right",
            "Alt+↑" => "Alt+Up",
            "Shift+↑ / Shift+↓" => "Shift+Up / Shift+Down",
            "← / →" => "Left / Right",
            "↑ / ↓ / 1-9" => "Up / Down / 1-9",
            text => text,
        }
    }

    /// Every action is plain ASCII, so both modes read the same.
    pub fn display_action(&self, ascii: bool) -> &'static str {
        let _ = ascii;
        self.action
    }

    /// [`Self::chord`], with arrows spelled out in ASCII mode.
    pub fn display_chord(&self, ascii: bool) -> Cow<'static, str> {
        if !ascii {
            return Cow::Borrowed(self.chord);
        }
        let mut text = String::new();
        for c in self.chord.chars() {
            match c {
                '←' => text.push_str("Left"),
                '→' => text.push_str("Right"),
                '↑' => text.push_str("Up"),
                '↓' => text.push_str("Down"),
                c => text.push(c),
            }
        }
        Cow::Owned(text)
    }

    /// [`Self::label`]; ASCII mode turns its ` · ` into a comma.
    pub fn display_label(&self, ascii: bool) -> Cow<'static, str> {
        if ascii {
            Cow::Owned(self.label.replace(" · ", ", "))
        } else {
            Cow::Borrowed(self.label)
        }
    }
}

#[cfg(test)]
mod tests {
    use unicode_width::UnicodeWidthStr;

    use super::*;

    #[test]
    fn panel_labels_are_short_and_never_cut() {
        for spec in KEYS {
            for ascii in [false, true] {
                let label = spec.display_label(ascii);
                assert!(label.width() <= MAX_LABEL_WIDTH, "{label:?} is too long");
                assert!(!label.contains('…'), "{label:?}");
            }
        }
    }

    /// A key listed twice under one heading would need its context spelled out.
    #[test]
    fn no_chord_appears_twice_under_one_heading() {
        for group in KeyGroup::ALL {
            let mut seen = std::collections::HashSet::new();
            for spec in KEYS.iter().filter(|k| k.group == group) {
                assert!(
                    seen.insert(spec.chord),
                    "{:?} twice in {group:?}",
                    spec.chord
                );
            }
            assert!(!seen.is_empty(), "{group:?} has no keys");
        }
    }

    #[test]
    fn ascii_chords_spell_out_arrows() {
        let spec = KEYS.iter().find(|k| k.keys == "Shift+↑ / Shift+↓").unwrap();
        assert_eq!(spec.display_chord(false), "Shift+↑/↓");
        assert_eq!(spec.display_chord(true), "Shift+Up/Down");
        for spec in KEYS {
            assert!(spec.display_chord(true).is_ascii(), "{:?}", spec.chord);
        }
    }
}
