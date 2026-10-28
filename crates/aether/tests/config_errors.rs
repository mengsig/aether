#![allow(clippy::expect_used, clippy::unwrap_used)]

use std::{path::Path, process::Command};

fn isolated_command(root: &Path, args: &[&str]) -> Command {
    let mut command = Command::new(env!("CARGO_BIN_EXE_aether"));
    command
        .args(args)
        .arg("--socket")
        .arg(root.join("a.sock"))
        .env("HOME", root.join("home"))
        .env("XDG_CONFIG_HOME", root.join("xdg-config"))
        .env("XDG_DATA_HOME", root.join("xdg-data"))
        .env("XDG_CACHE_HOME", root.join("xdg-cache"))
        .env("XDG_RUNTIME_DIR", root.join("runtime"))
        .env("AETHER_CONFIG_DIR", root.join("config"))
        .env("AETHER_DATA_DIR", root.join("data"))
        .env_remove("AETHER_SOCKET")
        .env_remove("AETHER_TCP_ADDR")
        .env_remove("AETHER_MCP_TOKEN");
    command
}

#[test]
fn doctor_and_foreground_daemon_report_all_unknown_settings() {
    for args in [vec!["doctor"], vec!["daemon", "--foreground"]] {
        let root = tempfile::tempdir().unwrap();
        std::fs::create_dir(root.path().join("config")).unwrap();
        std::fs::write(
            root.path().join("config/config.toml"),
            "colour = 1\n[nonsense]\nvalue = true\n",
        )
        .unwrap();
        let output = isolated_command(root.path(), &args).output().unwrap();
        let text = format!(
            "{}{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
        assert!(!output.status.success(), "{args:?}: {text}");
        assert!(text.contains("`colour`, `nonsense`"), "{args:?}: {text}");
        assert!(!text.contains("..") && !text.contains("; ."), "{text}");
    }
}
