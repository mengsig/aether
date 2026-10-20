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
    let effects = key(&mut app, KeyCode::Char('e'), KeyModifiers::ALT);
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
fn file_reference_underlines_preserve_error_and_search_styles() {
    use ratatui::{layout::Rect, text::Line};

    let theme = crate::ui::theme::Theme::resolve(Default::default());
    let target = "missing-notes.txt";
    let matches = vec![(
        references::Reference {
            target: target.into(),
            label: target.into(),
        },
        vec![(5, 5 + target.len())],
    )];
    for style in [theme.failure(), theme.find_hit(true), theme.find_hit(false)] {
        let mut line = Line::from(ratatui::text::Span::styled(
            format!("cat: {target}: No such file"),
            style,
        ));
        let mut hits = Vec::new();
        mark_references(
            &mut line,
            Rect::new(0, 0, 80, 1),
            &matches,
            crate::app::transcript::CellId(1),
            &mut hits,
            &theme,
        );
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].area, Rect::new(5, 0, target.len() as u16, 1));
        assert!(line.spans.iter().all(|span| {
            span.style.fg == style.fg
                && span.style.bg == style.bg
                && span.style.add_modifier.contains(style.add_modifier)
        }));
        assert_eq!(line.spans[1].content, target);
        assert!(
            line.spans[1]
                .style
                .add_modifier
                .contains(theme.file_reference().add_modifier)
        );
    }
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

#[test]
fn accepted_async_preview_requests_redraw_but_stale_reply_does_not() {
    let (_dir, mut app) = setup();
    let effects = app.open_reference(references::Reference {
        target: "notes.md".into(),
        label: "notes".into(),
    });
    let id = app.viewer.request;
    app.dirty = false;
    load(&mut app, effects);
    assert!(app.dirty);
    app.dirty = false;
    app.reduce(AppEvent::PreviewReady {
        id: id.wrapping_sub(1),
        result: Err("stale".into()),
    });
    assert!(!app.dirty);
    assert!(matches!(app.viewer.preview, Some(Ok(Preview::Text { .. }))));
}

#[test]
fn trailing_newline_does_not_claim_truncation_but_real_limits_do() {
    let (dir, mut app) = setup();
    for (contents, truncated) in [
        ("A complete file.\n".to_owned(), false),
        ("x".repeat(70 * 1024), true),
        ("x\n".repeat(4001), true),
    ] {
        std::fs::write(dir.path().join("notes.md"), contents).unwrap();
        let effects = app.open_reference(references::Reference {
            target: "notes.md".into(),
            label: "notes".into(),
        });
        load(&mut app, effects);
        let Some(Ok(Preview::Text { rows, .. })) = &app.viewer.preview else {
            panic!("text preview")
        };
        let text = crate::ui::cells::text::plain(rows).join("\n");
        assert_eq!(text.contains("Preview truncated"), truncated);
        assert_eq!(text.contains("Alt+E opens"), truncated);
        assert!(!rows.is_empty());
    }
}

#[test]
fn external_open_waits_for_canonical_outside_confirmation() {
    let (_dir, mut app) = setup();
    let outside = tempfile::NamedTempFile::new().unwrap();
    std::fs::write(outside.path(), "outside").unwrap();
    let effects = app.open_reference(references::Reference {
        target: outside.path().display().to_string(),
        label: "outside".into(),
    });
    load(&mut app, effects);
    assert!(key(&mut app, KeyCode::Char('e'), KeyModifiers::ALT).is_empty());
    let effects = key(&mut app, KeyCode::Enter, KeyModifiers::NONE);
    load(&mut app, effects);
    assert!(matches!(
        &key(&mut app, KeyCode::Char('e'), KeyModifiers::ALT)[..],
        [Effect::OpenFile(_)]
    ));
}

#[test]
fn split_viewer_keeps_every_letter_in_new_and_existing_drafts() {
    let (_dir, mut app) = setup();
    key(&mut app, KeyCode::Char('o'), KeyModifiers::ALT);
    let effects = key(&mut app, KeyCode::Enter, KeyModifiers::NONE);
    load(&mut app, effects);
    assert!(app.viewer.focused && !app.viewer.fullscreen);
    for initial in ["", "existing "] {
        app.input.set_text(initial);
        for character in "orange outlook".chars() {
            let effects = key(&mut app, KeyCode::Char(character), KeyModifiers::NONE);
            assert!(
                !effects
                    .iter()
                    .any(|effect| matches!(effect, Effect::OpenFile(_)))
            );
        }
        assert_eq!(app.input.text, format!("{initial}orange outlook"));
        let effects = key(&mut app, KeyCode::Char('e'), KeyModifiers::ALT);
        assert!(matches!(&effects[..], [Effect::OpenFile(_)]));
        assert_eq!(app.input.text, format!("{initial}orange outlook"));
    }
}

#[test]
fn picker_discards_previous_preview_and_survives_layout_changes() {
    let (_dir, mut app) = setup();
    key(&mut app, KeyCode::Char('o'), KeyModifiers::ALT);
    let effects = key(&mut app, KeyCode::Enter, KeyModifiers::NONE);
    load(&mut app, effects);
    let previous = app.viewer.request;
    assert!(app.viewer.path().is_some());
    key(&mut app, KeyCode::Char('o'), KeyModifiers::ALT);
    assert!(app.viewer.pending.is_none());
    assert!(app.viewer.preview.is_none());
    assert!(app.viewer.path().is_none());
    app.reduce(AppEvent::PreviewReady {
        id: previous,
        result: Err("late old file".into()),
    });
    assert!(app.viewer.preview.is_none());
    for event in [
        AppEvent::Key(KeyEvent::new(KeyCode::Char('z'), KeyModifiers::ALT)),
        AppEvent::Key(KeyEvent::new(KeyCode::Char('-'), KeyModifiers::ALT)),
        AppEvent::Key(KeyEvent::new(KeyCode::Char('='), KeyModifiers::ALT)),
        AppEvent::Resize(140, 40),
    ] {
        let effects = app.reduce(event);
        assert!(
            !effects
                .iter()
                .any(|effect| matches!(effect, Effect::PreviewFile(_)))
        );
        assert_eq!(app.viewer.picker.len(), 1);
        assert!(app.viewer.pending.is_none());
    }
    assert!(key(&mut app, KeyCode::Char('e'), KeyModifiers::ALT).is_empty());
    let screen = render_with(&app, &mut ViewCache::default(), 140, 40);
    assert!(screen.contains("Files · Tab"));
    let effects = key(&mut app, KeyCode::Enter, KeyModifiers::NONE);
    assert!(matches!(&effects[..], [Effect::PreviewFile(_)]));
    assert!(app.viewer.picker.is_empty());
}

#[test]
fn focused_board_owns_its_space_and_escape_before_the_viewer() {
    let (_dir, mut app) = setup();
    for fullscreen in [false, true] {
        app.viewer.open = true;
        app.viewer.focused = true;
        app.viewer.fullscreen = fullscreen;
        key(&mut app, KeyCode::Char('g'), KeyModifiers::CONTROL);
        assert!(app.board.focused());
        let mut cache = ViewCache::default();
        render_with(&app, &mut cache, 160, 40);
        assert!(cache.viewer_area.is_empty());
        assert!(!cache.regions.board.is_empty());
        key(&mut app, KeyCode::Esc, KeyModifiers::NONE);
        assert!(!app.board.focused());
        assert!(app.viewer.open);
        render_with(&app, &mut cache, 160, 40);
        assert!(!cache.viewer_area.is_empty());
    }
}

#[test]
fn fullscreen_keys_cannot_send_hidden_draft_and_tab_reveals_chat() {
    let (_dir, mut app) = setup();
    key(&mut app, KeyCode::Char('o'), KeyModifiers::ALT);
    let effects = key(&mut app, KeyCode::Enter, KeyModifiers::NONE);
    load(&mut app, effects);
    app.input.set_text("existing draft");
    key(&mut app, KeyCode::Char('z'), KeyModifiers::ALT);
    assert!(app.viewer.fullscreen && app.viewer.focused);
    assert!(key(&mut app, KeyCode::Enter, KeyModifiers::NONE).is_empty());
    key(&mut app, KeyCode::Char('x'), KeyModifiers::NONE);
    key(&mut app, KeyCode::Char('X'), KeyModifiers::SHIFT);
    assert!(key(&mut app, KeyCode::Char('m'), KeyModifiers::CONTROL).is_empty());
    assert_eq!(app.input.text, "existing draft");
    key(&mut app, KeyCode::Tab, KeyModifiers::NONE);
    assert!(!app.viewer.fullscreen && !app.viewer.focused);
    key(&mut app, KeyCode::Char('z'), KeyModifiers::ALT);
    key(&mut app, KeyCode::Char('c'), KeyModifiers::CONTROL);
    assert!(!app.viewer.open);
    assert_eq!(app.input.text, "existing draft");
}

#[test]
fn fullscreen_blocks_paste_and_edit_shortcuts_but_split_accepts_paste() {
    let (_dir, mut app) = setup();
    app.viewer.open = true;
    app.viewer.fullscreen = true;
    app.input.set_text("kept");
    let cursor = app.input.cursor;
    assert!(app.reduce(AppEvent::Paste(" hidden".into())).is_empty());
    assert!(
        app.live_flash()
            .unwrap()
            .contains("file viewer is full screen")
    );
    for (character, modifiers) in [
        ('w', KeyModifiers::CONTROL),
        ('u', KeyModifiers::CONTROL),
        ('k', KeyModifiers::CONTROL),
        ('y', KeyModifiers::CONTROL),
        ('v', KeyModifiers::CONTROL),
        ('v', KeyModifiers::ALT),
    ] {
        assert!(key(&mut app, KeyCode::Char(character), modifiers).is_empty());
        assert_eq!(app.input.text, "kept");
        assert_eq!(app.input.cursor, cursor);
        assert_eq!(app.input.image_reads, 0);
    }
    app.input.image_reads = 1;
    app.reduce(AppEvent::ClipboardRead(Ok(
        crate::image_input::ClipboardContent::Text(" hidden clipboard".into()),
    )));
    assert_eq!(app.input.image_reads, 0);
    assert_eq!(app.input.text, "kept");
    app.viewer.fullscreen = false;
    app.reduce(AppEvent::Paste(" visible".into()));
    assert_eq!(app.input.text, "kept visible");
}

#[test]
fn late_image_results_do_not_attach_to_a_hidden_draft() {
    let (_dir, mut app) = setup();
    app.viewer.open = true;
    app.viewer.fullscreen = true;
    let image = aether_core::attachment::ImageAttachment {
        media_type: "image/png".into(),
        data: "iVBORw0KGgo=".into(),
        name: Some("paste.png".into()),
    };
    for event in [
        AppEvent::ClipboardRead(Ok(crate::image_input::ClipboardContent::Image(
            image.clone(),
        ))),
        AppEvent::ImageRead(Ok(image.clone())),
    ] {
        app.input.image_reads = 1;
        app.reduce(event);
        assert_eq!(app.input.image_reads, 0);
        assert!(app.input.images.is_empty());
        assert!(app.input.text.is_empty());
    }
    assert!(app.reduce(AppEvent::TerminalImagePaste).is_empty());
    assert_eq!(app.input.image_reads, 0);
    app.input.image_reads = 1;
    app.reduce(AppEvent::ImageRead(Err("read failed".into())));
    assert!(app.live_flash().unwrap().contains("read failed"));
    app.viewer.fullscreen = false;
    app.input.image_reads = 1;
    app.reduce(AppEvent::ClipboardRead(Ok(
        crate::image_input::ClipboardContent::Image(image),
    )));
    assert_eq!(app.input.images.len(), 1);
    assert_eq!(app.input.image_reads, 0);
}

#[test]
fn binary_preview_error_names_the_external_open_shortcut() {
    let (dir, mut app) = setup();
    std::fs::write(dir.path().join("notes.md"), b"binary\0bytes").unwrap();
    let effects = app.open_reference(references::Reference {
        target: "notes.md".into(),
        label: "notes".into(),
    });
    load(&mut app, effects);
    let Some(Ok(Preview::Failed { error, .. })) = &app.viewer.preview else {
        panic!("binary preview error")
    };
    assert!(error.contains("Alt+E"));
    assert!(!error.contains("use o"));
}

#[test]
fn viewer_scroll_is_bounded_and_chat_escape_keeps_the_pane() {
    let (_dir, mut app) = setup();
    key(&mut app, KeyCode::Char('o'), KeyModifiers::ALT);
    key(&mut app, KeyCode::Down, KeyModifiers::NONE);
    assert_eq!(app.viewer.scroll, 0);
    assert_eq!(app.viewer.cursor, 0);
    let effects = key(&mut app, KeyCode::Enter, KeyModifiers::NONE);
    load(&mut app, effects);
    for _ in 0..10 {
        key(&mut app, KeyCode::PageDown, KeyModifiers::NONE);
    }
    assert_eq!(app.viewer.scroll, 0);
    key(&mut app, KeyCode::Tab, KeyModifiers::NONE);
    key(&mut app, KeyCode::Esc, KeyModifiers::NONE);
    assert!(app.viewer.open);
}
