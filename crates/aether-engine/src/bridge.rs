//! The MCP bridge: a stdio JSON-RPC server an engine spawns, forwarding
//! every tool call to the daemon as an authenticated engine tool request.
//!
//! stdout is the protocol channel; nothing else may be written to it.

use std::path::{Path, PathBuf};
use std::time::Duration;

use aether_client::{Client, ClientError};
use aether_core::id::AgentId;
use aether_protocol::compat::{MIN_PROTOCOL_VERSION, served_window};
use aether_protocol::{Command, PROTOCOL_VERSION as AETHER_PROTOCOL_VERSION, Reply};
use serde_json::{Value, json};
use tokio::io::{AsyncBufRead, AsyncWrite, AsyncWriteExt as _};
use tokio::task::JoinSet;

const PROTOCOL_VERSION: &str = "2025-06-18";
const SUPPORTED_PROTOCOL_VERSIONS: &[&str] =
    &["2024-11-05", "2025-03-26", "2025-06-18", "2025-11-25"];

/// The permission prompt handler Claude Code calls through
/// `--permission-prompt-tool`. It is the bridge's own, not the daemon's.
fn permission_tool() -> Value {
    json!({
        "name": "permission",
        "description": "Ask Aether whether a tool call may run. Used by the engine's permission prompts.",
        "inputSchema": {
            "type": "object",
            "properties": {
                "tool_name": { "type": "string" },
                "input": { "type": "object" },
                "tool_use_id": { "type": "string" }
            },
            "required": ["tool_name", "input"]
        }
    })
}

/// What the bridge advertises for this turn: the tools the daemon says this
/// agent may use through Aether - its harness tools and any MCP server's -
/// plus the permission handler. Asked per call so a server discovered after
/// the turn started is offered too.
async fn tool_list(client: &Client, agent: AgentId, token: &str) -> anyhow::Result<Vec<Value>> {
    let Reply::EngineTools { tools } = client
        .request(Command::EngineTools {
            agent,
            token: token.to_owned(),
        })
        .await?
    else {
        anyhow::bail!("daemon did not answer with a tool list");
    };
    let mut list: Vec<Value> = tools
        .into_iter()
        .map(|spec| {
            json!({
                "name": spec.name,
                "description": spec.description,
                "inputSchema": spec.input_schema
            })
        })
        .collect();
    list.push(permission_tool());
    Ok(list)
}

// Reject excess daemon work rather than queueing behind indefinite approval waits.
const MAX_IN_FLIGHT: usize = 32;

/// Stands in for the daemon's tools while the daemon cannot be reached, so the
/// reason reaches the model's tool list rather than a closed MCP server.
fn unavailable_tool(reason: &str) -> Value {
    json!({
        "name": "aether_unavailable",
        "description": reason,
        "inputSchema": { "type": "object", "properties": {} }
    })
}

/// The waits before each reconnect to the daemon: bounded, so a daemon that
/// stays away fails the call in about a second rather than hanging the turn.
const RECONNECT_BACKOFF: [Duration; 2] = [Duration::from_millis(200), Duration::from_millis(800)];

/// Why the bridge has no connection, in terms the operator can act on.
struct NoConnection {
    reason: String,
    /// The socket could not be reached at all, as opposed to a daemon that
    /// answered and refused this bridge: only the first can heal by itself.
    transient: bool,
}

/// Whether an error means the connection to the daemon dropped or never
/// opened, rather than a daemon that answered and refused.
fn is_transport_drop(error: &ClientError) -> bool {
    matches!(
        error,
        ClientError::Io(_) | ClientError::Disconnected | ClientError::DeliveryUnconfirmed(_)
    )
}

/// A connection to the daemon, reconnecting with bounded backoff while the
/// socket cannot be reached, or why there is none: the socket, both protocol
/// windows and the fix.
async fn connect(socket: &Path) -> Result<Client, NoConnection> {
    let mut waits = RECONNECT_BACKOFF.iter();
    loop {
        let error = match Client::connect(socket).await {
            Ok(client) => return Ok(client),
            Err(error) => error,
        };
        let transient = is_transport_drop(&error);
        match waits.next().filter(|_| transient) {
            Some(wait) => tokio::time::sleep(*wait).await,
            None => {
                return Err(NoConnection {
                    reason: unreachable_daemon(socket, &error),
                    transient,
                });
            }
        }
    }
}

fn unreachable_daemon(socket: &Path, error: &ClientError) -> String {
    let daemon = match error {
        ClientError::Incompatible(refusal) => match served_window(refusal) {
            Some(window) => format!(
                "the daemon serves protocol {} to {}",
                window.start(),
                window.end()
            ),
            None => "the daemon did not say which protocol it serves".to_owned(),
        },
        _ => "the daemon's protocol is unknown".to_owned(),
    };
    format!(
        "Aether's tools are unavailable: the MCP bridge could not connect to the daemon at {} \
         ({error}). This bridge speaks protocol {MIN_PROTOCOL_VERSION} to \
         {AETHER_PROTOCOL_VERSION}; {daemon}. Fix: run `aether stop`, then start Aether again \
         so the daemon comes from the same build as this bridge.",
        socket.display()
    )
}

fn tool_error(id: Value, text: &str) -> Value {
    json!({
        "jsonrpc": "2.0", "id": id,
        "result": { "content": [{ "type": "text", "text": text }], "isError": true }
    })
}

/// A call whose connection to the daemon failed: never a bare "connection
/// closed", so the model and its host can tell whether to call again.
fn transport_failed(id: Value, tool: &str, retryable: bool, cause: &str) -> Value {
    let tool = if tool.is_empty() {
        "(unnamed tool)"
    } else {
        tool
    };
    let failure = aether_core::mcp::tool_transport_failed(tool, retryable, cause);
    json!({
        "jsonrpc": "2.0", "id": id,
        "result": {
            "content": [{ "type": "text", "text": failure.to_string() }],
            "structuredContent": failure,
            "isError": true
        }
    })
}

/// Whether the model gave this call an idempotency key, which makes sending it
/// to the daemon again one effect rather than two.
fn carries_idempotency_key(input: &Value) -> bool {
    input
        .get("idempotency_key")
        .and_then(Value::as_str)
        .is_some_and(|key| !key.trim().is_empty())
}

/// Serve MCP on stdin/stdout until the engine closes the pipe. A daemon that
/// cannot be reached is reported per request rather than by exiting.
pub async fn run(socket: PathBuf, agent: AgentId, token: String) -> anyhow::Result<()> {
    serve(
        socket,
        agent,
        token,
        tokio::io::BufReader::new(tokio::io::stdin()),
        tokio::io::stdout(),
    )
    .await
}

async fn serve(
    socket: PathBuf,
    agent: AgentId,
    token: String,
    input: impl AsyncBufRead + Unpin,
    mut output: impl AsyncWrite + Unpin,
) -> anyhow::Result<()> {
    let mut lines = crate::output::BoundedLines::new(input);
    let mut pending = JoinSet::new();
    loop {
        let response = tokio::select! {
            Some(completed) = pending.join_next(), if !pending.is_empty() => completed?,
            line = lines.next_line() => {
                let Some(line) = line? else {
                    pending.shutdown().await;
                    return Ok(());
                };
                if line.trim().is_empty() {
                    continue;
                }
                let req: Value = match serde_json::from_str(&line) {
                    Ok(req) => req,
                    Err(error) => {
                        tracing::warn!(%error, "bridge: malformed request");
                        continue;
                    }
                };
                let Some(id) = req.get("id") else { continue };
                if matches!(req["method"].as_str(), Some("tools/call" | "tools/list")) {
                    if pending.len() >= MAX_IN_FLIGHT {
                        json!({ "jsonrpc": "2.0", "id": id, "error": {
                            "code": -32603, "message": "bridge has too many in-flight daemon requests; retry after one completes"
                        } })
                    } else {
                        let socket = socket.clone();
                        let token = token.clone();
                        pending.spawn(async move { handle_request(&socket, agent, &token, req).await });
                        continue;
                    }
                } else {
                    handle_request(&socket, agent, &token, req).await
                }
            }
        };
        let mut text = serde_json::to_string(&response)?;
        text.push('\n');
        output.write_all(text.as_bytes()).await?;
        output.flush().await?;
    }
}

async fn handle_request(socket: &Path, agent: AgentId, token: &str, req: Value) -> Value {
    let id = req["id"].clone();
    let method = req.get("method").and_then(Value::as_str).unwrap_or("");
    match method {
        "initialize" => tracing::info!(
            client = %req["params"]["clientInfo"]["name"].as_str().unwrap_or("?"),
            %agent,
            "bridge: engine connected"
        ),
        "tools/call" => tracing::info!(
            tool = %req["params"]["name"].as_str().unwrap_or("?"),
            %agent,
            "bridge: tool call"
        ),
        _ => tracing::debug!(method, %agent, "bridge: request"),
    }
    match method {
        "initialize" => json!({
            "jsonrpc": "2.0", "id": id,
            "result": {
                "protocolVersion": req["params"].get("protocolVersion")
                    .and_then(Value::as_str)
                    .filter(|version| SUPPORTED_PROTOCOL_VERSIONS.contains(version))
                    .unwrap_or(PROTOCOL_VERSION),
                "capabilities": { "tools": {} },
                "serverInfo": { "name": "aether", "version": env!("CARGO_PKG_VERSION") }
            }
        }),
        "ping" => json!({ "jsonrpc": "2.0", "id": id, "result": {} }),
        "tools/list" => list_tools(socket, agent, token, id).await,
        "tools/call" => call_tool(socket, agent, token, id, &req["params"]).await,
        other => {
            json!({ "jsonrpc": "2.0", "id": id, "error": { "code": -32601, "message": format!("unknown method {other}") } })
        }
    }
}

async fn list_tools(socket: &Path, agent: AgentId, token: &str, id: Value) -> Value {
    let client = match connect(socket).await {
        Ok(client) => client,
        Err(NoConnection { reason, .. }) => {
            tracing::warn!(%agent, "bridge: {reason}");
            let tools = [permission_tool(), unavailable_tool(&reason)];
            return json!({ "jsonrpc": "2.0", "id": id, "result": { "tools": tools } });
        }
    };
    match tool_list(&client.without_request_timeout(), agent, token).await {
        Ok(tools) => json!({ "jsonrpc": "2.0", "id": id, "result": { "tools": tools } }),
        Err(e) => {
            json!({ "jsonrpc": "2.0", "id": id, "error": { "code": -32603, "message": e.to_string() } })
        }
    }
}

async fn call_tool(socket: &Path, agent: AgentId, token: &str, id: Value, params: &Value) -> Value {
    let name = params
        .get("name")
        .and_then(Value::as_str)
        .unwrap_or("")
        .to_owned();
    let input = params.get("arguments").cloned().unwrap_or(json!({}));
    let replay_safe = carries_idempotency_key(&input);
    let mut waits = RECONNECT_BACKOFF.iter();
    loop {
        let client = match connect(socket).await {
            Ok(client) => client,
            Err(NoConnection { reason, transient }) => {
                tracing::warn!(%agent, tool = %name, "bridge: {reason}");
                // Nothing reached the daemon, so calling again is safe.
                return match transient {
                    true => transport_failed(id, &name, true, &reason),
                    false => tool_error(id, &reason),
                };
            }
        };
        // Approval and native wait replies deliberately have no deadline.
        let reply = client
            .without_request_timeout()
            .request(Command::EngineTool {
                agent,
                token: token.to_owned(),
                name: name.clone(),
                input: input.clone(),
            })
            .await;
        let error = match reply {
            Err(error) if is_transport_drop(&error) => error,
            reply => return answered(id, reply),
        };
        // The daemon may already have run it: only a keyed call is sent again.
        let Some(wait) = waits.next().filter(|_| replay_safe) else {
            tracing::warn!(%agent, tool = %name, %error, "bridge: the daemon connection dropped mid-call");
            return transport_failed(
                id,
                &name,
                false,
                &format!("the connection to the Aether daemon dropped mid-call: {error}"),
            );
        };
        tracing::info!(%agent, tool = %name, %error, "bridge: replaying a keyed call after a dropped connection");
        tokio::time::sleep(*wait).await;
    }
}

/// The engine's answer to a call the daemon replied to, or refused.
fn answered(id: Value, reply: Result<Reply, ClientError>) -> Value {
    match reply {
        Ok(Reply::EngineTool { content, is_error }) => json!({
            "jsonrpc": "2.0", "id": id,
            "result": { "content": [{ "type": "text", "text": content }], "isError": is_error }
        }),
        Ok(other) => {
            json!({ "jsonrpc": "2.0", "id": id, "error": { "code": -32603, "message": format!("unexpected reply {other:?}") } })
        }
        Err(e) => {
            json!({ "jsonrpc": "2.0", "id": id, "error": { "code": -32603, "message": e.to_string() } })
        }
    }
}

#[cfg(test)]
mod tests;
