//! Aether as an MCP server: its agent, task, message and approval surface
//! offered to any process that speaks MCP, over stdio or streamable HTTP.
//!
//! Every tool is one protocol command sent to the daemon over the ordinary
//! client socket. Nothing is authorized or recorded here: the daemon decides
//! and writes the same events it would for a person at the CLI, so an
//! orchestrator driving Aether leaves the same trace a person does.
//!
//! What a caller reaches is its [`Surface`], and the credential picks it: the
//! operator's token gets the fleet, a running turn's per-turn token gets that
//! agent's own tools through the same channel the engine bridge uses.

pub mod http;
pub mod stdio;

use aether_core::id::AgentId;
use aether_core::model::ToolSpec;
use aether_protocol::{AgentQuery, Command, CreateAgent, CreateTask, EventQuery, Reply, TaskQuery};
use serde::Deserialize;
use serde_json::{Value, json};

/// The preferred revision when the client does not name a supported one.
const PROTOCOL_VERSION: &str = "2025-06-18";
const SUPPORTED_PROTOCOL_VERSIONS: &[&str] =
    &["2024-11-05", "2025-03-26", "2025-06-18", "2025-11-25"];

/// What a caller may reach through this server. The credential decides it:
/// the operator's token is the fleet, a per-turn token is one turn.
#[derive(Debug)]
pub enum Surface {
    /// The whole fleet-control surface: agents, tasks, messages, events.
    Fleet,
    /// One running engine turn. It reaches that agent's own tools and
    /// nothing else - no `create_agent`, no other agent's inbox, no
    /// fleet-wide event log (`VISION.md` §5.2).
    Turn(TurnSurface),
}

/// A running turn, as the daemon confirmed it when the token was checked.
#[derive(Debug)]
pub struct TurnSurface {
    pub agent: AgentId,
    /// The per-turn token, replayed on every call this turn makes.
    pub token: String,
    /// What the daemon says this turn may use, as of this request.
    pub tools: Vec<ToolSpec>,
}

/// What answering a message produced.
#[derive(Debug)]
pub enum Answer {
    /// A notification: nothing to send back.
    None,
    /// The JSON-RPC response.
    Reply(Value),
    /// The caller's credential does not reach what it named. Kept apart from
    /// a tool that failed, so a transport can refuse it as a transport does.
    Denied(String),
}

impl Surface {
    /// The tools this caller may see. A turn is offered exactly what the
    /// daemon listed for it, so the advertised set and the accepted set are
    /// the same set.
    fn tool_list(&self) -> Vec<Value> {
        match self {
            Self::Fleet => fleet_tools(),
            Self::Turn(turn) => turn
                .tools
                .iter()
                .map(|spec| tool(&spec.name, &spec.description, spec.input_schema.clone()))
                .collect(),
        }
    }
}

// Where a tool call goes. Shared with `serve`, so it lives one level up.
pub use crate::sink::{CommandSink, DaemonSink};

/// The fleet-control tools, in the order a person would read them. Reached
/// with the operator's token only.
pub fn fleet_tools() -> Vec<Value> {
    let agent = json!({"type": "string", "description": "Agent id."});
    vec![
        tool(
            "status",
            "Daemon status: fleet counts, usage, engines, MCP servers.",
            json!({"type": "object", "properties": {}}),
        ),
        tool(
            "list_agents",
            "Agents, optionally filtered by parent, run, status or liveness.",
            json!({
                "type": "object",
                "properties": {
                    "parent": agent,
                    "run": {"type": "string"},
                    "statuses": {"type": "array", "items": {"type": "string"}},
                    "live_only": {"type": "boolean"},
                    "include_archived": {"type": "boolean", "description": "Include agents cleared from the default view (default false)."}
                }
            }),
        ),
        tool(
            "get_agent",
            "One agent: its role, model, scope, status and usage.",
            json!({"type": "object", "properties": {"id": agent}, "required": ["id"]}),
        ),
        tool(
            "get_tree",
            "The live graph: every agent in a run, with its children and usage.",
            json!({
                "type": "object",
                "properties": {
                    "run": {"type": "string"},
                    "include_archived": {"type": "boolean", "description": "Include agents cleared from the default view (default false)."}
                }
            }),
        ),
        tool(
            "create_agent",
            "Create an agent, optionally with an objective it starts on.",
            json!({
                "type": "object",
                "properties": {
                    "role": {"type": "string", "description": "Opaque role label."},
                    "name": {"type": "string"},
                    "objective": {"type": "string"},
                    "model": {"type": "string", "description": "provider/model, or a fallback chain: provider/model,provider/model. Every link faces the same guards as the first, and a subscription model may not fall back onto a metered API."},
                    "parent": agent,
                    "effort": {"type": "string", "enum": ["low", "medium", "high", "xhigh", "max"], "description": "Required when parent is set; choose explicitly."},
                    "start": {"type": "boolean", "description": "Start it now (default true)."}
                },
                "required": ["role"]
            }),
        ),
        tool(
            "send_message",
            "Deliver a message to an agent's inbox; wakes it if it is waiting.",
            json!({
                "type": "object",
                "properties": {"agent": agent, "content": {"type": "string"}},
                "required": ["agent", "content"]
            }),
        ),
        tool(
            "cancel_agent",
            "Cancel an agent and everything below it.",
            json!({
                "type": "object",
                "properties": {"agent": agent, "reason": {"type": "string"}},
                "required": ["agent"]
            }),
        ),
        tool(
            "list_tasks",
            "Tasks, optionally filtered by owner or run.",
            json!({
                "type": "object",
                "properties": {
                    "owner": agent,
                    "run": {"type": "string"}
                }
            }),
        ),
        tool(
            "create_task",
            "Create a task, optionally assigned to an agent.",
            json!({
                "type": "object",
                "properties": {
                    "objective": {"type": "string"},
                    "owner": agent,
                    "dependencies": {"type": "array", "items": {"type": "string"}}
                },
                "required": ["objective"]
            }),
        ),
        tool(
            "list_approvals",
            "Tool calls waiting for a person to answer.",
            json!({"type": "object", "properties": {}}),
        ),
        tool(
            "list_events",
            "Events after a sequence number, oldest first.",
            json!({
                "type": "object",
                "properties": {
                    "after": {"type": "integer", "minimum": 0},
                    "limit": {"type": "integer", "minimum": 1},
                    "agent": agent,
                    "families": {"type": "array", "items": {"type": "string"}}
                }
            }),
        ),
    ]
}

fn tool(name: &str, description: &str, schema: Value) -> Value {
    json!({ "name": name, "description": description, "inputSchema": schema })
}

/// Answer one JSON-RPC message for `surface`.
pub async fn handle(sink: &dyn CommandSink, surface: &Surface, request: &Value) -> Answer {
    let id = request.get("id").cloned();
    let method = request.get("method").and_then(Value::as_str).unwrap_or("");
    let params = request.get("params").cloned().unwrap_or_else(|| json!({}));
    let Some(id) = id else {
        return Answer::None; // a notification
    };
    let result = match method {
        "initialize" => Ok(json!({
            "protocolVersion": params
                .get("protocolVersion")
                .and_then(Value::as_str)
                .filter(|version| SUPPORTED_PROTOCOL_VERSIONS.contains(version))
                .unwrap_or(PROTOCOL_VERSION),
            "capabilities": { "tools": {} },
            "serverInfo": { "name": "aether", "version": env!("CARGO_PKG_VERSION") }
        })),
        "ping" => Ok(json!({})),
        "tools/list" => Ok(json!({ "tools": surface.tool_list() })),
        "tools/call" => {
            let name = params
                .get("name")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_owned();
            let arguments = params
                .get("arguments")
                .cloned()
                .unwrap_or_else(|| json!({}));
            let called = match surface {
                Surface::Fleet => call_fleet_tool(sink, &name, arguments).await,
                Surface::Turn(turn) => {
                    // Refused as a credential failure, not as a tool result:
                    // naming a tool this turn was not offered is the caller
                    // reaching past what its token authorizes.
                    if !turn.tools.iter().any(|spec| spec.name == name) {
                        return Answer::Denied(format!(
                            "`{name}` is not one of this turn's tools; a per-turn token reaches \
                             the agent's own tools and nothing else"
                        ));
                    }
                    call_turn_tool(sink, turn, name, arguments).await
                }
            };
            // A tool that fails is a result with isError, not a JSON-RPC
            // error: the caller's model is meant to read it and try again.
            Ok(match called {
                Ok(value) => json!({
                    "content": [{"type": "text", "text": pretty(&value)}],
                    "structuredContent": value,
                    "isError": false
                }),
                Err(e) => json!({
                    "content": [{"type": "text", "text": e}],
                    "isError": true
                }),
            })
        }
        other => Err(format!("unknown method {other}")),
    };
    Answer::Reply(match result {
        Ok(result) => json!({"jsonrpc": "2.0", "id": id, "result": result}),
        Err(message) => {
            json!({"jsonrpc": "2.0", "id": id, "error": {"code": -32601, "message": message}})
        }
    })
}

fn pretty(value: &Value) -> String {
    serde_json::to_string_pretty(value).unwrap_or_else(|_| value.to_string())
}

/// Run one fleet tool: turn its arguments into a protocol command, send it,
/// and hand back the daemon's reply as JSON.
async fn call_fleet_tool(
    sink: &dyn CommandSink,
    name: &str,
    arguments: Value,
) -> Result<Value, String> {
    let command = command_for(name, arguments)?;
    let reply = sink.call(command).await.map_err(|e| e.to_string())?;
    serde_json::to_value(&reply).map_err(|e| format!("cannot encode reply: {e}"))
}

/// Run one of the turn's own tools, through the same daemon channel the
/// engine bridge uses - so the call is gated, keyed and recorded identically
/// however the turn reached it.
async fn call_turn_tool(
    sink: &dyn CommandSink,
    turn: &TurnSurface,
    name: String,
    arguments: Value,
) -> Result<Value, String> {
    let reply = sink
        .call(Command::EngineTool {
            agent: turn.agent,
            token: turn.token.clone(),
            name,
            input: arguments,
        })
        .await
        .map_err(|e| e.to_string())?;
    match reply {
        Reply::EngineTool { content, is_error } if is_error => Err(content),
        Reply::EngineTool { content, .. } => Ok(json!({ "content": content })),
        other => Err(format!("unexpected reply {other:?}")),
    }
}

/// Arguments shared by the tools that name an agent.
#[derive(Debug, Deserialize)]
struct AgentArg {
    agent: String,
    #[serde(default)]
    content: String,
    #[serde(default)]
    reason: Option<String>,
}

#[derive(Debug, Deserialize)]
struct IdArg {
    id: String,
}

#[derive(Debug, Default, Deserialize)]
struct ListAgentsArgs {
    #[serde(default)]
    parent: Option<String>,
    #[serde(default)]
    run: Option<String>,
    #[serde(default)]
    statuses: Vec<aether_core::agent::AgentStatusKind>,
    #[serde(default)]
    live_only: bool,
    #[serde(default)]
    include_archived: bool,
}

#[derive(Debug, Default, Deserialize)]
struct CreateAgentArgs {
    role: String,
    #[serde(default)]
    name: Option<String>,
    #[serde(default)]
    objective: Option<String>,
    #[serde(default)]
    model: Option<String>,
    #[serde(default)]
    parent: Option<String>,
    #[serde(default)]
    effort: Option<String>,
    #[serde(default = "yes")]
    start: bool,
}

fn yes() -> bool {
    true
}

#[derive(Debug, Default, Deserialize)]
struct ListTasksArgs {
    #[serde(default)]
    owner: Option<String>,
    #[serde(default)]
    run: Option<String>,
}

#[derive(Debug, Deserialize)]
struct CreateTaskArgs {
    objective: String,
    #[serde(default)]
    owner: Option<String>,
    #[serde(default)]
    dependencies: Vec<String>,
}

#[derive(Debug, Default, Deserialize)]
struct ListEventsArgs {
    #[serde(default)]
    after: u64,
    #[serde(default = "default_limit")]
    limit: usize,
    #[serde(default)]
    agent: Option<String>,
    #[serde(default)]
    families: Vec<String>,
}

fn default_limit() -> usize {
    100
}

#[derive(Debug, Default, Deserialize)]
struct RunArg {
    #[serde(default)]
    run: Option<String>,
    #[serde(default)]
    include_archived: bool,
}

/// Deserialize a tool's arguments, naming the tool in any complaint.
fn args<T: serde::de::DeserializeOwned>(tool: &str, value: Value) -> Result<T, String> {
    serde_json::from_value(value).map_err(|e| format!("{tool}: {e}"))
}

fn command_for(name: &str, arguments: Value) -> Result<Command, String> {
    Ok(match name {
        "status" => Command::Status,
        "list_agents" => {
            let a: ListAgentsArgs = args(name, arguments)?;
            Command::ListAgents {
                query: AgentQuery {
                    parent: a.parent.as_deref().map(id).transpose()?,
                    run: a.run.as_deref().map(id).transpose()?,
                    statuses: a.statuses,
                    live_only: a.live_only,
                    include_archived: a.include_archived,
                },
            }
        }
        "get_agent" => {
            let a: IdArg = args(name, arguments)?;
            Command::GetAgent { id: id(&a.id)? }
        }
        "get_tree" => {
            let a: RunArg = args(name, arguments)?;
            Command::GetTree {
                run: a.run.as_deref().map(id).transpose()?,
                include_archived: a.include_archived,
            }
        }
        "create_agent" => {
            let a: CreateAgentArgs = args(name, arguments)?;
            let chain = a
                .model
                .as_deref()
                .map(|m| {
                    aether_core::model::ModelChain::parse(m).ok_or_else(|| {
                        format!("model `{m}` is not provider/model[,provider/model]")
                    })
                })
                .transpose()?;
            Command::CreateAgent(CreateAgent {
                role: a.role,
                name: a.name,
                objective: a.objective,
                model: chain.as_ref().map(|c| c.primary.clone()),
                fallbacks: chain.map(|c| c.fallbacks).unwrap_or_default(),
                scope: None,
                parent: a.parent.as_deref().map(id).transpose()?,
                config: a
                    .effort
                    .as_deref()
                    .map(|effort| parse_field(effort, "create_agent.effort"))
                    .transpose()?
                    .map(|effort| aether_core::agent::AgentConfig {
                        effort: Some(effort),
                        ..Default::default()
                    }),
                // Left to the runtime: a child inherits its parent's policy.
                approval: None,
                run: None,
                start: a.start,
                idempotency_key: None,
            })
        }
        "send_message" => {
            let a: AgentArg = args(name, arguments)?;
            if a.content.trim().is_empty() {
                return Err("send_message: content is empty".into());
            }
            Command::SendMessage {
                to: id(&a.agent)?,
                content: a.content,
                // The MCP tool addresses one agent; fan-out is not in its schema.
                subtree: false,
                idempotency_key: None,
            }
        }
        "cancel_agent" => {
            let a: AgentArg = args(name, arguments)?;
            Command::Cancel {
                agent: id(&a.agent)?,
                reason: a.reason,
                idempotency_key: None,
            }
        }
        "list_tasks" => {
            let a: ListTasksArgs = args(name, arguments)?;
            Command::ListTasks {
                query: TaskQuery {
                    owner: a.owner.as_deref().map(id).transpose()?,
                    run: a.run.as_deref().map(id).transpose()?,
                    statuses: Vec::new(),
                },
            }
        }
        "create_task" => {
            let a: CreateTaskArgs = args(name, arguments)?;
            Command::CreateTask(CreateTask {
                objective: a.objective,
                owner: a.owner.as_deref().map(id).transpose()?,
                parent_task: None,
                dependencies: a
                    .dependencies
                    .iter()
                    .map(|d| id(d))
                    .collect::<Result<Vec<_>, _>>()?,
                priority: Default::default(),
                idempotency_key: None,
            })
        }
        "list_approvals" => Command::ListApprovals,
        "list_events" => {
            let a: ListEventsArgs = args(name, arguments)?;
            Command::GetEvents {
                after: a.after,
                limit: a.limit.clamp(1, 1000),
                query: EventQuery {
                    agent: a.agent.as_deref().map(id).transpose()?,
                    families: a.families,
                    ..Default::default()
                },
            }
        }
        other => return Err(format!("unknown tool `{other}`")),
    })
}

/// Parse any of Aether's ULID-shaped ids, naming the offender on failure.
fn id<T: std::str::FromStr>(text: &str) -> Result<T, String>
where
    T::Err: std::fmt::Display,
{
    text.parse().map_err(|e| format!("`{text}`: {e}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_advertised_tool_maps_to_a_command() {
        for tool in fleet_tools() {
            let name = tool["name"].as_str().expect("name");
            // Required arguments are missing here, so a tool either builds a
            // command from nothing or complains about its own arguments -
            // never "unknown tool".
            let result = command_for(name, json!({}));
            if let Err(e) = result {
                assert!(!e.contains("unknown tool"), "{name}: {e}");
            }
        }
        assert!(
            command_for("nope", json!({}))
                .expect_err("unknown")
                .contains("unknown tool")
        );
    }

    #[test]
    fn a_malformed_id_is_refused_with_the_offending_text() {
        let e = command_for("get_agent", json!({"id": "not-an-id"})).expect_err("bad id");
        assert!(e.contains("not-an-id"), "{e}");
    }
}

fn parse_field<T: std::str::FromStr>(text: &str, field: &str) -> Result<T, String>
where
    T::Err: std::fmt::Display,
{
    text.parse()
        .map_err(|error| format!("{field}: `{text}`: {error}"))
}

#[cfg(test)]
mod effort_tests {
    use super::*;
    #[test]
    fn invalid_effort_names_the_create_agent_field() {
        let error = command_for(
            "create_agent",
            serde_json::json!({"role":"worker", "effort":"enormous"}),
        )
        .unwrap_err();
        assert!(error.contains("create_agent.effort"), "{error}");
    }
}
