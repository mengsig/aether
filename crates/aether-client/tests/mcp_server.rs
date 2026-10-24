#![allow(clippy::expect_used, clippy::unwrap_used, clippy::panic)]

//! Aether as an MCP server, driven by Aether's own MCP client.
//!
//! The daemon is replaced by a double that records the protocol commands it
//! receives, so the test asserts the thing that matters: an MCP tool call
//! becomes exactly the protocol command a person's CLI would send.

use std::sync::{Arc, Mutex};

use aether_protocol::{Command, DaemonStatus, Reply};
use async_trait::async_trait;
use serde_json::{Value, json};

/// A stand-in daemon: answers plausibly and remembers what it was asked.
#[derive(Debug, Default)]
struct RecordingSink {
    seen: Mutex<Vec<Command>>,
}

impl RecordingSink {
    fn seen(&self) -> Vec<Command> {
        self.seen.lock().expect("lock").clone()
    }
}

#[async_trait]
impl aether_client::mcp::CommandSink for RecordingSink {
    async fn call(&self, command: Command) -> anyhow::Result<Reply> {
        self.seen.lock().expect("lock").push(command.clone());
        Ok(match command {
            Command::Status => Reply::Status(DaemonStatus {
                problems: Vec::new(),
                instance_id: "inst".into(),
                version: "test".into(),
                pid: 1,
                started_at: aether_core::time::now(),
                last_seq: 7,
                agents_live: 2,
                agents_running: 1,
                agents_waiting: 0,
                agents_blocked: 0,
                agents_suspended: 0,
                agents_paused: 0,
                tasks_active: 1,
                pending_approvals: 0,
                default_model: None,
                usage_total: Default::default(),
                yolo: false,
                engines: vec!["claude-code/1".into()],
                providers: Vec::new(),
                resource_leases: Vec::new(),
                runs: Vec::new(),
                approval_default: Default::default(),
                network_default: Default::default(),
                max_concurrent_turns: None,
                max_concurrent_engine_turns: None,
                engine_turns_running: 0,
                run_budget_usd: None,
                run_hard_stop_usd: None,
                run_hard_stop_tokens: None,
                mcp: Vec::new(),
                low_disk: None,
                writes: None,
                bridge: None,
            }),
            Command::ListAgents { .. } => Reply::Agents(Vec::new()),
            Command::SendMessage { .. } => Reply::MessageSent {
                message: aether_core::id::MessageId::new(),
            },
            other => anyhow::bail!("the double was not asked for {other:?}"),
        })
    }
}

/// Speak MCP to the server over an in-process pipe, the way a real client
/// would over stdio.
async fn client_for(sink: Arc<RecordingSink>) -> aether_tools::mcp::McpClientHandle {
    let (ours, theirs) = tokio::io::duplex(64 * 1024);
    let (server_reader, server_writer) = tokio::io::split(theirs);
    tokio::spawn(async move {
        let _ = aether_client::mcp::stdio::serve(
            &*sink,
            tokio::io::BufReader::new(server_reader),
            server_writer,
        )
        .await;
    });
    let (reader, writer) = tokio::io::split(ours);
    aether_tools::mcp::McpClientHandle::over_streams(
        Box::new(writer),
        Box::new(tokio::io::BufReader::new(reader)),
        std::time::Duration::from_secs(10),
    )
    .await
    .expect("handshake")
}

/// Long enough to pass the strength floor, and obviously a fixture.
const TOKEN: &str = "test-operator-token-0123456789abcdef";
/// The per-turn token the recording daemon says is live, and the agent it
/// belongs to.
const TURN_TOKEN: &str = "01JZZZZZZZZZZZZZZZZZZZZZZZ";

fn live_agent() -> aether_core::id::AgentId {
    aether_core::id::AgentId::new()
}

fn config(addr: &str) -> aether_client::mcp::http::HttpConfig {
    aether_client::mcp::http::HttpConfig {
        addr: addr.parse().expect("addr"),
        token: TOKEN.to_owned(),
        allow_remote: false,
    }
}

/// Serve on a free loopback port and return the address plus the handle.
async fn serve_http(
    sink: Arc<dyn aether_client::mcp::CommandSink>,
    shutdown: tokio_util::sync::CancellationToken,
) -> (
    std::net::SocketAddr,
    tokio::task::JoinHandle<anyhow::Result<()>>,
) {
    let (tx, rx) = tokio::sync::oneshot::channel();
    let serving = tokio::spawn(async move {
        // Port 0: the kernel picks a free loopback port for the test.
        aether_client::mcp::http::serve(sink, config("127.0.0.1:0"), shutdown, move |bound| {
            let _ = tx.send(bound);
        })
        .await
    });
    (rx.await.expect("bound address"), serving)
}

#[path = "mcp_server/http.rs"]
mod http;
#[path = "mcp_server/stdio.rs"]
mod stdio;
#[path = "mcp_server/turn_tokens.rs"]
mod turn_tokens;
