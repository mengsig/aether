use super::*;
use aether_protocol::{ClientFrame, ErrorCode, ErrorReply, ServerFrame, Welcome};
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;
use tokio::io::{AsyncBufReadExt as _, BufReader, DuplexStream, Lines, ReadHalf, WriteHalf};
use tokio::net::UnixListener;
use tokio::sync::{Notify, mpsc};
use tokio::task::JoinHandle;

const TOKEN: &str = "turn-secret";
const DEADLINE: Duration = Duration::from_secs(5);

#[tokio::test]
async fn oversized_bridge_record_fails_explicitly() {
    let input = vec![b'x'; crate::output::MAX_LINE_BYTES + 1];
    let error = serve(
        PathBuf::new(),
        AgentId::new(),
        TOKEN.into(),
        &input[..],
        tokio::io::sink(),
    )
    .await
    .expect_err("oversized record must fail");
    assert!(error.to_string().contains("native JSONL record exceeds"));
}

struct Fixture {
    input: WriteHalf<DuplexStream>,
    output: Lines<BufReader<ReadHalf<DuplexStream>>>,
    held: mpsc::UnboundedReceiver<()>,
    closed: mpsc::UnboundedReceiver<()>,
    release: Arc<Notify>,
    script: DaemonScript,
    bridge: JoinHandle<anyhow::Result<()>>,
    daemon: JoinHandle<()>,
    _dir: tempfile::TempDir,
}

/// What the mock daemon counts and how it misbehaves, shared by connections.
#[derive(Clone, Default)]
struct DaemonScript {
    /// How many `flaky` calls reached the daemon; the first is dropped.
    flaky: Arc<AtomicUsize>,
    /// How many tool lists it answered.
    lists: Arc<AtomicUsize>,
    /// Never answer a tool list, like a daemon too busy to.
    stall_lists: Arc<std::sync::atomic::AtomicBool>,
}

impl Drop for Fixture {
    fn drop(&mut self) {
        self.bridge.abort();
        self.daemon.abort();
    }
}

impl Fixture {
    fn new() -> Self {
        let dir = tempfile::tempdir().expect("temporary socket directory");
        let socket = dir.path().join("daemon.sock");
        let listener = UnixListener::bind(&socket).expect("listen");
        let agent = AgentId::new();
        let release = Arc::new(Notify::new());
        let (held_tx, held) = mpsc::unbounded_channel();
        let (closed_tx, closed) = mpsc::unbounded_channel();
        let daemon_release = release.clone();
        let script = DaemonScript::default();
        let daemon_script = script.clone();
        let daemon = tokio::spawn(async move {
            let mut connections = JoinSet::new();
            loop {
                tokio::select! {
                    connection = listener.accept() => {
                        let (stream, _) = connection.expect("accept");
                        connections.spawn(mock_connection(
                            stream, agent, daemon_release.clone(), held_tx.clone(), closed_tx.clone(),
                            daemon_script.clone(),
                        ));
                    }
                    done = connections.join_next(), if !connections.is_empty() => {
                        done.expect("connection task").expect("mock daemon must not panic");
                    }
                }
            }
        });
        let (engine, bridge_io) = tokio::io::duplex(64 * 1024);
        let (read, input) = tokio::io::split(engine);
        let (bridge_read, bridge_write) = tokio::io::split(bridge_io);
        let bridge = tokio::spawn(serve(
            socket,
            agent,
            TOKEN.into(),
            BufReader::new(bridge_read),
            bridge_write,
        ));
        Self {
            input,
            output: BufReader::new(read).lines(),
            held,
            closed,
            release,
            script,
            bridge,
            daemon,
            _dir: dir,
        }
    }

    async fn send(&mut self, request: Value) {
        let mut line = serde_json::to_string(&request).expect("encode request");
        line.push('\n');
        self.input
            .write_all(line.as_bytes())
            .await
            .expect("write request");
    }

    async fn reply(&mut self) -> Value {
        let line = tokio::time::timeout(DEADLINE, self.output.next_line())
            .await
            .expect("bridge reply must not block")
            .expect("read reply")
            .expect("reply before EOF");
        serde_json::from_str(&line).expect("one intact JSON reply per line")
    }

    async fn wait_held(&mut self) {
        tokio::time::timeout(DEADLINE, self.held.recv())
            .await
            .expect("request must reach daemon")
            .expect("held notification");
    }

    async fn close_input(&mut self) {
        self.input.shutdown().await.expect("stdio EOF");
        tokio::time::timeout(DEADLINE, &mut self.bridge)
            .await
            .expect("EOF must stop bridge even with held replies")
            .expect("bridge task")
            .expect("bridge succeeds");
    }
}

async fn mock_connection(
    stream: tokio::net::UnixStream,
    expected_agent: AgentId,
    release: Arc<Notify>,
    held: mpsc::UnboundedSender<()>,
    closed: mpsc::UnboundedSender<()>,
    script: DaemonScript,
) {
    let flaky = &script.flaky;
    let (read, mut write) = stream.into_split();
    let mut lines = BufReader::new(read).lines();
    while let Some(line) = lines.next_line().await.expect("read daemon frame") {
        let frame: ClientFrame = aether_protocol::decode(&line).expect("decode daemon frame");
        let result = match frame.command {
            Command::Hello(hello) => {
                assert!(
                    hello.token.is_none(),
                    "turn token belongs on engine commands"
                );
                Ok(Reply::Welcome(Welcome {
                    protocol_version: aether_protocol::PROTOCOL_VERSION,
                    daemon_version: "test".into(),
                    instance_id: "test".into(),
                    schema_version: 1,
                    pid: 1,
                    features: vec![],
                    daemon_protocol_version: None,
                    min_protocol_version: None,
                    idle_timeout_secs: None,
                    device: None,
                    submission_horizon: None,
                }))
            }
            Command::EngineTools { agent, token } => {
                assert_eq!(agent, expected_agent);
                assert_eq!(token, TOKEN);
                if script.stall_lists.load(Ordering::SeqCst) {
                    std::future::pending::<()>().await;
                }
                let listed_before = script.lists.fetch_add(1, Ordering::SeqCst);
                Ok(Reply::EngineTools {
                    tools: offered_tools(listed_before),
                })
            }
            Command::EngineTool {
                agent,
                token,
                name,
                input,
            } => {
                assert_eq!(agent, expected_agent);
                assert_eq!(token, TOKEN);
                match name.as_str() {
                    "wait_for" => {
                        held.send(()).expect("record held request");
                        // Like the daemon, this connection cannot dispatch another command during a wait.
                        tokio::select! {
                            _ = release.notified() => {}
                            next = lines.next_line() => {
                                assert!(next.expect("read during wait").is_none(), "wait connection must be independent");
                                closed.send(()).expect("record closed connection");
                                return;
                            }
                        }
                        Ok(Reply::EngineTool {
                            content: "released".into(),
                            is_error: false,
                        })
                    }
                    "send_message" => {
                        assert_eq!(input, json!({"content": "steer"}));
                        Ok(Reply::EngineTool {
                            content: "delivered".into(),
                            is_error: false,
                        })
                    }
                    // The first one drops the connection without a reply,
                    // like a daemon that restarts mid-call.
                    "flaky" | "flaky_unkeyed" | "flaky_rekeyed"
                        if flaky.fetch_add(1, Ordering::SeqCst) == 0 =>
                    {
                        return;
                    }
                    "always_drops" => {
                        flaky.fetch_add(1, Ordering::SeqCst);
                        return;
                    }
                    "flaky" | "flaky_unkeyed" | "flaky_rekeyed" => Ok(Reply::EngineTool {
                        content: "recovered".into(),
                        is_error: false,
                    }),
                    "refused" => Err(ErrorReply::new(ErrorCode::Unauthorized, "turn expired")),
                    "unexpected" => Ok(Reply::Ok),
                    other => panic!("unexpected tool {other}"),
                }
            }
            other => panic!("unexpected daemon command {other:?}"),
        };
        let line = aether_protocol::encode(&ServerFrame::Reply {
            id: frame.id,
            result,
        })
        .expect("encode daemon reply");
        write
            .write_all(line.as_bytes())
            .await
            .expect("write daemon reply");
    }
}

fn wait_request(id: usize) -> Value {
    json!({"jsonrpc": "2.0", "id": id, "method": "tools/call", "params": {"name": "wait_for"}})
}

#[tokio::test]
async fn initialization_only_advertises_supported_versions() {
    let mut fixture = Fixture::new();
    for (params, expected) in [
        (json!({"protocolVersion": "2024-11-05"}), "2024-11-05"),
        (json!({"protocolVersion": "2025-03-26"}), "2025-03-26"),
        (json!({"protocolVersion": "2025-06-18"}), "2025-06-18"),
        (json!({"protocolVersion": "2025-11-25"}), "2025-11-25"),
        (json!({"protocolVersion": "2099-01-01"}), "2025-06-18"),
        (json!({"protocolVersion": 42}), "2025-06-18"),
        (json!({"protocolVersion": null}), "2025-06-18"),
        (json!({}), "2025-06-18"),
    ] {
        fixture
            .send(json!({"jsonrpc": "2.0", "id": 1, "method": "initialize", "params": params}))
            .await;
        let reply = fixture.reply().await;
        assert_eq!(reply["result"]["protocolVersion"], expected, "{params}");
    }
    fixture.close_input().await;
}

#[test]
fn the_permission_handler_is_the_bridges_own() {
    let tool = permission_tool();
    assert_eq!(tool["name"], "permission");
    assert_eq!(
        tool["inputSchema"]["required"],
        json!(["tool_name", "input"])
    );
}

#[tokio::test]
async fn held_tool_does_not_block_later_tool_ping_or_tool_list() {
    let mut fixture = Fixture::new();
    fixture.send(wait_request(1)).await;
    fixture.wait_held().await;
    fixture
        .send(json!({"id": "message", "method": "tools/call", "params": {
            "name": "send_message", "arguments": {"content": "steer"}
        }}))
        .await;
    fixture.send(json!({"id": 3, "method": "ping"})).await;
    fixture.send(json!({"id": 4, "method": "tools/list"})).await;
    let mut ids = Vec::new();
    for _ in 0..3 {
        let reply = fixture.reply().await;
        match reply["id"].clone() {
            id if id == "message" => assert_eq!(reply["result"]["content"][0]["text"], "delivered"),
            id if id == 3 => assert_eq!(reply["result"], json!({})),
            id if id == 4 => {
                let tools = reply["result"]["tools"].as_array().expect("a tool list");
                assert_eq!(tools.len(), offered_tools(0).len() + 1);
                assert_eq!(tools.last(), Some(&permission_tool()));
            }
            id => panic!("unexpected reply while first call is still held: {id}"),
        }
        assert!(!ids.contains(&reply["id"]), "each id answered once");
        ids.push(reply["id"].clone());
    }
    fixture.release.notify_waiters();
    let reply = fixture.reply().await;
    assert_eq!(reply["id"], 1);
    assert_eq!(reply["result"]["content"][0]["text"], "released");
    fixture.close_input().await;
}

#[tokio::test]
async fn eof_cancels_held_calls_and_closes_their_daemon_connections() {
    let mut fixture = Fixture::new();
    fixture.send(wait_request(1)).await;
    fixture.wait_held().await;
    fixture.close_input().await;
    tokio::time::timeout(DEADLINE, fixture.closed.recv())
        .await
        .expect("pending client must close")
        .expect("closed notification");
    assert!(
        fixture
            .output
            .next_line()
            .await
            .expect("output EOF")
            .is_none()
    );
}

#[tokio::test]
async fn saturated_bridge_rejects_daemon_work_but_keeps_ping_responsive() {
    let mut fixture = Fixture::new();
    for id in 0..MAX_IN_FLIGHT {
        fixture.send(wait_request(id)).await;
        fixture.wait_held().await;
    }
    fixture.send(wait_request(MAX_IN_FLIGHT)).await;
    let reply = fixture.reply().await;
    assert_eq!(reply["id"], MAX_IN_FLIGHT);
    assert_eq!(reply["error"]["code"], -32603);
    assert!(
        reply["error"]["message"]
            .as_str()
            .expect("busy error")
            .contains("in-flight")
    );
    fixture.send(json!({"id": "ping", "method": "ping"})).await;
    assert_eq!(fixture.reply().await["result"], json!({}));
    fixture.release.notify_waiters();
    for _ in 0..MAX_IN_FLIGHT {
        assert_eq!(fixture.reply().await["result"]["isError"], false);
    }
    fixture
        .send(json!({"id": "after", "method": "tools/call", "params": {
            "name": "send_message", "arguments": {"content": "steer"}
        }}))
        .await;
    assert_eq!(fixture.reply().await["id"], "after");
    fixture.close_input().await;
}

#[tokio::test]
async fn protocol_behavior_and_daemon_errors_are_preserved() {
    let mut fixture = Fixture::new();
    fixture
        .input
        .write_all(
            b"\nnot-json\n{\"method\":\"tools/call\",\"params\":{\"name\":\"send_message\"}}\n",
        )
        .await
        .expect("blank, malformed and notification inputs");
    fixture
        .send(json!({"id": 1, "method": "initialize", "params": {"protocolVersion": "test"}}))
        .await;
    let reply = fixture.reply().await;
    assert_eq!(reply["id"], 1);
    assert_eq!(reply["result"]["protocolVersion"], "2025-06-18");
    fixture.send(json!({"id": null, "method": "unknown"})).await;
    let reply = fixture.reply().await;
    assert_eq!(reply["id"], Value::Null);
    assert_eq!(reply["error"]["code"], -32601);
    for (id, name, message) in [
        (2, "refused", "turn expired"),
        (3, "unexpected", "unexpected reply"),
    ] {
        fixture
            .send(json!({"id": id, "method": "tools/call", "params": {"name": name}}))
            .await;
        let reply = fixture.reply().await;
        assert_eq!(reply["id"], id);
        assert_eq!(reply["error"]["code"], -32603);
        assert!(
            reply["error"]["message"]
                .as_str()
                .expect("daemon error")
                .contains(message)
        );
    }
    fixture.close_input().await;
}

/// A daemon newer than this bridge: it refuses every hello this build can
/// send, in the words the daemon uses.
fn refusing_daemon(socket: &Path) -> JoinHandle<()> {
    let listener = UnixListener::bind(socket).expect("listen");
    tokio::spawn(async move {
        loop {
            let (stream, _) = listener.accept().await.expect("accept");
            let (read, mut write) = stream.into_split();
            let mut lines = BufReader::new(read).lines();
            let Some(line) = lines.next_line().await.expect("read hello") else {
                continue;
            };
            let frame: ClientFrame = aether_protocol::decode(&line).expect("decode hello");
            let refusal = format!(
                "client protocol {} is not served: this daemon speaks 90 to 91",
                aether_protocol::PROTOCOL_VERSION
            );
            let line = aether_protocol::encode(&ServerFrame::Reply {
                id: frame.id,
                result: Err(ErrorReply::new(ErrorCode::Incompatible, refusal)),
            })
            .expect("encode refusal");
            write.write_all(line.as_bytes()).await.expect("refuse");
        }
    })
}

/// Drive a bridge through a whole engine session against a daemon it cannot
/// use, and return the explanation each tool call answered with.
async fn session_without_a_daemon(socket: PathBuf) -> String {
    let (engine, bridge_io) = tokio::io::duplex(64 * 1024);
    let (read, mut input) = tokio::io::split(engine);
    let (bridge_read, bridge_write) = tokio::io::split(bridge_io);
    let bridge = tokio::spawn(serve(
        socket,
        AgentId::new(),
        TOKEN.into(),
        BufReader::new(bridge_read),
        bridge_write,
    ));
    let mut output = BufReader::new(read).lines();
    let mut exchange = async |request: Value| {
        let mut line = serde_json::to_string(&request).expect("encode request");
        line.push('\n');
        input
            .write_all(line.as_bytes())
            .await
            .expect("write request");
        let reply = tokio::time::timeout(DEADLINE, output.next_line())
            .await
            .expect("bridge reply must not block")
            .expect("read reply")
            .expect("the bridge keeps serving");
        serde_json::from_str::<Value>(&reply).expect("one JSON reply per line")
    };
    let initialized =
        exchange(json!({"jsonrpc": "2.0", "id": 1, "method": "initialize", "params": {}})).await;
    assert_eq!(initialized["result"]["serverInfo"]["name"], "aether");
    let listed = exchange(json!({"jsonrpc": "2.0", "id": 2, "method": "tools/list"})).await;
    let tools = listed["result"]["tools"].as_array().expect("a tool list");
    let names: Vec<_> = tools.iter().map(|tool| tool["name"].clone()).collect();
    assert_eq!(
        names,
        vec![json!("permission"), json!("aether_unavailable")]
    );
    let reason = tools[1]["description"]
        .as_str()
        .expect("description")
        .to_owned();
    for (id, name) in [
        (3, "send_message"),
        (4, "permission"),
        (5, "aether_unavailable"),
    ] {
        let reply = exchange(json!({"jsonrpc": "2.0", "id": id, "method": "tools/call",
            "params": {"name": name, "arguments": {}}}))
        .await;
        assert_eq!(reply["id"], id);
        assert_eq!(reply["result"]["isError"], true, "{reply}");
        let text = reply["result"]["content"][0]["text"]
            .as_str()
            .expect("a text result");
        assert_eq!(explanation(text), reason, "{name}");
    }
    input.shutdown().await.expect("stdio EOF");
    tokio::time::timeout(DEADLINE, bridge)
        .await
        .expect("EOF stops the bridge")
        .expect("bridge task")
        .expect("bridge succeeds");
    reason
}

#[tokio::test]
async fn a_bridge_without_a_daemon_explains_itself_on_every_tool_call() {
    let dir = tempfile::tempdir().expect("temporary socket directory");
    let socket = dir.path().join("absent.sock");
    let reason = session_without_a_daemon(socket.clone()).await;
    let ours = format!(
        "speaks protocol {} to {}",
        aether_protocol::compat::MIN_PROTOCOL_VERSION,
        aether_protocol::PROTOCOL_VERSION
    );
    for expected in [
        &socket.display().to_string(),
        &ours,
        "unknown",
        "`aether stop`",
    ] {
        assert!(reason.contains(expected), "missing {expected}: {reason}");
    }
}

#[tokio::test]
async fn a_bridge_against_an_incompatible_daemon_names_both_windows() {
    let dir = tempfile::tempdir().expect("temporary socket directory");
    let socket = dir.path().join("daemon.sock");
    let daemon = refusing_daemon(&socket);
    let reason = session_without_a_daemon(socket.clone()).await;
    daemon.abort();
    let ours = format!(
        "speaks protocol {} to {}",
        aether_protocol::compat::MIN_PROTOCOL_VERSION,
        aether_protocol::PROTOCOL_VERSION
    );
    for expected in [
        &socket.display().to_string(),
        &ours,
        "the daemon serves protocol 90 to 91",
        "`aether stop`",
    ] {
        assert!(reason.contains(expected), "missing {expected}: {reason}");
    }
}

/// What a failed call says to the model: the text itself, or the cause of a
/// structured transport failure, which also says calling again is safe since
/// nothing reached a daemon.
fn explanation(text: &str) -> String {
    match serde_json::from_str::<Value>(text) {
        Ok(failure) if failure["error"] == "tool_transport_failed" => {
            assert_eq!(failure["retryable"], true, "nothing was sent: {failure}");
            failure["cause"].as_str().expect("a cause").to_owned()
        }
        _ => text.to_owned(),
    }
}

/// The mock daemon's tools: `flaky` and `always_drops` declare an
/// `idempotency_key`, so a keyed call to them is one effect when sent again;
/// `flaky_rekeyed` declares it only once a list has been given before.
fn offered_tools(listed_before: usize) -> Vec<aether_core::model::ToolSpec> {
    let spec = |name: &str, properties: Value| aether_core::model::ToolSpec {
        name: name.into(),
        description: format!("{name} (test)"),
        input_schema: json!({"type": "object", "properties": properties}),
        strict: false,
    };
    let keyed = json!({"idempotency_key": {"type": "string"}});
    let rekeyed = if listed_before == 0 {
        json!({})
    } else {
        keyed.clone()
    };
    vec![
        spec("flaky", keyed.clone()),
        spec("always_drops", keyed),
        spec("flaky_unkeyed", json!({})),
        spec("flaky_rekeyed", rekeyed),
    ]
}

fn call_request(id: usize, name: &str, arguments: Value) -> Value {
    json!({"jsonrpc": "2.0", "id": id, "method": "tools/call",
        "params": {"name": name, "arguments": arguments}})
}

fn flaky_request(id: usize, arguments: Value) -> Value {
    call_request(id, "flaky", arguments)
}

#[tokio::test]
async fn a_key_the_tool_does_not_declare_does_not_make_a_dropped_call_replayable() {
    let mut fixture = Fixture::new();
    fixture
        .send(call_request(
            1,
            "flaky_unkeyed",
            json!({"idempotency_key": "once"}),
        ))
        .await;
    let reply = fixture.reply().await;
    let failure = &reply["result"]["structuredContent"];
    assert_eq!(failure["error"], "tool_transport_failed", "{reply}");
    assert_eq!(failure["retryable"], false, "nothing promised one effect");
    assert_eq!(
        fixture.script.flaky.load(Ordering::SeqCst),
        1,
        "sent exactly once"
    );
    fixture.close_input().await;
}

#[tokio::test]
async fn a_keyed_call_whose_daemon_keeps_dropping_fails_retryable() {
    let mut fixture = Fixture::new();
    fixture
        .send(call_request(
            1,
            "always_drops",
            json!({"idempotency_key": "once"}),
        ))
        .await;
    let reply = fixture.reply().await;
    let failure = &reply["result"]["structuredContent"];
    assert_eq!(failure["error"], "tool_transport_failed", "{reply}");
    assert_eq!(
        failure["retryable"], true,
        "a keyed call stays safe to send again"
    );
    assert_eq!(
        fixture.script.flaky.load(Ordering::SeqCst),
        RECONNECT_BACKOFF.len() + 1,
        "the first try and one replay per backoff, then no more"
    );
    fixture.close_input().await;
}

#[tokio::test]
async fn a_keyed_call_whose_daemon_connection_drops_is_replayed_and_succeeds() {
    let mut fixture = Fixture::new();
    fixture
        .send(flaky_request(1, json!({"idempotency_key": "once"})))
        .await;
    let reply = fixture.reply().await;
    assert_eq!(reply["id"], 1);
    assert_eq!(reply["result"]["isError"], false, "{reply}");
    assert_eq!(reply["result"]["content"][0]["text"], "recovered");
    assert_eq!(
        fixture.script.flaky.load(Ordering::SeqCst),
        2,
        "dropped once, then replayed"
    );
    fixture.close_input().await;
}

#[tokio::test]
async fn an_unkeyed_call_whose_daemon_connection_drops_is_not_replayed_and_says_so() {
    let mut fixture = Fixture::new();
    fixture.send(flaky_request(1, json!({}))).await;
    let reply = fixture.reply().await;
    assert_eq!(reply["id"], 1);
    assert_eq!(reply["result"]["isError"], true, "{reply}");
    let failure = &reply["result"]["structuredContent"];
    assert_eq!(failure["error"], "tool_transport_failed");
    assert_eq!(failure["tool"], "flaky");
    assert_eq!(
        failure["retryable"], false,
        "the daemon may already have run it"
    );
    let cause = failure["cause"].as_str().expect("a cause");
    assert!(cause.contains("dropped mid-call"), "{cause}");
    let text = reply["result"]["content"][0]["text"]
        .as_str()
        .expect("text");
    assert_eq!(
        serde_json::from_str::<Value>(text).expect("JSON text"),
        *failure
    );
    assert_eq!(
        fixture.script.flaky.load(Ordering::SeqCst),
        1,
        "sent exactly once"
    );
    fixture.close_input().await;
}

#[tokio::test]
async fn replay_is_decided_by_the_schema_the_call_was_sent_under() {
    let mut fixture = Fixture::new();
    fixture
        .send(call_request(
            1,
            "flaky_rekeyed",
            json!({"idempotency_key": "once"}),
        ))
        .await;
    let reply = fixture.reply().await;
    let failure = &reply["result"]["structuredContent"];
    assert_eq!(failure["error"], "tool_transport_failed", "{reply}");
    assert_eq!(
        failure["retryable"], false,
        "the tool that ran it promised nothing"
    );
    assert_eq!(
        fixture.script.flaky.load(Ordering::SeqCst),
        1,
        "sent exactly once"
    );
    assert_eq!(
        fixture.script.lists.load(Ordering::SeqCst),
        1,
        "asked once, before sending"
    );
    fixture.close_input().await;
}

#[tokio::test]
async fn a_daemon_that_never_lists_its_tools_cannot_stall_a_keyed_call() {
    let mut fixture = Fixture::new();
    fixture.script.stall_lists.store(true, Ordering::SeqCst);
    fixture
        .send(flaky_request(1, json!({"idempotency_key": "once"})))
        .await;
    let line = tokio::time::timeout(SCHEMA_LOOKUP_TIMEOUT + DEADLINE, fixture.output.next_line())
        .await
        .expect("a stalled tool list is bounded")
        .expect("read reply")
        .expect("reply before EOF");
    let reply: Value = serde_json::from_str(&line).expect("JSON reply");
    let failure = &reply["result"]["structuredContent"];
    assert_eq!(failure["error"], "tool_transport_failed", "{reply}");
    assert_eq!(failure["retryable"], false, "unknown safety is not safe");
    assert_eq!(
        fixture.script.flaky.load(Ordering::SeqCst),
        1,
        "sent exactly once"
    );
    fixture.close_input().await;
}
