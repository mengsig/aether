//! The bottom pane (SPEC §1.1): strips, status row, composer or the active
//! view, status line and hint row. Integration renders the parts in
//! `ui::layout::FrameLayout` order.

pub mod agents_strip;
pub mod approval;
pub mod composer;
pub mod find;
pub mod footer;
pub mod pending;
pub mod popup;
pub mod shortcuts;
pub mod status_line;
pub mod status_row;

use std::fmt;

use crossterm::event::KeyEvent;
use ratatui::Frame;
use ratatui::layout::Rect;

use aether_core::id::{AgentId, ApprovalId};

use crate::app::{App, Effect};
use crate::commands::SlashCommand;
use crate::ui::theme::tokens::Theme;

/// Bottom-pane state that outlives a frame. Holds no `Rect`s or `Line`s.
#[derive(Debug, Default)]
pub struct BottomPane {
    /// Composer extras; the draft text itself stays in `App::input`. W4.
    pub composer: composer::ComposerState,
    /// `/`, `@` or Ctrl+R list above the composer; typing still edits the draft. W4.
    pub popup: Option<popup::Popup>,
    /// `?` shortcuts panel. W4.
    pub shortcuts: bool,
    /// Key rows the `?` panel has scrolled past; reset when it opens.
    pub shortcuts_scroll: usize,
    /// Ctrl+F find bar. W4.
    pub find: Option<find::FindBar>,
    /// Approval view state; shown whenever an undeferred approval is due. W5.
    pub approval: approval::ApprovalView,
    /// Slash views (`/model`, `/memory`, …), topmost last. W7 implements, W6 pushes.
    pub view_stack: Vec<Box<dyn BottomView>>,
}

impl BottomPane {
    pub fn top_view(&self) -> Option<&dyn BottomView> {
        self.view_stack.last().map(|v| v.as_ref())
    }

    pub fn push_view(&mut self, view: Box<dyn BottomView>) {
        self.view_stack.push(view);
    }

    /// Whether a view of `kind` is open anywhere in the stack.
    pub fn showing(&self, kind: crate::ui::views::ViewKind) -> bool {
        self.view_stack.iter().any(|v| v.kind() == Some(kind))
    }
}

/// What a view answers to one key.
#[derive(Debug)]
pub enum ViewOutcome {
    /// Not for this view; dispatch continues (board → global → composer).
    Ignored,
    /// Handled; the view stays open.
    Consumed,
    /// Handled; the view closes after `actions` are applied.
    Close(Vec<ViewAction>),
    /// Handled; the view stays open and `actions` are applied.
    Act(Vec<ViewAction>),
}

/// What a view asks the app to do. Applied by `App::apply_view_actions` (W6),
/// so views never mutate `App` themselves.
#[derive(Debug)]
pub enum ViewAction {
    SetKey {
        action: crate::keys::KeyAction,
        chord: Option<crate::keys::KeyChord>,
    },
    ResetKeys,
    /// Paint a theme without saving; None restores the saved one.
    PreviewTheme(Option<crate::ui::theme::ThemeChoice>),
    /// Run a parsed slash command through the normal command path.
    Command(SlashCommand),
    /// A raw effect (requests carry their own `Then`).
    Effect(Box<Effect>),
    /// Put text into the composer at the cursor.
    InsertText(String),
    /// Make `agent` the viewed agent.
    ViewAgent(AgentId),
    /// Open another view above this one (list → pager).
    Push(Box<dyn BottomView>),
    Open(crate::ui::views::ViewKind), // via `App::open_view`, which may arm app state first
    /// Transient hint-row feedback for 2 s.
    Flash(String),
    /// Answer an approval (approval view only).
    Answer(ApprovalId, crate::app::approval_flow::ApprovalAnswer),
    /// Pin a workspace to the top of the picker, or unpin it; saved at once.
    PinWorkspace {
        path: std::path::PathBuf,
        pinned: bool,
    },
}

/// What Ctrl+C does to the top view (§4.2 rung 1).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CancelOutcome {
    Close,
    /// The view handled it internally (e.g. cleared its filter).
    Consumed,
}

/// A bottom-pane view replacing the composer while open (Codex `BottomPaneView`).
/// Dispatch pops the view off the stack, calls `handle_key(key, &app)`, then
/// re-pushes it unless it closed, so a view may read `App` while being mutated.
/// `Send` because `App` moves into the plain front end's task.
pub trait BottomView: fmt::Debug + Send {
    /// Local keys: configured modified/function shortcuts bypass this handler.
    fn handle_key(&mut self, key: KeyEvent, app: &App) -> ViewOutcome;

    /// A chord the view claims for itself; ignored ones go on to the
    /// global keys. Opt-in, so a view's letter keys never act on Ctrl+C.
    fn handle_chord(&mut self, key: KeyEvent, app: &App) -> ViewOutcome {
        let _ = (key, app);
        ViewOutcome::Ignored
    }

    /// Only scrollable lists and pagers opt in; confirmations keep their defaults.
    fn wheel(&mut self, _up: bool, _app: &App) -> ViewOutcome {
        ViewOutcome::Consumed
    }

    /// Rows wanted at `width`, padding included; layout may give fewer.
    fn desired_height(&self, width: u16, app: &App) -> u16;

    fn render(&self, frame: &mut Frame, area: Rect, app: &App, theme: &Theme);

    /// Bracketed paste while the view is open; ignored by default.
    fn handle_paste(&mut self, text: &str) -> ViewOutcome {
        let _ = text;
        ViewOutcome::Ignored
    }

    fn on_ctrl_c(&mut self) -> CancelOutcome {
        CancelOutcome::Close
    }

    /// The artifact/report pager is the one full-screen view (§8 Phase 6).
    fn fullscreen(&self) -> bool {
        false
    }

    /// Hint-row text while this view is open (`enter confirm · esc back`).
    fn hints(&self) -> &'static str {
        ""
    }

    /// Which slash view this is, for views whose data the reducer refreshes live.
    fn kind(&self) -> Option<crate::ui::views::ViewKind> {
        None
    }

    /// Closed on disconnect: answering it would act on a connection that is gone.
    fn abandon_on_disconnect(&self) -> bool {
        false
    }
}
