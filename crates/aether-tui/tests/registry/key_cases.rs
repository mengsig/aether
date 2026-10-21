use aether_tui::commands::KeyScope;
use crossterm::event::{KeyCode, KeyModifiers};

/// What the row needs before it can do anything.
#[derive(Clone, Copy, Debug)]
pub(super) enum Stage {
    /// The viewed agent mid-turn; no approval.
    Busy,
    /// The viewed agent idle, nothing typed.
    Idle,
    /// A draft in the composer, so an edit has something to edit.
    Draft,
    /// One sent message in history, empty composer.
    History,
    /// A child agent viewed, so there is a parent to go back to.
    Child,
    /// The docked board focused, cursor on a live child of the viewed root.
    Board,
    /// An approval showing.
    Approval,
    /// An approval from a child of the viewed agent showing.
    ChildApproval,
    /// The viewed agent mid-turn with one message queued for after it.
    Queued,
    /// A word cut with Ctrl+W, so the kill ring has something to paste.
    Killed,
    Viewer,
    /// A PDF page shown as text, with pages either side.
    Document,
    VoiceCapture,
    VoiceRecovery,
}

pub(super) struct KeyCase {
    pub keys: &'static str,
    pub scope: KeyScope,
    pub code: KeyCode,
    pub mods: KeyModifiers,
    pub stage: Stage,
}

pub(super) fn cases() -> Vec<KeyCase> {
    use KeyScope::{Global, Input, Overlay, Panel};
    use Stage::*;
    let none = KeyModifiers::NONE;
    let ctrl = KeyModifiers::CONTROL;
    let alt = KeyModifiers::ALT;
    let shift = KeyModifiers::SHIFT;
    let case = |keys, scope, code, mods, stage| KeyCase {
        keys,
        scope,
        code,
        mods,
        stage,
    };
    // One representative chord per row of KEYS (more where the row lists several).
    vec![
        case("Enter", Input, KeyCode::Enter, none, Draft),
        case("Tab", Input, KeyCode::Tab, none, Draft),
        case(
            "Shift+Enter / Alt+Enter / Ctrl+J",
            Input,
            KeyCode::Enter,
            shift,
            Draft,
        ),
        case(
            "Shift+Enter / Alt+Enter / Ctrl+J",
            Input,
            KeyCode::Enter,
            alt,
            Draft,
        ),
        case(
            "Shift+Enter / Alt+Enter / Ctrl+J",
            Input,
            KeyCode::Char('j'),
            ctrl,
            Draft,
        ),
        case("↑ / ↓", Input, KeyCode::Up, none, History),
        case("Shift+←", Input, KeyCode::Left, shift, Queued),
        case("?", Input, KeyCode::Char('?'), none, Idle),
        case("/", Input, KeyCode::Char('/'), none, Idle),
        case("@", Input, KeyCode::Char('@'), none, Idle),
        case("Ctrl+A / Ctrl+E", Input, KeyCode::Char('a'), ctrl, Draft),
        case(
            "Ctrl+W / Ctrl+U / Ctrl+K",
            Input,
            KeyCode::Char('w'),
            ctrl,
            Draft,
        ),
        case("Ctrl+Y", Input, KeyCode::Char('y'), ctrl, Killed),
        case("Ctrl+V / Alt+V", Input, KeyCode::Char('v'), ctrl, Idle),
        case("Ctrl+V / Alt+V", Input, KeyCode::Char('v'), alt, Idle),
        case(
            "Alt+B / Alt+F / Ctrl+← / Ctrl+→",
            Input,
            KeyCode::Char('b'),
            alt,
            Draft,
        ),
        case("Esc", Global, KeyCode::Esc, none, Busy),
        case("Ctrl+C", Global, KeyCode::Char('c'), ctrl, Busy),
        case("Ctrl+C", Global, KeyCode::Char('c'), ctrl, Idle),
        case("Ctrl+D", Global, KeyCode::Char('d'), ctrl, Idle),
        case("Ctrl+G", Global, KeyCode::Char('g'), ctrl, Busy),
        case("Ctrl+B", Global, KeyCode::Char('b'), ctrl, Busy),
        case("Ctrl+O", Global, KeyCode::Char('o'), ctrl, Busy),
        case("Ctrl+O", Global, KeyCode::Char('o'), ctrl, Idle),
        case("Alt+O", Global, KeyCode::Char('o'), alt, Idle),
        case("Alt+E", Global, KeyCode::Char('e'), alt, Viewer),
        case("Alt+W", Global, KeyCode::Char('w'), alt, Viewer),
        case("Alt+Z", Global, KeyCode::Char('z'), alt, Viewer),
        case("Alt+-", Global, KeyCode::Char('-'), alt, Viewer),
        case("Alt+=", Global, KeyCode::Char('='), alt, Viewer),
        case("Alt+T", Global, KeyCode::Char('t'), alt, Document),
        case("Alt+N", Global, KeyCode::Char('n'), alt, Idle),
        case("Alt+↑", Global, KeyCode::Up, alt, Child),
        case("Ctrl+T", Global, KeyCode::Char('t'), ctrl, Busy),
        case("Ctrl+R", Global, KeyCode::Char('r'), ctrl, History),
        case("Ctrl+F", Global, KeyCode::Char('f'), ctrl, Busy),
        case("Alt+, / Alt+.", Global, KeyCode::Char(','), alt, Busy),
        case("Alt+, / Alt+.", Global, KeyCode::Char('.'), alt, Busy),
        case("PgUp / PgDn", Global, KeyCode::PageUp, none, Busy),
        case("Shift+↑ / Shift+↓", Global, KeyCode::Up, shift, Busy),
        case("Ctrl+Home / Ctrl+End", Global, KeyCode::End, ctrl, Busy),
        case("F4", Global, KeyCode::F(4), none, VoiceCapture),
        case("F5", Global, KeyCode::F(5), none, VoiceRecovery),
        case("↑ / ↓", Panel, KeyCode::Up, none, Board),
        case("← / →", Panel, KeyCode::Left, none, Board),
        case("Home / End", Panel, KeyCode::Home, none, Board),
        case("Enter", Panel, KeyCode::Enter, none, Board),
        case("Shift+X", Panel, KeyCode::Char('X'), none, Board),
        case("Shift+D", Panel, KeyCode::Char('d'), shift, Board),
        case("Shift+P", Panel, KeyCode::Char('P'), shift, Board),
        case("Shift+I", Panel, KeyCode::Char('i'), shift, Board),
        case(".", Panel, KeyCode::Char('.'), none, Board),
        case("Esc / Ctrl+G", Panel, KeyCode::Esc, none, Board),
        case("Esc / Ctrl+G", Panel, KeyCode::Char('g'), ctrl, Board),
        case("y / p / n", Overlay, KeyCode::Char('y'), none, Approval),
        case("a", Overlay, KeyCode::Char('a'), none, Approval),
        case("o", Overlay, KeyCode::Char('o'), none, ChildApproval),
        case("↑ / ↓ / 1-9", Overlay, KeyCode::Down, none, Approval),
        case("Enter", Overlay, KeyCode::Enter, none, Approval),
    ]
}
