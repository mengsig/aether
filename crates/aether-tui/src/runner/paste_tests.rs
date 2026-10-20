#![allow(clippy::expect_used)]

/// Ctrl+V and Alt+V as legacy and kitty-protocol terminals send them, and the
/// terminal's own paste, through the real input loop with a fake `wl-paste`.
#[test]
fn paste_keys_reach_the_clipboard_through_the_running_input_loop() {
    let script =
        std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../scripts/test_tui_paste.py");
    assert!(script.is_file(), "{}", script.display());
    let status = std::process::Command::new("python3")
        .arg(script)
        .arg(std::env::current_exe().expect("test executable"))
        .status()
        .expect("PTY controller");
    assert!(status.success(), "paste controller: {status}");
}
