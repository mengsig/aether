use super::*;

#[tokio::test]
async fn smart_paste_reads_the_macos_image_class_that_was_offered() {
    for (class, hex, media_type) in [
        ("JPEG", "FFD8FF00", "image/jpeg"),
        ("GIFf", "474946383961", "image/gif"),
        ("WebP", "524946460000000057454250", "image/webp"),
    ] {
        let script = format!(
            r#"case "$*" in
            '-e clipboard info') printf '{{«class {class}», 12}}' ;;
            '-e get the clipboard as «class {class}»') printf '«data {class}{hex}»' ;;
            *) exit 64 ;;
            esac"#
        );
        let (_dir, program) = fake_helper(&script);
        let content = paste_with(ClipboardTool::Osascript, &program, "/nonexistent/pbpaste")
            .await
            .expect("macOS image");
        let ClipboardContent::Image(image) = content else {
            panic!("an image was offered");
        };
        assert_eq!(image.media_type, media_type);
        assert_eq!(image.size_bytes(), (hex.len() / 2) as u64);
    }
}

#[tokio::test]
async fn smart_paste_falls_back_to_text_when_macos_image_coercion_fails() {
    let (_dir, program) = fake_helper(
        r#"case "$*" in
        '-e clipboard info') printf '{«class JPEG», 14}' ;;
        '-e get the clipboard as «class JPEG»') exit 1 ;;
        '') printf ' fallback text\n' ;;
        *) exit 64 ;;
        esac"#,
    );
    let content = paste_with(ClipboardTool::Osascript, &program, &program)
        .await
        .expect("text after failed coercion");
    assert!(matches!(content, ClipboardContent::Text(text) if text == " fallback text\n"));
    let (_text_dir, refused_text) = fake_helper("exit 1");
    let error = paste_with(ClipboardTool::Osascript, &program, &refused_text)
        .await
        .expect_err("failed text fallback");
    assert_eq!(error, NO_CONTENT);
}

#[tokio::test]
async fn smart_paste_reports_truncated_macos_hex_as_too_large() {
    let (dir, program) = fake_helper(
        r#"case "$*" in
        '-e clipboard info') printf '{«class PNGf», 9000000}' ;;
        '-e get the clipboard as «class PNGf»') cat "${0%/*}/hex" ;;
        *) exit 64 ;;
        esac"#,
    );
    let mut printed = String::from("«data PNGf89504E470D0A1A0A");
    printed.push_str(&"00".repeat(MAX_IMAGE_BYTES + 64));
    printed.push('»');
    std::fs::write(dir.path().join("hex"), printed).expect("oversized hex");
    let error = paste_with(ClipboardTool::Osascript, &program, "/nonexistent/pbpaste")
        .await
        .expect_err("truncated image");
    assert!(error.contains("too large"), "{error}");
    assert_ne!(error, NO_CONTENT);
}

#[tokio::test]
async fn smart_paste_prefers_each_macos_plain_text_class() {
    for class in ["utf8", "utxt", "TEXT"] {
        let script = format!(
            r#"case "$*" in
            '-e clipboard info') printf '{{«class {class}», 4}}, {{«class PNGf», 14}}' ;;
            '') printf 'text' ;;
            *) exit 64 ;;
            esac"#
        );
        let (_dir, program) = fake_helper(&script);
        let content = paste_with(ClipboardTool::Osascript, &program, &program)
            .await
            .expect("offered text");
        assert!(matches!(content, ClipboardContent::Text(text) if text == "text"));
        assert_eq!(
            offered_text_type(ClipboardTool::Osascript, &format!("«class {class}»")),
            Some(class)
        );
    }
}
