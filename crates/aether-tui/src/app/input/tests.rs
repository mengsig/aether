//! W6 dispatch tests: ladders, focus, `q`, view actions, chords.

use aether_core::agent::AgentStatus;
use aether_core::id::AgentId;
use aether_core::model::Effort;
use aether_protocol::Command;
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers, MouseEvent, MouseEventKind};
use ratatui::Frame;
use ratatui::layout::Rect;

use crate::app::approval_flow::TYPING_GUARD_MS;
use crate::app::approval_flow::fixtures::read_card;
use crate::app::board::BoardFocus;
use crate::app::transcript::ScrollIntent;
use crate::app::{App, AppEvent, Effect};
use crate::commands::SlashCommand;
use crate::prefs::{MouseMode, UiPrefs};
use crate::testing::populated_app;
use crate::ui::bottom::find::FindBar;
use crate::ui::bottom::popup::{Popup, PopupKind};
use crate::ui::bottom::{BottomView, CancelOutcome, ViewAction, ViewOutcome};
use crate::ui::theme::tokens::{Theme, ThemeMode};

const DOCKED: (u16, u16) = (120, 40);
const NARROW: (u16, u16) = (80, 40);

/// One running agent, viewed, no approvals (those belong to W5's flow).
fn app() -> App {
    let mut app = populated_app();
    app.approvals.clear();
    app.size = DOCKED;
    app
}

fn viewed(app: &App) -> AgentId {
    app.selected.expect("the fixture views its agent")
}

fn set_status(app: &mut App, status: AgentStatus) {
    let id = viewed(app);
    app.tree
        .nodes
        .get_mut(&id)
        .expect("viewed node")
        .agent
        .status = status;
}

/// Adds a child of the viewed agent and views it.
fn view_child(app: &mut App) -> (AgentId, AgentId) {
    let parent = viewed(app);
    let mut child = app.tree.nodes[&parent].clone();
    let id = AgentId::new();
    child.agent.id = id;
    child.agent.parent = Some(parent);
    child.agent.name = "child".into();
    child.children.clear();
    app.tree.nodes.insert(id, child);
    app.tree
        .nodes
        .get_mut(&parent)
        .expect("parent node")
        .children
        .push(id);
    app.select(id);
    (parent, id)
}

fn key(code: KeyCode) -> KeyEvent {
    KeyEvent::new(code, KeyModifiers::NONE)
}

fn ctrl(c: char) -> KeyEvent {
    KeyEvent::new(KeyCode::Char(c), KeyModifiers::CONTROL)
}

fn alt(code: KeyCode) -> KeyEvent {
    KeyEvent::new(code, KeyModifiers::ALT)
}

fn press(app: &mut App, event: KeyEvent) -> Vec<Effect> {
    app.dispatch_key(event)
}

fn type_text(app: &mut App, text: &str) {
    for c in text.chars() {
        press(app, key(KeyCode::Char(c)));
    }
}

fn quits(effects: &[Effect]) -> bool {
    effects.iter().any(|e| matches!(e, Effect::Quit))
}

fn interrupts(effects: &[Effect], agent: AgentId) -> bool {
    effects.iter().any(|e| {
        matches!(e, Effect::Request { command: Command::Interrupt { agent: a , idempotency_key: None, }, .. } if *a == agent)
    })
}

fn scrolls(app: &App) -> Vec<ScrollIntent> {
    app.scroll_log.since(0).map(|r| r.intent).collect()
}

/// A bottom view that ignores keys (so the ladders see them) unless told otherwise.
#[derive(Debug, Default)]
struct StubView {
    keep_on_ctrl_c: bool,
    consume_esc: bool,
    act: Option<fn() -> Vec<ViewAction>>,
    fullscreen: bool,
}

impl BottomView for StubView {
    fn handle_key(&mut self, key: KeyEvent, _app: &App) -> ViewOutcome {
        if let Some(act) = self.act {
            return ViewOutcome::Act(act());
        }
        if self.consume_esc && key.code == KeyCode::Esc {
            return ViewOutcome::Consumed;
        }
        ViewOutcome::Ignored
    }

    fn desired_height(&self, _width: u16, _app: &App) -> u16 {
        1
    }

    fn render(&self, _frame: &mut Frame, _area: Rect, _app: &App, _theme: &Theme) {}

    fn fullscreen(&self) -> bool {
        self.fullscreen
    }

    fn on_ctrl_c(&mut self) -> CancelOutcome {
        if self.keep_on_ctrl_c {
            CancelOutcome::Consumed
        } else {
            CancelOutcome::Close
        }
    }
}

fn history_popup(saved: &str) -> Popup {
    Popup {
        saved_draft: Some(saved.into()),
        ..Popup::new(PopupKind::History)
    }
}

// ----- Esc ladder ----------------------------------------------------------

#[test]
fn esc_rung1_closes_popup_and_restores_the_history_draft() {
    let mut app = app();
    app.input.set_text("half typed");
    app.bottom.popup = Some(history_popup("my draft"));
    let effects = app.esc_ladder();
    assert!(effects.is_empty());
    assert!(app.bottom.popup.is_none());
    assert_eq!(app.input.text, "my draft");
}

#[test]
fn esc_rung1_closes_shortcuts_then_find_then_the_view() {
    let mut app = app();
    app.bottom.shortcuts = true;
    app.bottom.find = Some(FindBar::default());
    app.bottom.push_view(Box::new(StubView::default()));
    app.esc_ladder();
    assert!(!app.bottom.shortcuts);
    assert!(app.bottom.find.is_some());
    app.esc_ladder();
    assert!(app.bottom.find.is_none());
    assert_eq!(app.bottom.view_stack.len(), 1);
    let effects = press(&mut app, key(KeyCode::Esc));
    assert!(app.bottom.view_stack.is_empty());
    assert!(!interrupts(&effects, viewed(&app)), "closing comes first");
}

#[test]
fn esc_rung2_docked_board_hands_focus_back_and_stays_open() {
    let mut app = app();
    press(&mut app, ctrl('g'));
    assert!(app.board.focused());
    let effects = press(&mut app, key(KeyCode::Esc));
    assert!(app.board.open);
    assert_eq!(app.board.focus, BoardFocus::BottomPane);
    assert!(
        !interrupts(&effects, viewed(&app)),
        "Esc in the board never interrupts"
    );
}

#[test]
fn esc_rung2_overlay_board_closes() {
    let mut app = app();
    app.size = NARROW;
    press(&mut app, ctrl('g'));
    press(&mut app, key(KeyCode::Esc));
    assert!(!app.board.open);
    assert_eq!(app.board.focus, BoardFocus::BottomPane);
}

#[test]
fn esc_rung3_interrupts_the_running_viewed_agent() {
    let mut app = app();
    let effects = press(&mut app, key(KeyCode::Esc));
    assert!(interrupts(&effects, viewed(&app)));
}

#[test]
fn esc_rung4_does_nothing_when_idle() {
    let mut app = app();
    set_status(&mut app, AgentStatus::Ready);
    app.input.set_text("keep me");
    let effects = press(&mut app, key(KeyCode::Esc));
    assert!(effects.is_empty());
    assert_eq!(app.input.text, "keep me");
    assert!(!app.quit_requested);
}

#[test]
fn esc_rung1_denies_a_shown_approval() {
    let mut app = populated_app();
    read_card(&mut app);
    let effects = press(&mut app, key(KeyCode::Esc));
    assert!(!effects.is_empty(), "deny is a request");
    assert!(!interrupts(&effects, viewed(&app)));
}

// ----- Ctrl+C ladder -------------------------------------------------------

#[test]
fn ctrl_c_rung1_closes_the_view_unless_it_consumes() {
    let mut app = app();
    app.bottom.push_view(Box::new(StubView {
        keep_on_ctrl_c: true,
        ..Default::default()
    }));
    press(&mut app, ctrl('c'));
    assert_eq!(app.bottom.view_stack.len(), 1, "the view handled it itself");
    app.bottom.view_stack.clear();
    app.bottom.push_view(Box::new(StubView::default()));
    let effects = press(&mut app, ctrl('c'));
    assert!(app.bottom.view_stack.is_empty());
    assert!(effects.is_empty());
}

#[test]
fn ctrl_c_rung1_2_closes_popups_and_cancels_searches() {
    let mut app = app();
    app.bottom.popup = Some(history_popup("before search"));
    app.ctrl_c_ladder();
    assert!(app.bottom.popup.is_none());
    assert_eq!(app.input.text, "before search");
    app.bottom.find = Some(FindBar::default());
    app.ctrl_c_ladder();
    assert!(app.bottom.find.is_none());
    app.bottom.shortcuts = true;
    app.ctrl_c_ladder();
    assert!(!app.bottom.shortcuts);
    assert_eq!(
        app.input.text, "before search",
        "search rungs keep the draft"
    );
}

#[test]
fn ctrl_c_rung3_clears_the_draft_into_history() {
    let mut app = app();
    type_text(&mut app, "unsent words");
    let effects = press(&mut app, ctrl('c'));
    assert!(effects.is_empty(), "no interrupt while a draft is cleared");
    assert!(app.input.text.is_empty());
    press(&mut app, key(KeyCode::Up));
    assert_eq!(app.input.text, "unsent words", "↑ recovers it");
}

#[test]
fn ctrl_c_rung4_interrupts_the_running_viewed_agent() {
    let mut app = app();
    let effects = press(&mut app, ctrl('c'));
    assert!(interrupts(&effects, viewed(&app)));
    assert!(!quits(&effects));
}

#[test]
fn ctrl_c_rung5_detaches_with_the_exit_message() {
    let mut app = app();
    set_status(&mut app, AgentStatus::Ready);
    let effects = press(&mut app, ctrl('c'));
    assert!(quits(&effects));
    assert!(app.quit_requested);
    assert_eq!(
        app.exit_message.as_deref(),
        Some("Agents keep running in the daemon. Reattach with: aether")
    );
}

#[test]
fn detach_counts_running_agents_and_who_needs_you() {
    let mut app = populated_app();
    view_child(&mut app);
    app.detach();
    assert_eq!(
        app.exit_message.as_deref(),
        Some("Agents keep running in the daemon (2 running · 1 needs you). Reattach with: aether")
    );
    // Zero counts say nothing, so they are left out.
    for id in app.needs_you_agents() {
        app.approvals.retain(|a| a.agent_id != id);
    }
    app.detach();
    assert_eq!(
        app.exit_message.as_deref(),
        Some("Agents keep running in the daemon (2 running). Reattach with: aether")
    );
}

#[test]
fn ctrl_c_rung1_defers_a_shown_approval() {
    let mut app = populated_app();
    let effects = press(&mut app, ctrl('c'));
    assert!(effects.is_empty());
    assert_eq!(app.bottom.approval.deferred.len(), 1);
    assert!(!app.quit_requested);
}

// ----- q never quits -------------------------------------------------------

type Setup = fn(&mut App);

#[test]
fn q_never_quits_in_any_state() {
    let states: [(&str, Setup); 7] = [
        ("empty composer, running", |_| {}),
        ("empty composer, idle", |app| {
            set_status(app, AgentStatus::Ready)
        }),
        ("draft", |app| app.input.set_text("draft")),
        ("board focused", |app| {
            app.dispatch_key(ctrl('g'));
        }),
        ("board overlay", |app| {
            app.size = NARROW;
            app.dispatch_key(ctrl('g'));
        }),
        ("view open", |app| {
            app.bottom.push_view(Box::new(StubView::default()))
        }),
        ("no agent", |app| app.selected = None),
    ];
    for (name, setup) in states {
        let mut app = app();
        setup(&mut app);
        for _ in 0..3 {
            let effects = press(&mut app, key(KeyCode::Char('q')));
            assert!(!quits(&effects), "{name}: q quit");
        }
        assert!(!app.quit_requested, "{name}: q armed a quit");
        assert!(app.exit_message.is_none(), "{name}");
    }
}

#[test]
fn q_types_into_the_composer() {
    let mut app = app();
    type_text(&mut app, "qq");
    assert_eq!(app.input.text, "qq");
}

// ----- focus model (§1.3) --------------------------------------------------

#[test]
fn ctrl_g_opens_and_focuses_a_hidden_board() {
    let mut app = app();
    assert!(!app.board.open);
    press(&mut app, ctrl('g'));
    assert!(app.board.focused());
    assert_eq!(
        app.board.cursor, app.selected,
        "cursor starts on the viewed agent"
    );
}

#[test]
fn ctrl_g_focuses_an_open_board_and_toggles_back_to_the_composer() {
    let mut app = app();
    app.board.open = true;
    app.board.focus = BoardFocus::BottomPane;
    press(&mut app, ctrl('g'));
    assert!(app.board.focused(), "Ctrl+G never hides an open board");
    press(&mut app, ctrl('g'));
    assert!(app.board.open, "the docked board stays on screen");
    assert_eq!(app.board.focus, BoardFocus::BottomPane);
    press(&mut app, ctrl('g'));
    press(&mut app, key(KeyCode::Esc));
    assert!(app.board.open);
    assert_eq!(app.board.focus, BoardFocus::BottomPane);
    app.run_slash(SlashCommand::Agent { name: None });
    assert!(app.board.focused());
}

#[test]
fn ctrl_b_hides_the_board_focused_or_not() {
    for focus in [BoardFocus::Board, BoardFocus::BottomPane] {
        let mut app = app();
        app.board.open = true;
        app.board.focus = focus;
        press(&mut app, ctrl('b'));
        assert!(!app.board.open, "{focus:?}");
        assert_eq!(app.board.focus, BoardFocus::BottomPane, "{focus:?}");
    }
}

#[test]
fn ctrl_b_shows_a_docked_board_without_taking_focus() {
    let mut app = app();
    app.input.set_text("half a thought");
    press(&mut app, ctrl('b'));
    assert!(app.board.open);
    assert_eq!(app.board.focus, BoardFocus::BottomPane);
    type_text(&mut app, "!");
    assert_eq!(app.input.text, "half a thought!", "the composer kept focus");
}

#[test]
fn ctrl_b_shows_the_overlay_focused_and_both_chords_put_it_away() {
    for chord in ['b', 'g'] {
        let mut app = app();
        app.size = NARROW;
        press(&mut app, ctrl('b'));
        assert!(app.board.focused(), "the overlay covers the composer");
        press(&mut app, ctrl(chord));
        assert!(!app.board.open, "Ctrl+{chord} in the overlay");
        assert_eq!(app.board.focus, BoardFocus::BottomPane);
    }
}

#[test]
fn an_open_view_holds_ctrl_g_and_the_overlay_but_not_a_docked_board() {
    let mut app = app();
    app.bottom.push_view(Box::new(StubView::default()));
    press(&mut app, ctrl('g'));
    assert!(!app.board.open);
    let flash = app.live_flash().unwrap_or_default().to_owned();
    assert!(flash.contains("esc closes this view first"), "{flash}");
    press(&mut app, ctrl('b'));
    assert!(app.board.open, "a docked board leaves the view its pane");
    assert_eq!(app.board.focus, BoardFocus::BottomPane);
    press(&mut app, ctrl('g'));
    assert!(!app.board.focused(), "the view keeps the keys");

    let mut app = self::app();
    app.size = NARROW;
    app.bottom.popup = Some(Popup::new(PopupKind::History));
    press(&mut app, ctrl('b'));
    assert!(!app.board.open, "an overlay would hide the popup");
    let flash = app.live_flash().unwrap_or_default().to_owned();
    assert!(flash.contains("esc closes the popup first"), "{flash}");
}

#[test]
fn ctrl_b_never_shows_a_board_a_fullscreen_view_would_hide() {
    let dir = tempfile::tempdir().expect("prefs dir");
    let mut app = app();
    app.options.prefs_dir = Some(dir.path().to_path_buf());
    app.bottom.push_view(Box::new(StubView {
        fullscreen: true,
        ..StubView::default()
    }));
    press(&mut app, ctrl('b'));
    assert!(!app.board.open, "the pager paints over any board");
    assert_eq!(
        saved_board_open(&dir),
        None,
        "nothing was shown, nothing saved"
    );
    let flash = app.live_flash().unwrap_or_default().to_owned();
    assert!(flash.contains("esc closes this view first"), "{flash}");

    app.board.open = true;
    press(&mut app, ctrl('b'));
    assert!(!app.board.open, "hiding still works");
    assert_eq!(saved_board_open(&dir), Some(false));
}

#[test]
fn only_showing_or_hiding_the_board_is_saved() {
    let shown = tempfile::tempdir().expect("prefs dir");
    let mut app = app();
    app.options.prefs_dir = Some(shown.path().to_path_buf());
    press(&mut app, ctrl('b'));
    assert_eq!(saved_board_open(&shown), Some(true));

    let moved = tempfile::tempdir().expect("prefs dir");
    app.options.prefs_dir = Some(moved.path().to_path_buf());
    for chord in [ctrl('g'), ctrl('g'), ctrl('g'), key(KeyCode::Esc)] {
        press(&mut app, chord);
    }
    assert_eq!(saved_board_open(&moved), None, "focus moves are not saved");
    press(&mut app, ctrl('b'));
    assert_eq!(saved_board_open(&moved), Some(false));
}

#[test]
fn typing_in_the_board_is_dropped() {
    let mut app = app();
    press(&mut app, ctrl('g'));
    type_text(&mut app, "hello");
    assert!(app.input.text.is_empty());
    assert!(app.board.focused());
}

#[test]
fn enter_in_the_docked_board_views_and_returns_focus() {
    let mut app = app();
    let (parent, _child) = view_child(&mut app);
    press(&mut app, ctrl('g'));
    app.board.cursor = Some(parent);
    press(&mut app, key(KeyCode::Enter));
    assert_eq!(app.selected, Some(parent));
    assert!(app.board.open);
    assert_eq!(app.board.focus, BoardFocus::BottomPane);
}

#[test]
fn enter_in_the_overlay_board_views_and_closes() {
    let mut app = app();
    app.size = NARROW;
    let (parent, _child) = view_child(&mut app);
    press(&mut app, ctrl('g'));
    app.board.cursor = Some(parent);
    press(&mut app, key(KeyCode::Enter));
    assert_eq!(app.selected, Some(parent));
    assert!(!app.board.open);
}

#[test]
fn enter_on_an_agent_with_approvals_shows_them_again() {
    let mut app = populated_app();
    app.size = DOCKED;
    let agent = viewed(&app);
    let id = app.approvals[0].id;
    app.defer_approval(id);
    app.board.open = true;
    app.board.focus = BoardFocus::Board;
    app.board.cursor = Some(agent);
    press(&mut app, key(KeyCode::Enter));
    assert!(!app.bottom.approval.deferred.contains(&id));
}

/// The populated fixture with its approval due, the board focused, and
/// preferences saved to a scratch directory.
fn board_over_a_due_approval(size: (u16, u16)) -> (App, tempfile::TempDir) {
    let dir = tempfile::tempdir().expect("prefs dir");
    let mut app = populated_app();
    app.options.prefs_dir = Some(dir.path().to_path_buf());
    app.size = size;
    // Keys come a guard after the request, so the typing guard never holds it.
    let latest = app.approvals.iter().map(|a| a.requested_at).max();
    app.now = latest.expect("the fixture has an approval")
        + chrono::Duration::milliseconds(TYPING_GUARD_MS);
    assert!(
        app.due_approval().is_some(),
        "the fixture's approval is due"
    );
    (app, dir)
}

fn saved_board_open(dir: &tempfile::TempDir) -> Option<bool> {
    UiPrefs::load(dir.path()).expect("prefs load").board_open
}

#[test]
fn a_due_approval_takes_focus_back_from_a_docked_board() {
    let (mut app, dir) = board_over_a_due_approval(DOCKED);
    app.board.open = true;
    app.board.focus = BoardFocus::Board;
    app.reduce(AppEvent::Tick { frames: 1 });
    assert!(app.board.open, "a docked board stays on screen");
    assert_eq!(app.board.focus, BoardFocus::BottomPane);
    assert_eq!(saved_board_open(&dir), None);
}

#[test]
fn a_due_approval_closes_an_overlay_board_without_saving() {
    let (mut app, dir) = board_over_a_due_approval(NARROW);
    app.board.open = true;
    app.board.focus = BoardFocus::Board;
    app.reduce(AppEvent::Tick { frames: 1 });
    assert!(!app.board.open);
    assert_eq!(app.board.focus, BoardFocus::BottomPane);
    assert_eq!(saved_board_open(&dir), None);
}

#[test]
fn a_deferred_approval_leaves_the_board_focused() {
    let (mut app, _dir) = board_over_a_due_approval(DOCKED);
    let ids: Vec<_> = app.approvals.iter().map(|a| a.id).collect();
    for id in ids {
        app.defer_approval(id);
    }
    app.board.open = true;
    app.board.focus = BoardFocus::Board;
    app.reduce(AppEvent::Tick { frames: 1 });
    assert!(app.board.focused());
}

#[test]
fn ctrl_b_under_a_due_approval_only_shows_or_hides_a_docked_board() {
    let (mut app, dir) = board_over_a_due_approval(DOCKED);
    let pending = app.approvals.len();
    assert!(app.reduce(AppEvent::Key(ctrl('b'))).is_empty());
    assert!(app.board.open);
    assert_eq!(app.board.focus, BoardFocus::BottomPane);
    assert_eq!(saved_board_open(&dir), Some(true));
    assert!(app.reduce(AppEvent::Key(ctrl('b'))).is_empty());
    assert!(!app.board.open);
    assert_eq!(saved_board_open(&dir), Some(false));
    assert_eq!(app.approvals.len(), pending, "nothing was answered");
    assert!(app.due_approval().is_some());
}

#[test]
fn ctrl_b_under_a_due_approval_never_opens_an_overlay_but_closes_one() {
    let (mut app, dir) = board_over_a_due_approval(NARROW);
    assert!(app.reduce(AppEvent::Key(ctrl('b'))).is_empty());
    assert!(!app.board.open, "an overlay would cover the approval");
    assert_eq!(saved_board_open(&dir), None);
    // An open overlay always holds focus; the reducer never leaves it over a due card.
    app.board.open = true;
    app.board.focus = BoardFocus::Board;
    assert!(app.reduce(AppEvent::Key(ctrl('b'))).is_empty());
    assert!(!app.board.open);
    assert_eq!(app.board.focus, BoardFocus::BottomPane);
    assert_eq!(saved_board_open(&dir), Some(false));
}

#[test]
fn ctrl_g_under_a_due_approval_leaves_the_board_alone() {
    for size in [DOCKED, NARROW] {
        let (mut app, dir) = board_over_a_due_approval(size);
        assert!(app.reduce(AppEvent::Key(ctrl('g'))).is_empty());
        assert!(!app.board.open, "{size:?}");
        assert!(app.due_approval().is_some());
        let flash = app.live_flash().unwrap_or_default().to_owned();
        assert!(flash.contains("ctrl+c defers the approval"), "{flash}");
        assert_eq!(saved_board_open(&dir), None);
    }
}

#[test]
fn global_chords_still_work_from_the_board() {
    let mut app = app();
    let (parent, _child) = view_child(&mut app);
    press(&mut app, ctrl('g'));
    press(&mut app, ctrl('t'));
    assert!(app.details);
    press(&mut app, alt(KeyCode::Up));
    assert_eq!(app.selected, Some(parent));
    let effects = press(&mut app, ctrl('c'));
    assert!(interrupts(&effects, parent));
}

#[test]
fn board_arrows_move_fold_and_jump() {
    let mut app = app();
    view_child(&mut app);
    press(&mut app, ctrl('g'));
    for code in [KeyCode::Up, KeyCode::Down, KeyCode::Left, KeyCode::Right] {
        press(&mut app, key(code));
    }
    press(&mut app, key(KeyCode::Home));
    assert_eq!(app.board.cursor, app.tree.roots.first().copied());
}

#[test]
fn ctrl_r_from_the_board_focuses_the_composer_first() {
    let mut app = app();
    press(&mut app, ctrl('g'));
    press(&mut app, ctrl('r'));
    assert_eq!(app.board.focus, BoardFocus::BottomPane);
    assert!(app.bottom.popup.is_some());
}

// ----- dispatch order ------------------------------------------------------

#[test]
fn an_open_view_takes_keys_before_the_ladders_and_composer() {
    let mut app = app();
    app.bottom.push_view(Box::new(StubView {
        consume_esc: true,
        ..Default::default()
    }));
    let effects = press(&mut app, key(KeyCode::Esc));
    assert!(effects.is_empty(), "the view ate Esc: no interrupt");
    assert_eq!(app.bottom.view_stack.len(), 1);
    type_text(&mut app, "abc");
    assert!(
        app.input.text.is_empty(),
        "keys a view ignores are not typing"
    );
}

#[test]
fn every_key_notes_the_typing_guard() {
    let mut app = app();
    assert!(app.bottom.approval.last_key_at.is_none());
    press(&mut app, key(KeyCode::Char('x')));
    assert_eq!(app.bottom.approval.last_key_at, Some(app.now));
}

// ----- view actions --------------------------------------------------------

#[test]
fn apply_view_actions_applies_each_kind() {
    let mut app = app();
    let (parent, _child) = view_child(&mut app);
    let effects = app.apply_view_actions(vec![
        ViewAction::InsertText("hi".into()),
        ViewAction::ViewAgent(parent),
        ViewAction::Push(Box::new(StubView::default())),
        ViewAction::Flash("done".into()),
        ViewAction::Effect(Box::new(Effect::Quit)),
        ViewAction::Command(SlashCommand::Mouse(Some(MouseMode::Scroll))),
    ]);
    assert_eq!(app.selected, Some(parent));
    assert!(app.input.text.is_empty(), "the draft stays with its agent");
    assert_eq!(app.bottom.view_stack.len(), 1);
    assert_eq!(
        app.live_flash(),
        Some("mouse on · drag to select and copy; shift+drag selects in terminal")
    );
    assert!(quits(&effects));
    assert!(effects.iter().any(|e| matches!(e, Effect::SetMouse(true))));
    assert_eq!(app.mouse, MouseMode::Scroll);
}

#[test]
fn a_view_push_lands_above_its_parent() {
    let mut app = app();
    app.bottom.push_view(Box::new(StubView {
        act: Some(|| vec![ViewAction::Push(Box::new(StubView::default()))]),
        ..Default::default()
    }));
    press(&mut app, key(KeyCode::Enter));
    assert_eq!(app.bottom.view_stack.len(), 2);
    let top = format!("{:?}", app.bottom.top_view().expect("a view"));
    assert!(
        top.contains("act: None"),
        "the pushed view is on top: {top}"
    );
}

// ----- chords --------------------------------------------------------------

#[test]
fn alt_up_views_the_parent_and_stops_at_the_root() {
    let mut app = app();
    let (parent, _child) = view_child(&mut app);
    press(&mut app, alt(KeyCode::Up));
    assert_eq!(app.selected, Some(parent));
    let effects = press(&mut app, alt(KeyCode::Up));
    assert_eq!(app.selected, Some(parent));
    assert!(effects.is_empty());
}

#[test]
fn alt_comma_and_period_step_effort_and_stop_at_the_ends() {
    let effort_of = |effects: &[Effect]| {
        effects.iter().find_map(|e| match e {
            Effect::Request {
                command: Command::UpdateAgent { patch, .. },
                ..
            } => patch.effort,
            _ => None,
        })
    };
    let mut app = app();
    assert_eq!(
        effort_of(&press(&mut app, alt(KeyCode::Char('.')))),
        Some(Effort::XHigh)
    );
    assert_eq!(
        effort_of(&press(&mut app, alt(KeyCode::Char(',')))),
        Some(Effort::Medium)
    );
    assert_eq!(app.live_flash(), Some("effort medium"));
    let id = viewed(&app);
    app.tree
        .nodes
        .get_mut(&id)
        .expect("node")
        .agent
        .config
        .effort = Some(Effort::Max);
    assert!(press(&mut app, alt(KeyCode::Char('.'))).is_empty());
    app.tree
        .nodes
        .get_mut(&id)
        .expect("node")
        .agent
        .config
        .effort = Some(Effort::Low);
    assert!(press(&mut app, alt(KeyCode::Char(','))).is_empty());
}

#[test]
fn scroll_keys_push_intents_for_the_viewed_agent() {
    let mut app = app();
    let shift = |code| KeyEvent::new(code, KeyModifiers::SHIFT);
    let control = |code| KeyEvent::new(code, KeyModifiers::CONTROL);
    for event in [
        key(KeyCode::PageUp),
        key(KeyCode::PageDown),
        shift(KeyCode::Up),
        shift(KeyCode::Down),
        control(KeyCode::Home),
        control(KeyCode::End),
    ] {
        press(&mut app, event);
    }
    assert_eq!(
        scrolls(&app),
        [
            ScrollIntent::PageUp,
            ScrollIntent::PageDown,
            ScrollIntent::LineUp(1),
            ScrollIntent::LineDown(1),
            ScrollIntent::Top,
            ScrollIntent::Latest,
        ]
    );
    assert!(app.scroll_log.since(0).all(|r| r.agent == viewed(&app)));
}

#[test]
fn up_and_down_recall_history_and_never_scroll() {
    let mut app = app();
    type_text(&mut app, "first");
    press(&mut app, ctrl('c'));
    press(&mut app, key(KeyCode::Up));
    assert_eq!(app.input.text, "first");
    press(&mut app, key(KeyCode::Down));
    assert!(app.input.text.is_empty());
    assert!(scrolls(&app).is_empty());
}

#[test]
fn ctrl_t_toggles_details() {
    let mut app = app();
    press(&mut app, ctrl('t'));
    assert!(app.details);
    press(&mut app, ctrl('t'));
    assert!(!app.details);
}

#[test]
fn ctrl_d_detaches_only_from_an_empty_composer() {
    let mut app = app();
    app.input.set_text("x");
    assert!(!quits(&press(&mut app, ctrl('d'))));
    app.input.set_text("");
    app.bottom.push_view(Box::new(StubView::default()));
    assert!(!quits(&press(&mut app, ctrl('d'))), "not with a view open");
    app.bottom.view_stack.clear();
    assert!(quits(&press(&mut app, ctrl('d'))));
    assert!(app.exit_message.is_some());
}

#[test]
fn newline_chords_insert_a_newline() {
    let mut app = app();
    type_text(&mut app, "a");
    press(&mut app, KeyEvent::new(KeyCode::Enter, KeyModifiers::SHIFT));
    press(&mut app, alt(KeyCode::Enter));
    press(&mut app, ctrl('j'));
    assert_eq!(app.input.text, "a\n\n\n");
}

// ----- slash commands ------------------------------------------------------

#[test]
fn slash_mouse_toggles_off_and_scroll_and_sets_capture() {
    let mut app = app();
    assert_eq!(app.mouse, MouseMode::Scroll);
    let effects = app.run_slash(SlashCommand::Mouse(None));
    assert_eq!(app.mouse, MouseMode::Off);
    assert!(matches!(effects.as_slice(), [Effect::SetMouse(false)]));
    let effects = app.run_slash(SlashCommand::Mouse(None));
    assert_eq!(app.mouse, MouseMode::Scroll);
    assert!(matches!(effects.as_slice(), [Effect::SetMouse(true)]));
    let effects = app.run_slash(SlashCommand::Mouse(Some(MouseMode::Full)));
    assert_eq!(app.mouse, MouseMode::Full);
    assert!(matches!(effects.as_slice(), [Effect::SetMouse(true)]));
}

#[test]
fn the_wheel_scrolls_by_default_and_honours_mouse_off() {
    let wheel = |kind| MouseEvent {
        kind,
        column: 0,
        row: 0,
        modifiers: KeyModifiers::NONE,
    };
    let mut app = app();
    let mut cache = crate::ui::transcript::ViewCache::default();
    cache.regions.transcript = ratatui::layout::Rect::new(0, 0, 80, 20);
    app.dispatch_mouse(wheel(MouseEventKind::ScrollUp), &mut cache);
    app.dispatch_mouse(wheel(MouseEventKind::ScrollDown), &mut cache);
    app.run_slash(SlashCommand::Mouse(Some(MouseMode::Off)));
    app.dispatch_mouse(wheel(MouseEventKind::ScrollUp), &mut cache);
    assert_eq!(
        scrolls(&app),
        [ScrollIntent::LineUp(3), ScrollIntent::LineDown(3)]
    );
}

#[test]
fn slash_agent_opens_the_board_or_views_by_name() {
    let mut app = app();
    let (parent, _child) = view_child(&mut app);
    app.run_slash(SlashCommand::Agent { name: None });
    assert!(app.board.focused());
    app.run_slash(SlashCommand::Agent {
        name: Some("ASSISTANT".into()),
    });
    assert_eq!(app.selected, Some(parent));
    app.run_slash(SlashCommand::Agent {
        name: Some("nobody".into()),
    });
    assert_eq!(app.selected, Some(parent));
    assert_eq!(app.live_flash(), Some("no agent named nobody"));
}

#[test]
fn slash_agent_takes_the_board_name_of_a_same_named_sibling() {
    let mut app = app();
    let (parent, first) = view_child(&mut app);
    app.select(parent);
    let (_, second) = view_child(&mut app);
    let created = app.tree.nodes[&first].agent.created_at;
    app.tree
        .nodes
        .get_mut(&second)
        .expect("second")
        .agent
        .created_at = created + chrono::Duration::seconds(1);
    app.select(parent);
    app.run_slash(SlashCommand::Agent {
        name: Some("Child 2".into()),
    });
    assert_eq!(app.selected, Some(second));
    app.run_slash(SlashCommand::Agent {
        name: Some("child".into()),
    });
    assert_eq!(app.selected, Some(first));
}

#[test]
fn slash_cancel_asks_before_ending() {
    let mut app = app();
    let effects = app.run_slash(SlashCommand::Cancel);
    assert!(effects.is_empty(), "nothing ends until confirmed");
    assert_eq!(app.bottom.view_stack.len(), 1);
}

#[test]
fn slash_cancel_confirm_yes_ends_and_no_keeps() {
    let mut app = app();
    let agent = viewed(&app);
    app.run_slash(SlashCommand::Cancel);
    let effects = press(&mut app, key(KeyCode::Enter));
    assert!(effects.is_empty(), "the cursor starts on No");
    assert!(app.bottom.view_stack.is_empty());
    app.run_slash(SlashCommand::Cancel);
    press(&mut app, key(KeyCode::Up));
    let effects = press(&mut app, key(KeyCode::Enter));
    assert!(effects.iter().any(|e| matches!(
        e,
        Effect::Request { command: Command::Cancel { agent: a, .. }, .. } if *a == agent
    )));
}

#[test]
fn slash_theme_mode_retokens_and_bumps_the_epoch() {
    let mut app = app();
    let epoch = app.theme_epoch;
    app.run_slash(SlashCommand::SetTheme("nord-light".parse().unwrap()));
    assert_eq!(app.theme.spec().mode, ThemeMode::Light);
    assert_eq!(
        app.theme.spec().scheme,
        crate::ui::theme::Builtin::Nord.scheme()
    );
    assert_eq!(app.theme_choice.to_string(), "nord-light");
    assert_eq!(app.theme_epoch, epoch + 1);
    app.run_slash(SlashCommand::Motion(Some(true)));
    assert!(app.theme.spec().reduced_motion);
    assert_eq!(app.theme_epoch, epoch + 2);
}

#[test]
fn slash_quit_detaches() {
    let mut app = app();
    assert!(quits(&app.run_slash(SlashCommand::Quit)));
    assert!(app.exit_message.is_some());
}

#[test]
fn focused_board_passes_transcript_and_effort_keys_to_global_handlers() {
    let mut app = app();
    press(&mut app, ctrl('g'));
    let cursor = app.board.cursor;
    for event in [
        key(KeyCode::PageUp),
        key(KeyCode::PageDown),
        KeyEvent::new(KeyCode::Up, KeyModifiers::SHIFT),
        KeyEvent::new(KeyCode::Down, KeyModifiers::SHIFT),
        KeyEvent::new(KeyCode::Home, KeyModifiers::CONTROL),
        KeyEvent::new(KeyCode::End, KeyModifiers::CONTROL),
    ] {
        press(&mut app, event);
    }
    assert_eq!(
        scrolls(&app),
        [
            ScrollIntent::PageUp,
            ScrollIntent::PageDown,
            ScrollIntent::LineUp(1),
            ScrollIntent::LineDown(1),
            ScrollIntent::Top,
            ScrollIntent::Latest
        ]
    );
    assert_eq!(app.board.cursor, cursor);
    for code in [',', '.'] {
        let effects = press(&mut app, alt(KeyCode::Char(code)));
        assert!(effects.iter().any(|effect| matches!(
            effect,
            Effect::Request {
                command: Command::UpdateAgent { .. },
                ..
            }
        )));
    }
    press(&mut app, ctrl('f'));
    assert!(app.bottom.find.is_some());
    assert_eq!(app.board.focus, BoardFocus::BottomPane);
}

#[test]
fn focused_board_passes_voice_keys_and_empty_draft_quit() {
    let mut voice_app = app();
    voice_app.voice.available = true;
    let dir = tempfile::tempdir().expect("voice prefs");
    voice_app.options.prefs_dir = Some(dir.path().into());
    let effects = press(&mut voice_app, key(KeyCode::F(4)));
    assert!(
        effects
            .iter()
            .any(|e| matches!(e, Effect::Voice(crate::voice::Effect::Record { .. })))
    );
    let id = effects
        .iter()
        .find_map(|effect| match effect {
            Effect::Voice(crate::voice::Effect::Record { id, .. }) => Some(*id),
            _ => None,
        })
        .expect("record id");
    press(&mut voice_app, ctrl('g'));
    let effects = press(&mut voice_app, key(KeyCode::F(4)));
    assert!(
        effects
            .iter()
            .any(|e| matches!(e, Effect::Voice(crate::voice::Effect::Stop(_))))
    );
    voice_app.input.insert_char('x');
    voice_app.reduce(AppEvent::Voice(crate::voice::Reply::Recorded {
        id,
        result: Ok("recovered dictation".into()),
    }));
    voice_app.input.set_text("");
    press(&mut voice_app, key(KeyCode::F(5)));
    assert_eq!(voice_app.input.text, "recovered dictation");
    let mut app = app();
    press(&mut app, ctrl('g'));
    assert!(quits(&press(&mut app, ctrl('d'))));
}

#[test]
fn theme_picker_previews_without_saving_then_restores_or_commits() {
    let dir = tempfile::tempdir().unwrap();
    let mut app = app();
    app.options.prefs_dir = Some(dir.path().into());
    app.run_slash(SlashCommand::SetTheme("auto".parse().unwrap()));
    let original = app.theme.spec();
    let choice = app.theme_choice.clone();
    let path = dir.path().join(crate::prefs::FILE);
    let saved = std::fs::read(&path).unwrap();
    app.run_slash(SlashCommand::Theme);
    press(&mut app, key(KeyCode::Down));
    assert_ne!(app.theme.spec().mode, original.mode);
    for _ in 0..2 {
        press(&mut app, key(KeyCode::Down));
    }
    assert_eq!(
        app.theme.spec().scheme,
        crate::ui::theme::Builtin::Nord.scheme()
    );
    assert_eq!(app.theme_choice, choice);
    assert_eq!(std::fs::read(&path).unwrap(), saved);
    press(&mut app, key(KeyCode::Esc));
    assert_eq!(app.theme.spec(), original);
    assert!(app.committed_theme.is_none());
    app.run_slash(SlashCommand::Theme);
    for c in "nord".chars() {
        press(&mut app, key(KeyCode::Char(c)));
    }
    assert_eq!(
        app.theme.spec().scheme,
        crate::ui::theme::Builtin::Nord.scheme()
    );
    assert_eq!(std::fs::read(&path).unwrap(), saved);
    press(&mut app, key(KeyCode::Enter));
    assert_eq!(app.theme_choice.to_string(), "nord");
    assert!(app.committed_theme.is_none());
    assert_ne!(std::fs::read(&path).unwrap(), saved);
    assert!(
        std::fs::read_to_string(&path)
            .unwrap()
            .contains("theme = \"nord\"")
    );
}

#[test]
fn terminal_probe_upgrades_only_indexed_depth_on_positive_support() {
    use crate::ui::theme::ColorDepth;
    use crate::ui::theme::probe::TerminalProbe;
    use crate::ui::theme::tokens::{Rgb, TerminalPalette, ThemeSpec};
    for (before, supported, after) in [
        (ColorDepth::Indexed256, true, ColorDepth::Truecolor),
        (ColorDepth::Indexed256, false, ColorDepth::Indexed256),
        (ColorDepth::Basic16, true, ColorDepth::Basic16),
    ] {
        let mut app = app();
        app.theme = Theme::resolve(ThemeSpec {
            depth: before,
            ..app.theme.spec()
        });
        let palette = Some(TerminalPalette {
            fg: Rgb(255, 255, 255),
            bg: Rgb(0, 0, 0),
        });
        app.apply_terminal_probe(TerminalProbe {
            palette,
            truecolor: supported,
        });
        assert_eq!(app.theme.spec().depth, after);
        assert_eq!(app.theme.spec().palette, palette);
    }
}

#[test]
fn broken_theme_previews_its_fallback_without_flashing_or_saving() {
    use crate::ui::theme::{Builtin, files};
    let dir = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(files::dir(dir.path())).unwrap();
    std::fs::write(
        files::dir(dir.path()).join("broken.toml"),
        "base = \"nord\"\nrunning = \"invalid\"",
    )
    .unwrap();
    let mut app = app();
    app.options.definitions_dir = Some(dir.path().into());
    app.options.prefs_dir = Some(dir.path().into());
    let original = app.theme.spec();
    let choice = app.theme_choice.clone();
    let flash = app.live_flash().map(str::to_owned);
    app.preview_theme(Some("broken".parse().unwrap()));
    assert_eq!(app.live_flash(), flash.as_deref());
    assert_eq!(app.theme.spec().scheme, Builtin::Nord.scheme());
    assert_eq!(app.theme_choice, choice);
    assert!(!dir.path().join(crate::prefs::FILE).exists());
    app.preview_theme(None);
    assert_eq!(app.theme.spec(), original);
}

#[test]
fn ctrl_c_restores_the_theme_before_closing_its_picker() {
    let mut app = app();
    let original = app.theme.spec();
    app.run_slash(SlashCommand::Theme);
    press(&mut app, key(KeyCode::Down));
    assert!(app.committed_theme.is_some());
    press(&mut app, ctrl('c'));
    assert_eq!(app.theme.spec(), original);
    assert!(app.bottom.view_stack.is_empty());
}

#[test]
fn a_theme_picker_closed_by_a_click_does_not_keep_its_preview() {
    let mut app = app();
    let original = app.theme.spec();
    app.run_slash(SlashCommand::Theme);
    app.reduce(AppEvent::Key(key(KeyCode::Down)));
    assert!(app.committed_theme.is_some());
    assert_ne!(app.theme.spec(), original);
    // A click on the transcript or composer clears the view stack like this.
    app.bottom.view_stack.clear();
    app.reduce(AppEvent::Focus(true));
    assert_eq!(app.theme.spec(), original);
    assert!(app.committed_theme.is_none());
}
