use super::*;
use serde_json::{Value, json};

const FEEDBACK: &str = "mfb_01J00000000000000000000005";
const INVOCATION: &str = "inv_01J00000000000000000000006";

fn record(retracted: bool) -> Value {
    let mut value = json!({"id":FEEDBACK, "memory":MEMORY, "task":TASK,
        "verdict":"helped", "evidence":"review", "reason":"Release checked",
        "actor":{"actor":"user"}, "created_at":"2026-10-15T09:00:00Z"});
    if retracted {
        value["retracted_at"] = json!("2026-10-15T10:00:00Z");
        value["retraction_reason"] = json!("Review withdrawn");
    }
    value
}

async fn serve(listener: UnixListener, expected: Value, reply: Reply) {
    let (stream, _) = listener.accept().await.unwrap();
    let (read, mut write) = stream.into_split();
    let mut lines = BufReader::new(read).lines();
    let hello: ClientFrame =
        serde_json::from_str(&next_request(&mut lines).await.unwrap()).unwrap();
    assert!(matches!(hello.command, Command::Hello(_)));
    let welcome = serde_json::from_value(json!({"reply":"welcome", "data":{
        "protocol_version":aether_protocol::PROTOCOL_VERSION, "daemon_version":"stub",
        "instance_id":"stub", "schema_version":22, "pid":std::process::id(),
        "features":["memory_feedback"],
    }}))
    .unwrap();
    write_frame(
        &mut write,
        ServerFrame::Reply {
            id: hello.id,
            result: Ok(welcome),
        },
    )
    .await;
    let frame: ClientFrame =
        serde_json::from_str(&next_request(&mut lines).await.unwrap()).unwrap();
    assert_eq!(serde_json::to_value(frame.command).unwrap(), expected);
    write_frame(
        &mut write,
        ServerFrame::Reply {
            id: frame.id,
            result: Ok(reply),
        },
    )
    .await;
}

async fn exchange(args: &[&str], expected: Value, kind: &str, data: Value) -> Output {
    let root = tempfile::tempdir().unwrap();
    let listener = UnixListener::bind(root.path().join("a.sock")).unwrap();
    let reply = serde_json::from_value(json!({"reply":kind, "data":data})).unwrap();
    let server = tokio::spawn(serve(listener, expected, reply));
    let mut command = process::Command::from(isolated_command(root.path(), args));
    command.kill_on_drop(true);
    let output = tokio::time::timeout(Duration::from_secs(30), command.output())
        .await
        .unwrap()
        .unwrap();
    server.await.unwrap();
    assert!(output.status.success(), "{}", stderr(&output));
    assert!(output.stderr.is_empty());
    output
}

#[tokio::test]
async fn feedback_cli_sends_the_required_evidence_and_optional_invocation() {
    let mut expected = json!({"cmd":"record_memory_feedback", "memory":MEMORY, "task":TASK,
        "verdict":"helped", "evidence":"review", "reason":"Release checked"});
    let mut data = record(false);
    for invocation in [None, Some(INVOCATION)] {
        let mut args = vec![
            "memory",
            "feedback",
            "record",
            MEMORY,
            "--task",
            TASK,
            "--verdict",
            "helped",
            "--evidence",
            "review",
            "--reason",
            "Release checked",
            "--json",
        ];
        if let Some(invocation) = invocation {
            args.extend(["--invocation", invocation]);
            expected["invocation"] = json!(invocation);
            data["invocation"] = json!(invocation);
        }
        let output = exchange(&args, expected.clone(), "memory_feedback", data.clone()).await;
        let received: Value = serde_json::from_str(&stdout(&output)).unwrap();
        assert_eq!(received, data);
        assert_eq!(received["memory"], MEMORY);
    }
}

#[tokio::test]
async fn feedback_cli_retracts_and_lists_with_retraction_status() {
    let data = record(true);
    let retract =
        json!({"cmd":"retract_memory_feedback", "feedback":FEEDBACK, "reason":"Review withdrawn"});
    let output = exchange(
        &[
            "memory",
            "feedback",
            "retract",
            FEEDBACK,
            "--reason",
            "Review withdrawn",
            "--json",
        ],
        retract,
        "memory_feedback",
        data.clone(),
    )
    .await;
    assert_eq!(
        serde_json::from_str::<Value>(&stdout(&output)).unwrap(),
        data
    );
    for json_output in [false, true] {
        let mut args = vec!["memory", "feedback", "list", MEMORY];
        if json_output {
            args.push("--json");
        }
        let ranking = json!({"helped":3, "hurt":1, "factor":0.75 + 0.5 * 4.0 / 6.0});
        let output = exchange(
            &args,
            json!({"cmd":"memory_feedback", "memory":MEMORY}),
            "memory_feedback_list",
            json!({"records":[data], "truncated":false, "ranking":ranking}),
        )
        .await;
        if json_output {
            let printed = serde_json::from_str::<Value>(&stdout(&output)).unwrap();
            assert_eq!(printed["records"][0], data);
            assert_eq!(printed["ranking"]["helped"], 3);
            assert_eq!(printed["ranking"]["hurt"], 1);
        } else {
            assert!(stdout(&output).contains("(retracted)"));
            assert!(stdout(&output).contains(FEEDBACK));
            assert!(
                stdout(&output).contains(
                    "ranking factor 1.08 (3 helped, 1 hurt across the latest 20 tasks)\n"
                ),
                "{}",
                stdout(&output)
            );
        }
    }
}

#[tokio::test]
async fn feedback_cli_lists_an_older_daemons_reply_as_unranked() {
    let output = exchange(
        &["memory", "feedback", "list", MEMORY],
        json!({"cmd":"memory_feedback", "memory":MEMORY}),
        "memory_feedback_list",
        json!({"records":[record(false)], "truncated":false}),
    )
    .await;
    assert!(stdout(&output).contains(FEEDBACK));
    assert!(
        stdout(&output).contains(
            "ranking factor 1.00 (0 helped, 0 hurt across the latest 20 tasks; 2 needed to move recall)"
        ),
        "{}",
        stdout(&output)
    );
}

#[tokio::test]
async fn feedback_cli_lists_no_feedback_with_its_neutral_ranking() {
    let output = exchange(
        &["memory", "feedback", "list", MEMORY],
        json!({"cmd":"memory_feedback", "memory":MEMORY}),
        "memory_feedback_list",
        json!({"records":[], "truncated":false,
            "ranking":{"helped":0, "hurt":0, "factor":1.0}}),
    )
    .await;
    assert_eq!(
        stdout(&output),
        format!(
            "no feedback for {MEMORY}.\nranking factor 1.00 (0 helped, 0 hurt across the latest 20 tasks; 2 needed to move recall)\n"
        )
    );
}

#[tokio::test]
async fn feedback_cli_requires_support_and_rejects_invalid_arguments_before_connecting() {
    let old = cli(&["memory", "feedback", "list", MEMORY], Stub::default()).await;
    assert_eq!(old.status.code(), Some(1));
    assert!(stderr(&old).contains("memory feedback"));
    let root = tempfile::tempdir().unwrap();
    let valid = vec![
        "memory",
        "feedback",
        "record",
        MEMORY,
        "--task",
        TASK,
        "--verdict",
        "helped",
        "--evidence",
        "review",
        "--reason",
        "Release checked",
    ];
    for (index, value) in [(5, "bad-task"), (7, "good"), (9, "agent")] {
        let mut args = valid.clone();
        args[index] = value;
        let output = isolated_command(root.path(), &args).output().unwrap();
        assert_eq!(
            output.status.code(),
            Some(2),
            "{args:?}: {}",
            stderr(&output)
        );
    }
    let output = isolated_command(root.path(), &["memory", "feedback", "record", MEMORY])
        .output()
        .unwrap();
    assert_eq!(output.status.code(), Some(2));
}
