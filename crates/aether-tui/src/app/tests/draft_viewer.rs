use super::*;
use crate::testing::{app_with_transcript, push_message};

fn open_picker(app: &mut App) {
    app.reduce(AppEvent::Key(KeyEvent::new(
        KeyCode::Char('o'),
        KeyModifiers::ALT,
    )));
}

#[test]
fn file_picker_finds_an_earlier_message_when_the_latest_has_no_paths() {
    let mut app = app_with_transcript(&["[notes](notes.md)", "All done"]);
    open_picker(&mut app);
    assert!(app.viewer.open);
    assert_eq!(app.viewer.picker.len(), 1);
    assert_eq!(app.viewer.picker[0].target, "notes.md");
}

#[test]
fn file_picker_prefers_the_clicked_message_and_falls_back_if_it_has_no_paths() {
    let mut app = app_with_transcript(&["[old](old.md)", "[new](new.md)", "All done"]);
    let agent = app.selected.expect("agent");
    let cells = &app.transcripts[&agent].cells;
    let clicked = cells
        .iter()
        .find(|cell| matches!(&cell.model, CellModel::User { text, .. } if text == "[old](old.md)"))
        .expect("old message")
        .id;
    app.viewer.source_cell = Some((Some(agent), clicked));
    open_picker(&mut app);
    assert_eq!(app.viewer.picker[0].target, "old.md");
    let last = app.transcripts[&agent].cells.last().expect("last").id;
    app.viewer.source_cell = Some((Some(agent), last));
    open_picker(&mut app);
    assert_eq!(app.viewer.picker[0].target, "new.md");
}

#[test]
fn file_picker_search_is_limited_to_64_recent_messages() {
    let mut app = app_with_transcript(&["[old](old.md)"]);
    let agent = app.selected.expect("agent");
    for _ in 0..64 {
        push_message(
            &mut app,
            agent,
            aether_core::model::MessageRole::Assistant,
            "No paths here",
        );
    }
    open_picker(&mut app);
    assert!(!app.viewer.open);
    assert_eq!(
        app.live_flash(),
        Some("No local file references in recent messages")
    );
}

#[test]
fn file_picker_checks_the_active_message_before_recent_finished_messages() {
    let mut app = app_with_transcript(&["[old](old.md)"]);
    let agent = app.selected.expect("agent");
    app.transcripts.get_mut(&agent).expect("transcript").active = Some(Cell {
        id: super::super::transcript::CellId(u64::MAX),
        revision: Default::default(),
        at: app.now,
        model: CellModel::Assistant {
            text: "[active](active.md)".into(),
        },
    });
    open_picker(&mut app);
    assert!(app.viewer.open);
    assert_eq!(app.viewer.picker[0].target, "active.md");
}
