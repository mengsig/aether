use super::*;
use aether_tui::commands::KeySpec;
use cases::{KeyCase, Stage};
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

#[path = "key_cases.rs"]
mod cases;

/// A key that no longer does anything is worse than no key at all: the table
/// is what the help shows, so every row is driven through `dispatch_key`.
#[test]
fn every_key_in_the_table_does_something() {
    for case in &cases::cases() {
        assert_does_something(case);
    }
}

/// Both ways: every row has a case, and no case drives a row the table lacks.
#[test]
fn the_key_cases_and_the_table_agree() {
    let cases = cases::cases();
    let covered = |spec: &KeySpec| {
        cases
            .iter()
            .any(|c| c.keys == spec.keys && c.scope == spec.scope)
    };
    let missing: Vec<&str> = KEYS
        .iter()
        .filter(|k| !covered(k))
        .map(|k| k.keys)
        .collect();
    assert!(
        missing.is_empty(),
        "these key rows are in the help but nothing drives them: {missing:?}"
    );
    for case in &cases {
        assert!(
            KEYS.iter()
                .any(|k| k.keys == case.keys && k.scope == case.scope),
            "{:?} ({:?}) is exercised here but missing from the key table",
            case.keys,
            case.scope
        );
    }
}

/// SPEC §8 Phase 4: `q` typed in any state never exits; nothing binds it.
#[test]
fn q_typed_in_any_state_never_quits() {
    use Stage::*;
    let mut states: Vec<(String, App)> = [
        Busy,
        Idle,
        Draft,
        History,
        Child,
        Board,
        Approval,
        ChildApproval,
        Queued,
        Killed,
        VoiceCapture,
        VoiceRecovery,
    ]
    .into_iter()
    .map(|stage| (format!("{stage:?}"), staged(stage)))
    .collect();
    for (label, keys) in [
        ("the / popup", "/"),
        ("the ? panel", "?"),
        ("the @ popup", "@"),
        ("a slash view", "/settings\n"),
    ] {
        let mut app = staged(Stage::Idle);
        for c in keys.chars() {
            let code = if c == '\n' {
                KeyCode::Enter
            } else {
                KeyCode::Char(c)
            };
            app.dispatch_key(KeyEvent::new(code, KeyModifiers::NONE));
        }
        states.push((label.into(), app));
    }
    let mut find = staged(Stage::Idle);
    find.dispatch_key(KeyEvent::new(KeyCode::Char('f'), KeyModifiers::CONTROL));
    states.push(("find".into(), find));
    let mut narrow = staged(Stage::Board);
    narrow.size = (60, 40);
    states.push(("the board as an overlay".into(), narrow));

    for (label, mut app) in states {
        let effects = app.dispatch_key(KeyEvent::new(KeyCode::Char('q'), KeyModifiers::NONE));
        assert!(
            !app.quit_requested
                && !effects
                    .iter()
                    .any(|e| matches!(e, aether_tui::Effect::Quit)),
            "q quit from {label}"
        );
    }
}

fn assert_does_something(case: &KeyCase) {
    let mut app = staged(case.stage);
    let before = snapshot(&app);
    let effects = app.dispatch_key(KeyEvent::new(case.code, case.mods));
    assert!(
        before != snapshot(&app) || !effects.is_empty(),
        "the key {:?} ({:?}, {:?}) did nothing",
        case.keys,
        case.scope,
        case.stage
    );
}

/// Everything a key press may change. None of it moved and no effect: the key did nothing.
fn snapshot(app: &App) -> String {
    format!(
        "{:?}|{}|{:?}|{}|{}|{:?}|{:?}|{}|{:?}|{}|{}|{:?}|{:?}|{}|{}|{}|{:?}|{:?}",
        app.input.text,
        app.input.cursor,
        app.selected,
        app.quit_requested,
        app.details,
        app.board,
        app.bottom.popup,
        app.bottom.shortcuts,
        app.bottom.find,
        app.bottom.view_stack.len(),
        app.scroll_log.since(0).count(),
        app.mouse,
        app.flash,
        app.show_archived,
        app.live_flash().unwrap_or(""),
        app.bottom.approval.cursor,
        app.bottom.approval.deferred,
        app.viewer,
    )
}

fn stage_document(app: &mut App) {
    use aether_tui::viewer::{Pages, Preview, ReadRequest, View};
    app.viewer.open = true;
    app.board.open = false;
    app.viewer.pending = Some(ReadRequest {
        id: app.viewer.request,
        target: "report.pdf".into(),
        workspace: "/workspace".into(),
        approved_outside: None,
        width: 40,
        height: 20,
        protocol: aether_tui::viewer::probe::Protocol::HalfBlocks,
        cell_pixels: (0, 0),
        theme: app.theme,
        page: 2,
        view: View::Text,
    });
    app.viewer.preview = Some(Ok(Preview::Text {
        path: "/workspace/report.pdf".into(),
        rows: Vec::new(),
        pages: Some(Pages {
            current: 2,
            total: 3,
        }),
    }));
}

fn staged(stage: Stage) -> App {
    use aether_tui::testing::populated_app;
    let mut app = populated_app();
    app.size = (120, 40);
    if !matches!(stage, Stage::Approval | Stage::ChildApproval) {
        app.approvals.clear();
    }
    let viewed = app.selected.expect("the fixture views its agent");
    let press = |app: &mut App, code, mods| {
        app.dispatch_key(KeyEvent::new(code, mods));
    };
    match stage {
        Stage::Viewer => {
            app.viewer.open = true;
            app.board.open = false;
            app.viewer.preview = Some(Ok(aether_tui::viewer::Preview::Unsupported(
                std::path::PathBuf::from("preview.txt"),
            )));
        }
        Stage::Document => stage_document(&mut app),
        Stage::Busy | Stage::Approval => {}
        Stage::ChildApproval => {
            let child = add_child(&mut app, viewed);
            for approval in &mut app.approvals {
                approval.agent_id = child;
            }
        }
        Stage::Queued => {
            app.input.set_text("after this turn");
            press(&mut app, KeyCode::Tab, KeyModifiers::NONE);
        }
        Stage::Idle => {
            app.tree.nodes.get_mut(&viewed).expect("node").agent.status =
                aether_core::agent::AgentStatus::Ready;
        }
        Stage::Draft => app.input.set_text("a draft worth editing"),
        Stage::Killed => {
            app.input.set_text("a draft worth editing");
            press(&mut app, KeyCode::Char('w'), KeyModifiers::CONTROL);
        }
        Stage::History => {
            app.input.set_text("sent before");
            app.input.take();
        }
        Stage::Child => {
            let id = add_child(&mut app, viewed);
            app.select(id);
        }
        Stage::Board => {
            add_child(&mut app, viewed);
            press(&mut app, KeyCode::Char('g'), KeyModifiers::CONTROL);
            press(&mut app, KeyCode::Down, KeyModifiers::NONE);
        }
        Stage::VoiceCapture | Stage::VoiceRecovery => {
            app.voice.available = true;
            app.options.prefs_dir = Some("/dev/null/unused-voice-config".into());
            app.input.set_text("existing draft ");
            if matches!(stage, Stage::VoiceRecovery) {
                let effects = app.dispatch_key(KeyEvent::new(KeyCode::F(4), KeyModifiers::NONE));
                let id = effects
                    .iter()
                    .find_map(|effect| match effect {
                        aether_tui::Effect::Voice(aether_tui::voice::Effect::Record {
                            id, ..
                        }) => Some(*id),
                        _ => None,
                    })
                    .expect("recording starts without executing the fake effect");
                app.input.insert_str("edited");
                app.reduce(AppEvent::Voice(aether_tui::voice::Reply::Recorded {
                    id,
                    result: Ok("recovered words".into()),
                }));
            }
        }
    }
    if matches!(stage, Stage::Approval | Stage::ChildApproval) {
        // The card is on screen and has been read, so its answer keys count.
        app.reduce(AppEvent::Resize(120, 40));
        app.now += chrono::Duration::milliseconds(aether_tui::app::approval_flow::MIN_VISIBLE_MS);
    }
    app
}

/// A running child of `parent`, delivered the way the daemon's tree reply is.
fn add_child(app: &mut App, parent: aether_core::id::AgentId) -> aether_core::id::AgentId {
    let mut tree = app.tree.clone();
    let mut child = tree.nodes[&parent].clone();
    let id = aether_core::id::AgentId::new();
    child.agent.id = id;
    child.agent.parent = Some(parent);
    child.agent.name = "child".into();
    child.children.clear();
    tree.nodes.insert(id, child);
    tree.nodes
        .get_mut(&parent)
        .expect("parent")
        .children
        .push(id);
    app.reduce(AppEvent::Reply {
        then: aether_tui::app::Then::Tree,
        result: Ok(aether_protocol::Reply::Tree(tree)),
    });
    id
}
