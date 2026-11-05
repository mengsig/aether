//! `aether memory export|import|migration-status` against a stub daemon: the
//! command each sends, what it prints, and its refusal of an older daemon.
use super::*;
use serde_json::{Value, json};

const DIGEST: &str = "5f1e0c2a";

/// A daemon that welcomes with `features` and answers one command, if the
/// CLI sends one, with `reply`.
async fn serve(listener: UnixListener, features: Value, exchange: Option<(Value, Reply)>) {
    let (stream, _) = listener.accept().await.unwrap();
    let (read, mut write) = stream.into_split();
    let mut lines = BufReader::new(read).lines();
    let hello: ClientFrame =
        serde_json::from_str(&next_request(&mut lines).await.unwrap()).unwrap();
    let Command::Hello(greeting) = &hello.command else {
        panic!("the CLI greets first");
    };
    assert!(greeting.features.iter().any(|f| f == "memory_v2"));
    let welcome = serde_json::from_value(json!({"reply":"welcome", "data":{
        "protocol_version":aether_protocol::PROTOCOL_VERSION, "daemon_version":"stub",
        "instance_id":"stub", "schema_version":32, "pid":std::process::id(),
        "features":features,
    }}))
    .unwrap();
    let reply = |id, result| ServerFrame::Reply { id, result };
    write_frame(&mut write, reply(hello.id, Ok(welcome))).await;
    let Some((expected, answer)) = exchange else {
        return;
    };
    let frame: ClientFrame =
        serde_json::from_str(&next_request(&mut lines).await.unwrap()).unwrap();
    assert_eq!(serde_json::to_value(frame.command).unwrap(), expected);
    write_frame(&mut write, reply(frame.id, Ok(answer))).await;
}

async fn run(
    root: &std::path::Path,
    args: &[&str],
    features: Value,
    exchange: Option<(Value, &str, Value)>,
) -> Output {
    let listener = UnixListener::bind(root.join("a.sock")).unwrap();
    let exchange = exchange.map(|(expected, kind, data)| {
        let reply = serde_json::from_value(json!({"reply":kind, "data":data})).unwrap();
        (expected, reply)
    });
    let server = tokio::spawn(serve(listener, features, exchange));
    let mut command = process::Command::from(isolated_command(root, args));
    command.kill_on_drop(true);
    let output = tokio::time::timeout(Duration::from_secs(30), command.output())
        .await
        .unwrap()
        .unwrap();
    server.await.unwrap();
    std::fs::remove_file(root.join("a.sock")).unwrap();
    output
}

fn artifact() -> String {
    let header = json!({"format":"aether.memory-export", "version":1,
        "exported_at":"2026-11-05T08:00:00Z", "schema":32, "records":0});
    format!("{header}\n")
}

#[tokio::test]
async fn export_writes_the_artifact_whole_and_import_sends_it_back_as_a_dry_run() {
    let root = tempfile::tempdir().unwrap();
    let file = root.path().join("memories.jsonl");
    let path = file.to_str().unwrap();
    let export = json!({"artifact":artifact(), "records":0, "digest":DIGEST});
    let output = run(
        root.path(),
        &[
            "memory", "export", "--scope", "global", "-o", path, "--json",
        ],
        json!(["memory_v2"]),
        Some((
            json!({"cmd":"memory_export", "scope":{"scope":"global"}}),
            "memory_export",
            export,
        )),
    )
    .await;
    assert!(output.status.success(), "{}", stderr(&output));
    assert_eq!(std::fs::read_to_string(&file).unwrap(), artifact());
    let printed: Value = serde_json::from_str(&stdout(&output)).unwrap();
    assert_eq!(
        (printed["records"].clone(), printed["digest"].clone()),
        (json!(0), json!(DIGEST))
    );
    assert!(!root.path().join("memories.jsonl.partial").exists());

    let report = json!({"dry_run":true, "digest":DIGEST, "records":0, "inserted":0,
        "unchanged":0, "conflicts":[]});
    let output = run(
        root.path(),
        &["memory", "import", path, "--dry-run"],
        json!(["memory_v2"]),
        Some((
            json!({"cmd":"memory_import", "artifact":artifact(), "dry_run":true}),
            "memory_import",
            report,
        )),
    )
    .await;
    assert!(output.status.success(), "{}", stderr(&output));
    assert_eq!(
        stdout(&output).trim(),
        "would insert 0 of 0; 0 unchanged; 0 conflicts"
    );
}

#[tokio::test]
async fn an_older_daemon_without_memory_v2_is_refused_before_any_command_is_sent() {
    let root = tempfile::tempdir().unwrap();
    for args in [
        &["memory", "migration-status", "--json"][..],
        &["memory", "consolidate", "--dry-run"],
        &[
            "memory",
            "explain",
            "--invocation",
            "inv_01J00000000000000000000006",
        ],
    ] {
        let output = run(root.path(), args, json!(["memory_feedback"]), None).await;
        assert!(!output.status.success(), "{args:?}");
        assert!(
            stderr(&output).contains("Memory 2.0"),
            "{args:?}: {}",
            stderr(&output)
        );
    }
}

#[tokio::test]
async fn migration_status_prints_the_daemons_report_as_json() {
    let root = tempfile::tempdir().unwrap();
    let status = json!({"schema_version":32,
        "migrations":[{"version":32, "name":"memory 2.0 slot", "applied":true}],
        "memories":{"total":2, "admitted":1, "implicit":1, "candidates":0, "stale":0, "held_out":0},
        "backups":[]});
    let output = run(
        root.path(),
        &["memory", "migration-status", "--json"],
        json!(["memory_v2"]),
        Some((
            json!({"cmd":"memory_migration_status"}),
            "memory_migration_status",
            status.clone(),
        )),
    )
    .await;
    assert!(output.status.success(), "{}", stderr(&output));
    assert_eq!(
        serde_json::from_str::<Value>(&stdout(&output)).unwrap(),
        status
    );
}
