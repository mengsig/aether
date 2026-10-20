#![allow(clippy::expect_used, clippy::unwrap_used, clippy::panic)]
use super::*;
use crate::{
    app::{AppEvent, Effect},
    testing::{app_with_transcript, render_with},
    ui::transcript::ViewCache,
};
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers, MouseButton, MouseEvent, MouseEventKind};

fn key(app: &mut crate::App, code: KeyCode, modifiers: KeyModifiers) -> Vec<Effect> {
    app.reduce(AppEvent::Key(KeyEvent::new(code, modifiers)))
}

fn setup() -> (tempfile::TempDir, crate::App) {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(
        dir.path().join("notes.md"),
        "# Preview heading\nHello from a local file",
    )
    .unwrap();
    let mut app = app_with_transcript(&[]);
    let agent = app.selected.unwrap();
    crate::testing::push_message(
        &mut app,
        agent,
        aether_core::model::MessageRole::Assistant,
        "[notes](notes.md)",
    );
    app.options.cwd = Some(dir.path().to_owned());
    app.size = (160, 40);
    app.board.open = false;
    app.approvals.clear();
    (dir, app)
}

fn load(app: &mut crate::App, effects: Vec<Effect>) {
    assert_eq!(effects.len(), 1);
    let Effect::PreviewFile(request) = &effects[0] else {
        panic!("expected a preview effect")
    };
    let result = read(request).map_err(|e| format!("{e:#}"));
    app.reduce(AppEvent::PreviewReady {
        id: request.id,
        result,
    });
}

#[test]
fn picker_opens_markdown_beside_transcript_and_closes_during_streaming() {
    let (_dir, mut app) = setup();
    key(&mut app, KeyCode::Char('o'), KeyModifiers::ALT);
    assert_eq!(app.viewer.picker.len(), 1);
    let effects = key(&mut app, KeyCode::Enter, KeyModifiers::NONE);
    load(&mut app, effects);
    let mut cache = ViewCache::default();
    let screen = render_with(&app, &mut cache, 160, 40);
    assert!(screen.contains("Preview heading"), "{screen}");
    assert!(screen.contains("notes.md"));
    assert!(cache.regions.transcript.width <= 80);
    let agent = app.selected.unwrap();
    crate::testing::push_message(
        &mut app,
        agent,
        aether_core::model::MessageRole::Assistant,
        "Still streaming",
    );
    key(&mut app, KeyCode::Esc, KeyModifiers::NONE);
    assert!(!app.viewer.open);
    assert!(render_with(&app, &mut cache, 160, 40).contains("Still streaming"));
}

#[test]
fn click_opens_reference_but_drag_keeps_selection() {
    let (_dir, mut app) = setup();
    let mut cache = ViewCache::default();
    render_with(&app, &mut cache, 160, 40);
    let hit = cache.file_hits.first().expect("file hit").clone();
    let mouse = |kind| MouseEvent {
        kind,
        column: hit.area.x,
        row: hit.area.y,
        modifiers: KeyModifiers::NONE,
    };
    app.dispatch_mouse(mouse(MouseEventKind::Down(MouseButton::Left)), &mut cache);
    app.dispatch_mouse(mouse(MouseEventKind::Drag(MouseButton::Left)), &mut cache);
    assert!(
        app.dispatch_mouse(mouse(MouseEventKind::Up(MouseButton::Left)), &mut cache)
            .is_empty()
    );
    assert!(!app.viewer.open);
    app.dispatch_mouse(mouse(MouseEventKind::Down(MouseButton::Left)), &mut cache);
    let effects = app.dispatch_mouse(mouse(MouseEventKind::Up(MouseButton::Left)), &mut cache);
    assert!(matches!(&effects[..], [Effect::PreviewFile(_)]));
    assert!(app.viewer.open);
}

#[test]
fn outside_workspace_requires_second_action_and_stale_reads_do_not_reopen() {
    let (_dir, mut app) = setup();
    let outside = tempfile::NamedTempFile::new().unwrap();
    std::fs::write(outside.path(), "private").unwrap();
    let effects = app.open_reference(references::Reference {
        target: outside.path().display().to_string(),
        label: "outside".into(),
    });
    load(&mut app, effects);
    assert!(matches!(app.viewer.preview, Some(Ok(Preview::Outside(_)))));
    let effects = key(&mut app, KeyCode::Enter, KeyModifiers::NONE);
    load(&mut app, effects);
    assert!(matches!(app.viewer.preview, Some(Ok(Preview::Text { .. }))));
    let id = app.viewer.request;
    key(&mut app, KeyCode::Esc, KeyModifiers::NONE);
    app.reduce(AppEvent::PreviewReady {
        id,
        result: Err("stale".into()),
    });
    assert!(!app.viewer.open);
    assert!(app.viewer.preview.is_none());
}

#[test]
fn sizes_are_bounded_and_remote_sessions_never_read() {
    let (_dir, mut app) = setup();
    key(&mut app, KeyCode::Char('o'), KeyModifiers::ALT);
    for w in [0, 1, 20, 39, 40, 160] {
        for h in [0, 1, 40] {
            let area = ratatui::layout::Rect::new(0, 0, w, h);
            let (left, right) = split(area, &app.viewer);
            assert!(left.right() <= w && right.right() <= w);
            assert!(left.bottom() <= h && right.bottom() <= h);
        }
    }
    app.options.endpoint = Some(aether_client::Endpoint::Ssh(
        aether_client::SshRemote::new("test", "example.invalid").unwrap(),
    ));
    assert!(
        app.open_reference(references::Reference {
            target: "notes.md".into(),
            label: "notes".into()
        },)
            .is_empty()
    );
}

#[test]
fn text_limits_and_invalid_input_are_visible_errors() {
    let (dir, mut app) = setup();
    std::fs::write(dir.path().join("huge.txt"), vec![b'a'; 1024 * 1024 + 1]).unwrap();
    let effects = app.open_reference(references::Reference {
        target: "huge.txt".into(),
        label: "huge".into(),
    });
    load(&mut app, effects);
    assert!(
        matches!(&app.viewer.preview,Some(Ok(Preview::Failed {error,..})) if error.contains("1 MiB"))
    );
    std::fs::write(dir.path().join("bad.txt"), [0xff]).unwrap();
    let effects = app.open_reference(references::Reference {
        target: "bad.txt".into(),
        label: "bad".into(),
    });
    load(&mut app, effects);
    assert!(
        matches!(&app.viewer.preview,Some(Ok(Preview::Failed {error,..})) if error.contains("UTF-8"))
    );
}

#[test]
fn resizing_keeps_the_original_workspace_and_open_external_uses_argv() {
    let (dir, mut app) = setup();
    key(&mut app, KeyCode::Char('o'), KeyModifiers::ALT);
    let effects = key(&mut app, KeyCode::Enter, KeyModifiers::NONE);
    load(&mut app, effects);
    let other = tempfile::tempdir().unwrap();
    app.options.cwd = Some(other.path().to_owned());
    let effects = key(&mut app, KeyCode::Char('='), KeyModifiers::ALT);
    let Effect::PreviewFile(request) = &effects[0] else {
        panic!("preview effect")
    };
    assert_eq!(request.workspace, dir.path());
    assert_eq!(app.viewer.share, 55);
    load(&mut app, effects);
    let effects = key(&mut app, KeyCode::Char('o'), KeyModifiers::NONE);
    assert!(
        matches!(&effects[..],[Effect::OpenFile(path)] if *path == dir.path().join("notes.md").canonicalize().unwrap())
    );
    key(&mut app, KeyCode::Char('z'), KeyModifiers::ALT);
    assert!(app.viewer.fullscreen);
    key(&mut app, KeyCode::Tab, KeyModifiers::NONE);
    assert!(!app.viewer.focused);
}

#[test]
fn wrapped_unicode_file_references_have_hit_regions_on_both_rows() {
    let (_dir, mut app) = setup();
    let agent = app.selected.unwrap();
    let path = "./資料/long-name-for-the-wrapped-preview-file.png";
    crate::testing::push_message(
        &mut app,
        agent,
        aether_core::model::MessageRole::Assistant,
        path,
    );
    let mut cache = ViewCache::default();
    render_with(&app, &mut cache, 30, 40);
    let hits: Vec<_> = cache
        .file_hits
        .iter()
        .filter(|h| h.reference.target == path)
        .collect();
    assert!(hits.len() >= 2, "{hits:?}");
    assert!(hits.iter().all(|hit| hit.area.right() <= 30));
}

#[test]
fn ordinary_label_words_do_not_open_files_and_nested_names_choose_longer_path() {
    let (_dir, mut app) = setup();
    let agent = app.selected.unwrap();
    crate::testing::push_message(
        &mut app,
        agent,
        aether_core::model::MessageRole::Assistant,
        "notes outside a link; [notes](notes.md); a.png and longer/a.png",
    );
    let mut cache = ViewCache::default();
    render_with(&app, &mut cache, 160, 40);
    let top = cache
        .file_hits
        .iter()
        .find(|h| h.reference.target == "longer/a.png")
        .unwrap()
        .clone();
    let mouse = |kind| MouseEvent {
        kind,
        column: top.area.right() - 2,
        row: top.area.y,
        modifiers: KeyModifiers::NONE,
    };
    app.dispatch_mouse(mouse(MouseEventKind::Down(MouseButton::Left)), &mut cache);
    let effects = app.dispatch_mouse(mouse(MouseEventKind::Up(MouseButton::Left)), &mut cache);
    assert!(
        matches!(&effects[..],[Effect::PreviewFile(request)] if request.target == "longer/a.png")
    );
    assert!(
        !cache
            .file_hits
            .iter()
            .any(|h| h.reference.target == "notes.md" && h.area.y == top.area.y && h.area.x == 2)
    );
}

#[test]
fn approval_uses_full_width_even_when_viewer_is_fullscreen() {
    let mut app = crate::testing::populated_app();
    app.size = (160, 44);
    app.board.open = false;
    app.viewer.open = true;
    app.viewer.fullscreen = true;
    app.bottom
        .approval
        .note_key(app.now - chrono::Duration::seconds(3));
    app.track_approvals();
    app.now += chrono::Duration::seconds(3);
    assert!(app.shown_approval().is_some());
    let mut cache = ViewCache::default();
    render_with(&app, &mut cache, 160, 44);
    assert!(cache.viewer_area.is_empty());
    assert_eq!(cache.regions.transcript.width, 160);
}

#[test]
fn outside_confirmation_is_bound_to_the_displayed_canonical_target() {
    let (dir, mut app) = setup();
    let outside = tempfile::tempdir().unwrap();
    std::fs::write(outside.path().join("first.txt"), "first").unwrap();
    std::fs::write(outside.path().join("second.txt"), "second").unwrap();
    let link = dir.path().join("link.txt");
    std::os::unix::fs::symlink(outside.path().join("first.txt"), &link).unwrap();
    let effects = app.open_reference(references::Reference {
        target: "link.txt".into(),
        label: "link".into(),
    });
    load(&mut app, effects);
    assert!(
        matches!(&app.viewer.preview,Some(Ok(Preview::Outside(path))) if path.ends_with("first.txt"))
    );
    std::fs::remove_file(&link).unwrap();
    std::os::unix::fs::symlink(outside.path().join("second.txt"), &link).unwrap();
    let effects = key(&mut app, KeyCode::Enter, KeyModifiers::NONE);
    load(&mut app, effects);
    assert!(
        matches!(&app.viewer.preview,Some(Ok(Preview::Outside(path))) if path.ends_with("second.txt"))
    );
}
