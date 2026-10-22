//! JSON-RPC transports for the MCP client: a child process over stdio, and
//! streamable HTTP.
//!
//! Both are strictly request/response: one call is in flight per server at a
//! time. That keeps correlation trivial and matches how a stdio server is
//! usually written; the manager's per-server lock enforces it. Every request
//! is bounded by a timeout, so a server that stops answering fails the call
//! instead of hanging the turn.

use std::sync::Mutex;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

use async_trait::async_trait;
use serde_json::{Value, json};
use tokio::io::{AsyncBufRead, AsyncBufReadExt as _, AsyncWrite, AsyncWriteExt as _};

use super::{MAX_MESSAGE_BYTES, McpError};

/// Stray messages (server notifications, replies to a request we gave up on)
/// tolerated while waiting for one response before the stream is called lost.
const MAX_INTERLEAVED: usize = 64;

#[async_trait]
pub(crate) trait Transport: Send + Sync + std::fmt::Debug {
    fn set_protocol_version(&self, _version: &str) -> Result<(), McpError> {
        Ok(())
    }

    async fn request(&self, method: &str, params: Value) -> Result<Value, McpError>;
    async fn notify(&self, method: &str, params: Value) -> Result<(), McpError>;
}

fn envelope(id: u64, method: &str, params: Value) -> Value {
    json!({ "jsonrpc": "2.0", "id": id, "method": method, "params": params })
}

/// Turn a JSON-RPC response body into its `result`, or the server's error.
fn unwrap_result(message: Value, id: u64) -> Result<Value, McpError> {
    if message.get("jsonrpc").and_then(Value::as_str) != Some("2.0") {
        return Err(McpError::Protocol("response must use JSON-RPC 2.0".into()));
    }
    if message.get("id").and_then(Value::as_u64) != Some(id) {
        return Err(McpError::Protocol(
            "response id does not match the request".into(),
        ));
    }
    match (message.get("result"), message.get("error")) {
        (Some(result), None) => Ok(result.clone()),
        (None, Some(error)) => {
            let code = error.get("code").and_then(Value::as_i64).ok_or_else(|| {
                McpError::Protocol("response error must have an integer code".into())
            })?;
            let text = error
                .get("message")
                .and_then(Value::as_str)
                .ok_or_else(|| {
                    McpError::Protocol("response error must have a string message".into())
                })?;
            Err(McpError::Server {
                code,
                message: text.to_owned(),
            })
        }
        _ => Err(McpError::Protocol(
            "response must contain exactly one of result or error".into(),
        )),
    }
}

type Writer = Box<dyn AsyncWrite + Send + Unpin>;
type Reader = Box<dyn AsyncBufRead + Send + Unpin>;

struct StdioIo {
    writer: Writer,
    reader: Reader,
}

/// Newline-delimited JSON-RPC over a pair of pipes. Owns the child process
/// when it started one, so dropping the transport ends the server.
pub(crate) struct StdioTransport {
    io: tokio::sync::Mutex<StdioIo>,
    child: Mutex<Option<StdioChild>>,
    next_id: AtomicU64,
    timeout: Duration,
}

struct StdioChild {
    child: tokio::process::Child,
    #[cfg(target_os = "linux")]
    job: aether_core::id::ProcessId,
    #[cfg(target_os = "linux")]
    registration: Option<crate::background::journal::Registration>,
}

impl Drop for StdioChild {
    fn drop(&mut self) {
        // The unreaped leader pins its PID until this guard drops the Child.
        #[cfg(target_os = "linux")]
        let mut group_cleaned = true;
        if let Some(pid) = self.child.id()
            && let Err(error) = nix::sys::signal::killpg(
                nix::unistd::Pid::from_raw(pid as i32),
                nix::sys::signal::Signal::SIGKILL,
            )
            && error != nix::errno::Errno::ESRCH
        {
            tracing::error!(pid, %error, "cannot kill MCP server process group");
            #[cfg(target_os = "linux")]
            {
                group_cleaned = false;
            }
        }
        #[cfg(target_os = "linux")]
        match crate::background::journal::kill_job(self.job) {
            Ok(()) => {
                if group_cleaned
                    && let Some(registration) = self.registration.take()
                    && let Err(error) = registration.clear()
                {
                    tracing::error!(%error, "cannot clear MCP process journal");
                }
            }
            Err(error) => tracing::error!(%error, "cannot kill escaped MCP descendants"),
        }
    }
}

impl std::fmt::Debug for StdioTransport {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("StdioTransport")
            .field("timeout", &self.timeout)
            .finish_non_exhaustive()
    }
}

impl StdioTransport {
    /// Drive an already-connected pair of streams. Used by tests and by
    /// anything that speaks MCP over something other than a child process.
    pub(crate) fn over(writer: Writer, reader: Reader, timeout: Duration) -> Self {
        Self {
            io: tokio::sync::Mutex::new(StdioIo { writer, reader }),
            child: Mutex::new(None),
            next_id: AtomicU64::new(1),
            timeout,
        }
    }

    /// Spawn the server and speak to it over its stdin and stdout. Its stderr
    /// is inherited so a misconfigured server explains itself in the log.
    pub(crate) fn spawn(
        program: &std::path::Path,
        args: &[String],
        env: &std::collections::BTreeMap<String, String>,
        hidden_env: &[String],
        cwd: Option<&std::path::Path>,
        timeout: Duration,
        background: Option<&crate::background::BackgroundProcesses>,
    ) -> Result<Self, McpError> {
        if let Some(name) = env.keys().find(|key| hidden_env.contains(key)) {
            return Err(McpError::Transport(format!(
                "`env` sets `{name}`, a credential the daemon hides from tool children; \
                 remove it from the server's `env`"
            )));
        }
        let mut command = tokio::process::Command::new(program);
        command
            .args(args)
            .envs(env)
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::inherit())
            .kill_on_drop(true)
            .process_group(0);
        aether_spawn::reset_signals(command.as_std_mut());
        // A server runs an agent's tool calls, so it is a tool child: the
        // daemon's named credentials must not travel into it either.
        for name in hidden_env {
            command.env_remove(name);
        }
        if let Some(dir) = cwd {
            command.current_dir(dir);
        }
        #[cfg(target_os = "linux")]
        let job = aether_core::id::ProcessId::new();
        #[cfg(target_os = "linux")]
        command.env(crate::background::journal::JOB_ENV, job.to_string());
        let mut child = StdioChild {
            child: command.spawn().map_err(|e| {
                McpError::Transport(format!("cannot start `{}`: {e}", program.display()))
            })?,
            #[cfg(target_os = "linux")]
            job,
            #[cfg(target_os = "linux")]
            registration: None,
        };
        #[cfg(target_os = "linux")]
        if let Some(background) = background {
            child.registration = background
                .register_host_child(
                    job,
                    child
                        .child
                        .id()
                        .ok_or_else(|| McpError::Transport("MCP child has no PID".into()))?,
                )
                .map_err(|error| McpError::Transport(format!("journaling MCP process: {error}")))?;
        }
        #[cfg(not(target_os = "linux"))]
        let _ = background;
        let (Some(stdin), Some(stdout)) = (child.child.stdin.take(), child.child.stdout.take())
        else {
            return Err(McpError::Transport("server pipes were not created".into()));
        };
        let transport = Self::over(
            Box::new(stdin),
            Box::new(tokio::io::BufReader::new(stdout)),
            timeout,
        );
        *transport
            .child
            .lock()
            .map_err(|_| McpError::Transport("child lock poisoned".into()))? = Some(child);
        Ok(transport)
    }

    async fn send(io: &mut StdioIo, message: &Value) -> Result<(), McpError> {
        let mut line = serde_json::to_string(message)
            .map_err(|e| McpError::Transport(format!("cannot encode request: {e}")))?;
        line.push('\n');
        io.writer
            .write_all(line.as_bytes())
            .await
            .map_err(|e| McpError::Transport(format!("write failed: {e}")))?;
        io.writer
            .flush()
            .await
            .map_err(|e| McpError::Transport(format!("flush failed: {e}")))
    }

    /// Read until the reply with `id` arrives, skipping notifications.
    async fn read_reply(io: &mut StdioIo, id: u64) -> Result<Value, McpError> {
        for _ in 0..MAX_INTERLEAVED {
            let Some(line) = read_bounded_line(&mut io.reader).await? else {
                return Err(McpError::Closed);
            };
            let line = String::from_utf8_lossy(&line);
            let line = line.trim();
            if line.is_empty() {
                continue;
            }
            let message: Value = match serde_json::from_str(line) {
                Ok(v) => v,
                Err(e) => {
                    tracing::warn!(error = %e, "mcp: server sent a malformed line");
                    continue;
                }
            };
            if message.get("method").is_none()
                && message.get("id").and_then(Value::as_u64) == Some(id)
            {
                return unwrap_result(message, id);
            }
        }
        Err(McpError::Transport(
            "server did not answer within the tolerated number of messages".into(),
        ))
    }
}

/// One newline-terminated message, or `None` at end of stream. Bounded: a
/// server that writes forever without a newline would otherwise grow the
/// daemon's memory until the request timeout fires, tens of GB later.
async fn read_bounded_line(reader: &mut Reader) -> Result<Option<Vec<u8>>, McpError> {
    let mut line: Vec<u8> = Vec::new();
    loop {
        let (taken, complete) = {
            let buffered = reader
                .fill_buf()
                .await
                .map_err(|e| McpError::Transport(format!("read failed: {e}")))?;
            if buffered.is_empty() {
                return Ok((!line.is_empty()).then_some(line));
            }
            match buffered.iter().position(|b| *b == b'\n') {
                Some(at) => {
                    line.extend_from_slice(&buffered[..at]);
                    (at + 1, true)
                }
                None => {
                    line.extend_from_slice(buffered);
                    (buffered.len(), false)
                }
            }
        };
        reader.consume(taken);
        if line.len() > MAX_MESSAGE_BYTES {
            // Transport, not Protocol: the rest of the oversized line is
            // still in the pipe, so framing is lost and every later read
            // would be garbage. Fatal drops the connection and fails the
            // server, rather than leaving it advertised as Ready.
            return Err(McpError::Transport(format!(
                "a server message passed the {MAX_MESSAGE_BYTES}-byte cap; the stream can no \
                 longer be framed"
            )));
        }
        if complete {
            return Ok(Some(line));
        }
    }
}

#[async_trait]
impl Transport for StdioTransport {
    async fn request(&self, method: &str, params: Value) -> Result<Value, McpError> {
        let id = self.next_id.fetch_add(1, Ordering::Relaxed);
        let mut io = self.io.lock().await;
        tokio::time::timeout(self.timeout, async {
            Self::send(&mut io, &envelope(id, method, params)).await?;
            Self::read_reply(&mut io, id).await
        })
        .await
        .map_err(|_| McpError::Timeout)?
    }

    async fn notify(&self, method: &str, params: Value) -> Result<(), McpError> {
        let message = json!({ "jsonrpc": "2.0", "method": method, "params": params });
        let mut io = self.io.lock().await;
        tokio::time::timeout(self.timeout, Self::send(&mut io, &message))
            .await
            .map_err(|_| McpError::Timeout)?
    }
}

/// Streamable HTTP: every message is a POST to one endpoint; the answer is
/// either a JSON body or an SSE stream carrying it.
pub(crate) struct HttpTransport {
    client: reqwest::Client,
    url: String,
    /// Bearer token, read once from the environment the daemon started with.
    token: Option<String>,
    /// Session id the server assigned at initialize, echoed on later calls.
    session: Mutex<Option<String>>,
    protocol_version: Mutex<Option<String>>,
    next_id: AtomicU64,
}

impl std::fmt::Debug for HttpTransport {
    /// The token never reaches a log line, however this is formatted.
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("HttpTransport")
            .field("url", &self.url)
            .field("authenticated", &self.token.is_some())
            .finish_non_exhaustive()
    }
}

impl HttpTransport {
    pub(crate) fn new(
        url: String,
        token: Option<String>,
        timeout: Duration,
    ) -> Result<Self, McpError> {
        let client = reqwest::Client::builder()
            .timeout(timeout)
            // The endpoint the operator configured is the endpoint that is
            // spoken to. `check_url` refuses cleartext off loopback at load;
            // following a redirect would send the agent's tool arguments to
            // whatever host, or scheme, the server named instead.
            .redirect(reqwest::redirect::Policy::none())
            .build()
            .map_err(|e| McpError::Transport(format!("cannot build http client: {e}")))?;
        Ok(Self {
            client,
            url,
            token,
            session: Mutex::new(None),
            protocol_version: Mutex::new(None),
            next_id: AtomicU64::new(1),
        })
    }

    async fn post(&self, message: &Value) -> Result<Option<Value>, McpError> {
        let mut request = self
            .client
            .post(&self.url)
            .header("content-type", "application/json")
            .header("accept", "application/json, text/event-stream");
        let protocol_version = self
            .protocol_version
            .lock()
            .map_err(|_| McpError::Transport("protocol version lock poisoned".into()))?
            .clone();
        if let Some(version) = protocol_version {
            request = request.header("mcp-protocol-version", version);
        }
        if let Some(token) = &self.token {
            request = request.header("authorization", format!("Bearer {token}"));
        }
        let session = self
            .session
            .lock()
            .map_err(|_| McpError::Transport("session lock poisoned".into()))?
            .clone();
        if let Some(id) = session {
            request = request.header("mcp-session-id", id);
        }
        let response = request
            .json(message)
            .send()
            .await
            .map_err(|e| McpError::Transport(format!("request failed: {e}")))?;
        if let Some(id) = response
            .headers()
            .get("mcp-session-id")
            .and_then(|v| v.to_str().ok())
            .map(str::to_owned)
        {
            *self
                .session
                .lock()
                .map_err(|_| McpError::Transport("session lock poisoned".into()))? = Some(id);
        }
        let status = response.status();
        let content_type = response
            .headers()
            .get("content-type")
            .and_then(|v| v.to_str().ok())
            .unwrap_or_default()
            .to_owned();
        let body = read_bounded_body(response).await?;
        if !status.is_success() {
            // Never the body itself: it is the peer's text, it becomes the
            // server's durable health error, and a server that echoes the
            // request would write our own bearer token into the log.
            return Err(McpError::Transport(format!(
                "http {} with a {}-byte body",
                status.as_u16(),
                body.len()
            )));
        }
        if body.trim().is_empty() {
            return Ok(None);
        }
        if content_type.starts_with("text/event-stream") {
            // A notification carries no id and expects no reply.
            let Some(id) = message.get("id").and_then(Value::as_u64) else {
                return Ok(None);
            };
            return Ok(sse_reply(&body, id));
        }
        serde_json::from_str(&body)
            .map(Some)
            .map_err(|e| McpError::Transport(format!("malformed response body: {e}")))
    }
}

/// The whole response body as text, refusing one past the cap. reqwest
/// applies no limit of its own, so a hostile server could stream until the
/// daemon dies; a declared length over the cap is refused before any read.
async fn read_bounded_body(mut response: reqwest::Response) -> Result<String, McpError> {
    let too_big = |seen: usize| {
        McpError::Protocol(format!(
            "a server response of {seen} bytes passed the {MAX_MESSAGE_BYTES}-byte cap"
        ))
    };
    if let Some(declared) = response.content_length()
        && declared > MAX_MESSAGE_BYTES as u64
    {
        return Err(too_big(declared as usize));
    }
    let mut bytes: Vec<u8> = Vec::new();
    while let Some(chunk) = response
        .chunk()
        .await
        .map_err(|e| McpError::Transport(format!("cannot read response: {e}")))?
    {
        bytes.extend_from_slice(&chunk);
        if bytes.len() > MAX_MESSAGE_BYTES {
            return Err(too_big(bytes.len()));
        }
    }
    String::from_utf8(bytes)
        .map_err(|e| McpError::Transport(format!("response body is not utf-8: {e}")))
}

/// The reply to `id` carried by an SSE body. A server may put log
/// notifications, progress and pings on the stream before it answers, so the
/// answer is found by JSON-RPC id - the same correlation the stdio transport
/// does - rather than by taking whatever arrived first.
fn sse_reply(body: &str, id: u64) -> Option<Value> {
    sse_messages(body).into_iter().find(|message| {
        message.get("method").is_none() && message.get("id").and_then(Value::as_u64) == Some(id)
    })
}

/// Every JSON-RPC message carried by an SSE body, in order. One event's data
/// may span several `data:` lines, joined with newlines before parsing; a
/// blank line ends the event.
fn sse_messages(body: &str) -> Vec<Value> {
    let mut messages = Vec::new();
    let mut data = String::new();
    let end = |data: &mut String, messages: &mut Vec<Value>| {
        if !data.is_empty() {
            if let Ok(value) = serde_json::from_str(data) {
                messages.push(value);
            }
            data.clear();
        }
    };
    for line in body.lines() {
        match line.strip_prefix("data:") {
            Some(chunk) => {
                if !data.is_empty() {
                    data.push('\n');
                }
                data.push_str(chunk.strip_prefix(' ').unwrap_or(chunk));
            }
            None if line.trim().is_empty() => end(&mut data, &mut messages),
            None => {}
        }
    }
    end(&mut data, &mut messages);
    messages
}

#[async_trait]
impl Transport for HttpTransport {
    fn set_protocol_version(&self, version: &str) -> Result<(), McpError> {
        *self
            .protocol_version
            .lock()
            .map_err(|_| McpError::Transport("protocol version lock poisoned".into()))? =
            Some(version.to_owned());
        Ok(())
    }

    async fn request(&self, method: &str, params: Value) -> Result<Value, McpError> {
        let id = self.next_id.fetch_add(1, Ordering::Relaxed);
        match self.post(&envelope(id, method, params)).await? {
            Some(message) => unwrap_result(message, id),
            None => Err(McpError::Transport(
                "the server sent no reply to this request".into(),
            )),
        }
    }

    async fn notify(&self, method: &str, params: Value) -> Result<(), McpError> {
        let message = json!({ "jsonrpc": "2.0", "method": method, "params": params });
        self.post(&message).await.map(|_| ())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    async fn http_peer(
        replies: Vec<(&'static str, Option<&'static str>, Option<Value>)>,
    ) -> (HttpTransport, tokio::task::JoinHandle<()>) {
        use tokio::io::AsyncReadExt as _;

        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind");
        let address = listener.local_addr().expect("address");
        let serving = tokio::spawn(async move {
            for (method, version, reply) in replies {
                let (socket, _) = listener.accept().await.expect("accept");
                let mut socket = tokio::io::BufReader::new(socket);
                let mut headers = String::new();
                loop {
                    let mut line = String::new();
                    assert_ne!(socket.read_line(&mut line).await.expect("header"), 0);
                    if line == "\r\n" {
                        break;
                    }
                    headers.push_str(&line.to_ascii_lowercase());
                }
                let length: usize = headers
                    .lines()
                    .find_map(|line| line.strip_prefix("content-length: "))
                    .expect("length")
                    .trim()
                    .parse()
                    .expect("number");
                let mut body = vec![0; length];
                socket.read_exact(&mut body).await.expect("body");
                let request: Value = serde_json::from_slice(&body).expect("request JSON");
                assert_eq!(request["method"], method);
                if method == "initialize" {
                    assert_eq!(request["params"]["protocolVersion"], "2025-11-25");
                }
                let sent_version = headers
                    .lines()
                    .find_map(|line| line.strip_prefix("mcp-protocol-version: "));
                assert_eq!(sent_version, version);
                let body = reply.map(|v| v.to_string()).unwrap_or_default();
                let response = format!(
                    "HTTP/1.1 200 OK\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{body}",
                    body.len()
                );
                socket
                    .get_mut()
                    .write_all(response.as_bytes())
                    .await
                    .expect("reply");
            }
        });
        let transport = HttpTransport::new(
            format!("http://{address}/mcp"),
            None,
            Duration::from_secs(5),
        )
        .expect("transport");
        (transport, serving)
    }

    #[tokio::test]
    async fn http_json_replies_must_match_the_request_id() {
        for id in [json!(2), json!("1"), Value::Null] {
            let (transport, serving) = http_peer(vec![(
                "ping",
                None,
                Some(json!({"jsonrpc": "2.0", "id": id, "result": {}})),
            )])
            .await;
            assert!(matches!(
                transport.request("ping", json!({})).await,
                Err(McpError::Protocol(_))
            ));
            serving.await.expect("server");
        }
    }

    #[tokio::test]
    async fn http_posts_use_the_negotiated_version_after_initialization() {
        let (transport, serving) = http_peer(vec![
            (
                "initialize",
                None,
                Some(
                    json!({"jsonrpc": "2.0", "id": 1, "result": {"protocolVersion": "2025-03-26"}}),
                ),
            ),
            ("notifications/initialized", Some("2025-03-26"), None),
            (
                "tools/list",
                Some("2025-03-26"),
                Some(json!({"jsonrpc": "2.0", "id": 2, "result": {"tools": []}})),
            ),
            (
                "tools/call",
                Some("2025-03-26"),
                Some(json!({"jsonrpc": "2.0", "id": 3})),
            ),
            (
                "tools/call",
                Some("2025-03-26"),
                Some(json!({"jsonrpc": "2.0", "id": 4, "result": {"content": []}})),
            ),
        ])
        .await;
        let client = super::super::client::McpClient::new(std::sync::Arc::new(transport));
        client.initialize().await.expect("initialize");
        assert!(client.list_tools().await.expect("tools").tools.is_empty());
        assert!(matches!(
            client.call_tool("empty", json!({}), None).await,
            Err(McpError::Protocol(_))
        ));
        let empty = client
            .call_tool("empty", json!({}), None)
            .await
            .expect("valid empty success");
        assert!(empty.text.is_empty());
        assert!(!empty.is_error);
        serving.await.expect("server");
    }

    #[tokio::test]
    async fn stdio_missing_result_is_not_a_successful_empty_call() {
        let reply = b"{\"jsonrpc\":\"2.0\",\"id\":1}\n";
        let transport = StdioTransport::over(
            Box::new(tokio::io::sink()),
            Box::new(tokio::io::BufReader::new(std::io::Cursor::new(reply))),
            Duration::from_secs(5),
        );
        let client = super::super::client::McpClient::new(std::sync::Arc::new(transport));
        assert!(matches!(
            client.call_tool("empty", json!({}), None).await,
            Err(McpError::Protocol(_))
        ));
    }

    #[test]
    fn malformed_response_envelopes_are_protocol_errors() {
        for message in [
            json!({"jsonrpc": "2.0", "id": 1}),
            json!({"jsonrpc": "2.0", "id": 1, "result": {}, "error": {"code": 1, "message": "no"}}),
            json!({"jsonrpc": "1.0", "id": 1, "result": {}}),
            json!({"id": 1, "result": {}}),
            json!({"jsonrpc": "2.0", "id": 1, "error": null}),
            json!({"jsonrpc": "2.0", "id": 1, "error": {"code": "bad", "message": "no"}}),
        ] {
            assert!(
                matches!(
                    unwrap_result(message.clone(), 1),
                    Err(McpError::Protocol(_))
                ),
                "{message}"
            );
        }
        assert_eq!(
            unwrap_result(json!({"jsonrpc": "2.0", "id": 1, "result": null}), 1)
                .expect("explicit null"),
            Value::Null
        );
    }

    #[test]
    fn a_server_error_becomes_an_error_not_a_result() {
        let message =
            json!({"jsonrpc": "2.0", "id": 1, "error": {"code": -32601, "message": "no"}});
        match unwrap_result(message, 1) {
            Err(McpError::Server { code, message }) => {
                assert_eq!(code, -32601);
                assert_eq!(message, "no");
            }
            other => panic!("unexpected {other:?}"),
        }
    }

    #[test]
    fn an_sse_reply_is_found_by_id_and_anything_else_is_skipped() {
        let body = concat!(
            "event: message\n",
            "data: {\"jsonrpc\":\"2.0\",\"method\":\"notifications/message\",\"params\":{}}\n",
            "\n",
            "event: message\n",
            "data: {\"jsonrpc\":\"2.0\",\"id\":7,\"method\":\"ping\"}\n\n",
            "event: message\n",
            "data: {\"jsonrpc\":\"2.0\",\"id\":7,\"result\":{\"ok\":true}}\n",
            "\n",
        );
        let message = sse_reply(body, 7).expect("the reply to 7");
        assert_eq!(message["result"]["ok"], json!(true));
        assert!(sse_reply(body, 8).is_none(), "no reply is not some reply");
        assert!(sse_reply("event: ping\n\n", 1).is_none());

        // Several data lines are one event.
        let split = "data: {\"jsonrpc\":\"2.0\",\"id\":1,\ndata: \"result\":{}}\n\n";
        assert!(sse_reply(split, 1).is_some(), "{split}");
    }

    #[tokio::test]
    async fn a_configured_env_key_that_is_hidden_is_refused_by_name() {
        let env = std::collections::BTreeMap::from([("MCP_TOKEN".to_owned(), "x".to_owned())]);
        let error = StdioTransport::spawn(
            std::path::Path::new("true"),
            &[],
            &env,
            &["MCP_TOKEN".to_owned()],
            None,
            Duration::from_secs(5),
            None,
        )
        .unwrap_err();
        assert!(error.to_string().contains("MCP_TOKEN"), "{error}");

        let allowed = StdioTransport::spawn(
            std::path::Path::new("true"),
            &[],
            &env,
            &["OTHER".to_owned()],
            None,
            Duration::from_secs(5),
            None,
        );
        assert!(allowed.is_ok(), "an unrelated hidden name must not block");
    }

    #[cfg(target_os = "linux")]
    #[tokio::test]
    async fn dropping_stdio_kills_a_descendant_that_ignores_stdin() {
        let dir = tempfile::tempdir().unwrap();
        let background = crate::background::BackgroundProcesses::new();
        let store_path = dir.path().join("store");
        background.recover_host_jobs(&store_path).unwrap();
        let journal_path = dir.path().join("store.background-processes");
        let transport = StdioTransport::spawn(
            std::path::Path::new("bash"),
            &[
                "-c".into(),
                "sleep 60 </dev/null >/dev/null 2>&1 & echo $! > descendant; wait".into(),
            ],
            &Default::default(),
            &[],
            Some(dir.path()),
            Duration::from_secs(5),
            Some(&background),
        )
        .unwrap();
        assert_eq!(std::fs::read_dir(&journal_path).unwrap().count(), 1);
        let marker = dir.path().join("descendant");
        crate::testing::wait_for("MCP descendant ready", || {
            if std::fs::read_to_string(&marker).is_ok_and(|s| !s.trim().is_empty()) {
                crate::testing::Observed::Held
            } else {
                crate::testing::Observed::Pending("waiting for pid".into())
            }
        })
        .await;
        let pid: u32 = std::fs::read_to_string(marker)
            .unwrap()
            .trim()
            .parse()
            .unwrap();
        drop(transport);
        assert_eq!(std::fs::read_dir(&journal_path).unwrap().count(), 0);
        crate::testing::wait_for("MCP descendant killed", || {
            if std::fs::read_to_string(format!("/proc/{pid}/stat"))
                .is_ok_and(|s| !s.contains(") Z "))
            {
                crate::testing::Observed::Pending("still alive".into())
            } else {
                crate::testing::Observed::Held
            }
        })
        .await;
    }

    #[tokio::test]
    async fn a_line_without_an_end_is_refused_instead_of_being_buffered() {
        // A server that writes bytes and never a newline: unbounded, this
        // grows the daemon's memory until the request timeout fires.
        let flood = vec![b'x'; MAX_MESSAGE_BYTES + 1];
        let mut reader: Reader = Box::new(tokio::io::BufReader::new(std::io::Cursor::new(flood)));
        let e = read_bounded_line(&mut reader)
            .await
            .expect_err("an unbounded line must not be read");
        assert!(e.to_string().contains("cap"), "{e}");
        // Fatal, because the rest of the oversized line is still in the pipe:
        // the connection is desynchronized, so the server must be dropped and
        // marked failed rather than left advertised as Ready.
        assert!(
            e.is_fatal(),
            "framing loss is a transport failure, not a bad call: {e:?}"
        );
    }

    #[tokio::test]
    async fn a_line_at_the_cap_is_still_read() {
        let mut body = vec![b'x'; MAX_MESSAGE_BYTES];
        body.push(b'\n');
        let mut reader: Reader = Box::new(tokio::io::BufReader::new(std::io::Cursor::new(body)));
        let line = read_bounded_line(&mut reader)
            .await
            .expect("a line inside the cap is fine")
            .expect("a line");
        assert_eq!(line.len(), MAX_MESSAGE_BYTES);
        assert!(
            read_bounded_line(&mut reader).await.expect("eof").is_none(),
            "end of stream is None, not an empty line forever"
        );
    }

    #[tokio::test]
    async fn a_notification_before_the_reply_is_skipped_and_eof_is_reported() {
        let (client_side, mut server_side) = tokio::io::duplex(4096);
        let (reader, writer) = tokio::io::split(client_side);
        let transport = StdioTransport::over(
            Box::new(writer),
            Box::new(tokio::io::BufReader::new(reader)),
            Duration::from_secs(5),
        );
        tokio::spawn(async move {
            let mut line = String::new();
            let mut buf = tokio::io::BufReader::new(&mut server_side);
            let _ = buf.read_line(&mut line).await;
            let _ = buf
                .get_mut()
                .write_all(
                    b"{\"jsonrpc\":\"2.0\",\"method\":\"notifications/message\",\"params\":{}}\n\
                      {\"jsonrpc\":\"2.0\",\"id\":1,\"method\":\"ping\"}\n\
                      {\"jsonrpc\":\"2.0\",\"id\":1,\"result\":{\"pong\":true}}\n",
                )
                .await;
        });
        let result = transport.request("ping", json!({})).await.expect("reply");
        assert_eq!(result["pong"], json!(true));
        // The server is gone now: the next request fails the connection
        // instead of waiting for an answer that will never come.
        let e = transport
            .request("ping", json!({}))
            .await
            .expect_err("a dead server must not answer");
        assert!(e.is_fatal(), "{e}");
    }
}
