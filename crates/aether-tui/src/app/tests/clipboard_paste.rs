use super::*;
use crate::image_input::clipboard::tests::fake_wayland;
use crate::image_input::clipboard::{ClipboardTool, clipboard_tool, paste_with};
use crate::image_input::{ClipboardContent, ImageSource};
use crate::testing::app_with_transcript;

const PNG: &[u8] = b"\x89PNG\r\n\x1a\npixels";

fn request_clipboard(app: &mut App) {
    let draft = app.input.text.clone();
    assert_eq!(
        app.reduce(ctrl_key(KeyCode::Char('v'))),
        vec![Effect::PasteClipboard]
    );
    assert_eq!(app.input.image_reads, 1);
    assert_eq!(app.input.text, draft);
}

#[tokio::test]
async fn ctrl_v_attaches_an_offered_image_without_changing_the_draft() {
    let mut app = app_with_transcript(&[]);
    app.input.set_text("explain this");
    request_clipboard(&mut app);
    assert!(app.reduce(key(KeyCode::Enter)).is_empty());
    assert_eq!(app.input.text, "explain this", "send waits for the read");
    assert_eq!(
        app.live_flash(),
        Some("a paste or image is still being read: send again once it finishes")
    );
    let (_dir, program) = fake_wayland("image/png\n", PNG);
    let content = paste_with(ClipboardTool::WlPaste, &program, &program).await;
    assert!(app.reduce(AppEvent::ClipboardRead(content)).is_empty());
    assert_eq!(app.input.image_reads, 0);
    assert_eq!(app.input.images.len(), 1);
    assert_eq!(app.input.images[0].media_type, "image/png");
    assert_eq!(app.input.text, "explain this");
}

#[tokio::test]
async fn ctrl_v_text_uses_the_same_normalisation_and_placeholders_as_a_paste() {
    for text in [
        "  λ\r\ntrailing\n\n".to_owned(),
        "long text\r\n".repeat(500),
    ] {
        let mut app = app_with_transcript(&[]);
        app.input.set_text("before after");
        app.input.cursor = "before ".len();
        let mut reference = app_with_transcript(&[]);
        reference.input.set_text(&app.input.text);
        reference.input.cursor = app.input.cursor;
        reference.reduce(crate::runner::paste_event(text.clone()));
        request_clipboard(&mut app);
        assert!(app.reduce(key(KeyCode::Enter)).is_empty());
        assert!(
            app.live_flash()
                .expect("pending paste")
                .contains("a paste or image")
        );
        let (_dir, program) =
            fake_wayland("image/png\ntext/plain;charset=utf-8\n", text.as_bytes());
        let content = paste_with(ClipboardTool::WlPaste, &program, &program).await;
        assert!(app.reduce(AppEvent::ClipboardRead(content)).is_empty());
        assert_eq!(app.input.text, reference.input.text);
        assert_eq!(app.input.cursor, reference.input.cursor);
        assert_eq!(app.input.pastes, reference.input.pastes);
        assert_eq!(app.input.pastes.len(), usize::from(text.len() > 1000));
        assert_eq!(app.input.image_reads, 0);
        assert!(app.input.images.is_empty());
        let sent = app.reduce(key(KeyCode::Enter));
        let expected = format!("before {}after", text.replace("\r\n", "\n"));
        assert!(
            sent.iter().any(|effect| matches!(effect,
                Effect::Request { command: Command::SendMessage { content, .. }, .. }
                    if content == &expected
            )),
            "{sent:?}"
        );
    }
}

#[tokio::test]
async fn ctrl_v_empty_or_missing_clipboard_flashes_and_preserves_the_draft() {
    let (_dir, program) = fake_wayland("text/plain\n", b"");
    for helper in [program.as_str(), "/nonexistent/aether-clipboard"] {
        let mut app = app_with_transcript(&[]);
        app.input.set_text("keep this draft");
        let cursor = app.input.cursor;
        request_clipboard(&mut app);
        let content = paste_with(ClipboardTool::WlPaste, helper, helper).await;
        assert!(app.reduce(AppEvent::ClipboardRead(content)).is_empty());
        let flash = app.live_flash().expect("one-line error");
        assert!(flash.starts_with("nothing pasted: "), "{flash}");
        assert!(!flash.contains('\n'));
        assert_eq!(app.input.text, "keep this draft");
        assert_eq!(app.input.cursor, cursor);
        assert_eq!(app.input.image_reads, 0);
        assert!(app.input.images.is_empty());
    }
}

#[tokio::test]
async fn an_empty_terminal_paste_tries_the_clipboard_image() {
    let mut app = app_with_transcript(&[]);
    app.input.set_text("keep this draft");
    let event = crate::runner::paste_event(String::new());
    assert!(matches!(event, AppEvent::TerminalImagePaste));
    let effects = app.reduce(event);
    assert_eq!(effects, vec![Effect::AttachImage(ImageSource::Clipboard)]);
    assert_eq!(app.input.image_reads, 1);
    let (_dir, program) = fake_wayland("image/png\n", PNG);
    let content = paste_with(ClipboardTool::WlPaste, &program, &program)
        .await
        .expect("clipboard image");
    let ClipboardContent::Image(image) = content else {
        panic!("an image was offered");
    };
    app.reduce(AppEvent::ImageRead(Ok(image)));
    assert_eq!(app.input.images.len(), 1);
    assert_eq!(app.input.image_reads, 0);
    assert_eq!(app.input.text, "keep this draft");
}

#[test]
fn a_nonempty_terminal_paste_stays_text_without_reading_the_clipboard() {
    for text in [
        "ordinary text",
        "  leading\ntrailing  ",
        "/image is text until sent",
        " \t\n",
    ] {
        let mut app = app_with_transcript(&[]);
        app.input.set_text("draft ");
        assert!(
            app.reduce(crate::runner::paste_event(text.into()))
                .is_empty()
        );
        assert_eq!(app.input.text, format!("draft {text}"));
        assert_eq!(app.input.image_reads, 0);
        assert!(app.input.images.is_empty());
    }
}

#[test]
fn clipboard_paste_over_ssh_explains_where_the_clipboard_is() {
    for image_only in [false, true] {
        let mut app = app_with_transcript(&[]);
        app.input.set_text("draft");
        let error =
            clipboard_tool(false, true, true, true).expect_err("SSH has no local clipboard");
        if image_only {
            app.reduce(crate::runner::paste_event(String::new()));
            app.reduce(AppEvent::ImageRead(Err(error)));
        } else {
            request_clipboard(&mut app);
            app.reduce(AppEvent::ClipboardRead(Err(error)));
        }
        let flash = app.live_flash().expect("SSH hint");
        assert!(flash.contains("other end of this SSH session"), "{flash}");
        assert!(flash.contains("/image <path>"), "{flash}");
        assert!(!flash.contains('\n'));
        assert_eq!(app.input.text, "draft");
        assert_eq!(app.input.image_reads, 0);
        assert!(app.input.images.is_empty());
    }
}

#[test]
fn clipboard_reads_do_not_start_when_a_view_find_or_board_owns_input() {
    for owner in ['f', 'g', 'r', 's'] {
        let mut app = app_with_transcript(&[]);
        app.input.set_text("draft");
        if owner == 's' {
            app.run_slash(SlashCommand::Settings);
        } else {
            app.reduce(ctrl_key(KeyCode::Char(owner)));
        }
        assert!(app.reduce(ctrl_key(KeyCode::Char('v'))).is_empty());
        assert!(
            app.reduce(crate::runner::paste_event(String::new()))
                .is_empty()
        );
        assert_eq!(app.input.image_reads, 0);
        assert_eq!(app.input.text, "draft");
        assert!(app.input.images.is_empty());
    }
}

#[tokio::test]
async fn ctrl_v_text_keeps_image_path_detection_and_its_error_fallback() {
    let dir = tempfile::tempdir().expect("image directory");
    let path = dir.path().join("shot.png");
    for data in [PNG, b"not an image".as_slice()] {
        std::fs::write(&path, data).expect("image bytes");
        let text = format!("'{}'\n", path.display());
        let mut app = app_with_transcript(&[]);
        request_clipboard(&mut app);
        let (_helper, program) = fake_wayland("text/plain\n", text.as_bytes());
        let content = paste_with(ClipboardTool::WlPaste, &program, &program).await;
        assert!(app.reduce(AppEvent::ClipboardRead(content)).is_empty());
        assert_eq!(app.input.image_reads, 0);
        if data == PNG {
            assert_eq!(app.input.images.len(), 1);
            assert!(app.input.text.is_empty());
        } else {
            assert!(app.input.images.is_empty());
            assert_eq!(app.input.text, text);
            assert!(
                app.live_flash()
                    .expect("fallback hint")
                    .contains("pasted as text, not attached")
            );
        }
    }
}
