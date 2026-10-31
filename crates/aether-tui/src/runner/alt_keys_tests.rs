#![allow(clippy::expect_used)]

/// #351: every documented Alt shortcut, as legacy, kitty-protocol, split and
/// other terminals send it, through the real input loop; and keys typed after
/// Alt+], Alt+Shift+P, Alt+_ or a late terminal reply are never swallowed.
#[test]
fn alt_shortcuts_fire_through_the_running_input_loop_in_every_encoding() {
    let script =
        std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../scripts/test_tui_alt_keys.py");
    assert!(script.is_file(), "{}", script.display());
    let status = std::process::Command::new("python3")
        .arg(script)
        .arg(std::env::current_exe().expect("test executable"))
        .status()
        .expect("PTY controller");
    assert!(status.success(), "Alt key controller: {status}");
}
