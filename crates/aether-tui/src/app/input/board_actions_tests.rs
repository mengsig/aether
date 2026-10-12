//! Board lifecycle dispatch and reply sequencing regressions.

use aether_core::agent::AgentStatus;
use aether_core::id::AgentId;
use aether_protocol::{ArchiveTarget, Command, Reply};
use crossterm::event::{
    KeyCode, KeyEvent, KeyEventKind, KeyEventState, KeyModifiers, MouseButton, MouseEvent,
    MouseEventKind,
};

use crate::app::board::BoardFocus;
use crate::app::{App, AppEvent, Effect, Then};
use crate::testing::{populated_app, render_to};

fn fixture() -> (App, AgentId, AgentId) {
    let mut app = populated_app();
    app.approvals.clear();
    app.size = (100, 40);
    let root = app.selected.unwrap();
    let mut child = app.tree.nodes[&root].clone();
    let id = AgentId::new();
    child.agent.id = id;
    child.agent.name = "worker".into();
    child.agent.parent = Some(root);
    child.agent.status = AgentStatus::Running;
    child.children.clear();
    app.tree.nodes.insert(id, child);
    app.tree.nodes.get_mut(&root).unwrap().children.push(id);
    app.rebuild_board();
    focus(&mut app, id);
    (app, root, id)
}

fn focus(app: &mut App, id: AgentId) {
    app.board.open = true;
    app.board.focus = BoardFocus::Board;
    app.board.cursor = Some(id);
}

fn press(app: &mut App, code: KeyCode, modifiers: KeyModifiers) -> Vec<Effect> {
    app.dispatch_key(KeyEvent::new(code, modifiers))
}

fn reply(
    app: &mut App,
    then: Then,
    result: Result<Reply, aether_client::ClientError>,
) -> Vec<Effect> {
    app.reduce(AppEvent::Reply { then, result })
}

/// The confirmation's snapshot request, to be answered with its own `Then`.
fn snapshot_fetch(effects: &[Effect], id: AgentId, remove: bool) -> Then {
    let [
        Effect::Request {
            command:
                Command::GetTree {
                    include_archived: true,
                    ..
                },
            then,
        },
    ] = effects
    else {
        panic!("expected one snapshot fetch: {effects:?}");
    };
    assert!(
        matches!(then, Then::BoardConfirm { agent, remove: r, .. } if *agent == id && *r == remove)
    );
    then.clone()
}

fn confirm(app: &mut App, letter: char, id: AgentId, remove: bool) {
    let effects = press(app, KeyCode::Char(letter), KeyModifiers::NONE);
    let then = snapshot_fetch(&effects, id, remove);
    let tree = app.tree.clone();
    assert!(reply(app, then, Ok(Reply::Tree(tree))).is_empty());
    assert_eq!(app.bottom.view_stack.len(), 1);
    assert!(!app.board.focused());
}

#[test]
fn stop_and_delete_confirm_no_by_default_and_target_the_cursor() {
    for (letter, remove) in [('X', false), ('D', true)] {
        let (mut app, root, child) = fixture();
        confirm(&mut app, letter, child, remove);
        assert_eq!(app.selected, Some(root));
        assert!(press(&mut app, KeyCode::Enter, KeyModifiers::NONE).is_empty());
        assert!(app.bottom.view_stack.is_empty());
        focus(&mut app, child);
        confirm(&mut app, letter, child, remove);
        let effects = press(&mut app, KeyCode::Char('y'), KeyModifiers::NONE);
        assert!(
            matches!(&effects[..], [Effect::Request { command: Command::Cancel { agent, .. }, then: Then::BoardStopped { agent: target, remove: r } }] if *agent == child && *target == child && *r == remove)
        );
    }
}

#[test]
fn both_shift_encodings_dispatch_and_unshifted_letters_are_safe() {
    for code in ['X', 'D', 'P', 'I'] {
        for modifiers in [KeyModifiers::NONE, KeyModifiers::SHIFT] {
            let (mut app, _, _) = fixture();
            assert!(!press(&mut app, KeyCode::Char(code), modifiers).is_empty());
        }
        let (mut app, _, _) = fixture();
        assert!(
            !press(
                &mut app,
                KeyCode::Char(code.to_ascii_lowercase()),
                KeyModifiers::SHIFT
            )
            .is_empty()
        );
        let (mut app, _, _) = fixture();
        assert!(
            press(
                &mut app,
                KeyCode::Char(code.to_ascii_lowercase()),
                KeyModifiers::NONE
            )
            .is_empty()
        );
        assert!(app.bottom.view_stack.is_empty());
        for modifiers in [
            KeyModifiers::CONTROL,
            KeyModifiers::ALT,
            KeyModifiers::SHIFT | KeyModifiers::CONTROL,
        ] {
            let (mut app, _, _) = fixture();
            let effects = press(&mut app, KeyCode::Char(code), modifiers);
            assert!(!effects.iter().any(|e| matches!(
                e,
                Effect::Request {
                    command: Command::Cancel { .. }
                        | Command::Archive { .. }
                        | Command::Pause { .. }
                        | Command::Resume { .. }
                        | Command::Interrupt { .. },
                    ..
                }
            )));
        }
    }
}

#[test]
fn pause_resume_and_interrupt_use_cursor_not_viewed_agent() {
    let (mut app, root, child) = fixture();
    assert!(
        matches!(&press(&mut app, KeyCode::Char('P'), KeyModifiers::NONE)[..], [Effect::Request { command: Command::Pause { agent, subtree: true , idempotency_key: None, }, then: Then::Paused(target) }] if *agent == child && *target == child)
    );
    app.tree.nodes.get_mut(&child).unwrap().agent.paused = true;
    assert!(
        matches!(&press(&mut app, KeyCode::Char('p'), KeyModifiers::SHIFT)[..], [Effect::Request { command: Command::Resume { agent, subtree: true , idempotency_key: None, }, then: Then::Resumed(target) }] if *agent == child && *target == child)
    );
    assert!(
        matches!(&press(&mut app, KeyCode::Char('I'), KeyModifiers::SHIFT)[..], [Effect::Request { command: Command::Interrupt { agent , idempotency_key: None, }, then: Then::Interrupted(target) }] if *agent == child && *target == child)
    );
    assert_eq!(app.selected, Some(root));
    assert!(app.bottom.view_stack.is_empty());
}

#[test]
fn archive_restore_round_trip_preserves_the_viewed_agent() {
    let (mut app, root, child) = fixture();
    app.tree.nodes.get_mut(&child).unwrap().agent.status = AgentStatus::Completed { summary: None };
    let effects = press(&mut app, KeyCode::Char('D'), KeyModifiers::NONE);
    assert!(
        matches!(&effects[..], [Effect::Request { command: Command::Archive { target: ArchiveTarget::Agent(id), archived: true , idempotency_key: None, }, .. }] if *id == child)
    );
    let mut archived = app.tree.nodes[&child].agent.clone();
    archived.archived = true;
    reply(
        &mut app,
        Then::BoardArchived {
            agent: child,
            archived: true,
            after_stop: false,
        },
        Ok(Reply::Agents(vec![archived.clone()])),
    );
    assert!(
        app.live_flash()
            .unwrap()
            .contains(". shows archived; shift+d restores")
    );
    assert_eq!(app.selected, Some(root));
    app.show_archived = true;
    app.tree.nodes.get_mut(&child).unwrap().agent = archived.clone();
    app.rebuild_board();
    focus(&mut app, child);
    assert!(
        matches!(&press(&mut app, KeyCode::Char('d'), KeyModifiers::SHIFT)[..], [Effect::Request { command: Command::Archive { target: ArchiveTarget::Agent(id), archived: false , idempotency_key: None, }, .. }] if *id == child)
    );
    archived.archived = false;
    reply(
        &mut app,
        Then::BoardArchived {
            agent: child,
            archived: false,
            after_stop: false,
        },
        Ok(Reply::Agents(vec![archived])),
    );
    assert_eq!(app.live_flash(), Some("restored to the board"));
}

#[test]
fn live_delete_waits_for_terminal_snapshot_then_archives_only_the_agent() {
    let (mut app, _, child) = fixture();
    confirm(&mut app, 'D', child, true);
    press(&mut app, KeyCode::Char('y'), KeyModifiers::NONE);
    let effects = reply(
        &mut app,
        Then::BoardStopped {
            agent: child,
            remove: true,
        },
        Ok(Reply::Ok),
    );
    assert!(effects.iter().all(|e| !matches!(
        e,
        Effect::Request {
            command: Command::Archive { .. },
            ..
        }
    )));
    let tree = app.tree.clone();
    let effects = reply(&mut app, Then::Tree, Ok(Reply::Tree(tree)));
    assert!(effects.iter().all(|e| !matches!(
        e,
        Effect::Request {
            command: Command::Archive { .. },
            ..
        }
    )));
    let mut tree = app.tree.clone();
    tree.nodes.get_mut(&child).unwrap().agent.status = AgentStatus::Cancelled { reason: None };
    let effects = reply(&mut app, Then::Tree, Ok(Reply::Tree(tree)));
    assert!(effects.iter().any(|e| matches!(e, Effect::Request { command: Command::Archive { target: ArchiveTarget::Agent(id), archived: true , idempotency_key: None, }, then: Then::BoardArchived { after_stop: true, .. } } if *id == child)));
    assert!(app.board.pending_removals.is_empty());
}

#[test]
fn delete_failure_never_claims_removal_or_archives_after_failed_stop() {
    let (mut app, _, child) = fixture();
    let effects = reply(
        &mut app,
        Then::BoardStopped {
            agent: child,
            remove: true,
        },
        Err(aether_client::ClientError::Disconnected),
    );
    assert!(effects.is_empty());
    assert!(
        app.live_flash()
            .unwrap()
            .contains("removal was not attempted")
    );
    reply(
        &mut app,
        Then::BoardArchived {
            agent: child,
            archived: true,
            after_stop: true,
        },
        Err(aether_client::ClientError::Disconnected),
    );
    assert!(
        app.live_flash()
            .unwrap()
            .contains("removal did not complete")
    );
    assert!(!app.tree.nodes[&child].agent.archived);
    reply(
        &mut app,
        Then::BoardStopped {
            agent: child,
            remove: true,
        },
        Ok(Reply::Ok),
    );
    reply(
        &mut app,
        Then::Tree,
        Err(aether_client::ClientError::Disconnected),
    );
    assert!(app.board.pending_removals.is_empty());
    assert!(
        app.live_flash()
            .unwrap()
            .contains("removal did not complete")
    );
}

#[test]
fn removing_viewed_root_confirms_even_when_finished_and_selects_a_live_agent() {
    let (mut app, root, child) = fixture();
    app.tree.nodes.get_mut(&root).unwrap().agent.status = AgentStatus::Completed { summary: None };
    focus(&mut app, root);
    confirm(&mut app, 'D', root, true);
    assert!(press(&mut app, KeyCode::Enter, KeyModifiers::NONE).is_empty());
    focus(&mut app, root);
    confirm(&mut app, 'D', root, true);
    assert!(
        matches!(&press(&mut app, KeyCode::Char('y'), KeyModifiers::NONE)[..], [Effect::Request { command: Command::Archive { target: ArchiveTarget::Agent(id), archived: true , idempotency_key: None, }, .. }] if *id == root)
    );
    let mut archived = app.tree.nodes[&root].agent.clone();
    archived.archived = true;
    app.show_archived = true;
    app.options.focus = Some(root);
    reply(
        &mut app,
        Then::BoardArchived {
            agent: root,
            archived: true,
            after_stop: false,
        },
        Ok(Reply::Agents(vec![archived.clone()])),
    );
    assert_eq!(app.selected, Some(child));
    assert_eq!(app.board.cursor, Some(child));
    let mut tree = app.tree.clone();
    tree.nodes.get_mut(&root).unwrap().agent = archived;
    reply(&mut app, Then::Tree, Ok(Reply::Tree(tree)));
    assert_eq!(app.selected, Some(child));
}

#[test]
fn confirmation_counts_live_descendants_through_finished_and_archived_workers() {
    let (mut app, root, child) = fixture();
    let mut finished = app.tree.nodes[&child].clone();
    finished.agent.id = AgentId::new();
    let bridge = finished.agent.id;
    finished.agent.parent = Some(root);
    finished.agent.status = AgentStatus::Completed { summary: None };
    finished.agent.archived = true;
    finished.children = vec![child];
    app.tree.nodes.get_mut(&child).unwrap().agent.parent = Some(bridge);
    app.tree.nodes.get_mut(&root).unwrap().children = vec![bridge];
    app.tree.nodes.insert(bridge, finished);
    focus(&mut app, root);
    confirm(&mut app, 'X', root, false);
    let frame = render_to(&app, 100, 40);
    assert!(frame.contains("currently 1"), "{frame}");
    assert!(frame.contains("No"), "{frame}");
}

#[test]
fn disabled_actions_flash_and_mouse_never_dispatches_lifecycle_commands() {
    let (mut app, _, child) = fixture();
    for (status, archived, letter, reason) in [
        (AgentStatus::Ready, false, 'I', "no turn running"),
        (
            AgentStatus::Completed { summary: None },
            false,
            'P',
            "cannot pause",
        ),
        (AgentStatus::Ready, true, 'D', "cannot restore"),
    ] {
        let node = app.tree.nodes.get_mut(&child).unwrap();
        node.agent.status = status;
        node.agent.archived = archived;
        assert!(press(&mut app, KeyCode::Char(letter), KeyModifiers::NONE).is_empty());
        assert!(app.live_flash().unwrap().contains(reason));
    }
    let mut cache = crate::ui::transcript::ViewCache::default();
    cache.regions.board = ratatui::layout::Rect::new(60, 0, 40, 30);
    let click = |kind| MouseEvent {
        kind,
        column: 80,
        row: 5,
        modifiers: KeyModifiers::SHIFT,
    };
    let mut effects =
        app.dispatch_mouse(click(MouseEventKind::Down(MouseButton::Left)), &mut cache);
    effects.extend(app.dispatch_mouse(click(MouseEventKind::Up(MouseButton::Left)), &mut cache));
    assert!(effects.iter().all(|e| !matches!(
        e,
        Effect::Request {
            command: Command::Cancel { .. }
                | Command::Archive { .. }
                | Command::Interrupt { .. }
                | Command::Pause { .. }
                | Command::Resume { .. },
            ..
        }
    )));
}

#[test]
fn holding_a_lifecycle_key_does_not_repeat_the_action() {
    for letter in ['X', 'D', 'P', 'I'] {
        let (mut app, _, _) = fixture();
        let event = KeyEvent::new_with_kind(
            KeyCode::Char(letter),
            KeyModifiers::SHIFT,
            crossterm::event::KeyEventKind::Repeat,
        );
        assert!(app.dispatch_key(event).is_empty());
        assert!(app.bottom.view_stack.is_empty());
    }
}

#[test]
fn confirmation_fetch_failure_and_disconnect_stop_pending_removal() {
    let (mut app, _, child) = fixture();
    let effects = press(&mut app, KeyCode::Char('X'), KeyModifiers::NONE);
    let then = snapshot_fetch(&effects, child, false);
    let effects = reply(
        &mut app,
        then,
        Err(aether_client::ClientError::Disconnected),
    );
    assert!(effects.is_empty());
    assert!(app.bottom.view_stack.is_empty());
    assert!(app.board.confirmation_pending.is_none());
    assert!(app.live_flash().unwrap().contains("no action taken"));
    reply(
        &mut app,
        Then::BoardStopped {
            agent: child,
            remove: true,
        },
        Ok(Reply::Ok),
    );
    assert!(!app.board.pending_removals.is_empty());
    app.reduce(AppEvent::Disconnected { because: None });
    assert!(app.board.pending_removals.is_empty());
    assert!(
        app.live_flash()
            .unwrap()
            .contains("removal did not complete")
    );
}

#[test]
fn a_finished_root_still_confirms_while_chatting_with_its_worker() {
    let (mut app, root, child) = fixture();
    app.tree.nodes.get_mut(&root).unwrap().agent.status = AgentStatus::Completed { summary: None };
    app.select(child);
    focus(&mut app, root);
    confirm(&mut app, 'D', root, true);
    assert_eq!(app.selected, Some(child));
    assert!(press(&mut app, KeyCode::Enter, KeyModifiers::NONE).is_empty());
}

fn ends_or_removes(effects: &[Effect]) -> bool {
    effects.iter().any(|e| {
        matches!(
            e,
            Effect::Request {
                command: Command::Cancel { .. } | Command::Archive { .. },
                ..
            }
        )
    })
}

#[test]
fn only_a_fresh_press_answers_a_confirmation() {
    let (mut app, _, child) = fixture();
    confirm(&mut app, 'X', child, false);
    // Yes under the cursor, so even a held Enter would confirm.
    press(&mut app, KeyCode::Up, KeyModifiers::NONE);
    for kind in [KeyEventKind::Repeat, KeyEventKind::Release] {
        for code in [
            KeyCode::Char('y'),
            KeyCode::Enter,
            KeyCode::Char('n'),
            KeyCode::Esc,
        ] {
            let effects = app.dispatch_key(KeyEvent::new_with_kind(code, KeyModifiers::NONE, kind));
            assert!(
                effects.is_empty(),
                "{kind:?} {code:?} answered: {effects:?}"
            );
            assert_eq!(
                app.bottom.view_stack.len(),
                1,
                "{kind:?} {code:?} closed it"
            );
        }
    }
    let effects = press(&mut app, KeyCode::Char('y'), KeyModifiers::NONE);
    assert!(
        matches!(&effects[..], [Effect::Request { command: Command::Cancel { agent, .. }, .. }] if *agent == child)
    );
}

#[test]
fn caps_lock_letters_never_act_but_shift_still_does() {
    for letter in ['X', 'D', 'P', 'I'] {
        let (mut app, _, child) = fixture();
        if letter == 'D' {
            // The case that would archive at once, with no confirmation.
            app.tree.nodes.get_mut(&child).unwrap().agent.status =
                AgentStatus::Completed { summary: None };
        }
        let mut caps = KeyEvent::new(KeyCode::Char(letter), KeyModifiers::NONE);
        caps.state = KeyEventState::CAPS_LOCK;
        let effects = app.dispatch_key(caps);
        assert!(effects.is_empty(), "caps lock {letter} acted: {effects:?}");
        assert!(app.bottom.view_stack.is_empty());
        assert!(app.board.confirmation_pending.is_none());
        for c in [letter, letter.to_ascii_lowercase()] {
            let (mut app, _, _) = fixture();
            let mut shifted = KeyEvent::new(KeyCode::Char(c), KeyModifiers::SHIFT);
            shifted.state = KeyEventState::CAPS_LOCK;
            assert!(
                !app.dispatch_key(shifted).is_empty(),
                "shift+{c} under caps lock did nothing"
            );
        }
    }
}

#[test]
fn disconnect_abandons_confirmations_and_drops_stale_snapshots() {
    let (mut app, _, child) = fixture();
    confirm(&mut app, 'X', child, false);
    app.reduce(AppEvent::Disconnected { because: None });
    assert!(app.bottom.view_stack.is_empty());
    assert!(app.live_flash().unwrap().contains("abandoned"));
    app.reduce(AppEvent::Connected);
    assert!(!ends_or_removes(&press(
        &mut app,
        KeyCode::Char('y'),
        KeyModifiers::NONE
    )));

    // A snapshot from the old connection opens nothing, nor disturbs a newer fetch.
    focus(&mut app, child);
    let effects = press(&mut app, KeyCode::Char('X'), KeyModifiers::NONE);
    let stale = snapshot_fetch(&effects, child, false);
    app.reduce(AppEvent::Disconnected { because: None });
    assert!(app.board.confirmation_pending.is_none());
    assert!(app.live_flash().unwrap().contains("abandoned"));
    app.reduce(AppEvent::Connected);
    focus(&mut app, child);
    let effects = press(&mut app, KeyCode::Char('X'), KeyModifiers::NONE);
    let fresh = snapshot_fetch(&effects, child, false);
    let tree = app.tree.clone();
    assert!(reply(&mut app, stale, Ok(Reply::Tree(tree.clone()))).is_empty());
    assert!(app.bottom.view_stack.is_empty());
    assert_eq!(app.board.confirmation_pending, Some(child));
    reply(&mut app, fresh, Ok(Reply::Tree(tree)));
    assert_eq!(app.bottom.view_stack.len(), 1);
}

/// Click `name`'s tree row in a freshly drawn frame.
fn click_row(app: &mut App, name: &str) -> Vec<Effect> {
    let mut cache = crate::ui::transcript::ViewCache::default();
    let screen = crate::testing::render_with(app, &mut cache, 100, 40);
    let tree = cache.board.tree_area;
    let (row, column) = screen
        .lines()
        .enumerate()
        .skip(usize::from(tree.y))
        .take(usize::from(tree.height))
        .find_map(|(y, line)| {
            let x = line
                .chars()
                .skip(usize::from(tree.x))
                .collect::<String>()
                .find(name)?;
            Some((y as u16, tree.x + x as u16))
        })
        .unwrap_or_else(|| panic!("no {name} row in the tree:\n{screen}"));
    let click = |kind| MouseEvent {
        kind,
        column,
        row,
        modifiers: KeyModifiers::NONE,
    };
    let mut effects =
        app.dispatch_mouse(click(MouseEventKind::Down(MouseButton::Left)), &mut cache);
    effects.extend(app.dispatch_mouse(click(MouseEventKind::Up(MouseButton::Left)), &mut cache));
    effects
}

#[test]
fn a_tree_click_never_takes_the_keys_from_an_open_view() {
    let (mut app, _, _) = fixture();
    app.board.focus = BoardFocus::BottomPane;
    click_row(&mut app, "worker");
    assert!(
        app.board.focused(),
        "no view open: the click focuses the board"
    );

    let (mut app, root, _) = fixture();
    app.board.focus = BoardFocus::BottomPane;
    let theme = crate::ui::views::theme::ThemeView::new(&app);
    app.bottom.push_view(Box::new(theme));
    assert!(click_row(&mut app, "worker").is_empty());
    assert!(!app.board.focused());
    assert_eq!(app.selected, Some(root));
    assert_eq!(app.bottom.view_stack.len(), 1);
    let flash = app.live_flash().unwrap_or_default().to_owned();
    assert!(flash.contains("esc closes this view first"), "{flash}");

    let (mut app, _, child) = fixture();
    confirm(&mut app, 'X', child, false);
    assert!(click_row(&mut app, "worker").is_empty());
    assert!(!app.board.focused());
    assert_eq!(app.board.cursor, Some(child));
    press(&mut app, KeyCode::Down, KeyModifiers::NONE);
    assert_eq!(app.board.cursor, Some(child), "↓ answers the dialog");
    press(&mut app, KeyCode::Esc, KeyModifiers::NONE);
    assert!(app.bottom.view_stack.is_empty(), "esc closes the dialog");
    assert!(!app.board.focused());
}

/// The wheel must never carry a stop or remove confirmation onto Yes.
#[test]
fn a_board_confirmation_ignores_the_wheel() {
    let (mut app, _, child) = fixture();
    confirm(&mut app, 'D', child, true);
    let mut cache = crate::ui::transcript::ViewCache::default();
    crate::testing::render_with(&app, &mut cache, 100, 40);
    let r = cache.regions.view;
    for kind in [MouseEventKind::ScrollUp, MouseEventKind::ScrollDown] {
        let wheel = MouseEvent {
            kind,
            column: r.x,
            row: r.y,
            modifiers: KeyModifiers::NONE,
        };
        assert!(app.dispatch_mouse(wheel, &mut cache).is_empty());
    }
    assert!(press(&mut app, KeyCode::Enter, KeyModifiers::NONE).is_empty());
    assert!(app.bottom.view_stack.is_empty(), "enter answered no");
}

#[test]
fn duplicate_names_reach_composer_confirmations_and_notices() {
    let (mut app, root, _) = fixture();
    let mut second = app.tree.nodes[&root].clone();
    let id = AgentId::new();
    second.agent.id = id;
    second.agent.created_at += chrono::Duration::seconds(1);
    second.agent.status = AgentStatus::Ready;
    second.children.clear();
    app.tree.roots.push(id);
    app.tree.nodes.insert(id, second);
    app.selected = Some(id);
    app.rebuild_board();
    app.board.focus = BoardFocus::BottomPane;
    let name = format!("{} 2", app.tree.nodes[&root].agent.name);
    assert_eq!(app.agent_name(id), name);
    assert!(render_to(&app, 160, 44).contains(&format!("Message {name}")));
    app.bottom.approval.deny_target = Some(id);
    assert!(render_to(&app, 160, 44).contains(&format!("Tell {name} what to do instead")));
    for (letter, remove) in [('X', false), ('D', true)] {
        focus(&mut app, id);
        confirm(&mut app, letter, id, remove);
        assert!(render_to(&app, 160, 44).contains(&format!("{name}?")));
        app.bottom.view_stack.clear();
    }
    app.reduce(AppEvent::Reply {
        then: Then::Interrupted(id),
        result: Ok(Reply::Interrupted { was_running: false }),
    });
    assert!(app.live_flash().unwrap().contains(&name));
}

#[test]
fn duplicate_agent_interrupt_notice_uses_the_board_name() {
    use aether_core::event::{Actor, Event, EventDraft, EventKind};
    let (mut app, root, _) = fixture();
    let mut second = app.tree.nodes[&root].clone();
    let id = AgentId::new();
    second.agent.id = id;
    second.agent.created_at += chrono::Duration::seconds(1);
    second.children.clear();
    app.tree.roots.push(id);
    app.tree.nodes.insert(id, second);
    app.selected = Some(id);
    app.rebuild_board();
    let event = Event::from_draft(
        EventDraft::new(Actor::System, EventKind::AgentInterrupted { turn: 1 }).agent(id),
        aether_core::id::EventId::new(),
        1,
        app.now,
    );
    app.reduce(AppEvent::Frame(aether_client::SubscriptionFrame::Event(
        Box::new(event),
    )));
    let expected = format!(
        "Interrupted · tell {} 2 what to do differently",
        app.tree.nodes[&root].agent.name
    );
    assert!(render_to(&app, 160, 44).contains(&expected));
}
