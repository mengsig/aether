//! Aether as an MCP client: configured servers, their tools, and the
//! [`Tool`](crate::tool::Tool) implementation that routes a call to one.
//!
//! A discovered tool is an ordinary Aether tool. It carries a side-effect
//! class, so it goes through the same approval gate, idempotency keys and
//! resource keys as a built-in; nothing about MCP is special downstream.
//!
//! Health is the manager's, not the registry's: tools are offered only while
//! their server is `Ready`, and a server that dies is reported through the
//! change channel so the runtime can record it as an event.

mod client;
mod tool;
mod transport;

use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use aether_core::id::{AgentId, TaskId};
pub use aether_core::mcp::qualified_name;
use aether_core::mcp::{McpAuth, McpAuthState, McpServerHealth, McpServerState, McpToolInfo};
use aether_core::time::{Timestamp, now};
use aether_core::tool::SideEffectClass;

use client::McpClient;
pub use client::{Discovery, McpCallResult, RemoteTool};
pub use tool::McpTool;
use transport::{HttpTransport, StdioTransport, Transport};

/// How long any single MCP request may take before the call fails.
///
/// Deliberately under the tool runtime's own per-call timeout
/// (`ExecutionPolicy::default_timeout`): the runtime settles a timeout by
/// dropping the call's future, which would leave an unread reply on the
/// stream and the server still marked ready. Failing here first lets the
/// manager drop the connection and record the failure.
pub const DEFAULT_TIMEOUT: Duration = Duration::from_secs(20);

/// Caps on what a server may send. The peer is untrusted - a stdio child or
/// a remote endpoint - and an unbounded read is the daemon's memory and
/// every agent running in it. Each cap is reported where the thing it bounds
/// is read: a truncated description says so in the description, a server
/// that offers too many tools says so in its health, an oversized message
/// fails the call.
///
/// One JSON-RPC message, on either transport.
pub const MAX_MESSAGE_BYTES: usize = 4 << 20;
/// Tools taken from one server's `tools/list`, across all its pages.
pub const MAX_TOOLS: usize = 256;
/// Characters of a tool's description; the rest is cut with a marker.
pub const MAX_DESCRIPTION_CHARS: usize = 4_000;
/// Bytes of a tool's serialized `inputSchema`.
pub const MAX_SCHEMA_BYTES: usize = 64 * 1024;
/// Characters of a call's `structuredContent`. The same budget the text half
/// gets, because it lands in the same durable event.
pub const MAX_STRUCTURED_CHARS: usize = 30_000;

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum McpError {
    #[error("transport error: {0}")]
    Transport(String),
    #[error("the server closed the connection")]
    Closed,
    #[error("the server timed out")]
    Timeout,
    #[error("protocol error: {0}")]
    Protocol(String),
    #[error("server error {code}: {message}")]
    Server { code: i64, message: String },
    #[error("no MCP server named `{0}` is configured")]
    UnknownServer(String),
    /// Refused before it was sent; nothing reached the server.
    #[error("invalid arguments: {0}")]
    InvalidArguments(String),
    /// The server's required token is missing; it was not contacted.
    #[error("auth_failed: {0}")]
    AuthFailed(String),
}

impl McpError {
    /// Whether the connection is worth keeping. A protocol or application
    /// error says something about the call; a transport failure says the
    /// server is gone.
    fn is_fatal(&self) -> bool {
        matches!(self, Self::Transport(_) | Self::Closed | Self::Timeout)
    }
}

/// How to reach one server. `Debug` is written by hand so a token cannot
/// reach a log line through it.
#[derive(Clone, PartialEq, Eq)]
pub enum McpTransportSpec {
    Stdio {
        command: PathBuf,
        args: Vec<String>,
        env: BTreeMap<String, String>,
        /// Working directory for the server process; the daemon's own if unset.
        cwd: Option<PathBuf>,
    },
    /// Streamable HTTP. `token` is the resolved credential, never persisted.
    Http { url: String, token: Option<String> },
}

impl std::fmt::Debug for McpTransportSpec {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.describe())
    }
}

impl McpTransportSpec {
    /// One line a person can recognise the server by. Never carries a token.
    pub fn describe(&self) -> String {
        match self {
            Self::Stdio { command, args, .. } => {
                let mut text = format!("stdio: {}", command.display());
                for arg in args {
                    text.push(' ');
                    text.push_str(arg);
                }
                text
            }
            Self::Http { url, token } => {
                let auth = if token.is_some() { " (token)" } else { "" };
                format!("http: {url}{auth}")
            }
        }
    }
}

/// A configured server: how to reach it, and what its tools do to the world.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct McpServerSpec {
    pub name: String,
    pub transport: McpTransportSpec,
    /// Side-effect class per tool name, from configuration.
    pub classes: BTreeMap<String, SideEffectClass>,
    /// Whether each call carries its [`McpCaller`] in `params._meta`, and
    /// arguments that claim a caller are refused. Off unless configured.
    pub send_caller_meta: bool,
    /// Whether its token was found, when it names one. A server whose
    /// required token is missing is never contacted.
    pub auth: Option<McpAuth>,
}

impl McpServerSpec {
    /// Why the server must not be contacted, when its required token is
    /// missing. Names the variable, never a value.
    fn auth_refusal(&self) -> Option<McpError> {
        let auth = self.auth.as_ref()?;
        if auth.state != McpAuthState::AuthFailed {
            return None;
        }
        debug_assert!(
            !matches!(
                &self.transport,
                McpTransportSpec::Http { token: Some(_), .. }
            ),
            "a server with a token cannot be in auth_failed"
        );
        Some(McpError::AuthFailed(format!(
            "token environment variable `{}` is unset; set it, or mark the server \
             `auth = \"optional\"` to connect without one",
            auth.token_env
        )))
    }
}

/// `params._meta` key naming the agent a call is made for.
pub const META_AGENT_ID: &str = "aether/agent_id";
/// `params._meta` key naming that agent's task, when it has one.
pub const META_TASK_ID: &str = "aether/task_id";
/// Every key Aether sets in `_meta` starts with this; the model may set none.
const META_PREFIX: &str = "aether/";

/// Who a call is made for. Taken from the execution context and never from
/// the model's input: one server process is shared by every agent, and this
/// is how it tells them apart without a credential in the conversation.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct McpCaller {
    pub agent_id: AgentId,
    pub task_id: Option<TaskId>,
}

impl McpCaller {
    pub fn of(ctx: &crate::tool::ToolContext) -> Self {
        Self {
            agent_id: ctx.agent_id,
            task_id: ctx.task_id,
        }
    }

    /// The `_meta` entries a call carries.
    fn meta(&self) -> serde_json::Map<String, serde_json::Value> {
        let mut meta = serde_json::Map::new();
        meta.insert(META_AGENT_ID.to_owned(), self.agent_id.to_string().into());
        if let Some(task) = self.task_id {
            meta.insert(META_TASK_ID.to_owned(), task.to_string().into());
        }
        meta
    }
}

/// Refuse arguments that carry an `aether/` key where a careless server might
/// read caller identity: at the top level, or in an `_meta` object of their
/// own. The real `_meta` is built apart from them, so this is defense in
/// depth - and it makes a model's attempt to claim another agent visible.
fn refuse_spoofed_caller(arguments: &serde_json::Value) -> Result<(), McpError> {
    let Some(top) = arguments.as_object() else {
        return Ok(());
    };
    let nested = top.get("_meta").and_then(serde_json::Value::as_object);
    let spoofed = top
        .keys()
        .chain(nested.into_iter().flat_map(|meta| meta.keys()))
        .find(|key| {
            key.get(..META_PREFIX.len())
                .is_some_and(|prefix| prefix.eq_ignore_ascii_case(META_PREFIX))
        });
    match spoofed {
        Some(key) => Err(McpError::InvalidArguments(format!(
            "`{key}` is reserved: Aether sets caller identity itself, outside the arguments"
        ))),
        None => Ok(()),
    }
}

/// A tool as discovered: what agents are told about it, plus the schema the
/// model needs. [`McpServerHealth`] is the summary projection of this.
#[derive(Debug, Clone, PartialEq)]
struct DiscoveredTool {
    info: McpToolInfo,
    description: String,
    input_schema: serde_json::Value,
}

/// The class a tool is given: what config declares, else the conservative
/// answer. A server Aether did not classify is an outside service it knows
/// nothing about, so `External` is both the accurate class and the one the
/// default approval policy asks a person about.
fn classify(spec: &McpServerSpec, tool_name: &str) -> (SideEffectClass, bool) {
    match spec.classes.get(tool_name) {
        Some(class) => (*class, true),
        None => (SideEffectClass::External, false),
    }
}

/// What is known about one server right now. The single source of truth;
/// [`McpServerHealth`] is derived from it.
#[derive(Debug)]
struct SlotState {
    state: McpServerState,
    tools: Vec<DiscoveredTool>,
    error: Option<String>,
    checked_at: Option<Timestamp>,
}

/// A server's live connection and last known state.
#[derive(Debug)]
struct ServerSlot {
    spec: McpServerSpec,
    /// The connection, and the lock that keeps one call in flight at a time.
    client: tokio::sync::Mutex<Option<Arc<McpClient>>>,
    state: Mutex<SlotState>,
}

impl ServerSlot {
    /// The tools to offer, empty unless the server is ready.
    fn ready_tools(&self) -> Vec<DiscoveredTool> {
        match self.state.lock() {
            Ok(s) if s.state == McpServerState::Ready => s.tools.clone(),
            Ok(_) => Vec::new(),
            Err(_) => Vec::new(),
        }
    }

    fn health(&self) -> McpServerHealth {
        let unknown = McpServerHealth::unknown(&self.spec.name, self.spec.transport.describe());
        match self.state.lock() {
            Ok(s) => McpServerHealth {
                state: s.state,
                tools: s.tools.iter().map(|t| t.info.clone()).collect(),
                error: s.error.clone(),
                checked_at: s.checked_at,
                auth: self.spec.auth.clone(),
                ..unknown
            },
            // A poisoned lock means a panic while updating state; report the
            // server as unusable rather than inventing a state.
            Err(_) => McpServerHealth {
                state: McpServerState::Failed,
                error: Some("health lock poisoned".into()),
                auth: self.spec.auth.clone(),
                ..unknown
            },
        }
    }
}

/// Every configured MCP server. Owns the connections and the health record.
#[derive(Debug)]
pub struct McpManager {
    servers: BTreeMap<String, Arc<ServerSlot>>,
    /// Where health transitions go, for the runtime to record as events.
    changes: Option<tokio::sync::mpsc::UnboundedSender<McpServerHealth>>,
    /// Variables no stdio server may inherit; the registry's own list.
    hidden_env: Arc<[String]>,
    timeout: Duration,
    background: Option<Arc<crate::background::BackgroundProcesses>>,
}

impl McpManager {
    /// `changes` receives a health record every time a server's state or tool
    /// list changes; `None` records nothing, for tests and for the CLI.
    pub fn new(
        specs: Vec<McpServerSpec>,
        changes: Option<tokio::sync::mpsc::UnboundedSender<McpServerHealth>>,
        hidden_env: Arc<[String]>,
    ) -> Arc<Self> {
        Self::with_background(specs, changes, hidden_env, None)
    }

    pub fn with_background(
        specs: Vec<McpServerSpec>,
        changes: Option<tokio::sync::mpsc::UnboundedSender<McpServerHealth>>,
        hidden_env: Arc<[String]>,
        background: Option<Arc<crate::background::BackgroundProcesses>>,
    ) -> Arc<Self> {
        let servers = specs
            .into_iter()
            .map(|spec| {
                (
                    spec.name.clone(),
                    Arc::new(ServerSlot {
                        spec,
                        client: tokio::sync::Mutex::new(None),
                        state: Mutex::new(SlotState {
                            state: McpServerState::Unknown,
                            tools: Vec::new(),
                            error: None,
                            checked_at: None,
                        }),
                    }),
                )
            })
            .collect();
        Arc::new(Self {
            servers,
            changes,
            hidden_env,
            timeout: DEFAULT_TIMEOUT,
            background,
        })
    }

    pub fn is_empty(&self) -> bool {
        self.servers.is_empty()
    }

    /// Health of every configured server, in name order.
    pub fn health(&self) -> Vec<McpServerHealth> {
        self.servers.values().map(|s| s.health()).collect()
    }

    /// The tools currently offered: one per tool of every ready server.
    pub fn tools(self: &Arc<Self>) -> Vec<Arc<dyn crate::tool::Tool>> {
        let mut tools: Vec<Arc<dyn crate::tool::Tool>> = Vec::new();
        for slot in self.servers.values() {
            for discovered in slot.ready_tools() {
                tools.push(Arc::new(McpTool::new(
                    self.clone(),
                    slot.spec.name.clone(),
                    discovered.info,
                    discovered.description,
                    discovered.input_schema,
                )));
            }
        }
        tools
    }

    /// The tool with this qualified name, when a ready server offers it.
    pub fn tool(self: &Arc<Self>, qualified: &str) -> Option<Arc<dyn crate::tool::Tool>> {
        self.tools()
            .into_iter()
            .find(|t| t.descriptor().spec.name == qualified)
    }

    /// Contact every configured server and discover its tools. Errors are
    /// recorded as health, not returned: one unreachable server must not stop
    /// the others, and the daemon starts either way.
    pub async fn discover_all(&self) {
        for name in self.servers.keys() {
            if let Err(e) = self.discover(name).await {
                tracing::warn!(server = %name, error = %e, "mcp: discovery failed");
            }
        }
    }

    /// Reconnect one server and refresh its tool list.
    pub async fn discover(&self, name: &str) -> Result<McpServerHealth, McpError> {
        let slot = self
            .servers
            .get(name)
            .ok_or_else(|| McpError::UnknownServer(name.to_owned()))?
            .clone();
        let mut guard = slot.client.lock().await;
        *guard = None;
        match self.connect(&slot).await {
            Ok((client, tools, capped)) => {
                *guard = Some(client);
                Ok(self.publish(&slot, McpServerState::Ready, tools, capped))
            }
            Err(e) => {
                tracing::warn!(server = %name, error = %e, "mcp: server unavailable");
                self.publish(
                    &slot,
                    McpServerState::Failed,
                    Vec::new(),
                    Some(e.to_string()),
                );
                Err(e)
            }
        }
    }

    async fn connect(
        &self,
        slot: &ServerSlot,
    ) -> Result<(Arc<McpClient>, Vec<DiscoveredTool>, Option<String>), McpError> {
        if let Some(refusal) = slot.spec.auth_refusal() {
            return Err(refusal);
        }
        let transport: Arc<dyn Transport> = match &slot.spec.transport {
            McpTransportSpec::Stdio {
                command,
                args,
                env,
                cwd,
            } => Arc::new(StdioTransport::spawn(
                command,
                args,
                env,
                &self.hidden_env,
                cwd.as_deref(),
                self.timeout,
                self.background.as_deref(),
            )?),
            McpTransportSpec::Http { url, token } => Arc::new(HttpTransport::new(
                url.clone(),
                token.clone(),
                self.timeout,
            )?),
        };
        let client = Arc::new(McpClient::new(transport));
        client.initialize().await?;
        let discovery = client.list_tools().await?;
        if let Some(capped) = &discovery.capped {
            tracing::warn!(server = %slot.spec.name, "mcp: {capped}");
        }
        let tools: Vec<DiscoveredTool> = discovery
            .tools
            .into_iter()
            .map(|remote| discovered(&slot.spec, remote))
            .collect();
        if let Some(collision) = first_collision(&tools) {
            return Err(collision);
        }
        Ok((client, tools, discovery.capped))
    }

    /// Call a tool on a server for `caller`, connecting first if needed. A
    /// transport failure marks the server failed and withdraws its tools; the
    /// call reports the failure instead of retrying behind the model's back.
    pub async fn call(
        &self,
        server: &str,
        tool: &str,
        arguments: serde_json::Value,
        caller: &McpCaller,
    ) -> Result<McpCallResult, McpError> {
        let slot = self
            .servers
            .get(server)
            .ok_or_else(|| McpError::UnknownServer(server.to_owned()))?
            .clone();
        // A server not opted in is sent exactly what it was before: no
        // `_meta`, and no identity to spoof, so its arguments pass untouched.
        let meta = if slot.spec.send_caller_meta {
            refuse_spoofed_caller(&arguments)?;
            Some(caller.meta())
        } else {
            None
        };
        let mut guard = slot.client.lock().await;
        if guard.is_none() {
            match self.connect(&slot).await {
                Ok((client, tools, capped)) => {
                    *guard = Some(client);
                    self.publish(&slot, McpServerState::Ready, tools, capped);
                }
                Err(e) => {
                    self.publish(
                        &slot,
                        McpServerState::Failed,
                        Vec::new(),
                        Some(e.to_string()),
                    );
                    return Err(e);
                }
            }
        }
        let Some(client) = guard.clone() else {
            return Err(McpError::Transport("no connection after connect".into()));
        };
        match client.call_tool(tool, arguments, meta).await {
            Ok(result) => Ok(result),
            Err(e) if e.is_fatal() => {
                *guard = None;
                self.publish(
                    &slot,
                    McpServerState::Failed,
                    Vec::new(),
                    Some(e.to_string()),
                );
                Err(e)
            }
            Err(e) => Err(e),
        }
    }

    /// Record a server's state and announce it when it actually changed.
    /// `note` is why it failed, or - for a server that is `Ready` - the cap
    /// it passed at discovery, so a truncated tool list is visible.
    fn publish(
        &self,
        slot: &ServerSlot,
        state: McpServerState,
        tools: Vec<DiscoveredTool>,
        note: Option<String>,
    ) -> McpServerHealth {
        let changed = match slot.state.lock() {
            Ok(mut current) => {
                let changed = current.state != state || current.tools != tools;
                *current = SlotState {
                    state,
                    tools,
                    error: note,
                    checked_at: Some(now()),
                };
                changed
            }
            Err(_) => {
                tracing::error!(server = %slot.spec.name, "mcp: health lock poisoned");
                false
            }
        };
        let health = slot.health();
        if changed && let Some(tx) = &self.changes {
            let _ = tx.send(health.clone());
        }
        health
    }
}

/// One MCP server, spoken to directly. The manager is how agents reach a
/// configured server; this is for anything that drives one server itself -
/// a test, a tool that inspects a server, another front end.
#[derive(Debug)]
pub struct McpClientHandle {
    client: McpClient,
}

impl McpClientHandle {
    /// Handshake over an already-connected pair of streams.
    pub async fn over_streams(
        writer: Box<dyn tokio::io::AsyncWrite + Send + Unpin>,
        reader: Box<dyn tokio::io::AsyncBufRead + Send + Unpin>,
        timeout: Duration,
    ) -> Result<Self, McpError> {
        let client = McpClient::new(Arc::new(StdioTransport::over(writer, reader, timeout)));
        client.initialize().await?;
        Ok(Self { client })
    }

    /// The server's tools, plus any cap discovery hit taking them.
    pub async fn list_tools(&self) -> Result<Discovery, McpError> {
        self.client.list_tools().await
    }

    /// A call made for nobody in particular, so it carries no caller `_meta`.
    pub async fn call_tool(
        &self,
        name: &str,
        arguments: serde_json::Value,
    ) -> Result<McpCallResult, McpError> {
        self.client.call_tool(name, arguments, None).await
    }
}

/// Two of one server's tools whose names collapse to the same qualified
/// name, if any. Collisions *between* servers are refused at config load
/// with the same mapping; within one server nothing checked, so a server
/// offering `read.file` and `read_file` advertised one name twice and the
/// second was unreachable. Refused at discovery, so it becomes the server's
/// health error rather than a tool that silently disappears.
fn first_collision(tools: &[DiscoveredTool]) -> Option<McpError> {
    let mut seen: BTreeMap<&str, &str> = BTreeMap::new();
    for tool in tools {
        let qualified = tool.info.qualified_name.as_str();
        match seen.insert(qualified, tool.info.name.as_str()) {
            Some(first) if first != tool.info.name => {
                return Some(McpError::Protocol(format!(
                    "tools `{first}` and `{}` both become `{qualified}`; rename one on the \
                     server - Aether cannot tell them apart",
                    tool.info.name
                )));
            }
            Some(_) => {
                return Some(McpError::Protocol(format!(
                    "the server offers `{}` twice",
                    tool.info.name
                )));
            }
            None => {}
        }
    }
    None
}

fn discovered(spec: &McpServerSpec, remote: RemoteTool) -> DiscoveredTool {
    let (side_effect, declared) = classify(spec, &remote.name);
    DiscoveredTool {
        info: McpToolInfo {
            qualified_name: qualified_name(&spec.name, &remote.name),
            name: remote.name,
            side_effect,
            declared,
        },
        description: remote.description,
        input_schema: remote.input_schema,
    }
}

#[cfg(test)]
mod tests;
