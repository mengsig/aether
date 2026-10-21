use super::{Driver, Sent};
use crate::EngineEvent;
use crate::codex_app::tests::turn;
use crate::detect::EngineVersion;
use serde_json::json;

#[tokio::test]
async fn app_ownership_records_only_thread_start_and_confirmed_absolute_paths() {
    let dir = tempfile::tempdir().unwrap();
    let log = dir.path().join("session.jsonl");
    for session in [None, Some("user-session")] {
        for path in [Some(log.to_str().unwrap()), Some("relative.jsonl"), None] {
            let recorder = std::sync::Arc::new(crate::ownership_tests::Recorder::default());
            let mut t = turn(session, false);
            t.mcp = None;
            t.session_recorder = Some(recorder.clone());
            let (_, running, _) = handshake(
                t,
                None,
                vec![
                    json!({}),
                    json!({"thread": {"id": "new-thread", "path": path}}),
                ],
            )
            .await;
            assert!(running);
            let records = recorder.records.lock().unwrap();
            if session.is_none() {
                assert_eq!(records.len(), 1);
                assert_eq!(records[0].id, "new-thread");
                assert_eq!(
                    records[0].log_paths,
                    if path == Some(log.to_str().unwrap()) {
                        vec![log.clone()]
                    } else {
                        Vec::new()
                    }
                );
            } else {
                assert!(records.is_empty());
            }
        }
    }
}

#[tokio::test]
async fn app_ownership_failure_prevents_turn_submission() {
    let mut t = turn(None, false);
    t.mcp = None;
    t.session_recorder = Some(std::sync::Arc::new(crate::ownership_tests::Recorder {
        fail: true,
        ..Default::default()
    }));
    let (requests, running, events) = handshake(
        t,
        None,
        vec![json!({}), json!({"thread": {"id": "new-thread"}})],
    )
    .await;
    assert!(!running);
    assert_eq!(methods(&requests), ["initialize", "thread/start"]);
    assert!(events.iter().any(|event| matches!(event, EngineEvent::Done(outcome) if outcome.is_error && outcome.prompt_not_submitted && outcome.error.as_deref().is_some_and(|error| error.contains("storage unavailable")))));
}

#[tokio::test]
async fn startup_failure_before_turn_dispatch_proves_prompt_was_not_submitted() {
    check_startup_failure(false).await;
}

#[tokio::test]
async fn missing_turn_start_acknowledgement_does_not_prove_safe_replay() {
    check_startup_failure(true).await;
}

async fn check_startup_failure(dispatch: bool) {
    let mut child = tokio::process::Command::new("sh")
        .args(["-c", "cat >/dev/null"])
        .stdin(std::process::Stdio::piped())
        .kill_on_drop(true)
        .spawn()
        .unwrap();
    let (tx, mut rx) = tokio::sync::mpsc::channel(8);
    let mut driver = Driver::new(
        child.stdin.take().unwrap(),
        tx,
        turn(Some("retained"), false),
        None,
        Some(EngineVersion::new(0, 162, 0)),
    );
    if dispatch {
        driver
            .request(Sent::TurnStart, "turn/start", json!({}))
            .await;
    }
    driver
        .finish_error("codex app-server did not start the turn in time".into())
        .await;
    let Some(EngineEvent::Done(outcome)) = rx.recv().await else {
        panic!("expected failure")
    };
    assert_eq!(outcome.prompt_not_submitted, !dispatch);
    assert!(outcome.is_error);
    assert!(!outcome.closing_accounting);
    drop(driver);
    child.wait().await.unwrap();
}

/// Drive the startup handshake against canned responses, one per request
/// until the driver stops, and return the requests it wrote, whether it kept
/// the turn running, and its events.
async fn handshake(
    t: crate::EngineTurn,
    environment: Option<super::Environment>,
    replies: Vec<serde_json::Value>,
) -> (Vec<serde_json::Value>, bool, Vec<EngineEvent>) {
    handshake_responses(
        t,
        environment,
        replies
            .into_iter()
            .map(|result| json!({"result": result}))
            .collect(),
        Some(EngineVersion::new(0, 162, 0)),
    )
    .await
}

async fn handshake_responses(
    t: crate::EngineTurn,
    environment: Option<super::Environment>,
    replies: Vec<serde_json::Value>,
    version: Option<EngineVersion>,
) -> (Vec<serde_json::Value>, bool, Vec<EngineEvent>) {
    let dir = tempfile::tempdir().unwrap();
    let sent = dir.path().join("sent");
    let mut child = tokio::process::Command::new("sh")
        .args(["-c", "cat > \"$0\""])
        .arg(&sent)
        .stdin(std::process::Stdio::piped())
        .kill_on_drop(true)
        .spawn()
        .unwrap();
    let (tx, mut rx) = tokio::sync::mpsc::channel(16);
    let mut driver = Driver::new(child.stdin.take().unwrap(), tx, t, environment, version);
    let mut running = driver
        .request(Sent::Initialize, "initialize", json!({}))
        .await;
    for (id, mut response) in (1..).zip(replies) {
        assert!(running, "the driver stopped before reply {id}");
        response["id"] = json!(id);
        running = driver.line(&response.to_string()).await;
    }
    drop(driver);
    child.wait().await.unwrap();
    let requests = std::fs::read_to_string(&sent)
        .unwrap()
        .lines()
        .map(|line| serde_json::from_str(line).unwrap())
        .collect();
    let mut events = Vec::new();
    while let Ok(event) = rx.try_recv() {
        events.push(event);
    }
    (requests, running, events)
}

fn methods(requests: &[serde_json::Value]) -> Vec<&str> {
    requests
        .iter()
        .map(|r| r["method"].as_str().unwrap())
        .collect()
}

#[tokio::test]
async fn provider_command_filters_are_sent_on_every_start_and_resume() {
    for session in [None, Some("th")] {
        let mut t = turn(session, false);
        t.mcp = None;
        t.hidden_env = vec!["DAEMON_SECRET".into()];
        t.tool_hidden_env = [
            "PROVIDER_KEY",
            "CUSTOM_BEARER",
            "provider-2",
            "PROVIDER_KEY",
            "DAEMON_SECRET",
            "invalid.name",
            "glob*",
            "bad space",
            "",
            "Ü",
        ]
        .map(str::to_owned)
        .into();
        let engine = crate::codex_app::CodexAppEngine::new("codex", "0.162.0");
        assert!(
            !engine
                .args(&t)
                .iter()
                .any(|arg| arg.contains("shell_environment_policy"))
        );
        let replies = vec![
            json!({}),
            json!({
                "config": {"shell_environment_policy": {"exclude": null, "include_only": null}},
                "layers": [
                    {"name": {"type": "user"}, "version": "fixture", "config": {
                        "shell_environment_policy": {
                            "filters": {"EXISTING_PATTERN": "exclude"},
                            "set": {"PUBLIC": "visible"}
                        }
                    }},
                    {"name": {"type": "system"}, "version": "fixture", "config": {}}
                ]
            }),
            json!({"thread": {"id": "th"}}),
            json!({"turn": {"id": "tu"}}),
        ];
        let (requests, running, events) = handshake(t, None, replies).await;
        let method = if session.is_some() {
            "thread/resume"
        } else {
            "thread/start"
        };
        assert_eq!(
            methods(&requests),
            ["initialize", "config/read", method, "turn/start"]
        );
        assert_eq!(
            requests[1]["params"],
            json!({"includeLayers": true, "cwd": "/w"})
        );
        assert_eq!(
            requests[2]["params"]["config"],
            json!({"shell_environment_policy": {"filters": {
                "PROVIDER_KEY": "exclude",
                "CUSTOM_BEARER": "exclude",
                "provider-2": "exclude"
            }}})
        );
        assert!(running);
        let note = events
            .iter()
            .find_map(|event| match event {
                EngineEvent::ProtocolNote(note) => Some(note),
                _ => None,
            })
            .expect("dropped names must be reported");
        for name in ["invalid.name", "glob*", "bad space", "\"\"", "Ü"] {
            assert!(note.contains(name), "{note}");
        }
        assert!(!note.contains("DAEMON_SECRET"));
        assert!(!note.contains("CUSTOM_BEARER"));
    }
}

#[tokio::test]
async fn legacy_command_env_policies_are_preserved_with_a_notice() {
    for session in [None, Some("th")] {
        for policy in [
            json!({"exclude": ["OPERATOR_SECRET"]}),
            json!({"exclude": []}),
            json!({"include_only": ["PATH"]}),
            json!({"include_only": []}),
        ] {
            let mut t = turn(session, false);
            t.mcp = None;
            t.tool_hidden_env = vec!["PROVIDER_KEY".into()];
            let replies = vec![
                json!({}),
                json!({
                    "config": {"shell_environment_policy": {"exclude": null, "include_only": null}},
                    "layers": [
                        {"config": {}},
                        {"config": {"shell_environment_policy": policy}}
                    ]
                }),
                json!({"thread": {"id": "th"}}),
                json!({"turn": {"id": "tu"}}),
            ];
            let (requests, running, events) = handshake(t, None, replies).await;
            assert!(running);
            assert_eq!(requests[1]["method"], "config/read");
            assert!(requests[2]["params"].get("config").is_none());
            assert!(
                events
                    .iter()
                    .any(|event| matches!(event, EngineEvent::ProtocolNote(note)
                if note.contains("Provider credentials may remain visible")
                    && note.contains("legacy shell_environment_policy")))
            );
        }
    }
}

#[tokio::test]
async fn failed_or_uninspectable_command_env_config_is_visible_and_not_overridden() {
    for session in [None, Some("th")] {
        for response in [
            json!({"error": {"code": -32601, "message": "Method not found"}}),
            json!({"error": {"code": -32000, "message": "Could not load sensitive-config-value"}}),
            json!({"result": {}}),
            json!({"result": {"layers": null}}),
            json!({"result": {"layers": {}}}),
            json!({"result": {"layers": [{}]}}),
            json!({"result": {"layers": [{"config": null}]}}),
            json!({"result": {"layers": [{"config": {"shell_environment_policy": "invalid"}}]}}),
        ] {
            let mut t = turn(session, false);
            t.mcp = None;
            t.tool_hidden_env = vec!["PROVIDER_KEY".into()];
            let error_code = response["error"]["code"].as_i64();
            let replies = vec![
                json!({"result": {}}),
                response,
                json!({"result": {"thread": {"id": "th"}}}),
                json!({"result": {"turn": {"id": "tu"}}}),
            ];
            let (requests, running, events) =
                handshake_responses(t, None, replies, Some(EngineVersion::new(0, 162, 0))).await;
            assert!(running);
            assert_eq!(requests[1]["method"], "config/read");
            assert!(requests[2]["params"].get("config").is_none());
            let note = events
                .iter()
                .find_map(|event| match event {
                    EngineEvent::ProtocolNote(note) => Some(note),
                    _ => None,
                })
                .expect("skipped filters must produce a visible notice");
            assert!(note.contains("Provider credentials may remain visible"));
            assert!(note.contains("config/read"));
            assert!(!note.contains("sensitive-config-value"));
            if let Some(code) = error_code {
                assert!(note.contains(&format!("RPC error {code}")));
            }
        }
    }
}

#[tokio::test]
async fn old_or_unknown_codex_versions_skip_provider_filters_with_a_notice() {
    for session in [None, Some("th")] {
        for version in [Some(EngineVersion::new(0, 161, 0)), None] {
            let mut t = turn(session, false);
            t.mcp = None;
            t.tool_hidden_env = vec!["PROVIDER_KEY".into()];
            let replies = vec![
                json!({"result": {}}),
                json!({"result": {"thread": {"id": "th"}}}),
                json!({"result": {"turn": {"id": "tu"}}}),
            ];
            let (requests, running, events) = handshake_responses(t, None, replies, version).await;
            assert!(running);
            let method = if session.is_some() {
                "thread/resume"
            } else {
                "thread/start"
            };
            assert_eq!(methods(&requests), ["initialize", method, "turn/start"]);
            assert!(requests[1]["params"].get("config").is_none());
            assert!(events.iter().any(|event| matches!(event,
                EngineEvent::ProtocolNote(note)
                    if note.contains("Provider credentials may remain visible")
                        && note.contains("version is unknown or older")
                        && note.contains("0.162.0"))));
        }
    }
}

#[tokio::test]
async fn no_filterable_provider_names_skip_config_read() {
    let mut t = turn(None, false);
    t.mcp = None;
    t.hidden_env = vec!["DAEMON_SECRET".into()];
    t.tool_hidden_env = vec!["DAEMON_SECRET".into(), "glob*".into()];
    let (requests, running, events) =
        handshake(t, None, vec![json!({}), json!({"thread": {"id": "th"}})]).await;
    assert!(running);
    assert_eq!(
        methods(&requests),
        ["initialize", "thread/start", "turn/start"]
    );
    assert!(requests[1]["params"].get("config").is_none());
    assert!(
        events
            .iter()
            .any(|event| matches!(event, EngineEvent::ProtocolNote(note) if note.contains("glob*") && !note.contains("DAEMON_SECRET")))
    );
}

#[cfg(target_os = "linux")]
fn offline_turn(session: Option<&str>) -> crate::EngineTurn {
    let mut t = turn(session, false);
    t.network_access = false;
    t.mcp = None;
    t
}

#[cfg(target_os = "linux")]
fn sandbox() -> super::Environment {
    super::Environment {
        url: "ws://127.0.0.1:9".into(),
        token: "secret".into(),
    }
}

#[cfg(target_os = "linux")]
#[tokio::test]
async fn provider_filters_preserve_loopback_environment_registration() {
    let mut t = offline_turn(None);
    t.tool_hidden_env = vec!["PROVIDER_KEY".into()];
    let ours = json!({"environmentId": "aether-loopback", "cwd": "/w"});
    let replies = vec![
        json!({}),
        json!({}),
        json!({"layers": [{"config": {}}]}),
        json!({"thread": {"id": "th", "environments": [ours]}}),
        json!({"turn": {"id": "tu"}}),
    ];
    let (requests, running, events) = handshake(t, Some(sandbox()), replies).await;
    assert_eq!(
        methods(&requests),
        [
            "initialize",
            "environment/add",
            "config/read",
            "thread/start",
            "turn/start"
        ]
    );
    assert_eq!(requests[3]["params"]["environments"], json!([ours]));
    assert_eq!(requests[4]["params"]["environments"], json!([ours]));
    assert_eq!(
        requests[3]["params"]["config"]["shell_environment_policy"]["filters"],
        json!({"PROVIDER_KEY": "exclude"})
    );
    assert!(running);
    assert!(!refused(&events));
}

/// Whether the driver ended the turn with the loopback refusal.
#[cfg(target_os = "linux")]
fn refused(events: &[EngineEvent]) -> bool {
    events.iter().any(|e| {
        matches!(e, EngineEvent::Done(o) if o.is_error
            && o.error.as_deref().is_some_and(|m| m.contains("loopback-only sandbox")))
    })
}

/// A fresh thread is confirmed by its `thread/start` response, so no
/// `thread/read` races the rollout Codex has not written yet.
#[cfg(target_os = "linux")]
#[tokio::test]
async fn fresh_network_denied_thread_is_confirmed_before_the_turn() {
    let ours = json!({"environmentId": "aether-loopback", "cwd": "/w"});
    let replies = vec![
        json!({}),
        json!({}),
        json!({"thread": {"id": "th", "environments": [ours]}}),
        json!({"turn": {"id": "tu"}}),
    ];
    let (requests, running, events) = handshake(offline_turn(None), Some(sandbox()), replies).await;
    assert_eq!(
        methods(&requests),
        [
            "initialize",
            "environment/add",
            "thread/start",
            "turn/start"
        ]
    );
    assert_eq!(
        requests[1]["params"],
        json!({
            "environmentId": "aether-loopback",
            "execServerUrl": "ws://127.0.0.1:9",
            "authBearerToken": "secret",
        })
    );
    assert_eq!(requests[2]["params"]["environments"], json!([ours]));
    assert_eq!(requests[3]["params"]["environments"], json!([ours]));
    assert!(running);
    assert!(!refused(&events));
}

/// A fresh thread that also kept Codex's local environment never starts a turn.
#[cfg(target_os = "linux")]
#[tokio::test]
async fn fresh_thread_outside_the_sandbox_stops_before_the_turn() {
    let ours = json!({"environmentId": "aether-loopback", "cwd": "/w"});
    let local = json!({"environmentId": "local", "cwd": "/w"});
    for environments in [json!([local, ours]), json!(null), json!([])] {
        let replies = vec![
            json!({}),
            json!({}),
            json!({"thread": {"id": "th", "environments": environments}}),
        ];
        let (requests, running, events) =
            handshake(offline_turn(None), Some(sandbox()), replies).await;
        assert_eq!(
            methods(&requests),
            ["initialize", "environment/add", "thread/start"],
            "{environments}"
        );
        assert!(!running, "{environments}");
        assert!(refused(&events), "{environments}");
        let Some(EngineEvent::Done(outcome)) = events.last() else {
            panic!("expected a final outcome: {environments}")
        };
        assert!(outcome.prompt_not_submitted, "{environments}");
        assert!(
            !events
                .iter()
                .any(|e| matches!(e, EngineEvent::Session { .. })),
            "a refused thread must not be kept for resumption: {environments}"
        );
    }
}

/// Resuming forgets sticky environments, so the turn selects the sandbox and
/// `thread/read` then confirms it from the existing rollout.
#[cfg(target_os = "linux")]
#[tokio::test]
async fn resumed_network_denied_thread_is_confirmed_after_turn_start() {
    let ours = json!({"environmentId": "aether-loopback", "cwd": "/w"});
    let local = json!({"environmentId": "local", "cwd": "/w"});
    for (environments, confirmed) in [(json!([ours]), true), (json!([local, ours]), false)] {
        let replies = vec![
            json!({}),
            json!({}),
            json!({"thread": {"id": "th", "environments": [local, ours]}}),
            json!({"turn": {"id": "tu"}}),
            json!({"thread": {"environments": environments}}),
        ];
        let (requests, running, events) =
            handshake(offline_turn(Some("th")), Some(sandbox()), replies).await;
        assert_eq!(
            methods(&requests),
            [
                "initialize",
                "environment/add",
                "thread/resume",
                "turn/start",
                "thread/read"
            ]
        );
        assert!(requests[2]["params"].get("environments").is_none());
        assert_eq!(requests[3]["params"]["environments"], json!([ours]));
        assert_eq!(requests[4]["params"], json!({"threadId": "th"}));
        assert_eq!(running, confirmed);
        assert_eq!(refused(&events), !confirmed);
    }
}

#[tokio::test]
async fn network_allowed_turn_skips_the_loopback_sandbox() {
    let mut t = turn(None, false);
    t.mcp = None;
    let replies = vec![
        json!({}),
        json!({"thread": {"id": "th"}}),
        json!({"turn": {"id": "tu"}}),
    ];
    let (requests, running, _) = handshake(t, None, replies).await;
    assert_eq!(
        methods(&requests),
        ["initialize", "thread/start", "turn/start"]
    );
    assert!(requests[1]["params"].get("environments").is_none());
    assert!(requests[2]["params"].get("environments").is_none());
    assert!(running);
}

/// Developer instructions outlive the turn on thread/resume, so the offline note rides in
/// the denied turn's input and a later allowed turn on the same thread supersedes it.
#[cfg(target_os = "linux")]
#[tokio::test]
async fn thread_started_offline_and_resumed_with_network_is_told_network_is_back() {
    let ours = json!({"environmentId": "aether-loopback", "cwd": "/w"});
    let replies = vec![
        json!({}),
        json!({}),
        json!({"thread": {"id": "th", "environments": [ours]}}),
        json!({"turn": {"id": "tu"}}),
    ];
    let (requests, running, _) = handshake(offline_turn(None), Some(sandbox()), replies).await;
    assert!(running);
    assert_eq!(requests[2]["method"], "thread/start");
    assert_eq!(requests[2]["params"]["developerInstructions"], "# Aether");
    let offline_input = requests[3]["params"]["input"].clone();
    assert_eq!(
        offline_input[0]["text"],
        super::super::LOOPBACK_INSTRUCTIONS
    );

    // thread/resume returns each turn's user inputs, oldest first.
    let mut history = vec![offline_input];
    for expect_note in [true, false] {
        // Ungated: a gated network-allowed turn sends its own note instead.
        let mut t = turn(Some("th"), true);
        t.mcp = None;
        let turns: Vec<_> = history
            .iter()
            .map(|input| json!({"items": [{"type": "userMessage", "content": input}]}))
            .collect();
        let replies = vec![
            json!({}),
            json!({"thread": {"id": "th", "turns": turns}}),
            json!({"turn": {"id": "tu"}}),
        ];
        let (requests, running, _) = handshake(t, None, replies).await;
        assert!(running);
        assert_eq!(
            methods(&requests),
            ["initialize", "thread/resume", "turn/start"]
        );
        let input = requests[2]["params"]["input"].clone();
        let prompt = json!({"type": "text", "text": "do it"});
        if expect_note {
            let note = json!({"type": "text", "text": super::super::NETWORK_RESTORED_INSTRUCTIONS});
            assert_eq!(input, json!([note, prompt]));
        } else {
            // Already superseded: later allowed turns are unchanged.
            assert_eq!(input, json!([prompt]));
        }
        history.push(input);
    }
}

/// Codex only tells the model that commands lost their transport, so a namespace
/// that dies mid-turn must fail the turn itself.
#[cfg(target_os = "linux")]
#[tokio::test]
async fn sandbox_exit_mid_turn_fails_the_turn() {
    let ours = json!({"environmentId": "aether-loopback", "cwd": "/w"});
    let replies = [
        json!({"id": 1, "result": {}}),
        json!({"id": 2, "result": {}}),
        json!({"id": 3, "result": {"thread": {"id": "th", "environments": [ours]}}}),
        json!({"id": 4, "result": {"turn": {"id": "tu"}}}),
    ];
    let mut child = tokio::process::Command::new("sh")
        .args([
            "-c",
            // No exec: the shell keeps stdout open, as a live app-server does.
            r#"for reply in "$@"; do read -r _; printf '%s\n' "$reply"; done; cat >/dev/null"#,
            "sh",
        ])
        .args(replies.iter().map(ToString::to_string))
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .kill_on_drop(true)
        .process_group(0)
        .spawn()
        .unwrap();
    let stdout = child.stdout.take().unwrap();
    let (tx, mut rx) = tokio::sync::mpsc::channel(16);
    let mut driver = Driver::new(
        child.stdin.take().unwrap(),
        tx,
        offline_turn(None),
        Some(sandbox()),
        Some(EngineVersion::new(0, 162, 0)),
    );
    let (_input, input_rx) = tokio::sync::mpsc::channel(1);
    let (_decisions, decision_rx) = tokio::sync::mpsc::channel(1);
    let (_stop, stop_rx) = tokio::sync::mpsc::channel(1);
    let (exit_tx, exit_rx) = tokio::sync::oneshot::channel::<String>();
    let drive = tokio::spawn(async move {
        driver
            .drive(
                child,
                stdout,
                input_rx,
                decision_rx,
                stop_rx,
                tokio_util::sync::CancellationToken::new(),
                tokio::spawn(async { crate::output::StderrCapture::default() }),
                async { exit_rx.await.unwrap() },
            )
            .await;
    });
    // The thread's session event precedes turn/start; give its reply time to land.
    while !matches!(rx.recv().await, Some(EngineEvent::Session { .. })) {}
    tokio::time::sleep(std::time::Duration::from_millis(500)).await;
    exit_tx.send("signal: 9 (SIGKILL)".into()).unwrap();
    let outcome = tokio::time::timeout(std::time::Duration::from_secs(10), async {
        loop {
            match rx.recv().await {
                Some(EngineEvent::Done(outcome)) => break outcome,
                Some(_) => {}
                None => panic!("the driver ended without an outcome"),
            }
        }
    })
    .await
    .expect("a lost sandbox must end the turn");
    assert!(outcome.is_error);
    assert!(!outcome.closing_accounting);
    let error = outcome.error.unwrap();
    assert!(error.contains("sandbox exited mid-turn"), "{error}");
    assert!(error.contains("SIGKILL"), "{error}");
    assert!(!outcome.prompt_not_submitted);
    tokio::time::timeout(std::time::Duration::from_secs(5), drive)
        .await
        .expect("the driver must stop once the sandbox is gone")
        .unwrap();
}

#[tokio::test]
async fn a_refused_thread_resume_proves_prompt_was_not_submitted() {
    let mut child = tokio::process::Command::new("sh")
        .args(["-c", "cat >/dev/null"])
        .stdin(std::process::Stdio::piped())
        .kill_on_drop(true)
        .spawn()
        .unwrap();
    let (tx, mut rx) = tokio::sync::mpsc::channel(8);
    let mut driver = Driver::new(
        child.stdin.take().unwrap(),
        tx,
        turn(Some("retained"), false),
        None,
        Some(EngineVersion::new(0, 162, 0)),
    );
    driver
        .request(Sent::ThreadResume, "thread/resume", json!({}))
        .await;
    let refusal = json!({ "error": { "message": "getaddrinfo EAI_AGAIN chatgpt.com" } });
    driver.response(&json!(1), &refusal).await;
    let Some(EngineEvent::Done(outcome)) = rx.recv().await else {
        panic!("expected failure")
    };
    assert!(outcome.session_lost);
    assert!(outcome.prompt_not_submitted);
    drop(driver);
    child.wait().await.unwrap();
}
