#![allow(clippy::expect_used, clippy::unwrap_used, clippy::panic)]

//! `--plain` driven end to end over a pipe against a mock daemon. This is the
//! gate on the two things the module promises and nothing else exercises: it
//! reconnects when the daemon goes away and comes back (the status line says
//! so, so something has to be doing it), and it leaves on EOF instead of
//! hanging on a client that will never speak again.

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use aether_client::Client;
use aether_core::agent::{Agent, AgentConfig, AgentStats, AgentStatus, Role, Scope};
use aether_core::id::{AgentId, ApprovalId, RunId, ToolCallId};
use aether_core::model::{ModelRef, Usage};
use aether_core::time::now;
use aether_core::tool::{ApprovalDecision, ApprovalRequest, ApprovalStatus, SideEffectClass};
use aether_protocol::{
    AgentTree, ClientFrame, Command, ErrorCode, ErrorReply, PROTOCOL_VERSION, Reply, ServerFrame,
    TreeNode, Welcome, decode, encode,
};
use aether_tui::app::TuiOptions;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::{UnixListener, UnixStream};

/// How many handshakes the mock has seen. The second one is the reconnect.
#[derive(Debug)]
struct Mock {
    hellos: AtomicUsize,
    providers: AtomicUsize,
    messages: AtomicUsize,
    /// Images the messages carried between them.
    images: AtomicUsize,
    /// Backfills asked for: plain mode only asks once it has the tree.
    backfills: AtomicUsize,
    /// One tree for the whole session, so an approval can name its agent.
    tree: AgentTree,
    /// Pending until a `ResolveApproval` names it.
    approval: Mutex<Option<ApprovalRequest>>,
    resolved: Mutex<Vec<ApprovalDecision>>,
}

impl Mock {
    fn new() -> Mock {
        Mock {
            hellos: AtomicUsize::new(0),
            providers: AtomicUsize::new(0),
            messages: AtomicUsize::new(0),
            images: AtomicUsize::new(0),
            backfills: AtomicUsize::new(0),
            tree: tree(),
            approval: Mutex::new(None),
            resolved: Mutex::new(Vec::new()),
        }
    }

    /// The tree's agent asks to run a command, an hour ago: well past the typing guard.
    fn with_approval(self) -> Mock {
        let agent = self.tree.roots[0];
        *self.approval.lock().unwrap() = Some(ApprovalRequest {
            id: ApprovalId::new(),
            agent_id: agent,
            task_id: None,
            call_id: ToolCallId::new(),
            tool: "bash".into(),
            description: "run ls".into(),
            argument: Some("ls".into()),
            resources: Vec::new(),
            side_effect: SideEffectClass::Mutation,
            requested_at: now() - chrono::Duration::hours(1),
            status: ApprovalStatus::Pending,
        });
        self
    }
}

/// One live agent, so plain mode has something to report and `busy()` has an
/// answer. Ready, not running: a running agent never goes quiet.
fn tree() -> AgentTree {
    let at = now();
    let agent = Agent {
        id: AgentId::new(),
        parent: None,
        role: Role::new("assistant"),
        name: "assistant".into(),
        scope: Scope::default(),
        model: ModelRef::new("mock", "mock-1"),
        fallbacks: Vec::new(),
        status: AgentStatus::Ready,
        archived: false,
        current_task: None,
        inbox_cursor: 0,
        checkpoint: None,
        engine_session: None,
        engine_usage: None,
        engine_sent: None,
        status_since: None,
        paused: false,
        paused_via: None,
        resume_wait: None,
        restarted_from: None,
        restarted_as: None,
        config: AgentConfig::default(),
        stats: AgentStats::default(),
        run: RunId::new(),
        created_at: at,
        updated_at: at,
    };
    let id = agent.id;
    let mut nodes = std::collections::BTreeMap::new();
    nodes.insert(
        id,
        TreeNode {
            agent,
            children: vec![],
            task: None,
            waiting_on: vec![],
            lease: None,
            subtree_usage: Usage::default(),
            hidden_usage: None,
        },
    );
    AgentTree {
        roots: vec![id],
        nodes,
    }
}

fn reply_to(command: &Command, mock: &Mock) -> Result<Reply, ErrorReply> {
    match command {
        Command::Status => {
            let status = aether_tui::testing::loaded_app(0, 0, 0)
                .status
                .expect("fixture status");
            Ok(Reply::Status(status))
        }
        Command::Hello(_) => {
            mock.hellos.fetch_add(1, Ordering::SeqCst);
            Ok(Reply::Welcome(Welcome {
                protocol_version: PROTOCOL_VERSION,
                daemon_version: "mock".into(),
                instance_id: "mock".into(),
                schema_version: 1,
                pid: std::process::id(),
                features: Vec::new(),
                daemon_protocol_version: None,
                min_protocol_version: None,
                idle_timeout_secs: None,
                device: None,
                submission_horizon: None,
            }))
        }
        Command::Providers => {
            mock.providers.fetch_add(1, Ordering::SeqCst);
            Ok(Reply::Providers(Vec::new()))
        }
        Command::SendMessage { attachments, .. } => {
            mock.images.fetch_add(attachments.len(), Ordering::SeqCst);
            mock.messages.fetch_add(1, Ordering::SeqCst);
            Ok(Reply::MessageSent {
                message: aether_core::id::MessageId::new(),
            })
        }
        Command::GetTree { .. } => Ok(Reply::Tree(mock.tree.clone())),
        Command::GetTranscript { .. } => Ok(Reply::Transcript(Vec::new())),
        Command::Subscribe { .. } => Ok(Reply::Subscribed { id: 1, last_seq: 0 }),
        Command::ListApprovals => Ok(Reply::Approvals(
            mock.approval.lock().unwrap().iter().cloned().collect(),
        )),
        Command::ResolveApproval { id, decision, .. } => {
            let mut pending = mock.approval.lock().unwrap();
            if pending.as_ref().is_some_and(|a| a.id == *id) {
                *pending = None;
                mock.resolved.lock().unwrap().push(*decision);
            }
            Ok(Reply::Ok)
        }
        Command::ListModels => Ok(Reply::Models(Vec::new())),
        Command::AuthStatus => Ok(Reply::Auth(Vec::new())),
        Command::GetEvents { .. } => {
            mock.backfills.fetch_add(1, Ordering::SeqCst);
            Err(ErrorReply::new(
                ErrorCode::Internal,
                "the mock keeps no events".to_owned(),
            ))
        }
        other => Err(ErrorReply::new(
            ErrorCode::Internal,
            format!("the mock does not answer {other:?}"),
        )),
    }
}

/// Answer frames until the peer goes away.
async fn serve(stream: UnixStream, mock: Arc<Mock>) {
    let (read, mut write) = stream.into_split();
    let mut lines = BufReader::new(read).lines();
    while let Ok(Some(line)) = lines.next_line().await {
        let Ok(frame) = decode::<ClientFrame>(&line) else {
            continue;
        };
        let out = encode(&ServerFrame::Reply {
            id: frame.id,
            result: reply_to(&frame.command, &mock),
        })
        .expect("encode");
        if write.write_all(out.as_bytes()).await.is_err() {
            return;
        }
    }
}

/// Accept one connection on `socket` and answer it until aborted.
fn listen(socket: &std::path::Path, mock: Arc<Mock>) -> tokio::task::JoinHandle<()> {
    let listener = UnixListener::bind(socket).expect("bind");
    tokio::spawn(async move {
        let (stream, _) = listener.accept().await.expect("accept");
        serve(stream, mock).await;
    })
}

async fn wait_for_hellos(mock: &Mock, want: usize, within: Duration) {
    let deadline = Instant::now() + within;
    while mock.hellos.load(Ordering::SeqCst) < want {
        assert!(
            Instant::now() < deadline,
            "the daemon saw {} handshakes, wanted {want}",
            mock.hellos.load(Ordering::SeqCst)
        );
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn plain_reconnects_and_then_leaves_when_the_pipe_closes() {
    let dir = tempfile::tempdir().expect("tempdir");
    let socket = dir.path().join("aetherd.sock");
    let mock = Arc::new(Mock::new());

    let first = listen(&socket, Arc::clone(&mock));
    let client = Client::connect(&socket).await.expect("connect");
    let (writer, reader) = tokio::net::unix::pipe::pipe().expect("pipe");
    let run = tokio::spawn(aether_tui::plain::run_with_input(
        client,
        TuiOptions {
            socket_path: Some(socket.clone()),
            ..TuiOptions::default()
        },
        reader,
    ));
    wait_for_hellos(&mock, 1, Duration::from_secs(10)).await;

    // The daemon goes away: the socket closes and the file with it.
    first.abort();
    let _ = std::fs::remove_file(&socket);
    tokio::time::sleep(Duration::from_millis(300)).await;

    // And comes back. A second handshake can only be plain mode reconnecting.
    let second = listen(&socket, Arc::clone(&mock));
    wait_for_hellos(&mock, 2, Duration::from_secs(20)).await;

    // The pipe closes, the way a script or a screen reader ends a session.
    drop(writer);
    let left = tokio::time::timeout(Duration::from_secs(30), run)
        .await
        .expect("plain mode hung after EOF instead of draining and leaving");
    assert!(left.expect("plain mode panicked").is_ok());
    second.abort();
}

/// A pipe hands over the line after `/image` before the file is read.
#[tokio::test]
async fn the_plain_message_after_an_image_command_carries_the_image() {
    let directory = tempfile::tempdir().unwrap();
    let socket = directory.path().join("aether.sock");
    let shot = directory.path().join("shot.png");
    std::fs::write(&shot, b"\x89PNG\r\n\x1a\npixels").unwrap();
    let mock = Arc::new(Mock::new());
    let server = listen(&socket, mock.clone());
    let client = Client::connect(&socket).await.unwrap();
    let (mut writer, reader) = tokio::net::unix::pipe::pipe().unwrap();
    let run = tokio::spawn(aether_tui::plain::run_with_input(
        client,
        TuiOptions {
            socket_path: Some(socket),
            ..Default::default()
        },
        reader,
    ));
    // The tree first: a line typed before it would ask for a new agent.
    let deadline = Instant::now() + Duration::from_secs(5);
    while mock.backfills.load(Ordering::SeqCst) == 0 && Instant::now() < deadline {
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    assert_eq!(mock.backfills.load(Ordering::SeqCst), 1);
    let lines = format!("/image {}\nwhat is this?\n", shot.display());
    writer.write_all(lines.as_bytes()).await.unwrap();
    let deadline = Instant::now() + Duration::from_secs(5);
    while mock.messages.load(Ordering::SeqCst) == 0 && Instant::now() < deadline {
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    assert_eq!(mock.messages.load(Ordering::SeqCst), 1);
    assert_eq!(
        mock.images.load(Ordering::SeqCst),
        1,
        "the message left before its image was read"
    );
    writer.write_all(b"/quit\n").await.unwrap();
    tokio::time::timeout(Duration::from_secs(5), run)
        .await
        .unwrap()
        .unwrap()
        .unwrap();
    server.abort();
}

#[tokio::test]
async fn usage_does_not_swallow_the_next_plain_message() {
    let directory = tempfile::tempdir().unwrap();
    let socket = directory.path().join("aether.sock");
    let mock = Arc::new(Mock::new());
    let server = listen(&socket, mock.clone());
    let client = Client::connect(&socket).await.unwrap();
    let (mut writer, reader) = tokio::net::unix::pipe::pipe().unwrap();
    let run = tokio::spawn(aether_tui::plain::run_with_input(
        client,
        TuiOptions {
            socket_path: Some(socket),
            ..Default::default()
        },
        reader,
    ));
    writer.write_all(b"/usage\n").await.unwrap();
    let deadline = Instant::now() + Duration::from_secs(5);
    while mock.providers.load(Ordering::SeqCst) == 0 && Instant::now() < deadline {
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    assert_eq!(mock.providers.load(Ordering::SeqCst), 1);
    writer.write_all(b"hello after usage\n").await.unwrap();
    let deadline = Instant::now() + Duration::from_secs(5);
    while mock.messages.load(Ordering::SeqCst) == 0 && Instant::now() < deadline {
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    assert_eq!(
        mock.messages.load(Ordering::SeqCst),
        1,
        "the invisible usage overlay swallowed the message"
    );
    writer.write_all(b"/quit\n").await.unwrap();
    tokio::time::timeout(Duration::from_secs(5), run)
        .await
        .unwrap()
        .unwrap()
        .unwrap();
    server.abort();
}

/// Wait until `done` holds, or fail naming `what`.
async fn wait_until(what: &str, mut done: impl FnMut() -> bool) {
    let deadline = Instant::now() + Duration::from_secs(10);
    while !done() {
        assert!(Instant::now() < deadline, "timed out waiting for {what}");
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
}

/// Plain mode has applied the tree (it asked for the backfill), and the
/// approval list sent alongside it has had time to land too.
async fn settled(mock: &Mock) {
    wait_until("the tree", || mock.backfills.load(Ordering::SeqCst) > 0).await;
    tokio::time::sleep(Duration::from_millis(200)).await;
}

/// Spawn plain mode against a fresh mock on its own socket.
async fn start(
    mock: Arc<Mock>,
) -> (
    tempfile::TempDir,
    tokio::task::JoinHandle<()>,
    tokio::net::unix::pipe::Sender,
    tokio::task::JoinHandle<anyhow::Result<()>>,
) {
    let directory = tempfile::tempdir().unwrap();
    let socket = directory.path().join("aether.sock");
    let server = listen(&socket, mock);
    let client = Client::connect(&socket).await.unwrap();
    let (writer, reader) = tokio::net::unix::pipe::pipe().unwrap();
    let run = tokio::spawn(aether_tui::plain::run_with_input(
        client,
        TuiOptions {
            socket_path: Some(socket),
            ..Default::default()
        },
        reader,
    ));
    (directory, server, writer, run)
}

async fn quit(
    mut writer: tokio::net::unix::pipe::Sender,
    run: tokio::task::JoinHandle<anyhow::Result<()>>,
) {
    writer.write_all(b"/quit\n").await.unwrap();
    tokio::time::timeout(Duration::from_secs(5), run)
        .await
        .unwrap()
        .unwrap()
        .unwrap();
}

/// While an approval waits, a typed message is refused rather than sent past
/// it, and a typed letter answers it.
#[tokio::test]
async fn a_typed_letter_answers_the_waiting_approval() {
    let mock = Arc::new(Mock::new().with_approval());
    let (_directory, server, mut writer, run) = start(Arc::clone(&mock)).await;
    settled(&mock).await;
    writer.write_all(b"hello\nn\n").await.unwrap();
    wait_until("the answer", || !mock.resolved.lock().unwrap().is_empty()).await;
    assert_eq!(*mock.resolved.lock().unwrap(), [ApprovalDecision::Denied]);
    assert_eq!(
        mock.messages.load(Ordering::SeqCst),
        0,
        "a message went past the waiting approval"
    );
    quit(writer, run).await;
    server.abort();
}

/// Views other than `/usage` need the interactive TUI; plain mode closes
/// them, so the next line is still a message.
#[tokio::test]
async fn an_interactive_view_does_not_swallow_the_next_plain_message() {
    let mock = Arc::new(Mock::new());
    let (_directory, server, mut writer, run) = start(Arc::clone(&mock)).await;
    settled(&mock).await;
    writer
        .write_all(b"/model\n/settings\nhello after views\n")
        .await
        .unwrap();
    wait_until("the message", || mock.messages.load(Ordering::SeqCst) > 0).await;
    assert_eq!(mock.messages.load(Ordering::SeqCst), 1);
    quit(writer, run).await;
    server.abort();
}

#[tokio::test]
async fn agent_board_command_does_not_capture_messages_or_quit_over_a_pipe() {
    let mock = Arc::new(Mock::new());
    let (_directory, server, mut writer, run) = start(Arc::clone(&mock)).await;
    settled(&mock).await;
    writer
        .write_all(b"/agent\nfirst message\nsecond message\n")
        .await
        .unwrap();
    wait_until("both separate messages", || {
        mock.messages.load(Ordering::SeqCst) >= 2
    })
    .await;
    assert_eq!(mock.messages.load(Ordering::SeqCst), 2);
    quit(writer, run).await;
    server.abort();
}

#[test]
fn plain_stdin_child() {
    let Some(socket) = std::env::var_os("AETHER_PLAIN_STDIN_TEST") else {
        return;
    };
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .unwrap();
    runtime.block_on(async {
        let client = Client::connect(&socket).await.unwrap();
        aether_tui::plain::run(
            client,
            TuiOptions {
                socket_path: Some(socket.into()),
                ..Default::default()
            },
        )
        .await
        .unwrap();
    });
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn plain_quit_exits_the_real_process_while_stdin_stays_open() {
    use std::io::Write as _;
    use std::process::{Command as ProcessCommand, Stdio};
    let mock = Arc::new(Mock::new().with_approval());
    let directory = tempfile::tempdir().unwrap();
    let socket = directory.path().join("a.sock");
    let server = listen(&socket, Arc::clone(&mock));
    let mut child = ProcessCommand::new(std::env::current_exe().unwrap())
        .args(["--exact", "plain_stdin_child", "--nocapture"])
        .env("AETHER_PLAIN_STDIN_TEST", &socket)
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .spawn()
        .unwrap();
    let mut stdin = child.stdin.take().unwrap();
    settled(&mock).await;
    stdin.write_all(b"/quit\n").unwrap();
    stdin.flush().unwrap();
    let deadline = Instant::now() + Duration::from_secs(5);
    let status = loop {
        if let Some(status) = child.try_wait().unwrap() {
            break status;
        }
        if Instant::now() >= deadline {
            child.kill().unwrap();
            child.wait().unwrap();
            server.abort();
            panic!("/quit waited for EOF while an approval was pending");
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    };
    assert!(status.success(), "{status}");
    assert!(mock.resolved.lock().unwrap().is_empty());
    drop(stdin);
    server.abort();
}

#[tokio::test]
async fn plain_quit_without_answering_a_pending_approval_keeps_input_open() {
    let mock = Arc::new(Mock::new().with_approval());
    let (_directory, server, writer, run) = start(Arc::clone(&mock)).await;
    settled(&mock).await;
    quit(writer, run).await;
    assert!(mock.resolved.lock().unwrap().is_empty());
    server.abort();
}
