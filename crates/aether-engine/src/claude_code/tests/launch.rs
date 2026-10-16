//! Native launch configuration and capability checks.

use super::*;
use crate::McpBridge;
use aether_core::model::Usage;
use serde_json::Value;

fn turn() -> EngineTurn {
    EngineTurn {
        images: Vec::new(),
        prompt: "hi".into(),
        system_prompt: "contract".into(),
        session: None,
        session_usage: Usage::default(),
        session_model_usage: None,
        new_session_id: "11111111-2222-4333-8444-555555555555".into(),
        session_recorder: None,
        model: Some("sonnet".into()),
        effort: Some(aether_core::model::Effort::High),
        cwd: PathBuf::from("/tmp"),
        extra_dirs: vec![],
        hidden_env: Vec::new(),
        tool_hidden_env: Vec::new(),
        temp_dir: None,
        build_env: Vec::new(),
        summary_only: false,
        lean_host_context: false,
        network_access: true,
        bypass_permissions: false,
        mcp: Some(McpBridge {
            command: PathBuf::from("/usr/bin/aether"),
            args: vec!["mcp-bridge".into()],
        }),
    }
}

#[tokio::test]
async fn ownership_is_recorded_at_fresh_spawn_even_without_init_output() {
    let dir = fake_claude("cat >/dev/null");
    let engine = ClaudeCodeEngine::new(dir.path().join("claude").to_string_lossy(), "2.1.265");
    for session in [None, Some("user-session".into())] {
        let recorder = std::sync::Arc::new(crate::ownership_tests::Recorder::default());
        let mut t = turn();
        t.session = session.clone();
        t.cwd = dir.path().into();
        t.session_recorder = Some(recorder.clone());
        let cancel = CancellationToken::new();
        let mut run = engine.run(t.clone(), cancel.clone()).await.unwrap();
        {
            let records = recorder.records.lock().unwrap();
            if session.is_none() {
                assert_eq!(records.len(), 1);
                assert_eq!(records[0].engine, EngineKind::CLAUDE_CODE);
                assert_eq!(records[0].id, t.new_session_id);
                assert_eq!(records[0].log_paths, session_log_paths(&t));
            } else {
                assert!(records.is_empty());
            }
        }
        cancel.cancel();
        while run.events.recv().await.is_some() {}
    }
}

#[tokio::test]
async fn failed_spawn_never_establishes_ownership() {
    let dir = tempfile::tempdir().unwrap();
    let engine = ClaudeCodeEngine::new(dir.path().join("absent").to_string_lossy(), "2.1.265");
    let recorder = std::sync::Arc::new(crate::ownership_tests::Recorder::default());
    let mut t = turn();
    t.cwd = dir.path().into();
    t.session_recorder = Some(recorder.clone());
    assert!(engine.run(t, CancellationToken::new()).await.is_err());
    assert!(recorder.records.lock().unwrap().is_empty());
}

#[tokio::test]
async fn ownership_recording_failure_stops_claude_launch() {
    let dir = fake_claude("cat >/dev/null");
    let engine = ClaudeCodeEngine::new(dir.path().join("claude").to_string_lossy(), "2.1.265");
    let mut t = turn();
    t.cwd = dir.path().into();
    t.session_recorder = Some(std::sync::Arc::new(crate::ownership_tests::Recorder {
        fail: true,
        ..Default::default()
    }));
    let cancel = CancellationToken::new();
    let error = engine.run(t, cancel.clone()).await.unwrap_err();
    assert!(matches!(error, EngineError::SessionOwnership(_)));
    assert!(error.to_string().contains("storage unavailable"));
    assert!(cancel.is_cancelled());
}

#[tokio::test]
async fn fresh_and_resumed_claude_get_the_agent_temp_dir() {
    let dir = fake_claude(
        r#"printf '%s|%s|%s' "$TMPDIR" "$TMP" "$TEMP" > "$TMPDIR/seen"; cat >/dev/null"#,
    );
    let engine = ClaudeCodeEngine::new(dir.path().join("claude").to_string_lossy(), "2.1.265");
    let leaf = dir.path().join("0123456789AB");
    std::fs::create_dir(&leaf).unwrap();
    for session in [None, Some("existing".to_owned())] {
        let mut t = turn();
        t.session = session.clone();
        t.cwd = dir.path().into();
        t.temp_dir = Some(leaf.clone());
        let cancel = CancellationToken::new();
        let mut run = engine.run(t, cancel.clone()).await.unwrap();
        let seen = leaf.join("seen");
        for _ in 0..200 {
            if std::fs::read_to_string(&seen).is_ok_and(|s| !s.is_empty()) {
                break;
            }
            tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        }
        cancel.cancel();
        while run.events.recv().await.is_some() {}
        let seen = std::fs::read_to_string(&seen).unwrap();
        assert_eq!(seen, format!("{0}|{0}|{0}", leaf.display()), "{session:?}");
        std::fs::remove_file(leaf.join("seen")).unwrap();
    }
}

#[tokio::test]
async fn claude_builds_into_the_agents_build_cache() {
    let dir = fake_claude(
        r#"printf '%s' "$CARGO_TARGET_DIR" > "$CARGO_TARGET_DIR/seen"; cat >/dev/null"#,
    );
    let engine = ClaudeCodeEngine::new(dir.path().join("claude").to_string_lossy(), "2.1.265");
    let cache = dir.path().join("cache/cargo-target");
    std::fs::create_dir_all(&cache).unwrap();
    let mut t = turn();
    t.cwd = dir.path().into();
    t.hidden_env = vec!["CARGO_TARGET_DIR".into()];
    t.build_env = vec![("CARGO_TARGET_DIR".into(), cache.clone())];
    let cancel = CancellationToken::new();
    let mut run = engine.run(t, cancel.clone()).await.unwrap();
    let seen = cache.join("seen");
    for _ in 0..200 {
        if std::fs::read_to_string(&seen).is_ok_and(|s| !s.is_empty()) {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(10)).await;
    }
    cancel.cancel();
    while run.events.recv().await.is_some() {}
    assert_eq!(
        std::fs::read_to_string(&seen).unwrap(),
        cache.display().to_string(),
        "exported after hidden names are stripped"
    );
}

#[test]
fn a_sandboxed_claude_may_write_only_its_own_temp_dir_beyond_the_defaults() {
    let engine = ClaudeCodeEngine::new("claude", "2.1.265");
    let settings = |t: &EngineTurn| -> Value {
        let args = engine.args(t);
        serde_json::from_str(&args[args.iter().position(|a| a == "--settings").unwrap() + 1])
            .unwrap()
    };
    let mut t = turn();
    t.temp_dir = Some(PathBuf::from("/var/t/0123456789AB"));
    t.network_access = false;
    assert_eq!(
        settings(&t)["sandbox"]["filesystem"],
        json!({"allowWrite": ["/var/t/0123456789AB"]})
    );
    t.build_env = vec![(
        "CARGO_TARGET_DIR".into(),
        "/var/cache/r/run/cargo-target".into(),
    )];
    assert_eq!(
        settings(&t)["sandbox"]["filesystem"],
        json!({"allowWrite": ["/var/t/0123456789AB", "/var/cache/r/run/cargo-target"]}),
        "and its build cache"
    );
    t.build_env.clear();
    t.temp_dir = None;
    assert!(settings(&t)["sandbox"].get("filesystem").is_none());
    t.temp_dir = Some(PathBuf::from("/var/t/0123456789AB"));
    t.network_access = true;
    assert!(
        settings(&t).get("sandbox").is_none(),
        "an online turn is not sandboxed"
    );
    t.build_env = vec![(
        "CARGO_TARGET_DIR".into(),
        "/var/cache/r/run/cargo-target".into(),
    )];
    let online = settings(&t);
    assert_eq!(
        online["sandbox"],
        json!({"filesystem": {"allowWrite": ["/var/cache/r/run/cargo-target"]}}),
        "an inherited sandbox may still write the build cache, and is not enabled by Aether"
    );
    t.summary_only = true;
    assert!(
        settings(&t).get("sandbox").is_none(),
        "a summary runs no builds"
    );
}

#[test]
fn aether_tools_are_loaded_before_fresh_and_resumed_prompts() {
    let engine = ClaudeCodeEngine::new("claude", "2.1.265");
    for session in [None, Some("existing".to_owned())] {
        let mut turn = turn();
        turn.session = session;
        let args = engine.args(&turn);
        let at = args.iter().position(|arg| arg == "--mcp-config").unwrap();
        let config: Value = serde_json::from_str(&args[at + 1]).unwrap();
        assert_eq!(config["mcpServers"]["aether"]["alwaysLoad"], true);
        // Other native MCP servers retain their normal discovery settings.
        assert!(!args.contains(&"--strict-mcp-config".into()));
    }
}

#[test]
fn native_web_tools_stay_available_unless_scope_denies_network() {
    let engine = ClaudeCodeEngine::new("claude", "test");
    for session in [None, Some("existing".to_owned())] {
        let mut t = turn();
        t.session = session;
        let args = engine.args(&t);
        assert!(
            args.windows(2)
                .any(|pair| pair == ["--disallowedTools", NATIVE_COORDINATION_TOOLS])
        );
        assert!(!args.contains(&"--tools".into()));
        t.network_access = false;
        let args = engine.args(&t);
        assert!(
            args.windows(2)
                .any(|pair| pair == ["--setting-sources", ""])
        );
        assert!(args.contains(&"--strict-mcp-config".into()));
        let settings: Value = serde_json::from_str(
            &args[args
                .iter()
                .position(|arg| arg == "--settings")
                .expect("settings")
                + 1],
        )
        .expect("JSON settings");
        assert!(
            settings["env"]
                .get("CLAUDE_CODE_DISABLE_1M_CONTEXT")
                .is_none()
        );
        assert_eq!(args.iter().filter(|arg| *arg == "--settings").count(), 1);
        assert_eq!(settings["sandbox"]["enabled"], true);
        assert_eq!(settings["sandbox"]["failIfUnavailable"], true);
        assert_eq!(settings["sandbox"]["allowUnsandboxedCommands"], false);
        assert_eq!(settings["sandbox"]["network"]["strictAllowlist"], true);
        assert_eq!(
            settings["sandbox"]["network"]["deniedDomains"],
            json!(["*"])
        );

        let denied = format!("{NATIVE_COORDINATION_TOOLS},WebSearch,WebFetch");
        assert!(
            args.windows(2)
                .any(|pair| pair == ["--disallowedTools", &denied])
        );
    }
}

#[test]
fn native_coordination_is_disabled_without_removing_workspace_capabilities() {
    let denied: Vec<_> = NATIVE_COORDINATION_TOOLS.split(',').collect();
    for tool in [
        "Task",
        "Agent",
        "ListAgents",
        "SendMessage",
        "ScheduleWakeup",
        "CronCreate",
        "CronDelete",
        "CronList",
        "Workflow",
        "RemoteTrigger",
    ] {
        assert!(denied.contains(&tool), "native coordination tool {tool}");
    }
    for tool in [
        "Bash",
        "Read",
        "Write",
        "Edit",
        "WebSearch",
        "WebFetch",
        "TaskOutput",
        "TaskStop",
        "ReportFindings",
        "Skill",
    ] {
        assert!(!denied.contains(&tool), "native workspace tool {tool}");
    }
}

#[tokio::test]
async fn older_claude_cannot_silently_ignore_network_denial() {
    let mut t = turn();
    t.network_access = false;
    let result = ClaudeCodeEngine::new("not-installed", "2.1.218")
        .run(t, CancellationToken::new())
        .await;
    assert!(matches!(result, Err(EngineError::Unsupported(_))));
}

#[test]
fn native_context_is_preserved_and_hooks_follow_isolation_on_every_launch() {
    let engine = ClaudeCodeEngine::new("claude", "2.1.263");
    let mut t = turn();
    for session in [None, Some("existing".into())] {
        for network_access in [true, false] {
            for summary in [false, true] {
                t.session = session.clone();
                t.summary_only = summary;
                t.network_access = network_access;
                t.model = Some("fable[1m]".into());
                let args = engine.args(&t);
                let settings: Value = serde_json::from_str(
                    &args[args.iter().position(|a| a == "--settings").unwrap() + 1],
                )
                .unwrap();
                assert!(
                    settings["env"]
                        .get("CLAUDE_CODE_DISABLE_1M_CONTEXT")
                        .is_none()
                );
                assert!(settings["env"].get("CLAUDE_CODE_THRIFTY_SONIC").is_none());
                assert_eq!(
                    settings.get("disableAllHooks"),
                    (summary || !network_access).then_some(&json!(true))
                );
                assert_eq!(
                    args[args.iter().position(|a| a == "--model").unwrap() + 1],
                    "fable[1m]"
                );
            }
        }
    }
}

#[test]
fn disabling_hooks_leaves_permission_rules_untouched() {
    let engine = ClaudeCodeEngine::new("claude", "test");
    let mut t = turn();
    t.network_access = false;
    let args = engine.args(&t);
    let settings: Value =
        serde_json::from_str(&args[args.iter().position(|a| a == "--settings").unwrap() + 1])
            .unwrap();
    assert_eq!(settings["disableAllHooks"], true);
    // The offline sandbox's own deny rules are set deliberately; hooks
    // being off must not add to or remove from them.
    assert_eq!(
        settings["permissions"],
        json!({"deny": ["WebFetch(domain:*)", "WebSearch"]})
    );

    t.network_access = true;
    let args = engine.args(&t);
    let settings: Value =
        serde_json::from_str(&args[args.iter().position(|a| a == "--settings").unwrap() + 1])
            .unwrap();
    assert!(settings.get("disableAllHooks").is_none());
    // A normal online turn inherits the operator's permission rules
    // untouched (via --setting-sources); Aether adds no override key.
    assert!(settings.get("permissions").is_none());
}

#[test]
fn prompt_cache_ttl_preserves_native_default_and_accepts_explicit_overrides() {
    let t = turn();
    let default_settings = |engine: &ClaudeCodeEngine| {
        let args = engine.args(&t);
        let settings: Value =
            serde_json::from_str(&args[args.iter().position(|a| a == "--settings").unwrap() + 1])
                .unwrap();
        settings["env"]["CLAUDE_CODE_PROMPT_CACHE_TTL"].clone()
    };

    let engine = ClaudeCodeEngine::new("claude", "test");
    assert_eq!(default_settings(&engine), Value::Null);

    let five_minutes = engine
        .clone()
        .with_prompt_cache_ttl(aether_core::model::PromptCacheTtl::FiveMinutes);
    assert_eq!(default_settings(&five_minutes), "5m");

    let engine = engine.with_prompt_cache_ttl(aether_core::model::PromptCacheTtl::OneHour);
    assert_eq!(default_settings(&engine), "1h");
}

#[test]
fn checkpoint_turn_disables_tools_mcp_and_hooks() {
    let mut t = turn();
    t.summary_only = true;
    t.mcp = None;
    t.bypass_permissions = false;
    let args = ClaudeCodeEngine::new("claude", "test").args(&t);
    assert!(args.windows(2).any(|pair| pair == ["--tools", ""]));
    assert!(args.contains(&"--strict-mcp-config".into()));
    assert!(
        args.contains(
            &json!({
                "disableAllHooks": true,
                "env": {}
            })
            .to_string()
        )
    );
}

const LEAN_FLAGS: [&str; 2] = ["--disable-slash-commands", "--strict-mcp-config"];

fn occurrences(args: &[String], flag: &str) -> usize {
    args.iter().filter(|arg| *arg == flag).count()
}

/// The launch with `lean_host_context` off and on, for one turn shape.
fn full_and_lean(network_access: bool, summary_only: bool) -> (Vec<String>, Vec<String>) {
    let engine = ClaudeCodeEngine::new("claude", "test");
    let mut t = turn();
    t.network_access = network_access;
    t.summary_only = summary_only;
    let full = engine.args(&t);
    t.lean_host_context = true;
    (full, engine.args(&t))
}

#[test]
fn full_host_context_launch_keeps_native_skills_and_mcp_servers() {
    let (full, _) = full_and_lean(true, false);
    for flag in LEAN_FLAGS {
        assert_eq!(occurrences(&full, flag), 0, "{flag}");
    }
    assert!(!full.contains(&"--setting-sources".into()));
}

#[test]
fn lean_online_launch_adds_both_flags_and_keeps_aether_mcp() {
    let (full, lean) = full_and_lean(true, false);
    for flag in LEAN_FLAGS {
        assert_eq!(occurrences(&lean, flag), 1, "{flag}");
    }
    let without: Vec<_> = lean
        .iter()
        .filter(|arg| !LEAN_FLAGS.contains(&arg.as_str()))
        .cloned()
        .collect();
    assert_eq!(without, full);
    // Lean keeps the operator's settings, CLAUDE.md and hooks.
    assert!(!lean.contains(&"--setting-sources".into()));
    let at = lean.iter().position(|arg| arg == "--mcp-config").unwrap();
    let config: Value = serde_json::from_str(&lean[at + 1]).unwrap();
    assert!(config["mcpServers"]["aether"].is_object());
}

#[test]
fn lean_offline_launch_adds_only_the_missing_flag() {
    let (full, lean) = full_and_lean(false, false);
    assert_eq!(occurrences(&full, "--strict-mcp-config"), 1);
    for flag in LEAN_FLAGS {
        assert_eq!(occurrences(&lean, flag), 1, "{flag}");
    }
    let without: Vec<_> = lean
        .iter()
        .filter(|arg| *arg != "--disable-slash-commands")
        .cloned()
        .collect();
    assert_eq!(without, full);
}

#[test]
fn lean_summary_launch_is_the_summary_launch() {
    let (full, lean) = full_and_lean(true, true);
    assert_eq!(lean, full);
    for flag in LEAN_FLAGS {
        assert_eq!(occurrences(&lean, flag), 1, "{flag}");
    }
}

#[test]
fn lean_offline_summary_launch_is_the_offline_summary_launch() {
    let (full, lean) = full_and_lean(false, true);
    assert_eq!(lean, full);
    for flag in LEAN_FLAGS {
        assert_eq!(occurrences(&lean, flag), 1, "{flag}");
    }
}

#[test]
fn args_for_fresh_session_route_permissions_through_aether() {
    let engine = ClaudeCodeEngine::new("claude", "2.1.263");
    let args = engine.args(&turn());
    let joined = args.join(" ");
    assert!(joined.contains("--session-id 11111111-2222-4333-8444-555555555555"));
    assert!(joined.contains("--permission-prompt-tool mcp__aether__permission"));
    assert!(joined.contains("--input-format stream-json"));
    assert!(joined.contains("--replay-user-messages"));
    assert!(joined.contains("--model sonnet"));
    assert!(joined.contains("--effort high"));
    assert!(joined.contains("--append-system-prompt contract"));
    assert!(!args.iter().any(|arg| arg == "--system-prompt"));
    assert!(joined.contains("\"mcpServers\""));
    assert_eq!(engine.adapter(), "claude-code/2.1.263");
}

#[test]
fn args_for_resume_and_bypass() {
    let engine = ClaudeCodeEngine::new("claude", "x");
    let mut t = turn();
    t.session = Some("abc".into());
    t.bypass_permissions = true;
    t.model = Some("default".into());
    let joined = engine.args(&t).join(" ");
    assert!(joined.contains("--resume abc"));
    assert!(!joined.contains("--session-id"));
    assert!(joined.contains("--permission-mode bypassPermissions"));
    assert!(!joined.contains("--permission-prompt-tool"));
    assert!(!joined.contains("--model"));
}

/// Every spawn goes through Aether's `delegate`, never the
/// CLI's own subagent tool - on the default path and under
/// bypass-permissions, where nothing else would gate it either.
#[test]
fn the_native_subagent_tool_is_always_disallowed() {
    let engine = ClaudeCodeEngine::new("claude", "x");
    assert!(
        engine
            .args(&turn())
            .join(" ")
            .contains("--disallowedTools Task,Agent")
    );
    let mut bypassed = turn();
    bypassed.bypass_permissions = true;
    assert!(
        engine
            .args(&bypassed)
            .join(" ")
            .contains("--disallowedTools Task,Agent")
    );
}

#[test]
fn offline_prerequisites_are_checked_without_restricting_online_turns() {
    let old = ClaudeCodeEngine::new("claude", "2.1.218");
    assert!(old.validate_network_policy(true).is_ok());
    assert!(old.validate_offline_support(|_| true).is_err());
    let current = ClaudeCodeEngine::new("claude", "2.1.265");
    assert!(current.validate_offline_support(|_| true).is_ok());
    if cfg!(target_os = "linux") {
        let error = current
            .validate_offline_support(|name| name == "bwrap")
            .unwrap_err()
            .to_string();
        assert!(error.contains("socat"), "{error}");
        assert!(error.contains("offline"), "{error}");
    }
}

/// Writes an executable shell script named `claude` under a fresh temp
/// dir and returns that dir, so `detect_verbose_on` can be pointed at it
/// instead of the host's real CLI.
fn fake_claude(script: &str) -> tempfile::TempDir {
    let dir = tempfile::tempdir().unwrap();
    crate::test_script::write(
        &dir.path().join("claude"),
        &format!("#!/bin/sh\n{script}\n"),
    );
    dir
}

/// Another name for the fake `claude`, written as that script is: a copy
/// would hold a write fd on it in this process.
fn copy_fake(path: &std::path::Path, program: &str) {
    let script = std::fs::read_to_string(path.join("claude")).unwrap();
    crate::test_script::write(&path.join(program), &script);
}

fn install_fake_sandbox_dependencies(path: &std::path::Path) {
    for program in ["bwrap", "socat"] {
        copy_fake(path, program);
    }
}

#[cfg(target_os = "linux")]
#[tokio::test]
async fn detected_engine_reports_missing_offline_dependencies() {
    let dir = fake_claude("echo '2.1.265 (Claude Code)'");
    let engine = ClaudeCodeEngine::detect_verbose_on(Some(dir.path()))
        .await
        .unwrap();
    let path = Some(dir.path().as_os_str());
    let both_missing = engine.limitation_on(path).unwrap();
    assert!(both_missing.contains("missing: bwrap, socat"));
    copy_fake(dir.path(), "bwrap");
    let limitation = engine.limitation_on(path).unwrap();
    assert!(limitation.contains("missing: socat."));
    assert!(!limitation.contains("bwrap"));
    assert!(limitation.contains("host authority"));
    assert!(limitation.contains("without network=false when offline execution is not required"));
    assert!(limitation.contains("fail_task"));
    assert!(limitation.contains("rather than waiting"));
    let error = engine.validate_offline_support(|name| executable_on_path(name, path));
    assert!(matches!(error, Err(EngineError::Unsupported(reason)) if reason == limitation));
    assert!(engine.validate_network_policy(true).is_ok());
    install_fake_sandbox_dependencies(dir.path());
    assert_eq!(engine.limitation_on(path), None);
}

#[tokio::test]
async fn detect_reports_a_good_version() {
    let dir = fake_claude("echo '2.1.265 (Claude Code)'");
    let engine = ClaudeCodeEngine::detect_verbose_on(Some(dir.path()))
        .await
        .expect("a supported version detects");
    assert_eq!(engine.version, "2.1.265");
    install_fake_sandbox_dependencies(dir.path());
    assert_eq!(engine.limitation_on(Some(dir.path().as_os_str())), None);
}

/// The floor is inclusive: exactly `MIN_VERSION` carries no limitation,
/// not just versions comfortably above it - `<` flipped to `<=` would
/// still pass every other fixture here.
#[tokio::test]
async fn detect_accepts_exactly_the_minimum_version() {
    let dir = fake_claude(&format!("echo '{MIN_VERSION} (Claude Code)'"));
    let engine = ClaudeCodeEngine::detect_verbose_on(Some(dir.path()))
        .await
        .expect("the minimum version itself is supported");
    install_fake_sandbox_dependencies(dir.path());
    assert_eq!(engine.limitation_on(Some(dir.path().as_os_str())), None);
}

/// Version was not always a registration gate -
/// any `claude` on PATH registered, and the floor only ever gated
/// network-denied turns inside `validate_offline_support`. The version
/// preflight first regressed that by refusing to register a
/// below-floor `claude` at all (`DetectError::TooOld`), which is the
/// assertion this test used to make. It now asserts the fix instead:
/// registration always succeeds once a version parses, and the gap is
/// carried as a limitation rather than a refusal.
#[tokio::test]
async fn detect_registers_a_below_floor_version_with_a_limitation() {
    let dir = fake_claude("echo '2.0.0 (Claude Code)'");
    let engine = ClaudeCodeEngine::detect_verbose_on(Some(dir.path()))
        .await
        .expect("a below-floor version still registers");
    assert_eq!(engine.version, "2.0.0");
    let limitation = engine.limitation().expect("the gap is recorded");
    assert!(limitation.contains("2.1.219"), "{limitation}");
    assert!(limitation.contains("2.0.0"), "{limitation}");
}

#[tokio::test]
async fn detect_reports_unparsable_output() {
    let dir = fake_claude("echo 'not a version'");
    let error = ClaudeCodeEngine::detect_verbose_on(Some(dir.path()))
        .await
        .unwrap_err();
    assert!(
        matches!(&error, DetectError::UnparsableVersion { .. }),
        "{error:?}"
    );
    assert!(error.to_string().contains("not a version"));
}

#[tokio::test]
async fn detect_reports_a_nonzero_exit() {
    let dir = fake_claude("echo 'license expired' >&2\nexit 1");
    let error = ClaudeCodeEngine::detect_verbose_on(Some(dir.path()))
        .await
        .unwrap_err();
    assert!(
        matches!(&error, DetectError::ExitFailure { .. }),
        "{error:?}"
    );
    assert!(error.to_string().contains("license expired"));
}

#[tokio::test]
async fn detect_reports_an_absent_program() {
    let dir = tempfile::tempdir().unwrap();
    let error = ClaudeCodeEngine::detect_verbose_on(Some(dir.path()))
        .await
        .unwrap_err();
    assert!(
        matches!(&error, DetectError::NotInstalled { .. }),
        "{error:?}"
    );
    assert!(error.to_string().contains("claude"));
}

#[path = "telemetry.rs"]
mod telemetry;
