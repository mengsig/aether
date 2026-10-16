#![allow(clippy::expect_used, clippy::unwrap_used, clippy::panic)]

use aether_core::{
    agent::Agent,
    id::ApprovalId,
    tool::{ApprovalDecision, ApprovalRequest},
};
use aether_protocol::{ClientFrame, Command, ErrorCode, ErrorReply, Reply, ServerFrame, Welcome};
use serde_json::{Value, json};
use std::{process::Output, time::Duration};
use tokio::{
    io::{AsyncBufReadExt, AsyncWriteExt, BufReader},
    net::UnixListener,
};

const ROOT: &str = "agt_01J00000000000000000000001";
const CHILD: &str = "agt_01J00000000000000000000002";
const OTHER: &str = "agt_01J00000000000000000000003";
const RUN: &str = "run_01J00000000000000000000004";
const AT: &str = "2026-09-11T03:00:00Z";

fn agents() -> Vec<Agent> {
    [ROOT, CHILD, OTHER]
        .into_iter()
        .map(|id| {
            serde_json::from_value(json!({
        "id": id, "name": "worker", "role": "worker", "scope": {},
        "model": {"provider":"mock", "model":"m1"}, "status":{"state":"ready"},
        "parent": if id == CHILD { Some(ROOT) } else { None }, "config": {},
        "run": if id == OTHER { "run_01J00000000000000000000005" } else { RUN },
        "stats":{"usage":{"input_tokens":0,"output_tokens":0,"cache_read_tokens":0,
            "cache_write_tokens":0,"cost_usd":0.0},"turns":0}, "created_at":AT,"updated_at":AT
    })).unwrap()
        })
        .collect()
}

fn requests() -> Vec<ApprovalRequest> {
    [ROOT, CHILD, OTHER]
        .into_iter()
        .enumerate()
        .map(|(index, agent)| {
            serde_json::from_value(json!({
                "id":format!("apr_01J0000000000000000000000{}", index + 1),
                "agent_id":agent, "call_id":"call_01J00000000000000000000001", "tool":"shell", "description":"execute command",
                "argument":"echo hello", "resources":[], "side_effect":"external",
                "requested_at":AT, "status":{"state":"pending"}
            }))
            .unwrap()
        })
        .collect()
}

#[derive(Default)]
struct Fixture {
    empty: bool,
    error: Option<ErrorCode>,
}

fn response(
    command: Command,
    fixture: &Fixture,
    seen: &mut Vec<(ApprovalId, ApprovalDecision)>,
) -> Result<Reply, ErrorReply> {
    match command {
        Command::Hello(_) => Ok(Reply::Welcome(Welcome {
            protocol_version: aether_protocol::PROTOCOL_VERSION,
            daemon_version: "stub".into(),
            instance_id: "stub".into(),
            schema_version: 1,
            pid: std::process::id(),
            features: vec![],
            daemon_protocol_version: None,
            min_protocol_version: None,
            idle_timeout_secs: None,
            device: None,
            submission_horizon: None,
        })),
        Command::ListAgents { query } => {
            assert!(query.include_archived);
            assert!(!query.live_only);
            Ok(Reply::Agents(agents()))
        }
        Command::ListApprovals => Ok(Reply::Approvals(if fixture.empty {
            vec![]
        } else {
            requests()
        })),
        Command::ResolveApproval {
            id,
            decision,
            remember,
            ..
        } => {
            assert!(remember.is_none());
            assert!(!seen.iter().any(|(previous, _)| previous == &id));
            seen.push((id, decision));
            if seen.len() == 1
                && let Some(code) = fixture.error
            {
                return Err(ErrorReply::new(
                    code,
                    format!("approval {id} already resolved"),
                ));
            }
            Ok(Reply::Ok)
        }
        other => panic!("unexpected command: {other:?}"),
    }
}

async fn serve(listener: UnixListener, fixture: Fixture) -> Vec<(ApprovalId, ApprovalDecision)> {
    let Ok(connection) = tokio::time::timeout(Duration::from_secs(2), listener.accept()).await
    else {
        return Vec::new();
    };
    let (stream, _) = connection.unwrap();
    let (read, mut write) = stream.into_split();
    let mut lines = BufReader::new(read).lines();
    let mut seen = Vec::new();
    while let Some(line) = lines.next_line().await.unwrap() {
        let frame: ClientFrame = serde_json::from_str(&line).unwrap();
        let result = response(frame.command, &fixture, &mut seen);
        let reply = ServerFrame::Reply {
            id: frame.id,
            result,
        };
        write
            .write_all(aether_protocol::encode(&reply).unwrap().as_bytes())
            .await
            .unwrap();
    }
    seen
}

async fn run(
    args: &[&str],
    input: &str,
    fixture: Fixture,
) -> (Output, Vec<(ApprovalId, ApprovalDecision)>) {
    let root = tempfile::tempdir().unwrap();
    let socket = root.path().join("a.sock");
    let listener = UnixListener::bind(&socket).unwrap();
    let server = tokio::spawn(serve(listener, fixture));
    let mut child = tokio::process::Command::new(env!("CARGO_BIN_EXE_aether"))
        .args(args)
        .arg("--socket")
        .arg(socket)
        .env("AETHER_CONFIG_DIR", root.path().join("config"))
        .env("AETHER_DATA_DIR", root.path().join("data"))
        .env_remove("AETHER_TCP_ADDR")
        .env_remove("AETHER_MCP_TOKEN")
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .kill_on_drop(true)
        .spawn()
        .unwrap();
    child
        .stdin
        .take()
        .unwrap()
        .write_all(input.as_bytes())
        .await
        .unwrap();
    tokio::time::timeout(Duration::from_secs(5), async {
        let output = child.wait_with_output().await.unwrap();
        let seen = server.await.unwrap();
        (output, seen)
    })
    .await
    .expect("CLI or fixture hung")
}

fn stdout(output: &Output) -> String {
    String::from_utf8(output.stdout.clone()).unwrap()
}
fn stderr(output: &Output) -> String {
    String::from_utf8(output.stderr.clone()).unwrap()
}

#[tokio::test]
async fn lists_all_pending_columns_and_stable_json() {
    let (out, seen) = run(&["approvals"], "", Fixture::default()).await;
    assert!(out.status.success(), "{}", stderr(&out));
    assert!(seen.is_empty());
    for value in [
        "agent",
        "tool",
        "arguments",
        "side effect",
        "age",
        ROOT,
        CHILD,
        OTHER,
        "shell",
        "echo hello",
        "external",
    ] {
        assert!(stdout(&out).contains(value), "missing {value}");
    }
    let (out, _) = run(&["approvals", "--json"], "", Fixture::default()).await;
    assert!(out.status.success(), "{}", stderr(&out));
    assert_eq!(
        serde_json::from_slice::<Value>(&out.stdout).unwrap(),
        serde_json::to_value(requests()).unwrap()
    );
}

#[tokio::test]
async fn scopes_listing_and_batches_to_agent_subtree_and_run() {
    for (flag, id, count) in [
        ("--agent", CHILD, 1),
        ("--subtree", ROOT, 2),
        ("--run", RUN, 2),
    ] {
        let (out, _) = run(&["approvals", flag, id, "--json"], "", Fixture::default()).await;
        let listed: Vec<ApprovalRequest> = serde_json::from_slice(&out.stdout).unwrap();
        assert_eq!(listed.len(), count);
        for verb in ["approve", "deny"] {
            let (out, seen) = run(
                &[verb, "--all-pending", flag, id, "--yes"],
                "",
                Fixture::default(),
            )
            .await;
            assert!(out.status.success(), "{}", stderr(&out));
            assert_eq!(
                seen.iter().map(|(id, _)| *id).collect::<Vec<_>>(),
                listed.iter().map(|r| r.id).collect::<Vec<_>>()
            );
            assert!(seen.iter().all(|(_, d)| *d
                == if verb == "approve" {
                    ApprovalDecision::Approved
                } else {
                    ApprovalDecision::Denied
                }));
            assert!(stdout(&out).contains(&format!("{count} pending approvals")));
        }
    }
}

#[tokio::test]
async fn refuses_unscoped_or_ambiguous_batches_before_connecting() {
    for args in [
        vec!["approve", "--all-pending"],
        vec!["deny", "--all-pending", "--everywhere"],
        vec!["approve", "--all-pending", "--agent", ROOT, "--run", RUN],
        vec!["approve", "--all-pending", "--agent", ROOT, "--remember"],
    ] {
        let (out, seen) = run(&args, "", Fixture::default()).await;
        assert!(!out.status.success());
        assert!(seen.is_empty());
    }
}

#[tokio::test]
async fn confirms_snapshot_count_and_defaults_to_no() {
    for input in ["", "no\n", "yes\n"] {
        let (out, seen) = run(
            &["approve", "--all-pending", "--run", RUN],
            input,
            Fixture::default(),
        )
        .await;
        assert!(stderr(&out).contains("Approve 2 pending approvals? [y/N]"));
        assert_eq!(out.status.success(), input == "yes\n");
        assert_eq!(seen.len(), if input == "yes\n" { 2 } else { 0 });
    }
}

#[tokio::test]
async fn concurrent_resolution_continues_but_other_errors_fail() {
    for json in [false, true] {
        let mut args = vec!["approve", "--all-pending", "--run", RUN, "--yes"];
        if json {
            args.push("--json");
        }
        let (out, seen) = run(
            &args,
            "",
            Fixture {
                error: Some(ErrorCode::InvalidArgument),
                ..Default::default()
            },
        )
        .await;
        assert!(out.status.success(), "{}", stderr(&out));
        assert_eq!(seen.len(), 2);
        if json {
            assert_eq!(
                serde_json::from_slice::<Value>(&out.stdout).unwrap(),
                json!({"decision":"approved", "selected":2, "answered":1, "already_resolved":[requests()[0].id]})
            );
        } else {
            assert!(stdout(&out).contains("approved 1 pending approvals; 1 already resolved"));
        }
    }
    let (out, seen) = run(
        &["deny", "--all-pending", "--run", RUN, "--yes"],
        "",
        Fixture {
            error: Some(ErrorCode::Forbidden),
            ..Default::default()
        },
    )
    .await;
    assert!(!out.status.success());
    assert_eq!(seen.len(), 1);
    assert!(stderr(&out).contains("Forbidden"));
}

#[tokio::test]
async fn empty_batch_does_not_prompt() {
    let (out, seen) = run(
        &["deny", "--all-pending", "--run", RUN],
        "",
        Fixture {
            empty: true,
            ..Default::default()
        },
    )
    .await;
    assert!(out.status.success(), "{}", stderr(&out));
    assert!(seen.is_empty());
    assert!(!stderr(&out).contains("[y/N]"));
    assert!(stdout(&out).contains("denied 0 pending approvals"));
}

#[tokio::test]
async fn single_approval_still_resolves_one_id() {
    let id = requests()[0].id.to_string();
    let (out, seen) = run(&["approve", &id], "", Fixture::default()).await;
    assert!(out.status.success(), "{}", stderr(&out));
    assert_eq!(seen, vec![(requests()[0].id, ApprovalDecision::Approved)]);
}
