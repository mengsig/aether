use super::*;
use aether_core::notification::{Notification, NotificationState};
use serde_json::Value;

const UNREAD: &str = "ntf_01J00000000000000000000010";
const READ: &str = "ntf_01J00000000000000000000011";
const RESOLVED: &str = "ntf_01J00000000000000000000012";
const UNKNOWN: &str = "ntf_01J00000000000000000000099";

fn notification(id: &str, kind: &str, state: &str, title: &str) -> Notification {
    serde_json::from_value(
        json!({"id":id, "source":"archon", "kind":kind, "title":title,
        "state":state, "created_at":"2026-10-15T09:00:00Z", "updated_at":AT}),
    )
    .unwrap()
}

fn center() -> Vec<Notification> {
    vec![
        notification(UNREAD, "question", "unread", "Merge #231?"),
        notification(READ, "info", "read", "Build finished"),
        notification(RESOLVED, "outcome", "resolved", "Shipped #229"),
    ]
}

fn answer(command: Command, center: &mut [Notification]) -> Result<Reply, ErrorReply> {
    match command {
        Command::ListNotifications { states, .. } => Ok(Reply::Notifications(
            center
                .iter()
                .filter(|n| states.is_empty() || states.contains(&n.state))
                .cloned()
                .collect(),
        )),
        Command::ResolveNotification { id: Some(id), .. } => {
            let found = center.iter_mut().find(|n| n.id == id);
            let n = found
                .ok_or_else(|| ErrorReply::new(ErrorCode::NotFound, "unknown notification"))?;
            n.state = NotificationState::Resolved;
            Ok(Reply::Notification(n.clone()))
        }
        other => panic!("unexpected command {other:?}"),
    }
}

async fn serve(
    listener: UnixListener,
    features: &'static [&'static str],
    mut center: Vec<Notification>,
) {
    let (stream, _) = listener.accept().await.unwrap();
    let (read, mut write) = stream.into_split();
    let mut lines = BufReader::new(read).lines();
    while let Some(line) = next_request(&mut lines).await {
        let frame: ClientFrame = serde_json::from_str(&line).unwrap();
        let result = match frame.command {
            Command::Hello(_) => Ok(serde_json::from_value(json!({"reply":"welcome", "data":{
                "protocol_version":aether_protocol::PROTOCOL_VERSION, "daemon_version":"stub",
                "instance_id":"stub", "schema_version":22, "pid":std::process::id(),
                "features":features,
            }}))
            .unwrap()),
            command => answer(command, &mut center),
        };
        write_frame(
            &mut write,
            ServerFrame::Reply {
                id: frame.id,
                result,
            },
        )
        .await;
    }
}

async fn run(args: &[&str], features: &'static [&'static str]) -> Output {
    run_with(args, features, center()).await
}

async fn run_with(
    args: &[&str],
    features: &'static [&'static str],
    center: Vec<Notification>,
) -> Output {
    let root = tempfile::tempdir().unwrap();
    let listener = UnixListener::bind(root.path().join("a.sock")).unwrap();
    let server = tokio::spawn(serve(listener, features, center));
    let mut command = process::Command::from(isolated_command(root.path(), args));
    command.kill_on_drop(true);
    let output = tokio::time::timeout(Duration::from_secs(30), command.output())
        .await
        .unwrap()
        .unwrap();
    server.await.unwrap();
    output
}

async fn run_ok(args: &[&str]) -> String {
    let output = run(args, &["notifications"]).await;
    assert!(output.status.success(), "{}", stderr(&output));
    stdout(&output)
}

#[tokio::test]
async fn list_shows_open_notifications_one_line_each() {
    let out = run_ok(&["notify", "list"]).await;
    let lines: Vec<&str> = out.lines().collect();
    assert_eq!(lines.len(), 2, "{out}");
    assert!(
        lines[0].starts_with(UNREAD) && lines[0].contains("question"),
        "{out}"
    );
    assert!(
        lines[0].ends_with("Merge #231?") && lines[0].contains('d'),
        "{out}"
    );
    assert!(
        lines[1].starts_with(READ) && lines[1].ends_with("Build finished"),
        "{out}"
    );
}

#[tokio::test]
async fn list_all_includes_resolved_ones() {
    let out = run_ok(&["notify", "list", "--all"]).await;
    assert_eq!(out.lines().count(), 3, "{out}");
    assert!(out.contains(RESOLVED), "{out}");
}

#[tokio::test]
async fn list_json_prints_the_protocol_records() {
    let out = run_ok(&["notify", "list", "--json"]).await;
    let records: Vec<Notification> = serde_json::from_str(&out).unwrap();
    assert_eq!(records, center()[..2]);
    let all: Value =
        serde_json::from_str(&run_ok(&["notify", "list", "--all", "--json"]).await).unwrap();
    assert_eq!(all.as_array().unwrap().len(), 3);
}

#[tokio::test]
async fn resolve_one_marks_it_resolved() {
    assert_eq!(
        run_ok(&["notify", "resolve", UNREAD]).await,
        format!("resolved {UNREAD}\n")
    );
    let json = run_ok(&["notify", "resolve", READ, "--json"]).await;
    let records: Vec<Notification> = serde_json::from_str(&json).unwrap();
    assert_eq!(records.len(), 1);
    assert_eq!(records[0].state, NotificationState::Resolved);
}

#[tokio::test]
async fn resolve_all_counts_the_open_ones() {
    assert_eq!(
        run_ok(&["notify", "resolve", "--all"]).await,
        "resolved 2\n"
    );
}

#[tokio::test]
async fn resolving_an_unknown_id_fails_with_a_clear_error() {
    let output = run(&["notify", "resolve", UNKNOWN], &["notifications"]).await;
    assert!(!output.status.success());
    assert!(stderr(&output).contains(UNKNOWN), "{}", stderr(&output));
    assert!(
        stderr(&output).contains("unknown notification"),
        "{}",
        stderr(&output)
    );
}

#[tokio::test]
async fn a_daemon_without_the_center_is_refused() {
    let output = run(&["notify", "list"], &[]).await;
    assert!(!output.status.success());
    assert!(
        stderr(&output).contains("no notification center"),
        "{}",
        stderr(&output)
    );
}

#[test]
fn resolve_needs_an_id_or_all_but_not_both() {
    for args in [
        &["notify", "resolve"][..],
        &["notify", "resolve", UNREAD, "--all"],
    ] {
        let output = isolated_command(std::path::Path::new("/nonexistent"), args)
            .output()
            .unwrap();
        assert_eq!(output.status.code(), Some(2), "{}", stderr(&output));
    }
}

#[tokio::test]
async fn resolve_all_prints_a_count_for_none_or_one() {
    let open = |n: usize| center().into_iter().take(n).collect::<Vec<_>>();
    for (center, expected) in [(open(0), "resolved 0\n"), (open(1), "resolved 1\n")] {
        let output = run_with(&["notify", "resolve", "--all"], &["notifications"], center).await;
        assert!(output.status.success(), "{}", stderr(&output));
        assert_eq!(stdout(&output), expected);
    }
}
