use super::*;
use aether_core::model::Effort;

#[tokio::test]
async fn exec_ownership_only_records_ids_discovered_on_fresh_launches() {
    let dir = fake_codex(
        "cat >/dev/null\necho '{\"type\":\"thread.started\",\"thread_id\":\"new-thread\"}'",
    );
    let engine = CodexEngine::new(dir.path().join("codex").to_string_lossy(), "0.162.0");
    for session in [None, Some("user-session".into())] {
        let recorder = std::sync::Arc::new(crate::ownership_tests::Recorder::default());
        let t = EngineTurn {
            redactor: Default::default(),
            session: session.clone(),
            cwd: dir.path().into(),
            session_recorder: Some(recorder.clone()),
            ..turn_with_effort(None)
        };
        let mut run = engine.run(t, CancellationToken::new()).await.unwrap();
        while run.events.recv().await.is_some() {}
        let records = recorder.records.lock().unwrap();
        if session.is_none() {
            assert_eq!(records.len(), 1);
            assert_eq!(records[0].engine, EngineKind::CODEX);
            assert_eq!(records[0].id, "new-thread");
            assert!(records[0].log_paths.is_empty());
        } else {
            assert!(
                records.is_empty(),
                "a resumed user session returning a different id is not evidence of creation"
            );
        }
    }
}

#[tokio::test]
async fn exec_ownership_storage_failure_is_visible_and_cancels_process() {
    let dir = fake_codex(
        "cat >/dev/null\necho '{\"type\":\"thread.started\",\"thread_id\":\"new-thread\"}'\nsleep 30",
    );
    let engine = CodexEngine::new(dir.path().join("codex").to_string_lossy(), "0.162.0");
    let t = EngineTurn {
        redactor: Default::default(),
        cwd: dir.path().into(),
        session_recorder: Some(std::sync::Arc::new(crate::ownership_tests::Recorder {
            fail: true,
            ..Default::default()
        })),
        ..turn_with_effort(None)
    };
    let cancel = CancellationToken::new();
    let mut run = engine.run(t, cancel.clone()).await.unwrap();
    let mut saw_error = false;
    tokio::time::timeout(std::time::Duration::from_secs(5), async {
        while let Some(event) = run.events.recv().await {
            if let EngineEvent::Done(outcome) = event {
                saw_error |= outcome.is_error
                    && outcome
                        .error
                        .as_deref()
                        .is_some_and(|error| error.contains("storage unavailable"));
            }
        }
    })
    .await
    .expect("failed ownership recording must stop the process");
    assert!(saw_error);
    assert!(cancel.is_cancelled());
}

/// Writes an executable shell script named `codex` under a fresh temp
/// dir and returns that dir, so `detect_verbose_on` can be pointed at it
/// instead of the host's real CLI.
fn fake_codex(script: &str) -> tempfile::TempDir {
    let dir = tempfile::tempdir().unwrap();
    crate::test_script::write(&dir.path().join("codex"), &format!("#!/bin/sh\n{script}\n"));
    dir
}

#[tokio::test]
async fn detect_reports_a_good_version() {
    let dir = fake_codex("echo 'codex-cli 0.154.0'");
    let engine = CodexEngine::detect_verbose_on(Some(dir.path()))
        .await
        .expect("a version detects");
    assert_eq!(engine.version, "0.154.0");
}

#[tokio::test]
async fn detect_reports_unparsable_output() {
    let dir = fake_codex("echo 'not a version'");
    let error = CodexEngine::detect_verbose_on(Some(dir.path()))
        .await
        .unwrap_err();
    assert!(
        matches!(&error, DetectError::UnparsableVersion { .. }),
        "{error:?}"
    );
}

#[tokio::test]
async fn detect_reports_a_nonzero_exit() {
    let dir = fake_codex("echo 'not logged in' >&2\nexit 1");
    let error = CodexEngine::detect_verbose_on(Some(dir.path()))
        .await
        .unwrap_err();
    assert!(
        matches!(&error, DetectError::ExitFailure { .. }),
        "{error:?}"
    );
}

#[tokio::test]
async fn detect_reports_an_absent_program() {
    let dir = tempfile::tempdir().unwrap();
    let error = CodexEngine::detect_verbose_on(Some(dir.path()))
        .await
        .unwrap_err();
    assert!(
        matches!(&error, DetectError::NotInstalled { .. }),
        "{error:?}"
    );
}

#[tokio::test]
async fn exec_replacement_file_is_readable_by_child_and_cleaned_up() {
    let dir = tempfile::tempdir().unwrap();
    let program = dir.path().join("fake-codex");
    crate::test_script::write(
        &program,
        r#"#!/usr/bin/env python3
import json, pathlib, sys
setting = next(a for a in sys.argv if a.startswith('model_instructions_file='))
path = pathlib.Path(json.loads(setting.split('=', 1)[1]))
assert path.read_text() == 'Aether replacement sentinel'
print(json.dumps({'type':'item.completed','item':{'type':'agent_message','text':str(path)}}), flush=True)
print(json.dumps({'type':'turn.completed','usage':{}}), flush=True)
"#,
    );
    let engine = CodexEngine::new(program.to_string_lossy(), "test");
    let turn = EngineTurn {
        redactor: Default::default(),
        prompt: "hello".into(),
        system_prompt: "Aether replacement sentinel".into(),
        new_session_id: "new".into(),
        session_recorder: None,
        cwd: dir.path().into(),
        summary_only: true,
        compact_session: false,
        auto_compact_tokens: None,
        lean_host_context: false,
        network_access: false,
        ..turn_with_effort(None)
    };
    let mut run = engine.run(turn, CancellationToken::new()).await.unwrap();
    let mut path = None;
    while let Some(event) = run.events.recv().await {
        if let EngineEvent::Assistant { content, .. } = event {
            path = Some(PathBuf::from(aether_core::model::text_of(&content)));
        }
    }
    let path = path.expect("child read replacement prompt");
    assert!(
        !path.exists(),
        "the temporary prompt must be removed when the driver exits"
    );
}

#[tokio::test]
async fn skipped_provider_filters_are_visible_before_any_exec_events() {
    let dir = tempfile::tempdir().unwrap();
    let program = dir.path().join("fake-codex");
    crate::test_script::write(
        &program,
        "#!/bin/sh\ncase \"$*\" in *shell_environment_policy*) exit 2 ;; esac\nexit 1\n",
    );
    for version in ["0.161.0", "0.162.0", "unknown"] {
        for session in [None, Some("thread-1".into())] {
            let engine = CodexEngine::new(program.to_string_lossy(), version);
            let turn = EngineTurn {
                redactor: Default::default(),
                cwd: dir.path().into(),
                session,
                tool_hidden_env: vec!["PROVIDER_TOKEN".into(), "INVALID.NAME".into()],
                ..turn_with_effort(None)
            };
            let mut run = engine.run(turn, CancellationToken::new()).await.unwrap();
            assert!(
                matches!(run.events.recv().await, Some(EngineEvent::ProtocolNote(note))
                if note.contains("Provider credentials may remain visible")
                    && note.contains("codex exec")
                    && note.contains("provider environment filters were not applied")),
                "version={version}"
            );
            assert!(
                matches!(run.events.recv().await, Some(EngineEvent::Done(outcome))
                if outcome.is_error && outcome.error.as_deref().is_some_and(|error|
                    error.contains("exit status: 1")))
            );
            assert!(run.events.recv().await.is_none());
        }
    }
}

#[tokio::test]
async fn exec_does_not_warn_for_credentials_already_removed_from_its_environment() {
    let dir = tempfile::tempdir().unwrap();
    let program = dir.path().join("fake-codex");
    crate::test_script::write(&program, "#!/bin/sh\nexit 1\n");
    let engine = CodexEngine::new(program.to_string_lossy(), "0.162.0");
    for tool_hidden_env in [Vec::new(), vec!["PROVIDER_TOKEN".into()]] {
        let turn = EngineTurn {
            redactor: Default::default(),
            cwd: dir.path().into(),
            hidden_env: vec!["PROVIDER_TOKEN".into()],
            tool_hidden_env,
            ..turn_with_effort(None)
        };
        let mut run = engine.run(turn, CancellationToken::new()).await.unwrap();
        assert!(
            matches!(run.events.recv().await, Some(EngineEvent::Done(outcome))
            if outcome.is_error)
        );
        assert!(run.events.recv().await.is_none());
    }
}

#[tokio::test]
async fn summary_exec_does_not_warn_about_disabled_commands() {
    let dir = tempfile::tempdir().unwrap();
    let program = dir.path().join("fake-codex");
    crate::test_script::write(&program, "#!/bin/sh\nexit 1\n");
    let engine = CodexEngine::new(program.to_string_lossy(), "0.162.0");
    let turn = EngineTurn {
        redactor: Default::default(),
        cwd: dir.path().into(),
        summary_only: true,
        compact_session: false,
        auto_compact_tokens: None,
        lean_host_context: false,
        tool_hidden_env: vec!["PROVIDER_TOKEN".into()],
        ..turn_with_effort(None)
    };
    let mut run = engine.run(turn, CancellationToken::new()).await.unwrap();
    assert!(
        matches!(run.events.recv().await, Some(EngineEvent::Done(outcome))
        if outcome.is_error)
    );
    assert!(run.events.recv().await.is_none());
}

#[test]
fn exec_commands_get_the_agent_temp_dir() {
    let engine = CodexEngine::new("codex", "0.162.0");
    let mut turn = turn_with_effort(None);
    assert!(
        !engine
            .args(&turn)
            .join(" ")
            .contains("shell_environment_policy")
    );
    turn.temp_dir = Some(PathBuf::from("/var/t/0123456789AB"));
    for session in [None, Some("thread-1".to_owned())] {
        turn.session = session;
        let args = engine.args(&turn);
        for name in aether_core::agent_temp::ENV_VARS {
            let set = format!("shell_environment_policy.set.{name}=\"/var/t/0123456789AB\"");
            assert!(
                args.windows(2).any(|pair| pair == ["-c", set.as_str()]),
                "{args:?}"
            );
        }
        assert_eq!(args.last().map(String::as_str), Some("-"));
    }
    turn.build_env = vec![(
        "CARGO_TARGET_DIR".into(),
        "/var/cache/r/run/cargo-target".into(),
    )];
    let args = engine.args(&turn);
    let set = "shell_environment_policy.set.CARGO_TARGET_DIR=\"/var/cache/r/run/cargo-target\"";
    assert!(
        args.windows(2).any(|pair| pair == ["-c", set]),
        "and its build cache: {args:?}"
    );
}

#[tokio::test]
async fn exec_warns_once_per_session_across_turns_and_engine_clones() {
    let dir = tempfile::tempdir().unwrap();
    let program = dir.path().join("fake-codex");
    crate::test_script::write(
        &program,
        r#"#!/usr/bin/env python3
import json, sys
assert not any('shell_environment_policy' in arg for arg in sys.argv)
session = sys.argv[sys.argv.index('resume') + 1] if 'resume' in sys.argv else 'thread-1'
if sys.stdin.read().strip() == 'replace':
    session = 'thread-3'
print(json.dumps({'type': 'thread.started', 'thread_id': session}))
print(json.dumps({'type': 'turn.completed', 'usage': {
    'input_tokens': 0, 'cached_input_tokens': 0, 'output_tokens': 0,
    'cache_write_input_tokens': 0, 'reasoning_output_tokens': 0}}))
"#,
    );
    let engine = CodexEngine::new(program.to_string_lossy(), "0.162.0");
    for (session, prompt, expected_notes) in [
        (None, "hi", 1),
        (Some("thread-1"), "hi", 0),
        (Some("thread-1"), "replace", 1),
        (Some("thread-3"), "hi", 0),
        (Some("thread-1"), "replace", 0),
        (Some("thread-2"), "hi", 1),
        (Some("thread-2"), "hi", 0),
    ] {
        let turn = EngineTurn {
            redactor: Default::default(),
            prompt: prompt.into(),
            cwd: dir.path().into(),
            session: session.map(str::to_owned),
            tool_hidden_env: vec!["PROVIDER_TOKEN".into()],
            ..turn_with_effort(None)
        };
        let mut run = engine
            .clone()
            .run(turn, CancellationToken::new())
            .await
            .unwrap();
        let mut notes = 0;
        let mut done = false;
        while let Some(event) = run.events.recv().await {
            match event {
                EngineEvent::ProtocolNote(note) => {
                    assert!(!done);
                    assert!(note.contains("provider environment filters were not applied"));
                    notes += 1;
                }
                EngineEvent::Done(outcome) => {
                    assert!(!outcome.is_error);
                    done = true;
                }
                _ => {}
            }
        }
        assert!(done);
        assert_eq!(notes, expected_notes, "session={session:?}");
    }
}

#[test]
fn args_shape() {
    let engine = CodexEngine::new("codex", "0.153.2");
    let turn = EngineTurn {
        redactor: Default::default(),
        system_prompt: "Aether checkpoint contract".into(),
        session: Some("thread-1".into()),
        model: Some("gpt-5.4".into()),
        mcp: Some(crate::McpBridge {
            command: PathBuf::from("/bin/aether"),
            args: vec!["mcp-bridge".into()],
        }),
        ..turn_with_effort(Some(Effort::Medium))
    };
    let args = engine.args(&turn);
    let joined = args.join(" ");
    assert!(joined.starts_with("exec --json"));
    assert!(joined.contains("features.multi_agent=false"));
    assert!(!joined.contains("features.hooks="));
    assert!(joined.contains("mcp_servers.aether.default_tools_approval_mode=\"approve\""));
    assert!(joined.contains("agents.enabled=false"));
    assert!(joined.contains("--skip-git-repo-check -C /w -m gpt-5.4"));
    assert!(joined.contains("model_reasoning_effort=\"medium\""));
    assert!(joined.contains("--dangerously-bypass-approvals-and-sandbox"));
    assert!(joined.contains("mcp_servers.aether.command=\"/bin/aether\""));
    assert!(joined.ends_with("resume thread-1 -"));
    assert_eq!(engine.adapter(), "codex/0.153.2");
}

/// `--image` takes every value up to the next flag: on a fresh run it must
/// sit before one, and on a resumed run it belongs to `resume`.
#[test]
fn images_are_passed_as_files_where_the_cli_reads_them() {
    let engine = CodexEngine::new("codex", "0.162.1");
    let images = vec![
        crate::EngineImage {
            path: PathBuf::from("/data/a.png"),
            media_type: "image/png".into(),
        },
        crate::EngineImage {
            path: PathBuf::from("/data/b.jpg"),
            media_type: "image/jpeg".into(),
        },
    ];
    let fresh = EngineTurn {
        redactor: Default::default(),
        images: images.clone(),
        ..turn_with_effort(None)
    };
    let joined = engine.args(&fresh).join(" ");
    assert!(
        joined.starts_with("exec --image /data/a.png --image /data/b.jpg --json "),
        "{joined}"
    );
    assert!(joined.ends_with(" -"), "the prompt still arrives on stdin");

    let resumed = EngineTurn {
        redactor: Default::default(),
        images,
        session: Some("thread-1".into()),
        ..turn_with_effort(None)
    };
    let joined = engine.args(&resumed).join(" ");
    assert!(joined.starts_with("exec --json "), "{joined}");
    assert!(
        joined.ends_with("resume --image /data/a.png --image /data/b.jpg thread-1 -"),
        "{joined}"
    );

    // Without images the arguments are exactly what they were.
    let plain = engine.args(&turn_with_effort(None)).join(" ");
    assert!(!plain.contains("--image"), "{plain}");
    assert!(plain.starts_with("exec --json -c features.multi_agent=false"));
}

#[test]
fn both_codex_launchers_carry_aether_s_compaction_threshold() {
    let mut turn = turn_with_effort(None);
    let exec = CodexEngine::new("codex", "test");
    let app = crate::codex_app::CodexAppEngine::new("codex", "test");
    for args in [exec.args(&turn), app.args(&turn)] {
        assert!(
            !args
                .iter()
                .any(|a| a.contains("model_auto_compact_token_limit"))
        );
    }
    turn.auto_compact_tokens = Some(272_000);
    for args in [exec.args(&turn), app.args(&turn)] {
        assert!(
            args.windows(2)
                .any(|pair| pair == ["-c", "model_auto_compact_token_limit=272000"]),
            "{args:?}"
        );
    }
}

fn turn_with_effort(effort: Option<Effort>) -> EngineTurn {
    EngineTurn {
        redactor: Default::default(),
        images: Vec::new(),
        prompt: "hi".into(),
        system_prompt: "contract".into(),
        session: None,
        session_usage: Usage::default(),
        session_model_usage: None,
        new_session_id: "n".into(),
        session_recorder: None,
        model: None,
        effort,
        cwd: PathBuf::from("/w"),
        extra_dirs: vec![],
        hidden_env: Vec::new(),
        tool_hidden_env: Vec::new(),
        temp_dir: None,
        build_env: Vec::new(),
        summary_only: false,
        compact_session: false,
        auto_compact_tokens: None,
        lean_host_context: false,
        network_access: true,
        bypass_permissions: false,
        mcp: None,
    }
}

/// Only explicitly configured effort overrides native settings.
#[test]
fn effort_overrides_are_explicit_and_native_defaults_are_preserved() {
    let engine = CodexEngine::new("codex", "test");
    let cases = [
        (Some(Effort::Low), "low"),
        (Some(Effort::Medium), "medium"),
        (Some(Effort::High), "high"),
        (Some(Effort::XHigh), "xhigh"),
        (Some(Effort::Max), "max"),
        (None, ""),
    ];
    for (effort, expected) in cases {
        let args = engine.args(&turn_with_effort(effort));
        let count = args
            .iter()
            .filter(|a| a.starts_with("model_reasoning_effort="))
            .count();
        assert_eq!(count, usize::from(effort.is_some()), "effort={effort:?}");
        if effort.is_some() {
            assert!(
                args.contains(&format!("model_reasoning_effort=\"{expected}\"")),
                "effort={effort:?} args={args:?}"
            );
        }
    }
}

#[test]
fn native_hooks_are_preserved_except_on_isolated_turns() {
    let engine = CodexEngine::new("codex", "test");
    for summary_only in [false, true] {
        for network_access in [false, true] {
            let mut t = turn_with_effort(None);
            t.summary_only = summary_only;
            t.network_access = network_access;
            let args = engine.args(&t);
            let count = args
                .iter()
                .filter(|arg| *arg == "features.hooks=false")
                .count();
            assert_eq!(
                count,
                usize::from(summary_only || !network_access),
                "summary_only={summary_only} network_access={network_access}"
            );
        }
    }
}

/// Search configuration stays native unless isolation requires disabling it.
#[test]
fn web_search_override_matches_network_and_summary_matrix() {
    let engine = CodexEngine::new("codex", "test");
    for network_access in [true, false] {
        for summary_only in [true, false] {
            let turn = EngineTurn {
                redactor: Default::default(),
                network_access,
                summary_only,
                compact_session: false,
                auto_compact_tokens: None,
                ..turn_with_effort(None)
            };
            let args = engine.args(&turn);
            let has_live = args.iter().any(|a| a == "web_search=\"live\"");
            let has_disabled = args.iter().any(|a| a == "web_search=\"disabled\"");
            assert!(
                !has_live,
                "network_access={network_access} summary_only={summary_only}"
            );
            let expect_disabled = !network_access || summary_only;
            assert_eq!(
                has_disabled, expect_disabled,
                "network_access={network_access} summary_only={summary_only}"
            );
        }
    }
}

#[test]
fn parses_thread_items_and_usage() {
    let mut st = ParseState::default();
    assert_eq!(
        parse_line(r#"{"type":"thread.started","thread_id":"01a0"}"#, &mut st),
        vec![EngineEvent::Session {
            id: "01a0".into(),
            model: None,
            tools: vec![]
        }]
    );
    assert!(parse_line(r#"{"type":"turn.started"}"#, &mut st).is_empty());
    let started = r#"{"type":"item.started","item":{"id":"i1","type":"command_execution","command":"ls","status":"in_progress"}}"#;
    assert!(
        matches!(&parse_line(started, &mut st)[0], EngineEvent::Assistant { content, .. } if matches!(&content[0], ContentBlock::ToolUse { name, .. } if name == "bash"))
    );
    let done = r#"{"type":"item.completed","item":{"id":"i1","type":"command_execution","command":"ls","aggregated_output":"a\nb\n","exit_code":0,"status":"completed"}}"#;
    assert_eq!(
        parse_line(done, &mut st),
        vec![EngineEvent::ToolResult {
            tool_use_id: "i1".into(),
            content: "a\nb\n\n[exit code 0]".into(),
            is_error: false
        }]
    );
    let msg =
        r#"{"type":"item.completed","item":{"id":"i2","type":"agent_message","text":"hello"}}"#;
    assert!(
        matches!(&parse_line(msg, &mut st)[1], EngineEvent::Assistant { content, .. } if content == &vec![ContentBlock::Text { text: "hello".into() }])
    );
    let completed = r#"{"type":"turn.completed","usage":{"input_tokens":100,"cached_input_tokens":40,"output_tokens":9}}"#;
    let usage = Usage {
        input_tokens: 60,
        output_tokens: 9,
        cache_read_tokens: 40,
        ..Default::default()
    };
    assert_eq!(
        usage.total_tokens(),
        109,
        "cached input is never counted twice"
    );
    match &parse_line(completed, &mut st)[..] {
        [
            EngineEvent::SessionUsage { session, total, .. },
            EngineEvent::UsageDelta { usage: delta },
            EngineEvent::Done(o),
        ] => {
            assert_eq!((session.as_str(), *total), ("01a0", usage));
            assert_eq!(
                *delta, usage,
                "billed as it arrives, for the runtime to price"
            );
            assert!(!o.is_error);
            assert!(o.closing_accounting);
            assert_eq!(o.usage, usage, "and the run's total closes the turn");
        }
        other => panic!("{other:?}"),
    }
    let failed = r#"{"type":"turn.failed","error":{"message":"You've hit your usage limit."}}"#;
    match &parse_line(failed, &mut st)[0] {
        EngineEvent::Done(o) => {
            assert!(o.is_error && o.error.as_deref().unwrap().contains("usage limit"));
            assert!(!o.closing_accounting);
        }
        other => panic!("{other:?}"),
    }
}

/// Without its answer in `Done`, a prose-only reply reads as a silent turn.
#[test]
fn a_turn_reports_its_last_agent_message_as_its_answer() {
    let message = |text: &str| {
        json!({"type":"item.completed","item":{"id":"m","type":"agent_message","text":text}})
            .to_string()
    };
    let done_text = |events: Vec<EngineEvent>| match events.last() {
        Some(EngineEvent::Done(o)) => o.text.clone(),
        other => panic!("{other:?}"),
    };
    let completed = r#"{"type":"turn.completed","usage":{"input_tokens":1,"output_tokens":1}}"#;
    let mut st = ParseState::default();
    parse_line(&message("working on it"), &mut st);
    parse_line(&message("all done"), &mut st);
    // A blank message is no answer, so it does not replace the last one.
    assert_eq!(parse_line(&message("  "), &mut st).len(), 2);
    assert_eq!(
        done_text(parse_line(completed, &mut st)),
        Some("all done".into())
    );
    // Each turn reports only its own answer.
    assert_eq!(done_text(parse_line(completed, &mut st)), None);
}

/// `codex exec` reports the thread's cumulative counters, so a run is billed
/// only what it added - on a resumed thread, what it added over the reading
/// the runtime persisted for it - with reasoning inside the output it is.
#[test]
fn usage_is_the_increase_over_the_resumed_threads_persisted_counters() {
    let mut turn = turn_with_effort(None);
    turn.session = Some("th".into());
    // What the previous run on this thread persisted: its last reading.
    turn.session_usage = Usage {
        input_tokens: 600,
        output_tokens: 50,
        cache_read_tokens: 400,
        reasoning_tokens: Some(20),
        ..Default::default()
    };
    let mut st = ParseState::for_turn(&turn, None);
    let completed = json!({"type":"turn.completed","usage":{
        "input_tokens":1_500,"cached_input_tokens":900,"cache_write_input_tokens":0,
        "output_tokens":80,"reasoning_output_tokens":30}})
    .to_string();
    let added = Usage {
        input_tokens: (1_500 - 900) - 600,
        output_tokens: 30,
        cache_read_tokens: 500,
        reasoning_tokens: Some(10),
        ..Default::default()
    };
    match &parse_line(&completed, &mut st)[..] {
        [
            EngineEvent::SessionUsage { session, total, .. },
            EngineEvent::UsageDelta { usage },
            EngineEvent::Done(o),
        ] => {
            assert_eq!(session, "th", "persisted against the resumed thread");
            assert_eq!(total.cache_read_tokens, 900, "the new reading");
            assert_eq!(*usage, added, "never the thread's history again");
            assert_eq!(o.usage, added);
        }
        other => panic!("{other:?}"),
    }
    // The same reading again adds nothing, so nothing more is billed.
    match &parse_line(&completed, &mut st)[..] {
        [EngineEvent::SessionUsage { .. }, EngineEvent::Done(o)] => assert_eq!(o.usage, added),
        other => panic!("{other:?}"),
    }
}

fn completed(usage: serde_json::Value) -> String {
    json!({"type":"turn.completed","usage":usage}).to_string()
}

/// A field the CLI leaves out keeps its last reading: read as zero, a counter
/// would restart and the next reading would bill the whole thread again. The
/// held reading is a guess, though, so it is said aloud and never priced.
#[test]
fn a_missing_counter_is_not_a_restart() {
    let mut st = ParseState::default();
    parse_line(
        &completed(json!({"input_tokens":1_000,"cached_input_tokens":800,"output_tokens":10})),
        &mut st,
    );
    let events = parse_line(
        &completed(json!({"input_tokens":1_200,"output_tokens":20})),
        &mut st,
    );
    match &events[..] {
        [
            EngineEvent::ProtocolNote(note),
            EngineEvent::UsageDelta { usage },
            EngineEvent::Done(o),
        ] => {
            assert!(note.contains("cached_input_tokens"), "{note}");
            assert_eq!(usage.cache_read_tokens, 0, "the cached counter held");
            assert_eq!((usage.input_tokens, usage.output_tokens), (200, 10));
            assert!(!usage.cost_complete, "an inexact split is not a price");
            assert_eq!(
                serde_json::to_value(usage).unwrap()["cost_gap"],
                "unreported"
            );
            assert_eq!(o.usage.total_tokens(), 1_200 + 20);
        }
        other => panic!("{other:?}"),
    }
}

/// Advancing input with the cached counter left out would bill every new
/// input token at the uncached rate; a garbled counter could underbill. The
/// delta says it is inexact, and so does every later one in the run: the
/// held counter settles on a reading after it.
#[test]
fn an_unreadable_counter_makes_the_runs_cost_inexact() {
    for (reading, unread) in [
        (
            json!({"input_tokens":1_000,"output_tokens":10}),
            "cached_input_tokens",
        ),
        (
            json!({"input_tokens":1_000,"cached_input_tokens":null,"output_tokens":10}),
            "cached_input_tokens",
        ),
        (
            json!({"input_tokens":"1000","cached_input_tokens":800,"output_tokens":10}),
            "input_tokens",
        ),
        (
            json!({"input_tokens":1_000,"cached_input_tokens":800,"output_tokens":-1}),
            "output_tokens",
        ),
        (
            json!({"input_tokens":1_000,"cached_input_tokens":800,"output_tokens":10,
                "cache_write_input_tokens":"many"}),
            "cache_write_input_tokens",
        ),
        (
            json!({"input_tokens":1_000,"cached_input_tokens":800,"output_tokens":10,
                "cache_write_input_tokens":null}),
            "cache_write_input_tokens",
        ),
        (
            json!({"input_tokens":1_000,"cached_input_tokens":800,"output_tokens":10,
                "reasoning_output_tokens":null}),
            "reasoning_output_tokens",
        ),
    ] {
        let mut st = ParseState::default();
        let events = parse_line(&completed(reading.clone()), &mut st);
        match &events[..] {
            [
                EngineEvent::ProtocolNote(note),
                EngineEvent::UsageDelta { usage },
                EngineEvent::Done(o),
            ] => {
                assert!(note.contains(unread), "{reading}: {note}");
                assert!(!usage.cost_complete, "{reading}");
                assert!(!o.usage.cost_complete, "{reading}");
            }
            other => panic!("{reading}: {other:?}"),
        }
        let settled = parse_line(
            &completed(json!({"input_tokens":2_000,"cached_input_tokens":1_500,
                "output_tokens":20,"cache_write_input_tokens":0,
                "reasoning_output_tokens":0})),
            &mut st,
        );
        match &settled[..] {
            [EngineEvent::UsageDelta { usage }, EngineEvent::Done(_)] => {
                assert!(!usage.cost_complete, "{reading}: may settle the held one");
            }
            other => panic!("{reading}: {other:?}"),
        }
    }
}

/// Every counter a CLI must send, read: an exact delta, and no note. The two
/// newer counters may be absent on the older CLIs this adapter serves.
#[test]
fn a_fully_reported_turn_stays_complete() {
    for reading in [
        json!({"input_tokens":1_000,"cached_input_tokens":800,"output_tokens":10}),
        json!({"input_tokens":1_000,"cached_input_tokens":800,"output_tokens":10,
            "cache_write_input_tokens":0,"reasoning_output_tokens":4}),
    ] {
        let mut st = ParseState::default();
        match &parse_line(&completed(reading.clone()), &mut st)[..] {
            [EngineEvent::UsageDelta { usage }, EngineEvent::Done(o)] => {
                assert!(usage.cost_complete, "{reading}");
                assert!(o.usage.cost_complete, "{reading}");
                assert_eq!(usage.total_tokens(), 1_010, "{reading}");
            }
            other => panic!("{reading}: {other:?}"),
        }
    }
}

/// A newer counter the CLI has sent can no longer be left out: the held
/// reading would price the turn's cache writes as none.
#[test]
fn a_newer_counter_left_out_after_it_was_sent_is_unread() {
    let mut st = ParseState::default();
    parse_line(
        &completed(
            json!({"input_tokens":1_000,"cached_input_tokens":800,"output_tokens":10,
            "cache_write_input_tokens":100,"reasoning_output_tokens":4}),
        ),
        &mut st,
    );
    let events = parse_line(
        &completed(json!({"input_tokens":2_000,"cached_input_tokens":1_500,
            "output_tokens":20,"reasoning_output_tokens":6})),
        &mut st,
    );
    match &events[..] {
        [
            EngineEvent::ProtocolNote(note),
            EngineEvent::UsageDelta { usage },
            EngineEvent::Done(_),
        ] => {
            assert!(note.contains("cache_write_input_tokens"), "{note}");
            assert!(!note.contains("reasoning_output_tokens"), "{note}");
            assert_eq!(usage.cache_write_tokens, 0, "the counter held");
            assert!(!usage.cost_complete, "a held cache write is not a price");
        }
        other => panic!("{other:?}"),
    }
}

/// A resumed thread's counted cache writes say its CLI sends the counter, so
/// a run that leaves it out is unread; a thread that never counted any, or a
/// different thread, may still be on a CLI that predates it.
#[test]
fn a_resumed_threads_counted_newer_counter_must_be_sent() {
    let reading =
        completed(json!({"input_tokens":2_000,"cached_input_tokens":1_000,"output_tokens":30}));
    let resumed = |cache_write_tokens| {
        let mut turn = turn_with_effort(None);
        turn.session = Some("th".into());
        turn.session_usage = Usage {
            input_tokens: 500,
            cache_read_tokens: 500,
            output_tokens: 10,
            cache_write_tokens,
            ..Default::default()
        };
        ParseState::for_turn(&turn, None)
    };

    let mut st = resumed(200);
    match &parse_line(&reading, &mut st)[..] {
        [
            EngineEvent::ProtocolNote(note),
            EngineEvent::SessionUsage { total, .. },
            EngineEvent::UsageDelta { usage },
            EngineEvent::Done(_),
        ] => {
            assert!(note.contains("cache_write_input_tokens"), "{note}");
            assert_eq!(total.cache_write_tokens, 200, "the held reading persists");
            assert!(!usage.cost_complete);
        }
        other => panic!("{other:?}"),
    }

    let mut st = resumed(0);
    match &parse_line(&reading, &mut st)[..] {
        [
            EngineEvent::SessionUsage { .. },
            EngineEvent::UsageDelta { usage },
            EngineEvent::Done(_),
        ] => assert!(usage.cost_complete),
        other => panic!("{other:?}"),
    }

    let mut st = resumed(200);
    parse_line(r#"{"type":"thread.started","thread_id":"other"}"#, &mut st);
    match &parse_line(&reading, &mut st)[..] {
        [
            EngineEvent::SessionUsage { session, .. },
            EngineEvent::UsageDelta { usage },
            EngineEvent::Done(_),
        ] => {
            assert_eq!(session, "other");
            assert!(usage.cost_complete, "not the thread that sent it");
        }
        other => panic!("{other:?}"),
    }
}

/// A CLI's version says which newer counters it always sends: from 0.145.0
/// a left-out cache write is unread even on a thread that never counted one,
/// reasoning from 0.125.0, and neither before.
#[test]
fn a_clis_version_says_which_newer_counters_it_must_send() {
    let reading =
        completed(json!({"input_tokens":2_000,"cached_input_tokens":1_000,"output_tokens":30}));
    let noted = |version: EngineVersion| {
        let mut turn = turn_with_effort(None);
        turn.session = Some("th".into());
        turn.session_usage = Usage {
            input_tokens: 500,
            cache_read_tokens: 500,
            output_tokens: 10,
            ..Default::default()
        };
        let mut st = ParseState::for_turn(&turn, Some(version));
        let events = parse_line(&reading, &mut st);
        let Some(EngineEvent::Done(outcome)) = events.last() else {
            panic!("{events:?}");
        };
        let note = events.iter().find_map(|e| match e {
            EngineEvent::ProtocolNote(note) => Some(note.clone()),
            _ => None,
        });
        assert_eq!(outcome.usage.cost_complete, note.is_none(), "{note:?}");
        note.unwrap_or_default()
    };

    let note = noted(EngineVersion::new(0, 145, 0));
    assert!(note.contains("cache_write_input_tokens"), "{note}");
    assert!(note.contains("reasoning_output_tokens"), "{note}");

    let note = noted(EngineVersion::new(0, 144, 6));
    assert!(!note.contains("cache_write_input_tokens"), "{note}");
    assert!(note.contains("reasoning_output_tokens"), "{note}");

    assert_eq!(
        noted(EngineVersion::new(0, 124, 0)),
        "",
        "an older CLI lost nothing"
    );

    // A fresh thread on a current CLI is held to the same.
    let mut st = ParseState::for_turn(&turn_with_effort(None), Some(EngineVersion::new(0, 159, 2)));
    parse_line(r#"{"type":"thread.started","thread_id":"new"}"#, &mut st);
    assert!(matches!(&parse_line(&reading, &mut st)[..],
        [EngineEvent::ProtocolNote(note), ..] if note.contains("cache_write_input_tokens")));
}

/// A `turn.completed` without a usage object read nothing: it names every
/// counter a CLI must send and leaves the run's cost inexact.
#[test]
fn a_turn_completed_without_usage_is_unread() {
    let mut st = ParseState::default();
    match &parse_line(r#"{"type":"turn.completed"}"#, &mut st)[..] {
        [EngineEvent::ProtocolNote(note), EngineEvent::Done(o)] => {
            for key in ["input_tokens", "cached_input_tokens", "output_tokens"] {
                assert!(note.contains(key), "{note}");
            }
            assert!(!note.contains("cache_write_input_tokens"), "{note}");
            assert_eq!(o.usage.total_tokens(), 0);
            assert!(!o.usage.cost_complete, "nothing read is not a price");
        }
        other => panic!("{other:?}"),
    }
}

/// A resumed thread whose reading leaves a counter out is still billed only
/// what it added over the persisted one - never its history again - and the
/// persisted baseline keeps the held counter, not a zero.
#[test]
fn an_unreadable_counter_on_a_resumed_thread_does_not_rebill_its_history() {
    let mut turn = turn_with_effort(None);
    turn.session = Some("th".into());
    turn.session_usage = Usage {
        input_tokens: 600,
        output_tokens: 50,
        cache_read_tokens: 400,
        ..Default::default()
    };
    let mut st = ParseState::for_turn(&turn, None);
    let events = parse_line(
        &completed(json!({"input_tokens":1_500,"output_tokens":80})),
        &mut st,
    );
    match &events[..] {
        [
            EngineEvent::ProtocolNote(note),
            EngineEvent::SessionUsage { session, total, .. },
            EngineEvent::UsageDelta { usage },
            EngineEvent::Done(_),
        ] => {
            assert!(note.contains("cached_input_tokens"), "{note}");
            assert_eq!(session, "th");
            assert_eq!(total.cache_read_tokens, 400, "the held reading persists");
            // 500 input the thread added (1,500 - 1,000), all read as fresh.
            assert_eq!(
                (
                    usage.input_tokens,
                    usage.cache_read_tokens,
                    usage.output_tokens
                ),
                (500, 0, 30),
                "never the thread's history again"
            );
            assert!(!usage.cost_complete);
        }
        other => panic!("{other:?}"),
    }
}

/// A run handed one thread's counters that ends up on another thread bills
/// that thread from zero, and persists its reading against it.
#[test]
fn a_different_thread_does_not_inherit_the_resumed_ones_counters() {
    let mut turn = turn_with_effort(None);
    turn.session = Some("old".into());
    turn.session_usage = Usage {
        input_tokens: 5_000,
        output_tokens: 500,
        ..Default::default()
    };
    let mut st = ParseState::for_turn(&turn, None);
    parse_line(r#"{"type":"thread.started","thread_id":"new"}"#, &mut st);
    let completed = r#"{"type":"turn.completed","usage":{"input_tokens":300,"cached_input_tokens":0,"output_tokens":20}}"#;
    match &parse_line(completed, &mut st)[..] {
        [
            EngineEvent::SessionUsage { session, .. },
            EngineEvent::UsageDelta { usage },
            EngineEvent::Done(_),
        ] => {
            assert_eq!(session, "new");
            assert_eq!((usage.input_tokens, usage.output_tokens), (300, 20));
        }
        other => panic!("{other:?}"),
    }
}

#[test]
fn failed_turn_counts_as_closing_accounting_only_with_its_own_usage() {
    for kind in ["turn.failed", "error"] {
        let mut state = ParseState::default();
        let events = parse_line(
            &json!({"type":kind,"error":{"message":"failure"},
            "usage":{"input_tokens":100,"cached_input_tokens":0,"output_tokens":10,
                "reasoning_output_tokens":0}})
            .to_string(),
            &mut state,
        );
        let Some(EngineEvent::Done(outcome)) = events.last() else {
            panic!("{events:?}");
        };
        assert!(outcome.is_error);
        assert_eq!(outcome.closing_accounting, kind == "turn.failed");
        assert_eq!(
            outcome.usage.total_tokens(),
            if kind == "turn.failed" { 110 } else { 0 }
        );
    }
}

#[tokio::test]
async fn run_reports_a_missing_workspace_not_a_missing_program() {
    let dir = tempfile::tempdir().unwrap();
    let engine = CodexEngine::new(dir.path().join("x").to_string_lossy(), "0.162.0");
    let mut t = turn_with_effort(None);
    t.cwd = dir.path().join("deleted");
    let error = engine.run(t, CancellationToken::new()).await.unwrap_err();
    assert!(
        matches!(&error, EngineError::WorkspaceMissing { path } if path == &dir.path().join("deleted")),
        "{error:?}"
    );
    assert_eq!(
        error.to_string(),
        format!(
            "workspace missing: {}",
            dir.path().join("deleted").display()
        )
    );
}

#[tokio::test]
async fn run_reports_a_missing_program_when_the_workspace_exists() {
    let dir = tempfile::tempdir().unwrap();
    let engine = CodexEngine::new(dir.path().join("absent").to_string_lossy(), "0.162.0");
    let mut t = turn_with_effort(None);
    t.cwd = dir.path().into();
    let error = engine.run(t, CancellationToken::new()).await.unwrap_err();
    assert!(
        matches!(&error, EngineError::NotInstalled { .. }),
        "{error:?}"
    );
    assert!(error.to_string().contains("not installed or not on PATH"));
}
