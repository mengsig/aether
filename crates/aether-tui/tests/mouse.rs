#![allow(clippy::expect_used, clippy::unwrap_used, clippy::panic)]

use aether_tui::app::{App, AppEvent, Effect, TuiOptions};
use aether_tui::prefs::{MouseMode, UiPrefs};
use aether_tui::testing::{app_with_transcript, populated_app};
use aether_tui::ui::{
    self,
    selection::{self, FrameRegions, Pane},
    transcript::ViewCache,
};
use crossterm::event::{KeyModifiers, MouseButton, MouseEvent, MouseEventKind};
use ratatui::{Terminal, backend::TestBackend, layout::Rect, style::Modifier};

fn mouse_app() -> App {
    let mut app = populated_app();
    app.size = (160, 44);
    app.approvals.clear();
    app.flash = None;
    app.board.focus = aether_tui::app::board::BoardFocus::BottomPane;
    app
}

fn event(kind: MouseEventKind, x: u16, y: u16) -> MouseEvent {
    MouseEvent {
        kind,
        column: x,
        row: y,
        modifiers: KeyModifiers::NONE,
    }
}

fn paint(app: &App, cache: &mut ViewCache) -> ratatui::buffer::Buffer {
    let mut terminal = Terminal::new(TestBackend::new(160, 44)).unwrap();
    let mut rendered = None;
    terminal
        .draw(|f| {
            ui::render(f, app, cache);
            rendered = Some(f.buffer_mut().clone());
        })
        .unwrap();
    rendered.expect("renderer buffer")
}

fn press(app: &mut App, cache: &mut ViewCache, x: u16, y: u16) {
    app.dispatch_mouse(event(MouseEventKind::Down(MouseButton::Left), x, y), cache);
    paint(app, cache);
}

fn drag(app: &mut App, cache: &mut ViewCache, x: u16, y: u16) {
    app.dispatch_mouse(event(MouseEventKind::Drag(MouseButton::Left), x, y), cache);
    paint(app, cache);
}

#[test]
fn regions_exclude_gutters_and_clamp_to_the_starting_pane() {
    let r = FrameRegions {
        transcript: Rect::new(0, 0, 100, 20),
        composer: Rect::new(2, 22, 96, 2),
        board: Rect::new(104, 0, 52, 40),
        ..Default::default()
    };
    assert_eq!(r.hit(99, 19), Some(Pane::Transcript));
    assert_eq!(r.hit(100, 19), None);
    assert_eq!(r.hit(104, 19), Some(Pane::Board));
    assert_eq!(r.hit(1, 22), None);
    assert_eq!(selection::clamp(r.transcript, 150, 30), Some((99, 19)));
    assert_eq!(selection::clamp(r.board, 0, 50), Some((104, 39)));
    assert_eq!(selection::clamp(Rect::default(), 0, 0), None);
}

#[test]
fn mouse_defaults_on_and_old_preferences_still_parse() {
    assert_eq!(MouseMode::default(), MouseMode::Scroll);
    assert_eq!(TuiOptions::default().mouse, MouseMode::Scroll);
    for (value, mode) in [
        ("off", MouseMode::Off),
        ("scroll", MouseMode::Scroll),
        ("full", MouseMode::Full),
    ] {
        let prefs: UiPrefs = toml::from_str(&format!("mouse = \"{value}\"\n")).unwrap();
        let app = App::new(TuiOptions {
            mouse: prefs.mouse.unwrap_or_default(),
            ..Default::default()
        });
        assert_eq!(app.mouse, mode);
    }
}

#[test]
fn wheel_routes_to_the_pointer_and_never_changes_composer_history() {
    let mut app = mouse_app();
    app.board.open = true;
    app.reduce(AppEvent::Paste("old draft".into()));
    app.reduce(AppEvent::Key(crossterm::event::KeyEvent::new(
        crossterm::event::KeyCode::Enter,
        KeyModifiers::NONE,
    )));
    // Sending follows the latest output; only the wheel's requests count below.
    let sent = app
        .scroll_log
        .since(0)
        .last()
        .map_or(0, |request| request.seq);
    let mut cache = ViewCache::default();
    paint(&app, &mut cache);
    let composer = cache.regions.composer;
    app.dispatch_mouse(
        event(MouseEventKind::ScrollUp, composer.x, composer.y),
        &mut cache,
    );
    assert!(app.input.text.is_empty());
    assert!(app.scroll_log.since(sent).next().is_none());
    let board = cache.regions.board;
    app.dispatch_mouse(
        event(MouseEventKind::ScrollDown, board.x, board.y),
        &mut cache,
    );
    assert_eq!(cache.board.wheel_delta, 3);
    assert!(app.scroll_log.since(sent).next().is_none());
    let transcript = cache.regions.transcript;
    app.dispatch_mouse(
        event(MouseEventKind::ScrollUp, transcript.x, transcript.y),
        &mut cache,
    );
    assert_eq!(app.scroll_log.since(sent).count(), 1);
    app.mouse = MouseMode::Off;
    app.dispatch_mouse(
        event(MouseEventKind::ScrollDown, transcript.x, transcript.y),
        &mut cache,
    );
    assert_eq!(app.scroll_log.since(sent).count(), 1);
}

#[test]
fn dragging_out_of_composer_copies_only_its_text_without_padding() {
    let mut app = mouse_app();
    app.board.open = true;
    app.reduce(AppEvent::Paste("hello selection".into()));
    let mut cache = ViewCache::default();
    paint(&app, &mut cache);
    let rect = cache.regions.composer;
    press(&mut app, &mut cache, rect.x, rect.y);
    drag(&mut app, &mut cache, 159, rect.y);
    assert_eq!(
        selection::extract(&app, &mut cache).unwrap(),
        "hello selection"
    );
    let screen = paint(&app, &mut cache);
    assert!(
        screen[(rect.x, rect.y)]
            .modifier
            .contains(Modifier::REVERSED)
    );
    assert!(
        !screen[(cache.regions.board.x, rect.y)]
            .modifier
            .contains(Modifier::REVERSED)
    );
    let effects = app.dispatch_mouse(
        event(MouseEventKind::Up(MouseButton::Left), 159, rect.y),
        &mut cache,
    );
    assert_eq!(effects, vec![Effect::Copy("hello selection".into())]);
    assert!(app.live_flash().is_none_or(|s| !s.contains("copied")));
}

#[test]
fn transcript_selection_survives_scroll_and_extends_with_the_wheel() {
    let messages: Vec<_> = (0..80)
        .map(|n| format!("unique transcript message {n}"))
        .collect();
    let mut app = app_with_transcript(&messages.iter().map(String::as_str).collect::<Vec<_>>());
    app.size = (160, 44);
    app.board.open = true;
    let mut cache = ViewCache::default();
    paint(&app, &mut cache);
    let rect = cache.regions.transcript;
    press(&mut app, &mut cache, rect.x, rect.y + 3);
    drag(&mut app, &mut cache, rect.right() - 1, rect.y + 8);
    let before = selection::extract(&app, &mut cache).unwrap();
    let start = cache.mouse.selection.as_ref().unwrap().start;
    app.dispatch_mouse(
        event(MouseEventKind::ScrollUp, rect.x, rect.y + 8),
        &mut cache,
    );
    paint(&app, &mut cache);
    assert_eq!(cache.mouse.selection.as_ref().unwrap().start, start);
    assert!(!selection::extract(&app, &mut cache).unwrap().is_empty());
    let effects = app.dispatch_mouse(
        event(
            MouseEventKind::Up(MouseButton::Left),
            rect.right() - 1,
            rect.y + 8,
        ),
        &mut cache,
    );
    assert!(
        effects
            .iter()
            .any(|effect| matches!(effect, Effect::Copy(_)))
    );
    let selected = selection::extract(&app, &mut cache).unwrap();
    app.dispatch_mouse(
        event(MouseEventKind::ScrollUp, rect.x, rect.y + 8),
        &mut cache,
    );
    paint(&app, &mut cache);
    assert_eq!(selection::extract(&app, &mut cache).unwrap(), selected);
    assert!(!before.is_empty());
}

#[test]
fn board_click_views_on_release_but_drag_does_not_act() {
    let mut app = mouse_app();
    app.board.open = true;
    let mut cache = ViewCache::default();
    paint(&app, &mut cache);
    let rect = cache.board.tree_area;
    let target = ui::board::tree::agent_at(&app.board.model, cache.board.tree_offset, 0).unwrap();
    press(&mut app, &mut cache, rect.x, rect.y);
    assert_eq!(cache.mouse.pending_board_click, Some(target));
    app.dispatch_mouse(
        event(MouseEventKind::Up(MouseButton::Left), rect.x, rect.y),
        &mut cache,
    );
    assert_eq!(app.selected, Some(target));
    assert_eq!(app.board.cursor, Some(target));
    press(&mut app, &mut cache, rect.x, rect.y);
    drag(&mut app, &mut cache, 0, rect.y + 1);
    let old = app.selected;
    let effects = app.dispatch_mouse(
        event(MouseEventKind::Up(MouseButton::Left), 0, rect.y + 1),
        &mut cache,
    );
    assert_eq!(app.selected, old);
    assert!(
        effects
            .iter()
            .all(|effect| matches!(effect, Effect::Copy(_)))
    );
}

#[test]
fn extraction_is_unicode_safe_trims_rows_and_is_bounded() {
    let mut text = String::new();
    selection::append_row(&mut text, "é界z   ", 1, 3).unwrap();
    selection::append_row(&mut text, "next  ", 0, 6).unwrap();
    assert_eq!(text, "界\nnext\n");
    let mut full = "x".repeat(selection::MAX_COPY_BYTES);
    assert!(selection::append_row(&mut full, "x", 0, 1).is_err());
}

#[test]
fn active_selection_frame_capture() {
    let mut app = mouse_app();
    app.board.open = true;
    app.flash = None;
    let mut cache = ViewCache::default();
    let screen = paint(&app, &mut cache);
    let rect = cache.regions.transcript;
    let first = (rect.y..rect.bottom())
        .find(|&y| (rect.x..rect.right()).any(|x| !screen[(x, y)].symbol().trim().is_empty()))
        .unwrap();
    press(&mut app, &mut cache, rect.x, first);
    drag(
        &mut app,
        &mut cache,
        rect.right() - 1,
        (first + 4).min(rect.bottom() - 1),
    );
    let screen = paint(&app, &mut cache);
    assert!(
        screen
            .content
            .iter()
            .any(|c| c.modifier.contains(Modifier::REVERSED))
    );
    if let Some(dir) = std::env::var_os("AETHER_MOUSE_CAPTURE_DIR") {
        let dir = std::path::PathBuf::from(dir);
        std::fs::create_dir_all(&dir).unwrap();
        let mut ansi = String::new();
        for y in 0..44 {
            for x in 0..160 {
                let cell = &screen[(x, y)];
                ansi.push_str("\x1b[0m");
                if cell.modifier.contains(Modifier::REVERSED) {
                    ansi.push_str("\x1b[7m");
                }
                if cell.modifier.contains(Modifier::BOLD) {
                    ansi.push_str("\x1b[1m");
                }
                if cell.modifier.contains(Modifier::DIM) {
                    ansi.push_str("\x1b[2m");
                }
                if let ratatui::style::Color::Rgb(r, g, b) = cell.fg {
                    ansi.push_str(&format!("\x1b[38;2;{r};{g};{b}m"));
                }
                if let ratatui::style::Color::Rgb(r, g, b) = cell.bg {
                    ansi.push_str(&format!("\x1b[48;2;{r};{g};{b}m"));
                }
                ansi.push_str(cell.symbol());
            }
            ansi.push_str("\x1b[0m\n");
        }
        std::fs::write(dir.join("active-selection.ansi"), ansi).unwrap();
    }
}

#[test]
fn composer_soft_wraps_join_and_hard_breaks_remain() {
    let mut app = mouse_app();
    app.board.open = true;
    app.reduce(AppEvent::Paste(format!(
        "{}\nsecond logical line",
        "word ".repeat(26)
    )));
    app.input.cursor = 0;
    let mut cache = ViewCache::default();
    paint(&app, &mut cache);
    let r = cache.regions.composer;
    assert!(r.height >= 3);
    press(&mut app, &mut cache, r.x, r.y);
    drag(&mut app, &mut cache, r.right() - 1, r.bottom() - 1);
    assert_eq!(
        selection::extract(&app, &mut cache).unwrap(),
        app.input
            .text
            .split('\n')
            .map(str::trim_end)
            .collect::<Vec<_>>()
            .join("\n")
    );
}

#[test]
fn drag_past_transcript_edge_scrolls_and_keeps_the_content_anchor() {
    let messages: Vec<_> = (0..80).map(|n| format!("edge message {n}")).collect();
    let mut app = app_with_transcript(&messages.iter().map(String::as_str).collect::<Vec<_>>());
    app.size = (160, 44);
    let mut cache = ViewCache::default();
    paint(&app, &mut cache);
    let r = cache.regions.transcript;
    press(&mut app, &mut cache, r.x, r.y + 6);
    let start = cache.mouse.selection.as_ref().unwrap().start;
    let before = app.scroll_log.since(0).count();
    drag(&mut app, &mut cache, r.x, r.y);
    drag(&mut app, &mut cache, r.right() - 1, r.bottom() + 4);
    assert!(app.scroll_log.since(0).count() > before);
    assert_eq!(cache.mouse.selection.as_ref().unwrap().start, start);
    assert!(
        selection::extract(&app, &mut cache)
            .unwrap()
            .contains("edge message")
    );
}

#[test]
fn new_click_clears_selection_and_shift_mouse_is_left_to_the_terminal() {
    let mut app = mouse_app();
    let mut cache = ViewCache::default();
    paint(&app, &mut cache);
    let r = cache.regions.composer;
    press(&mut app, &mut cache, r.x, r.y);
    drag(&mut app, &mut cache, r.x + 4, r.y);
    let old = cache.mouse.selection.as_ref().unwrap().end;
    let mut shifted = event(MouseEventKind::Down(MouseButton::Left), r.x + 1, r.y);
    shifted.modifiers = KeyModifiers::SHIFT;
    assert!(app.dispatch_mouse(shifted, &mut cache).is_empty());
    assert_eq!(cache.mouse.selection.as_ref().unwrap().end, old);
    app.dispatch_mouse(
        event(MouseEventKind::Down(MouseButton::Right), 159, 43),
        &mut cache,
    );
    assert!(cache.mouse.selection.is_none());
}

#[test]
fn board_wheel_scroll_persists_across_repaints() {
    use aether_client::ClientError;
    use aether_core::id::AgentId;
    use aether_protocol::Reply;
    use aether_tui::app::Then;
    let mut app = mouse_app();
    app.board.open = true;
    let mut tree = app.tree.clone();
    let template = tree.nodes.values().next().unwrap().clone();
    for n in 0..20 {
        let id = AgentId::new();
        let mut node = template.clone();
        node.agent.id = id;
        node.agent.name = format!("board-agent-{n}");
        node.children.clear();
        tree.roots.push(id);
        tree.nodes.insert(id, node);
    }
    app.reduce(AppEvent::Reply {
        then: Then::Tree,
        result: Ok::<_, ClientError>(Reply::Tree(tree)),
    });
    let mut cache = ViewCache::default();
    paint(&app, &mut cache);
    let board = cache.regions.board;
    let before = cache.board.tree_offset;
    app.dispatch_mouse(
        event(MouseEventKind::ScrollDown, board.x, board.y),
        &mut cache,
    );
    paint(&app, &mut cache);
    let after = cache.board.tree_offset;
    assert!(after > before);
    paint(&app, &mut cache);
    assert_eq!(cache.board.tree_offset, after);
}

#[test]
fn composer_padding_returns_focus_and_popups_block_transcript_hit_testing() {
    let mut app = mouse_app();
    app.board.open = true;
    app.board.focus = aether_tui::app::board::BoardFocus::Board;
    let mut cache = ViewCache::default();
    paint(&app, &mut cache);
    let block = cache.regions.composer_block;
    app.dispatch_mouse(
        event(MouseEventKind::Down(MouseButton::Left), block.x, block.y),
        &mut cache,
    );
    assert!(!app.board.focused());
    app.bottom.shortcuts = true;
    paint(&app, &mut cache);
    let popup = cache.regions.view;
    assert!(!popup.is_empty());
    assert_eq!(cache.regions.hit(popup.x, popup.y), Some(Pane::View));
    assert!(cache.regions.transcript.bottom() <= popup.y);
    let block = cache.regions.composer_block;
    app.dispatch_mouse(
        event(MouseEventKind::Down(MouseButton::Left), block.x, block.y),
        &mut cache,
    );
    paint(&app, &mut cache);
    assert!(!app.bottom.shortcuts);
    assert!(cache.mouse.selection.is_some());
}

#[test]
fn selecting_a_pinned_prompt_copies_its_displayed_excerpt() {
    let reply = "a long reply row\n\n".repeat(100);
    let mut app = app_with_transcript(&["first prompt line\nsecond prompt line", &reply]);
    app.size = (160, 44);
    let agent = app.selected.unwrap();
    app.scroll_log.push(
        agent,
        aether_tui::app::transcript::ScrollIntent::Reveal(app.transcripts[&agent].cells[1].id),
    );
    let mut cache = ViewCache::default();
    let screen = paint(&app, &mut cache);
    assert!(cache.mouse.pinned.is_some());
    let r = cache.regions.transcript;
    let shown: String = (r.x..r.right())
        .map(|x| screen[(x, r.y)].symbol())
        .collect();
    press(&mut app, &mut cache, r.x, r.y);
    let effects = app.dispatch_mouse(
        event(MouseEventKind::Up(MouseButton::Left), r.right() - 1, r.y),
        &mut cache,
    );
    assert_eq!(effects, vec![Effect::Copy(shown.trim_end().to_owned())]);
}

#[test]
fn pinned_coordination_prompts_copy_and_keep_selection_in_compact_and_full_views() {
    use aether_tui::app::transcript::{CellModel, ScrollIntent, UserFrom};

    for from in [
        UserFrom::Message {
            from: "peer".into(),
        },
        UserFrom::Task {
            from: "parent".into(),
        },
    ] {
        for (mode, details) in [("standard", false), ("focus", false), ("focus", true)] {
            let reply = "a long reply row\n\n".repeat(100);
            let mut app = app_with_transcript(&["coordination prompt", &reply]);
            app.size = (160, 44);
            app.view_mode = mode.into();
            app.details = details;
            let agent = app.selected.unwrap();
            let transcript = app.transcripts.get_mut(&agent).unwrap();
            transcript.cells[0].model = CellModel::User {
                from: from.clone(),
                text: "coordination prompt\nsecond line".into(),
            };
            app.scroll_log
                .push(agent, ScrollIntent::Reveal(transcript.cells[1].id));
            app.scroll_log.push(agent, ScrollIntent::LineDown(5));
            let mut cache = ViewCache::default();
            let screen = paint(&app, &mut cache);
            assert!(cache.mouse.pinned.is_some(), "{from:?} {mode} {details}");
            let r = cache.regions.transcript;
            let shown: String = (r.x..r.right())
                .map(|x| screen[(x, r.y)].symbol())
                .collect();
            press(&mut app, &mut cache, r.x, r.y);
            assert!(cache.mouse.selection.is_some(), "{from:?} {mode} {details}");
            let effects = app.dispatch_mouse(
                event(MouseEventKind::Up(MouseButton::Left), r.right() - 1, r.y),
                &mut cache,
            );
            assert_eq!(effects, vec![Effect::Copy(shown.trim_end().to_owned())]);
            paint(&app, &mut cache);
            assert_eq!(
                selection::extract(&app, &mut cache).unwrap(),
                shown.trim_end()
            );
        }
    }
}

#[test]
fn transcript_hit_testing_uses_the_last_displayed_top_when_new_content_arrives() {
    let mut app =
        app_with_transcript(&["first prompt", "a reply paragraph\n\n".repeat(100).as_str()]);
    app.size = (160, 44);
    let mut cache = ViewCache::default();
    paint(&app, &mut cache);
    let r = cache.regions.transcript;
    let before = selection::point(&app, &mut cache, Pane::Transcript, r.x, r.y + 3).unwrap();
    let agent = app.selected.unwrap();
    let now = app.now;
    app.transcripts.get_mut(&agent).unwrap().stream(
        aether_core::id::InvocationId::new(),
        &aether_protocol::Delta::Text {
            text: "new streamed paragraph\n\n".repeat(100),
        },
        now,
    );
    assert_eq!(
        selection::point(&app, &mut cache, Pane::Transcript, r.x, r.y + 3),
        Some(before)
    );
}

#[test]
fn board_click_waits_for_a_repaint_after_rows_change() {
    let mut app = mouse_app();
    app.board.open = true;
    let mut cache = ViewCache::default();
    paint(&app, &mut cache);
    let r = cache.board.tree_area;
    app.board.revision += 1;
    assert!(
        app.dispatch_mouse(
            event(MouseEventKind::Down(MouseButton::Left), r.x, r.y),
            &mut cache
        )
        .is_empty()
    );
    assert!(cache.mouse.pending_board_click.is_none());
    assert_eq!(
        app.live_flash(),
        Some("agents board changed; click the row again")
    );
}

#[test]
fn selecting_part_of_a_wide_grapheme_highlights_and_copies_the_whole_grapheme() {
    for (text, start, end, expected) in [("界Z", 1, 2, "界Z"), ("A界Z", 0, 1, "A界")] {
        let mut app = mouse_app();
        app.reduce(AppEvent::Paste(text.into()));
        let mut cache = ViewCache::default();
        paint(&app, &mut cache);
        let r = cache.regions.composer;
        press(&mut app, &mut cache, r.x + start, r.y);
        drag(&mut app, &mut cache, r.x + end, r.y);
        let screen = paint(&app, &mut cache);
        assert_eq!(selection::extract(&app, &mut cache).unwrap(), expected);
        for x in r.x..r.x + 3 {
            assert!(screen[(x, r.y)].modifier.contains(Modifier::REVERSED));
        }
    }
}

#[test]
fn wheel_over_cancel_keeps_the_no_default() {
    use crossterm::event::{KeyCode, KeyEvent};
    let mut app = mouse_app();
    app.reduce(AppEvent::Paste("/cancel".into()));
    app.reduce(AppEvent::Key(KeyEvent::new(
        KeyCode::Enter,
        KeyModifiers::NONE,
    )));
    assert!(app.bottom.top_view().is_some());
    let mut cache = ViewCache::default();
    paint(&app, &mut cache);
    let r = cache.regions.view;
    assert!(!r.is_empty());
    app.dispatch_mouse(event(MouseEventKind::ScrollUp, r.x, r.y), &mut cache);
    let effects = app.reduce(AppEvent::Key(KeyEvent::new(
        KeyCode::Enter,
        KeyModifiers::NONE,
    )));
    assert!(!effects.iter().any(|e| matches!(
        e,
        Effect::Request {
            command: aether_protocol::Command::Cancel { .. },
            ..
        }
    )));
}

fn streaming_selection() -> (App, ViewCache, aether_core::id::InvocationId) {
    let mut app = app_with_transcript(&["prompt"]);
    app.flash = None;
    app.size = (160, 44);
    let invocation = aether_core::id::InvocationId::new();
    app.transcripts
        .get_mut(&app.selected.unwrap())
        .unwrap()
        .stream(
            invocation,
            &aether_protocol::Delta::Text {
                text: "streamed answer".into(),
            },
            app.now,
        );
    let mut cache = ViewCache::default();
    let screen = paint(&app, &mut cache);
    let r = cache.regions.transcript;
    let y = (r.y..r.bottom())
        .find(|&y| {
            (r.x..r.right())
                .map(|x| screen[(x, y)].symbol())
                .collect::<String>()
                .contains("streamed answer")
        })
        .unwrap();
    press(&mut app, &mut cache, r.x, y);
    drag(&mut app, &mut cache, r.x + 30, y);
    assert!(
        selection::extract(&app, &mut cache)
            .unwrap()
            .contains("streamed")
    );
    (app, cache, invocation)
}

fn finish_stream(app: &mut App, invocation: aether_core::id::InvocationId) {
    use aether_core::event::{Actor, Event, EventDraft, EventKind};
    use aether_core::model::{ContentBlock, MessageRole};
    let agent = app.selected.unwrap();
    let event = Event::from_draft(
        EventDraft::new(
            Actor::System,
            EventKind::AgentMessage {
                role: MessageRole::Assistant,
                content: vec![ContentBlock::text("streamed answer")],
                invocation: Some(invocation),
            },
        )
        .agent(agent),
        aether_core::id::EventId::new(),
        999,
        app.now,
    );
    app.transcripts.get_mut(&agent).unwrap().ingest(
        &event,
        &aether_tui::app::transcript::IngestCtx {
            tree: &app.tree,
            now: app.now,
        },
    );
}

#[test]
fn finished_stream_selection_cannot_highlight_the_next_reply() {
    let (mut app, mut cache, invocation) = streaming_selection();
    let pointer = cache.mouse.selection.as_ref().unwrap().pointer;
    app.dispatch_mouse(
        event(
            MouseEventKind::Up(MouseButton::Left),
            pointer.column,
            pointer.row,
        ),
        &mut cache,
    );
    finish_stream(&mut app, invocation);
    let agent = app.selected.unwrap();
    let prompt = aether_core::event::Event::from_draft(
        aether_core::event::EventDraft::new(
            aether_core::event::Actor::user(),
            aether_core::event::EventKind::TaskCreated {
                objective: "next prompt".into(),
                parent_task: None,
                dependencies: Vec::new(),
                priority: aether_core::task::Priority::Normal,
            },
        )
        .agent(agent),
        aether_core::id::EventId::new(),
        1000,
        app.now,
    );
    app.transcripts.get_mut(&agent).unwrap().ingest(
        &prompt,
        &aether_tui::app::transcript::IngestCtx {
            tree: &app.tree,
            now: app.now,
        },
    );
    app.transcripts
        .get_mut(&app.selected.unwrap())
        .unwrap()
        .stream(
            invocation,
            &aether_protocol::Delta::Text {
                text: "unrelated next reply".into(),
            },
            app.now,
        );
    paint(&app, &mut cache);
    assert!(cache.mouse.selection.is_none());
}

#[test]
fn finishing_stream_during_drag_invalidates_before_release() {
    let (mut app, mut cache, invocation) = streaming_selection();
    let pointer = cache.mouse.selection.as_ref().unwrap().pointer;
    finish_stream(&mut app, invocation);
    selection::validate(&app, &mut cache);
    assert!(cache.mouse.selection.is_none());
    assert!(
        app.dispatch_mouse(
            event(
                MouseEventKind::Up(MouseButton::Left),
                pointer.column,
                pointer.row
            ),
            &mut cache
        )
        .is_empty()
    );
    assert!(app.live_flash().is_none());
}

#[test]
fn invalidating_selection_preserves_geometry_for_a_click_before_repaint() {
    let mut app = app_with_transcript(&["prompt", "answer"]);
    app.size = (160, 44);
    let mut cache = ViewCache::default();
    paint(&app, &mut cache);
    let r = cache.regions.transcript;
    press(&mut app, &mut cache, r.x, r.y + 3);
    let area = cache.mouse.transcript_area;
    let top = cache.mouse.displayed_top;
    let pinned = cache.mouse.pinned;
    app.details = !app.details;
    selection::validate(&app, &mut cache);
    assert!(cache.mouse.selection.is_none());
    assert_eq!(cache.mouse.transcript_area, area);
    assert_eq!(cache.mouse.displayed_top, top);
    assert_eq!(cache.mouse.pinned, pinned);
    app.dispatch_mouse(
        event(MouseEventKind::Down(MouseButton::Left), r.x, r.y + 3),
        &mut cache,
    );
    assert!(cache.mouse.selection.is_some());
}

#[test]
fn selection_is_invalidated_when_its_stream_endpoint_no_longer_resolves() {
    let (mut app, mut cache, _) = streaming_selection();
    app.transcripts
        .get_mut(&app.selected.unwrap())
        .unwrap()
        .stream
        .text
        .clear();
    selection::validate(&app, &mut cache);
    assert!(cache.mouse.selection.is_none());
}

#[test]
fn wheel_over_a_view_never_falls_back_to_key_dispatch() {
    use aether_tui::ui::bottom::{BottomView, ViewOutcome};
    #[derive(Debug)]
    struct KeyProbe(std::sync::Arc<std::sync::atomic::AtomicUsize>);
    impl BottomView for KeyProbe {
        fn handle_key(&mut self, _: crossterm::event::KeyEvent, _: &App) -> ViewOutcome {
            self.0.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
            ViewOutcome::Consumed
        }
        fn desired_height(&self, _: u16, _: &App) -> u16 {
            10
        }
        fn render(
            &self,
            _: &mut ratatui::Frame,
            _: Rect,
            _: &App,
            _: &aether_tui::ui::theme::tokens::Theme,
        ) {
        }
    }
    let calls = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let mut app = mouse_app();
    app.bottom.push_view(Box::new(KeyProbe(calls.clone())));
    let mut cache = ViewCache::default();
    paint(&app, &mut cache);
    let r = cache.regions.view;
    for kind in [MouseEventKind::ScrollUp, MouseEventKind::ScrollDown] {
        assert!(
            app.dispatch_mouse(event(kind, r.x, r.y), &mut cache)
                .is_empty()
        );
    }
    assert_eq!(calls.load(std::sync::atomic::Ordering::Relaxed), 0);
}

#[test]
fn wheel_still_navigates_the_scrollable_composer_popup() {
    let mut app = mouse_app();
    app.reduce(AppEvent::Key(crossterm::event::KeyEvent::new(
        crossterm::event::KeyCode::Char('/'),
        KeyModifiers::NONE,
    )));
    assert!(app.bottom.popup.is_some());
    let mut cache = ViewCache::default();
    paint(&app, &mut cache);
    let r = cache.regions.view;
    assert!(!r.is_empty());
    app.dispatch_mouse(event(MouseEventKind::ScrollDown, r.x, r.y), &mut cache);
    assert_eq!(app.bottom.popup.as_ref().unwrap().cursor, 3);
    assert_eq!(app.input.text, "/");
    app.dispatch_mouse(event(MouseEventKind::ScrollUp, r.x, r.y), &mut cache);
    assert_eq!(app.bottom.popup.as_ref().unwrap().cursor, 0);
}
