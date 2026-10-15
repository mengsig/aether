use super::*;
use std::time::Duration;

/// A `codex app-server` stand-in for one turn: answers the handshake,
/// refuses to start unless the thread carries `expected_policy`, raises
/// one prompt-rule approval and completes the turn on the answer.
fn fake_app_server(expected_policy: &str) -> tempfile::TempDir {
    let dir = tempfile::tempdir().unwrap();
    let script = format!(
        r#"#!/bin/sh
read -r init
echo '{{"jsonrpc":"2.0","id":1,"result":{{}}}}'
read -r start
case "$start" in
  *'"approvalPolicy":"{expected_policy}"'*) ;;
  *) echo '{{"jsonrpc":"2.0","id":2,"error":{{"code":1,"message":"unexpected approval policy"}}}}'; exit 0 ;;
esac
echo '{{"jsonrpc":"2.0","id":2,"result":{{"thread":{{"id":"t1"}},"model":"m"}}}}'
read -r turn
echo '{{"jsonrpc":"2.0","id":3,"result":{{"turn":{{"id":"u1"}}}}}}'
echo '{{"jsonrpc":"2.0","id":0,"method":"item/commandExecution/requestApproval","params":{{"reason":"`gh pr merge 7` requires approval by policy","command":"gh pr merge 7","cwd":"/w","availableDecisions":["accept","cancel"]}}}}'
read -r decision
case "$decision" in
  *'"decision":"accept"'*) echo '{{"jsonrpc":"2.0","method":"item/completed","params":{{"item":{{"type":"commandExecution","id":"exec-1","command":"gh pr merge 7","status":"completed","aggregatedOutput":"merged","exitCode":0}}}}}}' ;;
  *) echo '{{"jsonrpc":"2.0","method":"item/completed","params":{{"item":{{"type":"commandExecution","id":"exec-1","command":"gh pr merge 7","status":"declined"}}}}}}' ;;
esac
echo '{{"jsonrpc":"2.0","method":"thread/tokenUsage/updated","params":{{"tokenUsage":{{"total":{{"inputTokens":1,"outputTokens":1}},"last":{{"inputTokens":1,"outputTokens":1}}}}}}}}'
echo '{{"jsonrpc":"2.0","method":"turn/completed","params":{{"turn":{{"id":"u1","status":"completed","items":[]}}}}}}'
"#
    );
    crate::test_script::write(&dir.path().join("codex"), &script);
    dir
}

/// Drives one fake-server turn: the prompt reaches the runtime side as a
/// permission event, the decision answers it, and the turn ends. Returns
/// (command result is_error, outcome).
async fn run_prompted_turn(
    bypass: bool,
    allow: bool,
    expected_policy: &str,
) -> (Option<bool>, EngineOutcome) {
    let dir = fake_app_server(expected_policy);
    let engine = CodexAppEngine::new(dir.path().join("codex").to_string_lossy(), "test");
    let mut t = turn(None, bypass);
    t.cwd = dir.path().to_path_buf();
    t.mcp = None;
    let mut run = engine
        .run(t, CancellationToken::new())
        .await
        .expect("spawns");
    let decisions = run
        .decisions
        .take()
        .expect("codex asks through its protocol");
    let mut result_is_error = None;
    let outcome = tokio::time::timeout(Duration::from_secs(20), async {
        loop {
            match run
                .events
                .recv()
                .await
                .expect("done before the channel closes")
            {
                EngineEvent::Permission {
                    request_id,
                    tool,
                    input,
                    ..
                } => {
                    assert_eq!(tool, "bash");
                    assert_eq!(input["command"], "gh pr merge 7");
                    decisions
                        .send(PermissionDecision {
                            request_id,
                            allow,
                            message: None,
                            answer: None,
                        })
                        .await
                        .expect("driver listens");
                }
                EngineEvent::ToolResult { is_error, .. } => result_is_error = Some(is_error),
                EngineEvent::Done(outcome) => break outcome,
                _ => {}
            }
        }
    })
    .await
    .expect("the turn completes");
    (result_is_error, outcome)
}

#[tokio::test]
async fn a_bypass_turn_with_network_lets_aether_grant_a_native_prompt() {
    let (is_error, outcome) = run_prompted_turn(true, true, "on-request").await;
    assert_eq!(is_error, Some(false));
    assert!(!outcome.is_error, "{outcome:?}");
    assert_eq!(outcome.stop_reason.as_deref(), Some("completed"));
}

#[tokio::test]
async fn a_denied_native_prompt_stays_declined() {
    for bypass in [true, false] {
        let (is_error, outcome) = run_prompted_turn(bypass, false, "on-request").await;
        assert_eq!(is_error, Some(true), "bypass={bypass}");
        assert!(
            !outcome.is_error,
            "a refusal ends the turn normally: {outcome:?}"
        );
    }
}

/// The fake refuses any other policy, so a bypass turn that regressed to
/// `never` would fail to start rather than pass by accident.
#[tokio::test]
async fn the_fake_server_rejects_a_never_policy() {
    let dir = fake_app_server("never");
    let engine = CodexAppEngine::new(dir.path().join("codex").to_string_lossy(), "test");
    let mut t = turn(None, true);
    t.cwd = dir.path().to_path_buf();
    t.mcp = None;
    let mut run = engine
        .run(t, CancellationToken::new())
        .await
        .expect("spawns");
    let outcome = tokio::time::timeout(Duration::from_secs(20), async {
        loop {
            if let EngineEvent::Done(outcome) = run.events.recv().await.expect("done") {
                break outcome;
            }
        }
    })
    .await
    .expect("ends");
    assert!(outcome.is_error, "{outcome:?}");
    assert!(
        outcome
            .error
            .as_deref()
            .unwrap_or("")
            .contains("unexpected approval policy"),
        "{outcome:?}"
    );
}

#[test]
fn approvals_round_trip_through_decisions() {
    let mut s = TurnState::new(turn(None, false));
    let reply = s.server_request(
        &json!(0),
        "item/commandExecution/requestApproval",
        &json!({"command": "rm -rf build", "cwd": "/w", "reason": "clean"}),
    );
    match reply {
        ServerReply::Ask(EngineEvent::Permission {
            request_id,
            tool,
            input,
            side_effect,
        }) => {
            assert_eq!(request_id, "0");
            assert_eq!(tool, "bash");
            assert_eq!(input["command"], "rm -rf build");
            assert_eq!(side_effect, Some(SideEffectClass::External));
        }
        other => panic!("{other:?}"),
    }
    let (id, result) = s.decision_result("0", false, None).expect("stored");
    assert_eq!(id, json!(0));
    assert_eq!(result, json!({"decision": "decline"}));
    assert!(
        s.decision_result("0", true, None).is_none(),
        "answered once"
    );

    s.server_request(
        &json!(7),
        "item/permissions/requestApproval",
        &json!({"permissions": {"network": {"enabled": true}}, "reason": "fetch"}),
    );
    let (_, result) = s.decision_result("7", true, None).expect("stored");
    assert_eq!(result["permissions"]["network"]["enabled"], true);
    assert_eq!(result["scope"], "turn");

    // A question becomes a prompt for the person; the reply answers it.
    let reply = s.server_request(
        &json!(8),
        "item/tool/requestUserInput",
        &json!({"isBlocking": true, "questions": [
            {"id": "q1", "header": "Database", "question": "Which database?",
             "options": [{"label": "postgres", "description": "the default"}, {"label": "sqlite", "description": "embedded"}]},
            {"id": "q2", "header": "Region", "question": "Region?"}
        ]}),
    );
    match reply {
        ServerReply::Ask(EngineEvent::Question { request_id, prompt }) => {
            assert_eq!(request_id, "8");
            assert!(prompt.starts_with("Database: Which database?\n- postgres: the default\n- sqlite: embedded\n\nRegion: Region?"), "{prompt}");
        }
        other => panic!("{other:?}"),
    }
    let (_, result) = s
        .decision_result("8", true, Some("sqlite please"))
        .expect("stored");
    assert_eq!(result["answers"]["q1"]["answers"][0], "sqlite please");
    assert_eq!(result["answers"]["q2"]["answers"], json!([]));

    match s.server_request(&json!(9), "mcpServer/elicitation/request", &json!({})) {
        ServerReply::Now(result, _) => assert_eq!(result["action"], "decline"),
        other => panic!("{other:?}"),
    }
    assert!(matches!(
        s.server_request(&json!(10), "something/new", &json!({})),
        ServerReply::Unsupported
    ));
}

/// The request Codex 0.154 raises for an operator `decision="prompt"`
/// rule, verbatim shape: it becomes a permission event the runtime
/// answers, and each answer maps to the native decision.
#[test]
fn native_prompt_rule_requests_route_through_aether_decisions() {
    let request = json!({
        "kind": "command", "threadId": "t1", "turnId": "u1", "itemId": "exec-1",
        "environmentId": "local",
        "reason": "`/usr/bin/zsh -lc gh pr merge 7` requires approval by policy",
        "command": "/usr/bin/zsh -lc gh pr merge 7", "cwd": "/w",
        "commandActions": [{"type": "unknown", "command": "gh pr merge 7"}],
        "availableDecisions": ["accept", "cancel"],
    });
    for (bypass, allow, decision) in [(true, true, "accept"), (false, false, "decline")] {
        let mut s = TurnState::new(turn(None, bypass));
        let reply = s.server_request(&json!(0), "item/commandExecution/requestApproval", &request);
        let ServerReply::Ask(EngineEvent::Permission {
            request_id,
            tool,
            input,
            side_effect,
        }) = reply
        else {
            panic!("{reply:?}");
        };
        assert_eq!((request_id.as_str(), tool.as_str()), ("0", "bash"));
        assert_eq!(input["command"], "/usr/bin/zsh -lc gh pr merge 7");
        assert_eq!(input["tool_use_id"], "exec-1");
        assert_eq!(input["reason"], request["reason"]);
        assert_eq!(side_effect, Some(SideEffectClass::External));
        let (id, result) = s.decision_result("0", allow, None).expect("stored");
        assert_eq!((id, result), (json!(0), json!({ "decision": decision })));
        assert!(
            s.decision_result("0", allow, None).is_none(),
            "answered once"
        );
    }

    // A declined command completes as an error result, never a success.
    let mut s = TurnState::new(turn(None, true));
    s.notification("item/started", &json!({"item": {"type": "commandExecution", "id": "exec-1", "command": "/usr/bin/zsh -lc gh pr merge 7", "cwd": "/w", "status": "inProgress"}}));
    let events = s.notification("item/completed", &json!({"item": {"type": "commandExecution", "id": "exec-1", "command": "/usr/bin/zsh -lc gh pr merge 7", "status": "declined", "aggregatedOutput": null, "exitCode": null}}));
    assert!(
        matches!(&events[..], [EngineEvent::ToolResult { tool_use_id, is_error: true, .. }] if tool_use_id == "exec-1"),
        "{events:?}"
    );
}
