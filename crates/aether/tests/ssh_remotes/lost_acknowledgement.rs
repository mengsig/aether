//! A keyed send whose acknowledgement was lost: the next connection never
//! runs it twice unasked and is never refused.
use super::{Fixture, HORIZON, HostLines, next_frame, send_reply, welcome};
use aether_protocol::{
    Command as WireCommand, ErrorCode, ErrorReply, Reply, SubmissionResolution, UnprovenAbsence,
};
use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use tokio::io::{AsyncBufReadExt as _, BufReader};
use tokio::net::unix::OwnedWriteHalf;

/// What the host did, read by the test while the host is still serving.
#[derive(Default)]
struct Seen {
    /// Every command that ran, in order: a duplicate execution is two entries.
    executions: Vec<WireCommand>,
    resolutions: usize,
}

/// A host that loses the first send's acknowledgement.
struct Host {
    version: u32,
    /// Whether the lost send committed before its acknowledgement was lost.
    commits: bool,
    /// Whether the receipt is evicted before the client connects again.
    forgets: bool,
    lost: Option<WireCommand>,
    receipts: HashMap<String, Reply>,
    seen: Arc<Mutex<Seen>>,
}

fn key_of(command: &WireCommand) -> String {
    let keyed = command.keyed_request().expect("keyed").expect("key");
    assert!(keyed.key.starts_with("ssh-"), "{}", keyed.key);
    keyed.key.to_owned()
}

impl Host {
    fn new(version: u32, commits: bool, forgets: bool) -> (Self, Arc<Mutex<Seen>>) {
        let seen = Arc::new(Mutex::new(Seen::default()));
        let host = Self {
            version,
            commits,
            forgets,
            lost: None,
            receipts: HashMap::new(),
            seen: seen.clone(),
        };
        (host, seen)
    }

    async fn serve(mut self, listener: tokio::net::UnixListener, connections: usize) {
        assert!(connections > 1, "the loss and at least one reconnection");
        for _ in 0..connections {
            let (mut lines, mut write) = self.accept(&listener).await;
            while let Some(line) = lines.next_line().await.expect("line") {
                let frame: aether_protocol::ClientFrame =
                    serde_json::from_str(&line).expect("frame");
                let Some(result) = self.answer(frame.command) else {
                    break;
                };
                send_reply(&mut write, frame.id, result).await;
            }
        }
    }

    /// A connection past its handshake. A host older than the client refuses
    /// the first hello, as a real one does, and is greeted again in its own.
    async fn accept(&self, listener: &tokio::net::UnixListener) -> (HostLines, OwnedWriteHalf) {
        loop {
            let (stream, _) = listener.accept().await.expect("connect");
            let (read, mut write) = stream.into_split();
            let mut lines = BufReader::new(read).lines();
            let frame = next_frame(&mut lines).await;
            let WireCommand::Hello(hello) = &frame.command else {
                unreachable!("a connection opens with hello: {:?}", frame.command);
            };
            if hello.protocol_version == self.version {
                send_reply(&mut write, frame.id, Ok(welcome(self.version))).await;
                return (lines, write);
            }
            let refusal = ErrorReply {
                code: ErrorCode::Incompatible,
                message: format!("daemon serves protocols 16..={}", self.version),
            };
            send_reply(&mut write, frame.id, Err(refusal)).await;
        }
    }

    /// `None` drops the connection without answering: the lost acknowledgement.
    fn answer(&mut self, command: WireCommand) -> Option<Result<Reply, ErrorReply>> {
        match command {
            WireCommand::ListAgents { .. } => Some(Ok(Reply::Agents(Vec::new()))),
            WireCommand::ResolveSubmission { command, horizon } => {
                assert_eq!(
                    horizon.as_deref(),
                    Some(HORIZON),
                    "the horizon it was sent under"
                );
                Some(Ok(Reply::SubmissionResolved(self.resolve(&command))))
            }
            WireCommand::SendMessage { .. } if self.lost.is_none() => {
                if self.commits {
                    self.execute(&command);
                }
                if self.forgets {
                    self.receipts.clear();
                }
                self.lost = Some(command);
                None
            }
            WireCommand::SendMessage { .. } => Some(Ok(self.execute(&command))),
            other => unreachable!("unexpected command: {other:?}"),
        }
    }

    /// Run a send unless its receipt is held, as the daemon does.
    fn execute(&mut self, command: &WireCommand) -> Reply {
        let key = key_of(command);
        if let Some(receipt) = self.receipts.get(&key) {
            return receipt.clone();
        }
        let reply = Reply::MessageSent {
            message: aether_core::id::MessageId::new(),
        };
        self.seen
            .lock()
            .expect("seen")
            .executions
            .push(command.clone());
        self.receipts.insert(key, reply.clone());
        reply
    }

    fn resolve(&mut self, command: &WireCommand) -> SubmissionResolution {
        assert!(
            self.version >= 18,
            "an older host is never asked to resolve"
        );
        assert_eq!(
            Some(command),
            self.lost.as_ref(),
            "the command exactly as sent"
        );
        self.seen.lock().expect("seen").resolutions += 1;
        let horizon = HORIZON.to_owned();
        let retained_since = aether_core::time::now();
        if let Some(receipt) = self.receipts.get(&key_of(command)) {
            return SubmissionResolution::Executed {
                reply: Box::new(receipt.clone()),
                horizon,
            };
        }
        if self.forgets {
            return SubmissionResolution::Unknown {
                reason: UnprovenAbsence::Forgotten,
                horizon,
                retained_since,
            };
        }
        SubmissionResolution::AbsentSafe {
            horizon,
            retained_since,
        }
    }
}

struct Scene {
    fixture: Fixture,
    seen: Arc<Mutex<Seen>>,
    host: Option<tokio::task::JoinHandle<()>>,
}

impl Scene {
    /// Start a host and lose the acknowledgement of one send to it.
    fn after_a_lost_send(version: u32, commits: bool, forgets: bool, connections: usize) -> Self {
        let fixture = Fixture::new();
        fixture.write_profile("fake-host");
        let listener = tokio::net::UnixListener::bind(&fixture.socket).expect("listener");
        let (host, seen) = Host::new(version, commits, forgets);
        let host = Some(tokio::spawn(host.serve(listener, connections)));
        let agent = aether_core::id::AgentId::new().to_string();
        let send = fixture
            .cli(&["--remote", "box", "send", &agent, "exact payload"])
            .output()
            .expect("send");
        let error = String::from_utf8_lossy(&send.stderr);
        assert!(
            !send.status.success() && error.contains("delivery unconfirmed"),
            "{error}"
        );
        assert!(!error.contains("not sent"), "{error}");
        Self {
            fixture,
            seen,
            host,
        }
    }

    /// Run a command on a new connection, which must not be refused.
    fn connected(&self, args: &[&str]) -> (String, String) {
        let mut all = vec!["--remote", "box"];
        all.extend_from_slice(args);
        let output = self.fixture.cli(&all).output().expect("command");
        let error = String::from_utf8_lossy(&output.stderr).into_owned();
        assert!(output.status.success(), "{args:?} was held up: {error}");
        (String::from_utf8_lossy(&output.stdout).into_owned(), error)
    }

    fn executions(&self) -> Vec<WireCommand> {
        self.seen.lock().expect("seen").executions.clone()
    }

    fn resolutions(&self) -> usize {
        self.seen.lock().expect("seen").resolutions
    }

    /// The outbox entries still held, and the log of what settled the rest.
    fn outbox(&self) -> (Vec<serde_json::Value>, String) {
        let endpoints = self.fixture.directory.path().join("config/endpoints");
        let (mut entries, mut settled) = (Vec::new(), String::new());
        for endpoint in std::fs::read_dir(endpoints).expect("endpoint states") {
            let outbox = endpoint.expect("endpoint").path().join("outbox");
            for file in std::fs::read_dir(outbox).expect("outbox") {
                let path = file.expect("file").path();
                let text = std::fs::read_to_string(&path).expect("readable");
                if path.file_name().is_some_and(|name| name == "settled.jsonl") {
                    settled.push_str(&text);
                } else {
                    entries.push(serde_json::from_str(&text).expect("entry"));
                }
            }
        }
        (entries, settled)
    }

    /// A file in the outbox directory that is not an operation.
    fn plant_unreadable_file(&self, name: &str) -> std::path::PathBuf {
        let endpoints = self.fixture.directory.path().join("config/endpoints");
        let endpoint = std::fs::read_dir(endpoints)
            .expect("endpoint states")
            .next()
            .expect("one endpoint")
            .expect("endpoint");
        let path = endpoint.path().join("outbox").join(format!("{name}.json"));
        std::fs::write(&path, b"{ not json").expect("unreadable file");
        path
    }

    /// The one operation `outbox list` shows, by its key.
    fn listed_key(&self) -> String {
        let (listed, _) = self.connected(&["outbox", "list", "--json"]);
        let listed: Vec<serde_json::Value> = serde_json::from_str(&listed).expect("list");
        assert_eq!(listed.len(), 1, "{listed:?}");
        assert_eq!(listed[0]["command"]["content"], "exact payload");
        assert!(listed[0]["unconfirmed"].is_string(), "{listed:?}");
        listed[0]["command"]["idempotency_key"]
            .as_str()
            .expect("key")
            .to_owned()
    }

    /// Wait for the host to have served every connection it expected.
    async fn finish(&mut self) {
        let host = self.host.take().expect("a host is finished once");
        host.await.expect("host");
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_receipt_the_host_still_holds_settles_the_lost_send_without_running_it_again() {
    let mut scene = Scene::after_a_lost_send(18, true, false, 2);
    let (agents, notices) = scene.connected(&["agents", "--json"]);
    assert_eq!(agents.trim(), "[]");
    assert!(
        notices.contains("confirmed queued remote send_message"),
        "{notices}"
    );
    assert!(!notices.contains("unconfirmed"), "{notices}");
    scene.finish().await;
    assert_eq!(
        scene.executions().len(),
        1,
        "the receipt answered; nothing ran twice"
    );
    assert_eq!(scene.resolutions(), 1);
    let (entries, settled) = scene.outbox();
    assert!(entries.is_empty(), "{entries:?}");
    assert!(settled.contains(r#""settled":"executed""#), "{settled}");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn an_unreadable_outbox_file_holds_up_no_command_and_is_discarded_by_name() {
    let mut scene = Scene::after_a_lost_send(18, true, true, 5);
    let bad = scene.plant_unreadable_file("garbage");
    let (agents, notices) = scene.connected(&["agents", "--json"]);
    assert_eq!(agents.trim(), "[]", "the connection is not held up");
    assert!(notices.contains("unreadable outbox file"), "{notices}");

    let (listed, _) = scene.connected(&["outbox", "list", "--json"]);
    let listed: Vec<serde_json::Value> = serde_json::from_str(&listed).expect("list");
    assert_eq!(listed.len(), 2, "{listed:?}");
    let unreadable = listed.iter().find(|item| item["key"] == "garbage");
    let unreadable = unreadable.expect("the unreadable file is listed");
    assert!(
        unreadable["reason"]
            .as_str()
            .is_some_and(|r| r.starts_with("unreadable:"))
    );
    assert!(
        unreadable["path"]
            .as_str()
            .is_some_and(|p| p.ends_with("garbage.json"))
    );

    let (discarded, _) = scene.connected(&["outbox", "discard", "garbage"]);
    assert!(discarded.contains("Discarded garbage"), "{discarded}");
    assert!(!bad.exists());
    let (listed, _) = scene.connected(&["outbox", "list", "--json"]);
    let listed: Vec<serde_json::Value> = serde_json::from_str(&listed).expect("list");
    assert_eq!(listed.len(), 1, "only the real unconfirmed send remains");
    scene.finish().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_send_the_host_proves_never_ran_is_sent_again_under_the_same_key() {
    let mut scene = Scene::after_a_lost_send(18, false, false, 2);
    assert!(scene.executions().is_empty(), "it was lost before it ran");
    let (agents, notices) = scene.connected(&["agents", "--json"]);
    assert_eq!(agents.trim(), "[]");
    assert!(
        notices.contains("confirmed queued remote send_message"),
        "{notices}"
    );
    scene.finish().await;
    let executions = scene.executions();
    assert_eq!(executions.len(), 1, "it ran exactly once");
    assert!(
        matches!(&executions[0], WireCommand::SendMessage { content, .. } if content == "exact payload")
    );
    let (entries, settled) = scene.outbox();
    assert!(entries.is_empty(), "{entries:?}");
    assert!(settled.contains(r#""settled":"resent""#), "{settled}");
    assert!(settled.contains(&key_of(&executions[0])), "{settled}");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn an_evicted_receipt_leaves_the_send_unconfirmed_until_the_operator_decides() {
    let mut scene = Scene::after_a_lost_send(18, true, true, 5);
    let (agents, notices) = scene.connected(&["agents", "--json"]);
    assert_eq!(agents.trim(), "[]", "the connection is not held up");
    assert!(
        notices.contains("delivery unconfirmed") && notices.contains("outbox"),
        "{notices}"
    );
    assert!(notices.contains("dropped receipts"), "{notices}");
    let key = scene.listed_key();
    assert_eq!(
        scene.executions().len(),
        1,
        "two connections replayed nothing"
    );
    let (entries, settled) = scene.outbox();
    assert_eq!(entries.len(), 1);
    assert_eq!(
        entries[0]["horizon"], HORIZON,
        "the horizon is never rewritten"
    );
    assert_eq!(
        settled.matches(r#""settled":"unconfirmed""#).count(),
        1,
        "{settled}"
    );

    let (resent, _) = scene.connected(&["outbox", "resend", &key]);
    assert!(resent.contains("again"), "{resent}");
    let (listed, _) = scene.connected(&["outbox", "list", "--json"]);
    assert_eq!(listed.trim(), "[]");
    scene.finish().await;
    let executions = scene.executions();
    assert_eq!(
        executions.len(),
        2,
        "only the operator's choice ran it again"
    );
    assert_eq!(executions[0], executions[1], "same key, same payload");
    assert_eq!(key_of(&executions[1]), key);
    assert!(scene.outbox().1.contains(r#""settled":"sent_again""#));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_host_that_cannot_resolve_keeps_the_send_unconfirmed_and_the_connection_open() {
    let mut scene = Scene::after_a_lost_send(17, true, false, 4);
    let (agents, notices) = scene.connected(&["agents", "--json"]);
    assert_eq!(agents.trim(), "[]", "the connection is not held up");
    assert!(
        notices.contains("delivery unconfirmed") && notices.contains("older than 18"),
        "{notices}"
    );
    let key = scene.listed_key();
    let (discarded, _) = scene.connected(&["outbox", "discard", &key]);
    assert!(discarded.contains("Discarded"), "{discarded}");
    scene.finish().await;
    assert_eq!(scene.executions().len(), 1, "nothing was replayed");
    assert_eq!(scene.resolutions(), 0, "protocol 17 has no resolution");
    let (entries, settled) = scene.outbox();
    assert!(entries.is_empty(), "{entries:?}");
    assert!(settled.contains(r#""settled":"discarded""#), "{settled}");
}
