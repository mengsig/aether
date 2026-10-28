use super::{run_isolated, stderr, stdout};
use std::time::{Duration, Instant};

#[test]
fn display_flags_are_only_shown_for_the_tui() {
    let root = tempfile::tempdir().unwrap();
    for (args, visible) in [
        (vec!["--help"], true),
        (vec!["tui", "--help"], true),
        (vec!["run", "--help"], false),
        (vec!["schedule", "create", "--help"], false),
        (vec!["memory", "--help"], false),
        (vec!["memory", "list", "--help"], false),
        (vec!["daemon", "--help"], false),
    ] {
        let output = run_isolated(root.path(), &args);
        assert!(output.status.success(), "{}", stderr(&output));
        let help = stdout(&output);
        for flag in [
            "--theme",
            "--plain",
            "--ascii",
            "--reduced-motion",
            "--view-mode",
        ] {
            assert_eq!(help.contains(flag), visible, "{args:?}: {help}");
        }
    }
}

#[test]
fn daemon_actions_explain_the_commands_without_starting_anything() {
    let root = tempfile::tempdir().unwrap();
    for action in ["start", "stop", "restart", "status"] {
        let output = run_isolated(root.path(), &["daemon", action]);
        assert_eq!(output.status.code(), Some(2));
        let message = stderr(&output);
        assert!(
            message.contains(&format!("`aether daemon {action}` is not a command")),
            "{message}"
        );
        for hint in ["background", "--foreground", "aether stop", "aether status"] {
            assert!(message.contains(hint), "{message}");
        }
        assert!(!root.path().join("a.sock").exists());
        assert!(!root.path().join("data").exists());
    }
}

#[test]
fn stopping_an_absent_daemon_is_idempotent_in_text_and_json() {
    let root = tempfile::tempdir().unwrap();
    for _ in 0..2 {
        let output = run_isolated(root.path(), &["stop"]);
        assert!(output.status.success(), "{}", stderr(&output));
        assert_eq!(stdout(&output).trim(), "aetherd not running");
    }
    let output = run_isolated(root.path(), &["stop", "--json"]);
    assert!(output.status.success(), "{}", stderr(&output));
    let value: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(
        value,
        serde_json::json!({"stopping": false, "running": false})
    );
    let socket = root.path().join("a.sock");
    drop(std::os::unix::net::UnixListener::bind(&socket).unwrap());
    let stale = run_isolated(root.path(), &["stop"]);
    assert!(stale.status.success(), "{}", stderr(&stale));
    assert_eq!(stdout(&stale).trim(), "aetherd not running");
    assert!(socket.exists());
}

#[test]
fn login_lists_providers_and_cli_handoffs_require_a_terminal() {
    let root = tempfile::tempdir().unwrap();
    let output = run_isolated(root.path(), &["login"]);
    assert!(!output.status.success());
    let message = stderr(&output);
    assert!(
        message.contains("name a provider: aether login <provider>; providers:"),
        "{message}"
    );
    for provider in ["openai", "claude-code", "codex"] {
        assert!(message.contains(provider), "{message}");
    }
    for provider in ["claude-code", "codex"] {
        let began = Instant::now();
        let output = run_isolated(root.path(), &["login", provider]);
        assert!(began.elapsed() < Duration::from_secs(5));
        assert!(!output.status.success());
        let message = stderr(&output);
        assert!(
            message.contains("hands off to")
                && message.contains("needs a terminal; run it in a terminal"),
            "{message}"
        );
        assert!(output.stdout.is_empty());
        assert!(!output.stderr.contains(&0x1b));
    }
}

#[test]
fn malformed_config_fails_auto_start_promptly_and_names_the_key() {
    let root = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(root.path().join("config")).unwrap();
    std::fs::write(
        root.path().join("config/config.toml"),
        "bad_startup_key = true\n",
    )
    .unwrap();
    let began = Instant::now();
    let output = run_isolated(root.path(), &["models"]);
    assert!(began.elapsed() < Duration::from_secs(5));
    assert!(!output.status.success());
    let message = stderr(&output);
    assert!(message.contains("bad_startup_key"), "{message}");
    assert!(message.contains("before it listened"), "{message}");
}

#[test]
fn new_help_names_only_the_defaults_it_can_override() {
    let root = tempfile::tempdir().unwrap();
    let output = run_isolated(root.path(), &["new", "--help"]);
    assert!(output.status.success(), "{}", stderr(&output));
    let help = stdout(&output)
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ");
    assert!(
        help.contains("taskless agent eligible to own a task"),
        "{help}"
    );
    assert!(
        help.contains("default workspace and approval policy"),
        "{help}"
    );
    assert!(
        help.contains("default model unless --model names one"),
        "{help}"
    );
    assert!(help.contains("Starts the daemon if needed"), "{help}");
}

#[test]
fn a_held_socket_lock_preserves_stop_behavior_and_has_a_distinct_start_exit() {
    use std::os::unix::fs::OpenOptionsExt as _;

    let root = tempfile::tempdir().unwrap();
    let lock = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(root.path().join("a.sock.lock"))
        .unwrap();
    lock.try_lock().unwrap();
    let stopped = run_isolated(root.path(), &["stop"]);
    assert!(stopped.status.success(), "{}", stderr(&stopped));
    assert_eq!(stdout(&stopped).trim(), "aetherd already stopping");
    let started = run_isolated(root.path(), &["daemon", "--foreground"]);
    assert_eq!(
        started.status.code(),
        Some(i32::from(aether_client::DAEMON_LOCK_BUSY_EXIT)),
        "{}",
        stderr(&started)
    );
    assert!(stderr(&started).contains("another aether daemon"));
    assert!(!root.path().join("a.sock").exists());
}
