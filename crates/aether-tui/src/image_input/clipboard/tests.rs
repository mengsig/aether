use super::*;

#[cfg(unix)]
mod bounds;
#[cfg(unix)]
mod macos;

#[cfg(unix)]
pub(crate) fn fake_helper(script: &str) -> (tempfile::TempDir, String) {
    let dir = tempfile::tempdir().expect("helper directory");
    let path = dir.path().join("clipboard-helper");
    crate::voice::test_script::write(&path, &format!("#!/bin/sh\nset -eu\n{script}\n"));
    assert!(path.is_file());
    assert!(path.is_absolute());
    (dir, path.to_str().expect("helper path").to_owned())
}

#[cfg(unix)]
pub(crate) fn fake_wayland(listing: &str, data: &[u8]) -> (tempfile::TempDir, String) {
    let (dir, program) = fake_helper(
        r#"case "$*" in
        --list-types) cat "${0%/*}/types" ;;
        '--type image/png'|--no-newline\ --type\ *)
            cat "${0%/*}/data" ;;
        *) exit 64 ;;
        esac"#,
    );
    std::fs::write(dir.path().join("types"), listing).expect("offered types");
    std::fs::write(dir.path().join("data"), data).expect("clipboard bytes");
    assert!(dir.path().join("types").is_file());
    assert!(dir.path().join("data").is_file());
    (dir, program)
}

#[cfg(unix)]
#[tokio::test]
async fn image_only_paste_ignores_offered_plain_text() {
    let png = b"\x89PNG\r\n\x1a\npixels";
    let (_dir, program) = fake_wayland("text/plain\nimage/png\n", png);
    let bytes = clipboard_bytes(ClipboardTool::WlPaste, &program)
        .await
        .expect("forced clipboard image");
    assert_eq!(bytes, png);
    assert_eq!(
        image_attachment(&bytes, None).expect("image").media_type,
        "image/png"
    );
}

#[cfg(unix)]
#[tokio::test]
async fn smart_paste_prefers_an_offered_image_and_preserves_text_bytes() {
    let png = b"\x89PNG\r\n\x1a\npixels";
    // Screenshot tools, then browsers and chat apps that put text beside the image.
    for listing in [
        "image/png\n",
        "text/html\nimage/png\n",
        "text/plain\nimage/png\n",
        "text/html\ntext/_moz_htmlinfo\nUTF8_STRING\ntext/plain;charset=utf-8\nimage/png\n",
    ] {
        let (_dir, program) = fake_wayland(listing, png);
        let content = paste_with(ClipboardTool::WlPaste, &program, &program).await;
        let ClipboardContent::Image(image) = content.expect("image paste") else {
            panic!("no plain text was offered");
        };
        assert_eq!(image.media_type, "image/png");
        assert_eq!(image.size_bytes(), png.len() as u64);
    }

    let text = "  leading\r\nλ and trailing\n\n";
    for listing in [
        "text/plain;charset=utf-8\ntext/plain\n",
        "text/html\ntext/plain\n",
        "text/plain;charset=iso-8859-1\n",
        "UTF8_STRING\n",
        "TEXT\n",
        "STRING\n",
    ] {
        let (_dir, program) = fake_wayland(listing, text.as_bytes());
        let content = paste_with(ClipboardTool::WlPaste, &program, &program).await;
        assert!(matches!(content, Ok(ClipboardContent::Text(got)) if got == text));
    }
}

#[cfg(unix)]
#[tokio::test]
async fn smart_paste_reports_empty_missing_broken_and_invalid_contents() {
    for (listing, data) in [("", &b""[..]), ("text/plain\n", &b""[..])] {
        let (_dir, program) = fake_wayland(listing, data);
        let error = paste_with(ClipboardTool::WlPaste, &program, &program)
            .await
            .expect_err("empty clipboard");
        assert_eq!(error, NO_CONTENT);
        assert!(!error.contains('\n'));
    }
    let missing = paste_with(ClipboardTool::WlPaste, "/nonexistent/aether-clipboard", "")
        .await
        .expect_err("missing helper");
    assert!(missing.contains("install wl-clipboard"));
    assert!(missing.contains("/image <path>"));
    let (_dir, program) = fake_wayland("image/png\n", b"broken image");
    let error = paste_with(ClipboardTool::WlPaste, &program, &program)
        .await
        .expect_err("bad image must not fall back to text");
    assert!(error.contains("not a supported type"), "{error}");
    let (_dir, program) = fake_wayland("text/plain\n", b"\xff");
    let error = paste_with(ClipboardTool::WlPaste, &program, &program)
        .await
        .expect_err("invalid text");
    assert!(error.contains("not UTF-8"), "{error}");
}

#[cfg(unix)]
#[tokio::test]
async fn smart_paste_uses_xclip_targets_and_its_default_text_output() {
    let (_dir, program) = fake_helper(
        r#"case "$*" in
        '-selection clipboard -t TARGETS -o') printf 'TARGETS\ntext/html\nUTF8_STRING\n' ;;
        '-selection clipboard -o') printf '  text\n\n' ;;
        *) exit 64 ;;
        esac"#,
    );
    let content = paste_with(ClipboardTool::Xclip, &program, &program).await;
    assert!(matches!(content, Ok(ClipboardContent::Text(text)) if text == "  text\n\n"));
    let (_dir, program) = fake_helper(
        r#"case "$*" in
        '-selection clipboard -t TARGETS -o') printf 'UTF8_STRING\nimage/png\ntext/html\n' ;;
        '-selection clipboard -o -t image/png') printf '\211PNG\r\n\032\npixels' ;;
        *) exit 64 ;;
        esac"#,
    );
    let content = paste_with(ClipboardTool::Xclip, &program, &program).await;
    assert!(
        matches!(content, Ok(ClipboardContent::Image(image)) if image.media_type == "image/png")
    );
}

#[cfg(unix)]
#[tokio::test]
async fn smart_paste_probes_macos_before_using_png_or_pbpaste() {
    let (_dir, program) = fake_helper(
        r#"case "$*" in
        '-e clipboard info') printf '{«class PNGf», 14}' ;;
        '-e get the clipboard as «class PNGf»') printf '«data PNGf89504E470D0A1A0A706978656C73»' ;;
        *) exit 64 ;;
        esac"#,
    );
    let content = paste_with(ClipboardTool::Osascript, &program, "/nonexistent/pbpaste").await;
    assert!(matches!(content, Ok(ClipboardContent::Image(image)) if image.size_bytes() == 14));
    let (_dir, program) = fake_helper(
        r#"case "$*" in
        '-e clipboard info') printf '{«class utf8», 7}, {«class PNGf», 14}' ;;
        '-e get the clipboard as «class PNGf»') printf '«data PNGf89504E470D0A1A0A706978656C73»' ;;
        '') printf ' text\n\n' ;;
        *) exit 64 ;;
        esac"#,
    );
    let content = paste_with(ClipboardTool::Osascript, &program, &program).await;
    assert!(matches!(content, Ok(ClipboardContent::Image(image)) if image.size_bytes() == 14));
    let (_dir, program) = fake_helper(
        r#"case "$*" in
        '-e clipboard info') printf '{«class utf8», 7}' ;;
        '') printf ' text\n\n' ;;
        *) exit 64 ;;
        esac"#,
    );
    let content = paste_with(ClipboardTool::Osascript, &program, &program).await;
    assert!(matches!(content, Ok(ClipboardContent::Text(text)) if text == " text\n\n"));
}

#[cfg(unix)]
#[tokio::test]
async fn an_image_the_helper_refuses_falls_back_to_the_offered_text() {
    let (_dir, program) = fake_helper(
        r#"case "$*" in
        --list-types) printf 'image/png\ntext/plain\n' ;;
        '--type image/png') exit 1 ;;
        '--no-newline --type text/plain') printf 'words' ;;
        *) exit 64 ;;
        esac"#,
    );
    let content = paste_with(ClipboardTool::WlPaste, &program, &program).await;
    assert!(matches!(content, Ok(ClipboardContent::Text(text)) if text == "words"));
}

/// A file copied in a file manager, as `wl-paste` and `xclip` serve it.
#[cfg(unix)]
fn fake_file_manager(tool: ClipboardTool, uri_list: &str) -> (tempfile::TempDir, String) {
    let script = match tool {
        ClipboardTool::WlPaste => {
            r#"case "$*" in
            --list-types) printf 'x-special/gnome-copied-files\ntext/uri-list\n' ;;
            '--no-newline --type text/uri-list') cat "${0%/*}/uris" ;;
            *) exit 64 ;;
            esac"#
        }
        _ => {
            r#"case "$*" in
            '-selection clipboard -t TARGETS -o') printf 'TARGETS\ntext/uri-list\n' ;;
            '-selection clipboard -o -t text/uri-list') cat "${0%/*}/uris" ;;
            *) exit 64 ;;
            esac"#
        }
    };
    let (dir, program) = fake_helper(script);
    std::fs::write(dir.path().join("uris"), uri_list).expect("copied files");
    assert!(dir.path().join("uris").is_file());
    (dir, program)
}

#[cfg(unix)]
#[tokio::test]
async fn one_copied_image_file_is_attached_and_other_file_lists_are_text() {
    let dir = tempfile::tempdir().expect("copied files");
    let image = dir.path().join("shot one.png");
    std::fs::write(&image, b"\x89PNG\r\n\x1a\npixels").expect("image");
    let url = format!("file://{}", image.display()).replace(' ', "%20");
    for tool in [ClipboardTool::WlPaste, ClipboardTool::Xclip] {
        let (_helper, program) = fake_file_manager(tool, &format!("# copy\r\n{url}\r\n"));
        let content = paste_with(tool, &program, &program).await;
        let Ok(ClipboardContent::ImagePath { text, image }) = content else {
            panic!("{tool:?}: one image file was copied: {content:?}");
        };
        assert_eq!(text, url);
        assert_eq!(
            image.expect("readable image").name.as_deref(),
            Some("shot one.png")
        );

        let two = format!("{url}\r\nfile:///etc/hostname\r\n");
        let (_helper, program) = fake_file_manager(tool, &two);
        let content = paste_with(tool, &program, &program).await;
        let expected = format!("{url}\nfile:///etc/hostname");
        assert!(matches!(content, Ok(ClipboardContent::Text(text)) if text == expected));

        let (_helper, program) = fake_file_manager(tool, "# nothing\r\n");
        let error = paste_with(tool, &program, &program)
            .await
            .expect_err("no URLs");
        assert_eq!(error, NO_CONTENT);
    }
}

#[test]
fn the_clipboard_tool_follows_the_session() {
    assert_eq!(
        clipboard_tool(false, false, true, true),
        Ok(ClipboardTool::WlPaste)
    );
    assert_eq!(
        clipboard_tool(false, false, false, true),
        Ok(ClipboardTool::Xclip)
    );
    assert_eq!(
        clipboard_tool(true, false, false, false),
        Ok(ClipboardTool::Osascript)
    );
    for (macos, ssh, wayland, display) in [
        (false, true, true, true),
        (true, true, false, false),
        (false, false, false, false),
    ] {
        let hint = clipboard_tool(macos, ssh, wayland, display).expect_err("no tool");
        assert!(hint.contains("/image <path>"), "{hint}");
        assert!(!hint.contains('\n'), "one line: {hint}");
    }
    for tool in [ClipboardTool::WlPaste, ClipboardTool::Xclip] {
        let hint = tool.missing_hint();
        assert!(hint.starts_with("install "), "{hint}");
        assert!(hint.contains("/image <path>"), "{hint}");
    }
}

#[test]
fn the_offered_type_is_the_first_one_supported() {
    let listing = "TARGETS\ntext/plain\nimage/webp\nimage/png\n";
    assert_eq!(offered_image_type(listing), Some("image/png"));
    assert_eq!(offered_image_type("image/jpeg\r\n"), Some("image/jpeg"));
    assert_eq!(offered_image_type("text/plain\nimage/bmp\n"), None);
    assert_eq!(offered_image_type(""), None);
}

#[test]
fn the_macos_hex_dump_decodes_to_its_bytes() {
    assert_eq!(
        hex_payload("«data PNGf89504E470D0A1A0A»\n", "PNGf"),
        Some(b"\x89PNG\r\n\x1a\n".to_vec())
    );
    for broken in [
        "",
        "«data PNGf»",
        "«data PNGf895»",
        "«data PNGfzz»",
        "«data PNGf89",
    ] {
        assert_eq!(hex_payload(broken, "PNGf"), None, "{broken:?}");
    }
}

#[tokio::test]
async fn a_helper_gives_its_bytes_a_refusal_or_its_absence() {
    let printed = run_helper("sh", &["-c", "printf pixels"], 64).await;
    assert_eq!(printed, Ok(b"pixels".to_vec()));
    let refused = run_helper("sh", &["-c", "printf nothing; exit 1"], 64).await;
    assert_eq!(refused, Err(HelperError::Refused));
    let missing = run_helper("aether-no-such-clipboard-helper", &[], 64).await;
    assert_eq!(missing, Err(HelperError::Missing));
    // Output past the limit is cut there, and the helper is not waited on.
    let cut = run_helper("sh", &["-c", "printf 0123456789; exec sleep 30"], 4).await;
    assert_eq!(cut, Ok(b"0123".to_vec()));
}
