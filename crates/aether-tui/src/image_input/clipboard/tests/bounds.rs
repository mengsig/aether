use super::*;

#[tokio::test]
async fn smart_paste_times_out_a_hung_helper() {
    let (_dir, program) = fake_helper("exec sleep 30");
    let started = std::time::Instant::now();
    let error = paste_with(ClipboardTool::WlPaste, &program, &program)
        .await
        .expect_err("hung helper");
    assert!(error.contains("timed out"), "{error}");
    assert!(started.elapsed() < Duration::from_secs(10));
}

#[tokio::test]
async fn smart_paste_rejects_oversized_text_and_images() {
    for (listing, prefix, size, expected) in [
        (
            "text/plain\n",
            b"text".as_slice(),
            MAX_TEXT_BYTES + 1,
            "text exceeds 16 MiB",
        ),
        (
            "image/png\n",
            b"\x89PNG\r\n\x1a\n".as_slice(),
            MAX_IMAGE_BYTES + 1,
            "too large",
        ),
    ] {
        let mut bytes = prefix.to_vec();
        bytes.resize(size, b'x');
        let (_dir, program) = fake_wayland(listing, &bytes);
        let error = paste_with(ClipboardTool::WlPaste, &program, &program)
            .await
            .expect_err("oversized clipboard");
        assert!(error.contains(expected), "{error}");
        assert_ne!(error, NO_CONTENT);
    }
}

#[tokio::test]
async fn real_clipboard_helpers_are_unreachable_in_unit_tests() {
    for program in ["wl-paste", "xclip", "osascript", "pbpaste"] {
        let error = run_helper(program, &[], 64).await.expect_err("real helper");
        assert!(matches!(error, HelperError::Broken(_)));
        assert_eq!(
            error,
            HelperError::Broken("real clipboard helpers are disabled in tests".into())
        );
    }
}
