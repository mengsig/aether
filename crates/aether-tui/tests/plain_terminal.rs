#![allow(clippy::expect_used, clippy::unwrap_used, clippy::panic)]

//! What a script finds when earlier work left the store full: every agent in
//! it is finished. The daemon refuses a message to a finished agent, so plain
//! mode must start one of its own and send the message there instead — this
//! is the gate on that, driven end to end over a pipe against a mock daemon.

use std::sync::Arc;
use std::time::Duration;

use aether_client::Client;
use aether_core::agent::{Agent, AgentConfig, AgentStats, AgentStatus, Role, Scope};
use aether_core::event::{Actor, Event, EventDraft, EventKind};
use aether_core::id::{AgentId, EventId, MessageId, RunId};
use aether_core::model::{ContentBlock, MessageRole, ModelRef, Usage};
use aether_core::time::now;
use aether_protocol::{
    AgentTree, ClientFrame, Command, ErrorCode, ErrorReply, PROTOCOL_VERSION, Reply, ServerFrame,
    TreeNode, Welcome, decode, encode,
};
use aether_tui::app::TuiOptions;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::{UnixListener, UnixStream};
use tokio::sync::Mutex;

/// How long the mock holds a creation before answering. Long enough that the
/// typed line always arrives while only the finished agent is in the tree,
/// which is the ordering the hosted runner hit.
const CREATE_DELAY: Duration = Duration::from_millis(300);

/// The store the mock serves, and what plain mode did to it.
#[derive(Debug, Default)]
struct Store {
    agents: Vec<Agent>,
    trees_served: usize,
    /// Every message the daemon accepted, as (agent, text).
    delivered: Vec<(AgentId, String)>,
    /// Messages addressed to a finished agent, which the daemon refuses.
    refused: Vec<AgentId>,
    seq: u64,
}

fn agent(status: AgentStatus) -> Agent {
    let at = now();
    Agent {
        id: AgentId::new(),
        parent: None,
        role: Role::new("assistant"),
        name: "assistant".into(),
        scope: Scope::default(),
        model: ModelRef::new("mock", "mock-1"),
        fallbacks: Vec::new(),
        status,
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
    }
}

impl Store {
    fn tree(&mut self) -> AgentTree {
        self.trees_served += 1;
        let mut nodes = std::collections::BTreeMap::new();
        for a in &self.agents {
            nodes.insert(
                a.id,
                TreeNode {
                    agent: a.clone(),
                    children: vec![],
                    task: None,
                    waiting_on: vec![],
                    lease: None,
                    subtree_usage: Usage::default(),
                    hidden_usage: None,
                },
            );
        }
        AgentTree {
            roots: self.agents.iter().map(|a| a.id).collect(),
            nodes,
        }
    }

    fn event(&mut self, agent: AgentId, role: MessageRole, text: &str) -> Event {
        self.seq += 1;
        Event::from_draft(
            EventDraft::new(
                Actor::System,
                EventKind::AgentMessage {
                    role,
                    content: vec![ContentBlock::text(text)],
                    invocation: None,
                },
            )
            .agent(agent),
            EventId::new(),
            self.seq,
            now(),
        )
    }
}

/// Answer one command, and say what to push over the subscription after it.
async fn reply_to(
    command: &Command,
    store: &Mutex<Store>,
) -> (Result<Reply, ErrorReply>, Vec<Event>) {
    let plain = |reply| (Ok(reply), Vec::new());
    match command {
        Command::Status => {
            let status = aether_tui::testing::loaded_app(0, 0, 0)
                .status
                .expect("fixture status");
            plain(Reply::Status(status))
        }
        Command::Hello(_) => plain(Reply::Welcome(Welcome {
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
        })),
        Command::GetTree { .. } => plain(Reply::Tree(store.lock().await.tree())),
        Command::GetTranscript { .. } => plain(Reply::Transcript(Vec::new())),
        Command::Subscribe { .. } => plain(Reply::Subscribed { id: 1, last_seq: 0 }),
        Command::ListApprovals => plain(Reply::Approvals(Vec::new())),
        Command::ListModels => plain(Reply::Models(Vec::new())),
        Command::AuthStatus => plain(Reply::Auth(Vec::new())),
        Command::CreateAgent(_) => {
            tokio::time::sleep(CREATE_DELAY).await;
            let fresh = agent(AgentStatus::Ready);
            store.lock().await.agents.push(fresh.clone());
            plain(Reply::Agent(fresh))
        }
        Command::SendMessage {
            to,
            content,
            subtree: false,
            ..
        } => {
            let mut store = store.lock().await;
            let finished = store
                .agents
                .iter()
                .find(|a| a.id == *to)
                .is_some_and(|a| a.status.is_terminal());
            if finished {
                store.refused.push(*to);
                return (
                    Err(ErrorReply::new(
                        ErrorCode::InvalidArgument,
                        format!("agent {to} is completed"),
                    )),
                    Vec::new(),
                );
            }
            store.delivered.push((*to, content.clone()));
            // The daemon's own confirmation: the message becomes an event,
            // and the agent answers it.
            let said = store.event(*to, MessageRole::User, content);
            let reply = store.event(*to, MessageRole::Assistant, "mock reply");
            (
                Ok(Reply::MessageSent {
                    message: MessageId::new(),
                }),
                vec![said, reply],
            )
        }
        other => (
            Err(ErrorReply::new(
                ErrorCode::Internal,
                format!("the mock does not answer {other:?}"),
            )),
            Vec::new(),
        ),
    }
}

async fn serve(stream: UnixStream, store: Arc<Mutex<Store>>) {
    let (read, mut write) = stream.into_split();
    let mut lines = BufReader::new(read).lines();
    while let Ok(Some(line)) = lines.next_line().await {
        let Ok(frame) = decode::<ClientFrame>(&line) else {
            continue;
        };
        let (result, events) = reply_to(&frame.command, &store).await;
        let mut out = encode(&ServerFrame::Reply {
            id: frame.id,
            result,
        })
        .expect("encode");
        for event in events {
            out.push_str(&encode(&ServerFrame::Event { sub: 1, event }).expect("encode"));
        }
        if write.write_all(out.as_bytes()).await.is_err() {
            return;
        }
    }
}

async fn wait_for<F>(what: &str, store: &Mutex<Store>, mut done: F)
where
    F: FnMut(&Store) -> bool,
{
    for _ in 0..500 {
        if done(&*store.lock().await) {
            return;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    panic!("the mock daemon never saw {what}");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_store_of_finished_agents_gets_a_new_one_and_the_message_lands() {
    let dir = tempfile::tempdir().expect("tempdir");
    let socket = dir.path().join("aetherd.sock");
    let done = agent(AgentStatus::Completed { summary: None });
    let finished = done.id;
    let store = Arc::new(Mutex::new(Store {
        agents: vec![done],
        ..Store::default()
    }));

    let listener = UnixListener::bind(&socket).expect("bind");
    let served = tokio::spawn({
        let store = Arc::clone(&store);
        async move {
            let (stream, _) = listener.accept().await.expect("accept");
            serve(stream, store).await;
        }
    });
    let client = Client::connect(&socket).await.expect("connect");
    let (mut writer, reader) = tokio::net::unix::pipe::pipe().expect("pipe");
    let run = tokio::spawn(aether_tui::plain::run_with_input(
        client,
        TuiOptions {
            socket_path: Some(socket.clone()),
            auto_create_root: true,
            ..TuiOptions::default()
        },
        reader,
    ));

    // Type only once the tree has arrived: at that moment the finished agent
    // is the only one there, which is what made the hosted run address it.
    wait_for("the tree fetched", &store, |s| s.trees_served > 0).await;
    writer.write_all(b"hello\n").await.expect("write");
    wait_for("the message delivered", &store, |s| !s.delivered.is_empty()).await;

    drop(writer);
    // Leaving on EOF needs the message confirmed and the agent quiet; a
    // message the daemon refused would keep plain mode waiting instead.
    let left = tokio::time::timeout(Duration::from_secs(30), run)
        .await
        .expect("plain mode never settled after EOF");
    assert!(left.expect("plain mode panicked").is_ok());

    let store = store.lock().await;
    assert!(
        store.refused.is_empty(),
        "a finished agent was addressed: {:?}",
        store.refused
    );
    assert_eq!(store.delivered.len(), 1, "{:?}", store.delivered);
    let (to, text) = &store.delivered[0];
    assert_ne!(*to, finished);
    assert_eq!(text, "hello");
    served.abort();
}
