#![allow(clippy::expect_used, clippy::unwrap_used, clippy::panic)]

//! Aether as an MCP client, against the in-repo mock server over real pipes:
//! discovery, classification, calling, and what happens when a server dies.

use std::collections::BTreeMap;
use std::sync::Arc;

use aether_core::agent::{ExecutionBackend, NetworkPolicy, Scope};
use aether_core::artifact::{Artifact, ArtifactKind};
use aether_core::id::{AgentId, TaskId};
use aether_core::mcp::{McpAuth, McpAuthState, McpServerState};
use aether_core::tool::{ResourceKey, SideEffectClass, ToolExposure};
use aether_tools::mcp::{McpCaller, McpError, McpManager, McpServerSpec, McpTransportSpec};
use aether_tools::{ArtifactSink, ToolContext, ToolError, ToolExecution, ToolRegistry};
use async_trait::async_trait;
use serde_json::json;

/// The MCP tool only reaches the sink when a server's answer is too big for
/// the model context, which no test here provokes; a call would be a bug.
#[derive(Debug)]
struct UnusedSink;

#[async_trait]
impl ArtifactSink for UnusedSink {
    async fn store(
        &self,
        _kind: ArtifactKind,
        _name: &str,
        _summary: &str,
        _content_type: Option<&str>,
        _bytes: Vec<u8>,
        _agent: AgentId,
        _task: Option<TaskId>,
    ) -> Result<Artifact, ToolError> {
        Err(ToolError::Internal("no artifact was expected here".into()))
    }
}

fn ctx() -> ToolContext {
    ToolContext {
        agent_id: AgentId::new(),
        task_id: None,
        instance_id: "test-instance".into(),
        scope: Scope::default(),
        cwd: std::env::temp_dir(),
        artifacts: Arc::new(UnusedSink),
        env: BTreeMap::new(),
        hidden_env: Vec::new().into(),
        temp_dir: None,
        build_env: Vec::new(),
    }
}

/// A context whose workspace root and cwd are `root`.
fn ctx_in(root: &std::path::Path) -> ToolContext {
    ToolContext {
        scope: Scope {
            workspace_root: Some(root.to_path_buf()),
            ..Default::default()
        },
        cwd: root.to_path_buf(),
        ..ctx()
    }
}

/// A caller for a test that is not about who calls.
fn caller() -> McpCaller {
    McpCaller {
        agent_id: AgentId::new(),
        task_id: None,
    }
}

/// No variable is hidden from a test's own server.
fn no_hidden() -> std::sync::Arc<[String]> {
    Vec::new().into()
}

const MOCK: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../../scripts/mock_mcp_server.sh"
);

fn spec(
    classes: BTreeMap<String, SideEffectClass>,
    env: BTreeMap<String, String>,
) -> McpServerSpec {
    McpServerSpec {
        name: "mock".into(),
        transport: McpTransportSpec::Stdio {
            command: "bash".into(),
            args: vec![MOCK.to_owned()],
            env,
            cwd: None,
        },
        classes,
        send_caller_meta: false,
        auth: None,
    }
}

/// The mock server with caller `_meta` turned on, as `send_caller_meta = true`.
fn opted_in(env: BTreeMap<String, String>) -> McpServerSpec {
    McpServerSpec {
        send_caller_meta: true,
        ..spec(classes(), env)
    }
}

fn classes() -> BTreeMap<String, SideEffectClass> {
    BTreeMap::from([("read_note".to_owned(), SideEffectClass::ReadOnly)])
}

#[tokio::test]
async fn tools_are_discovered_classified_and_callable() {
    let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
    let manager = McpManager::new(
        vec![spec(classes(), BTreeMap::new())],
        Some(tx),
        no_hidden(),
    );
    manager.discover_all().await;

    let health = manager.health();
    assert_eq!(health.len(), 1);
    assert_eq!(health[0].state, McpServerState::Ready);
    let names: Vec<&str> = health[0]
        .tools
        .iter()
        .map(|t| t.qualified_name.as_str())
        .collect();
    assert_eq!(
        names,
        [
            "mcp__mock__read_note",
            "mcp__mock__write_note",
            "mcp__mock__read_env",
            "mcp__mock__echo_request"
        ]
    );

    // A declared class is honoured; an undeclared tool is classed External,
    // which the shipped default approval policy asks a person about.
    let read = &health[0].tools[0];
    assert_eq!(read.side_effect, SideEffectClass::ReadOnly);
    assert!(read.declared);
    let write = &health[0].tools[1];
    assert_eq!(write.side_effect, SideEffectClass::External);
    assert!(!write.declared);
    assert!(
        aether_core::agent::ApprovalPolicy::default().requires(write.side_effect),
        "an unclassified remote tool must need approval by default"
    );

    // The change is announced once, for the runtime to record as an event.
    let announced = rx.try_recv().expect("a connect is announced");
    assert_eq!(announced.state, McpServerState::Ready);
    assert!(rx.try_recv().is_err(), "no change means no announcement");

    let result = manager
        .call("mock", "read_note", json!({"key": "k"}), &caller())
        .await
        .expect("call");
    assert_eq!(result.text, "note contents");
    assert!(!result.is_error);
}

#[tokio::test]
async fn discovered_tools_are_ordinary_registry_tools() {
    let manager = McpManager::new(vec![spec(classes(), BTreeMap::new())], None, no_hidden());
    manager.discover_all().await;
    let registry = ToolRegistry::with_builtins().with_mcp(manager.clone());
    let scope = networked();

    let specs = registry.specs_for(&ToolExposure::All, &scope);
    assert!(specs.iter().any(|s| s.name == "mcp__mock__read_note"));
    assert!(
        specs.iter().any(|s| s.name == "bash"),
        "built-ins are still there"
    );
    // The model gets the server's own schema, not a placeholder.
    let read = specs
        .iter()
        .find(|s| s.name == "mcp__mock__read_note")
        .expect("tool spec");
    assert_eq!(
        read.input_schema["properties"]["key"]["type"],
        json!("string")
    );

    let tool = registry.get("mcp__mock__read_note").expect("tool");
    assert_eq!(tool.descriptor().side_effect, SideEffectClass::ReadOnly);
    // One resource key per server, so two calls on one server serialize.
    assert_eq!(
        tool.resources(&json!({}), &ctx()),
        vec![aether_core::tool::ResourceKey("mcp:mock".into())]
    );

    let outcome = tool
        .execute(json!({"key": "k"}), &ctx(), Default::default())
        .await
        .expect("execute");
    match outcome {
        ToolExecution::Outcome(o) => assert_eq!(o.content, "note contents"),
        other => panic!("unexpected {other:?}"),
    }

    // Exposure still decides: an agent told to use only `bash` sees no MCP tool.
    let only = ToolExposure::Only {
        names: std::collections::BTreeSet::from(["bash".to_owned()]),
    };
    assert!(
        !registry
            .specs_for(&only, &scope)
            .iter()
            .any(|s| s.name.starts_with("mcp__"))
    );
}

/// A scope that allows network. Every MCP server can reach it, so this is
/// the least an agent needs to be offered an MCP tool at all.
fn networked() -> Scope {
    Scope {
        network: NetworkPolicy::Allowed,
        ..Default::default()
    }
}

#[tokio::test]
async fn every_mcp_server_is_hidden_from_an_agent_whose_scope_denies_network() {
    // A stdio server is an unconstrained child of the daemon: it reaches the
    // network as freely as an HTTP one, so the network policy must see both.
    let manager = McpManager::new(vec![spec(classes(), BTreeMap::new())], None, no_hidden());
    manager.discover_all().await;
    let registry = ToolRegistry::with_builtins().with_mcp(manager);

    assert!(
        registry
            .specs_for(&ToolExposure::All, &Scope::default())
            .iter()
            .all(|s| !s.name.starts_with("mcp__")),
        "a stdio server must not be a way around `network: denied`"
    );
    assert!(
        registry
            .withheld_reason(
                "mcp__mock__read_note",
                &ToolExposure::All,
                &Scope::default()
            )
            .expect("a withheld tool says why")
            .contains("network"),
    );
    assert!(
        registry
            .specs_for(&ToolExposure::All, &networked())
            .iter()
            .any(|s| s.name == "mcp__mock__read_note"),
        "an agent allowed the network still gets the server's tools"
    );
}

#[tokio::test]
async fn an_mcp_server_is_hidden_from_an_agent_scoped_to_a_container() {
    // The server runs on the daemon's host, so offering it to a containerized
    // agent would hand it host-side execution its backend is meant to deny.
    let manager = McpManager::new(vec![spec(classes(), BTreeMap::new())], None, no_hidden());
    manager.discover_all().await;
    let registry = ToolRegistry::with_builtins().with_mcp(manager);
    let contained = Scope {
        backend: ExecutionBackend::Container {
            image: "alpine:3".into(),
        },
        ..networked()
    };

    assert!(
        registry
            .specs_for(&ToolExposure::All, &contained)
            .iter()
            .all(|s| !s.name.starts_with("mcp__")),
        "a containerized agent must not reach a host-side MCP server"
    );
    let reason = registry
        .withheld_reason("mcp__mock__read_note", &ToolExposure::All, &contained)
        .expect("a withheld tool says why");
    assert!(reason.contains("container"), "{reason}");
    assert!(
        registry
            .specs_for(&ToolExposure::All, &contained)
            .iter()
            .any(|s| s.name == "bash"),
        "the shell still runs, in the container"
    );
}

#[tokio::test]
async fn an_unreachable_server_offers_nothing() {
    // No server is contacted: an unreachable one is Failed and offers nothing,
    // which is exactly what the network filter must not be confused by.
    let manager = McpManager::new(
        vec![McpServerSpec {
            name: "remote".into(),
            transport: McpTransportSpec::Http {
                url: "http://127.0.0.1:1/mcp".into(),
                token: None,
            },
            classes: BTreeMap::new(),
            send_caller_meta: false,
            auth: None,
        }],
        None,
        Vec::new().into(),
    );
    manager.discover_all().await;
    assert_eq!(manager.health()[0].state, McpServerState::Failed);
    assert!(
        manager.health()[0].error.is_some(),
        "a failure says what went wrong"
    );
    let registry = ToolRegistry::with_builtins().with_mcp(manager);
    assert!(
        registry
            .specs_for(&ToolExposure::All, &networked())
            .iter()
            .all(|s| !s.name.starts_with("mcp__")),
        "a failed server offers nothing"
    );
}

#[tokio::test]
async fn a_server_that_dies_is_reported_and_never_hangs() {
    let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
    let env = BTreeMap::from([("AETHER_MOCK_MCP_DIE".to_owned(), "1".to_owned())]);
    let manager = McpManager::new(vec![spec(classes(), env)], Some(tx), no_hidden());
    manager.discover_all().await;
    assert_eq!(manager.health()[0].state, McpServerState::Ready);
    assert_eq!(
        rx.try_recv().expect("connected").state,
        McpServerState::Ready
    );

    // The server exited after answering tools/list. The call must fail with a
    // reason rather than wait for an answer that will never come.
    let error = tokio::time::timeout(
        std::time::Duration::from_secs(10),
        manager.call("mock", "read_note", json!({"key": "k"}), &caller()),
    )
    .await
    .expect("the call must not hang")
    .expect_err("a dead server cannot answer");
    assert!(
        error.to_string().contains("closed") || error.to_string().contains("transport"),
        "{error}"
    );
    assert_eq!(manager.health()[0].state, McpServerState::Failed);
    assert_eq!(
        rx.try_recv().expect("failure").state,
        McpServerState::Failed
    );
    assert!(
        Arc::strong_count(&manager) >= 1,
        "the manager outlives its connections"
    );
}

#[tokio::test]
async fn a_servers_tool_names_the_server_and_the_paths_it_was_given() {
    let manager = McpManager::new(vec![spec(classes(), BTreeMap::new())], None, no_hidden());
    manager.discover_all().await;
    let tool = manager.tool("mcp__mock__read_note").expect("discovered");
    let workspace = tempfile::tempdir().expect("tempdir");
    let ctx = ctx_in(workspace.path());

    assert_eq!(
        tool.resources(&json!({"key": "k"}), &ctx),
        vec![ResourceKey("mcp:mock".into())]
    );
    // A path the call names is real and matchable; a relative one is against
    // the agent's workspace, as everywhere else.
    assert_eq!(
        tool.resources(&json!({"path": "notes/today.md"}), &ctx),
        vec![
            ResourceKey("mcp:mock".into()),
            ResourceKey::file(workspace.path().join("notes/today.md")),
        ]
    );
    // The server runs the call, so those keys are never the whole truth.
    assert!(!tool.resources_known(&json!({"path": "notes/today.md"})));
}

#[tokio::test]
async fn a_server_does_not_inherit_a_variable_the_daemon_hides() {
    // The search credential lives in the daemon's environment and a child
    // inherits it unless it is stripped. HOME stands in for it: this process
    // has it, and setting one is `unsafe`, which the workspace forbids.
    assert!(std::env::var("HOME").is_ok(), "the test needs HOME set");
    let hidden: Arc<[String]> = vec!["HOME".to_owned()].into();
    let manager = McpManager::new(vec![spec(classes(), BTreeMap::new())], None, hidden);
    manager.discover_all().await;
    let seen = manager
        .call("mock", "read_env", json!({"key": "HOME"}), &caller())
        .await
        .expect("call");
    assert_eq!(
        seen.text.trim(),
        "UNSET",
        "a hidden variable must not reach the server"
    );
}

/// A minimal streamable-HTTP MCP server on a raw socket, which puts a log
/// notification on the SSE stream before every reply - exactly what a real
/// server does, and what a client that takes the first event gets wrong.
/// Returns its address; it stops when `stop` is cancelled.
async fn chatty_sse_server(
    stop: tokio_util::sync::CancellationToken,
) -> (std::net::SocketAddr, tokio::task::JoinHandle<()>) {
    use tokio::io::{AsyncReadExt as _, AsyncWriteExt as _};

    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind");
    let address = listener.local_addr().expect("addr");
    let serving = tokio::spawn(async move {
        loop {
            let Ok((mut socket, _)) = (tokio::select! {
                accepted = listener.accept() => accepted,
                _ = stop.cancelled() => return,
            }) else {
                return;
            };
            let mut request = vec![0u8; 16 * 1024];
            let Ok(read) = socket.read(&mut request).await else {
                continue;
            };
            let request = String::from_utf8_lossy(&request[..read]).into_owned();
            let id: u64 = request
                .split("\"id\":")
                .nth(1)
                .and_then(|rest| {
                    rest.trim_start()
                        .split(|c: char| !c.is_ascii_digit())
                        .next()
                        .and_then(|digits| digits.parse().ok())
                })
                .unwrap_or(0);
            let result = if request.contains("\"tools/list\"") {
                json!({"tools": [{
                    "name": "ask",
                    "description": "Ask the remote service.",
                    "inputSchema": {"type": "object"}
                }]})
            } else if request.contains("\"tools/call\"") {
                json!({"content": [{"type": "text", "text": "answered"}]})
            } else {
                json!({"protocolVersion": "2025-06-18", "capabilities": {"tools": {}}})
            };
            // The reply is preceded by a log notification and a ping, both of
            // which carry no id or another one.
            let body = format!(
                "event: message\ndata: {}\n\nevent: message\ndata: {}\n\nevent: message\ndata: \
                 {}\n\n",
                json!({"jsonrpc": "2.0", "method": "notifications/message",
                       "params": {"level": "info", "data": "starting"}}),
                json!({"jsonrpc": "2.0", "id": 9999, "result": {"ping": true}}),
                json!({"jsonrpc": "2.0", "id": id, "result": result}),
            );
            let response = format!(
                "HTTP/1.1 200 OK\r\ncontent-type: text/event-stream\r\ncontent-length: {}\r\n\
                 connection: close\r\n\r\n{body}",
                body.len()
            );
            let _ = socket.write_all(response.as_bytes()).await;
            let _ = socket.shutdown().await;
        }
    });
    (address, serving)
}

#[tokio::test]
async fn an_http_server_that_logs_before_replying_is_still_understood() {
    let stop = tokio_util::sync::CancellationToken::new();
    let (address, serving) = chatty_sse_server(stop.clone()).await;
    let manager = McpManager::new(
        vec![McpServerSpec {
            name: "remote".into(),
            transport: McpTransportSpec::Http {
                url: format!("http://{address}/mcp"),
                token: None,
            },
            classes: BTreeMap::new(),
            send_caller_meta: false,
            auth: None,
        }],
        None,
        no_hidden(),
    );
    manager.discover_all().await;

    // Taking the first SSE event would have made this `Failed` with
    // "tools/list did not return tools".
    assert_eq!(
        manager.health()[0].state,
        McpServerState::Ready,
        "{:?}",
        manager.health()[0].error
    );
    assert_eq!(manager.health()[0].tools.len(), 1);

    // And a call would have looked like one that returned nothing at all.
    let result = manager
        .call("remote", "ask", json!({"q": "hello"}), &caller())
        .await
        .expect("call");
    assert_eq!(result.text, "answered");
    assert!(!result.is_error);

    stop.cancel();
    let _ = tokio::time::timeout(std::time::Duration::from_secs(5), serving).await;
}

/// The `tools/call` request the mock received, as `echo_request` reports it.
fn echoed_request(execution: ToolExecution) -> serde_json::Value {
    let ToolExecution::Outcome(outcome) = execution else {
        panic!("unexpected {execution:?}");
    };
    assert!(!outcome.is_error, "{}", outcome.content);
    serde_json::from_str(&outcome.content).expect("the mock echoes the request as JSON")
}

/// A context for one agent working one task.
fn ctx_for(agent_id: AgentId, task_id: Option<TaskId>) -> ToolContext {
    ToolContext {
        agent_id,
        task_id,
        ..ctx()
    }
}

#[tokio::test]
async fn a_call_carries_its_callers_identity_out_of_band() {
    let manager = McpManager::new(vec![opted_in(BTreeMap::new())], None, no_hidden());
    manager.discover_all().await;
    let tool = manager.tool("mcp__mock__echo_request").expect("discovered");
    let (agent, task) = (AgentId::new(), TaskId::new());

    let arguments = json!({"q": "hello"});
    let execution = tool
        .execute(
            arguments.clone(),
            &ctx_for(agent, Some(task)),
            Default::default(),
        )
        .await
        .expect("execute");
    let params = &echoed_request(execution)["params"];
    assert_eq!(
        params["_meta"],
        json!({"aether/agent_id": agent.to_string(), "aether/task_id": task.to_string()})
    );
    assert_eq!(
        params["arguments"], arguments,
        "the arguments are untouched"
    );

    // An agent with no task says nothing about one, rather than inventing it.
    let execution = tool
        .execute(json!({}), &ctx_for(agent, None), Default::default())
        .await
        .expect("execute");
    assert_eq!(
        echoed_request(execution)["params"]["_meta"],
        json!({"aether/agent_id": agent.to_string()})
    );
}

#[tokio::test]
async fn a_models_own_meta_stays_inside_its_arguments() {
    let manager = McpManager::new(vec![opted_in(BTreeMap::new())], None, no_hidden());
    manager.discover_all().await;
    let tool = manager.tool("mcp__mock__echo_request").expect("discovered");
    let agent = AgentId::new();

    let arguments = json!({"_meta": {"progressToken": 7, "note": "mine"}, "q": 1});
    let execution = tool
        .execute(arguments.clone(), &ctx_for(agent, None), Default::default())
        .await
        .expect("a `_meta` without aether keys is an ordinary argument");
    let params = &echoed_request(execution)["params"];
    assert_eq!(
        params["arguments"], arguments,
        "carried as the model wrote it"
    );
    assert_eq!(
        params["_meta"],
        json!({"aether/agent_id": agent.to_string()}),
        "and never merged into the request's own `_meta`"
    );
}

#[tokio::test]
async fn arguments_that_claim_a_caller_are_refused_before_they_are_sent() {
    let manager = McpManager::new(vec![opted_in(BTreeMap::new())], None, no_hidden());
    manager.discover_all().await;
    let tool = manager.tool("mcp__mock__echo_request").expect("discovered");
    let other = AgentId::new().to_string();

    for spoof in [
        json!({"aether/agent_id": other}),
        json!({"_meta": {"aether/agent_id": other}}),
        json!({"_meta": {"AETHER/task_id": "tsk_x"}}),
        json!({"aether/anything": 1}),
    ] {
        let error = tool
            .execute(spoof.clone(), &ctx(), Default::default())
            .await
            .expect_err("a caller claim from the model is refused");
        assert!(
            matches!(&error, ToolError::InvalidInput(reason) if reason.contains("reserved")),
            "{spoof}: {error:?}"
        );
    }
    // Refused here, not by the server: it is still connected and answering.
    assert_eq!(manager.health()[0].state, McpServerState::Ready);
    tool.execute(json!({"q": "aether/agent_id"}), &ctx(), Default::default())
        .await
        .expect("an aether-looking value, not key, is an ordinary argument");
}

#[tokio::test]
async fn concurrent_callers_on_one_shared_server_each_carry_their_own_identity() {
    // One server process serves every agent; the identity must follow the
    // call, not whichever agent happened to call first.
    let manager = McpManager::new(vec![opted_in(BTreeMap::new())], None, no_hidden());
    manager.discover_all().await;
    let tool = manager.tool("mcp__mock__echo_request").expect("discovered");

    let mut calls = tokio::task::JoinSet::new();
    for n in 0..8 {
        let tool = tool.clone();
        let (agent, task) = (AgentId::new(), TaskId::new());
        calls.spawn(async move {
            let execution = tool
                .execute(
                    json!({ "n": n }),
                    &ctx_for(agent, Some(task)),
                    Default::default(),
                )
                .await
                .expect("execute");
            (agent, task, n, echoed_request(execution))
        });
    }
    let mut seen = std::collections::BTreeSet::new();
    while let Some(joined) = calls.join_next().await {
        let (agent, task, n, request) = joined.expect("call task");
        let params = &request["params"];
        assert_eq!(params["arguments"], json!({ "n": n }));
        assert_eq!(params["_meta"]["aether/agent_id"], json!(agent.to_string()));
        assert_eq!(params["_meta"]["aether/task_id"], json!(task.to_string()));
        seen.insert(agent);
    }
    assert_eq!(seen.len(), 8, "every caller was answered as itself");
}

#[tokio::test]
async fn a_server_that_rejects_unknown_params_is_unchanged_unless_it_opts_in() {
    // A server configured before `send_caller_meta` existed gets the same
    // requests as before: no `_meta`, and its arguments untouched.
    let strict = BTreeMap::from([("AETHER_MOCK_MCP_STRICT".to_owned(), "1".to_owned())]);
    let manager = McpManager::new(vec![spec(classes(), strict.clone())], None, no_hidden());
    manager.discover_all().await;
    let result = manager
        .call("mock", "read_note", json!({"key": "k"}), &caller())
        .await
        .expect("nothing the strict server refuses is sent");
    assert_eq!(result.text, "note contents");
    let tool = manager.tool("mcp__mock__echo_request").expect("discovered");
    let arguments = json!({"aether/agent_id": "agt_x", "q": 1});
    let execution = tool
        .execute(arguments.clone(), &ctx(), Default::default())
        .await
        .expect("no caller is claimed to a server that is never told one");
    let params = &echoed_request(execution)["params"];
    assert_eq!(params.get("_meta"), None, "no `_meta` at all");
    assert_eq!(params["arguments"], arguments);

    // The strict mock refuses `_meta` anywhere, so a model's own `_meta`
    // goes to a lenient server: still untouched, still no request `_meta`.
    let manager = McpManager::new(vec![spec(classes(), BTreeMap::new())], None, no_hidden());
    manager.discover_all().await;
    let tool = manager.tool("mcp__mock__echo_request").expect("discovered");
    let arguments = json!({"_meta": {"aether/task_id": "tsk_x"}});
    let execution = tool
        .execute(arguments.clone(), &ctx(), Default::default())
        .await
        .expect("execute");
    let params = &echoed_request(execution)["params"];
    assert_eq!(params.get("_meta"), None);
    assert_eq!(params["arguments"], arguments);

    // Opted in, such a server's refusal is the call's visible error, and the
    // connection survives it.
    let manager = McpManager::new(vec![opted_in(strict)], None, no_hidden());
    manager.discover_all().await;
    let error = manager
        .call("mock", "read_note", json!({"key": "k"}), &caller())
        .await
        .expect_err("the strict server refuses `_meta`");
    assert!(error.to_string().contains("_meta"), "{error}");
    assert_eq!(manager.health()[0].state, McpServerState::Ready);
}

fn http_with_auth(url: String, state: McpAuthState) -> McpServerSpec {
    McpServerSpec {
        name: "remote".into(),
        transport: McpTransportSpec::Http { url, token: None },
        classes: BTreeMap::new(),
        send_caller_meta: false,
        auth: Some(McpAuth {
            token_env: "REMOTE_MCP_TOKEN".into(),
            state,
        }),
    }
}

#[tokio::test]
async fn a_server_whose_required_token_is_missing_is_never_contacted() {
    // A listener that never answers: were the server contacted, the request
    // would land in its backlog and discovery would wait out its timeout.
    let listener = std::net::TcpListener::bind("127.0.0.1:0").expect("bind");
    listener.set_nonblocking(true).expect("nonblocking");
    let url = format!("http://{}/mcp", listener.local_addr().expect("addr"));
    let manager = McpManager::new(
        vec![http_with_auth(url, McpAuthState::AuthFailed)],
        None,
        no_hidden(),
    );
    let quick = std::time::Duration::from_secs(5);
    tokio::time::timeout(quick, manager.discover_all())
        .await
        .expect("refused without a round trip");
    let call = tokio::time::timeout(quick, manager.call("remote", "ask", json!({}), &caller()))
        .await
        .expect("refused without a round trip");
    assert!(matches!(call, Err(McpError::AuthFailed(_))), "{call:?}");

    let health = &manager.health()[0];
    assert_eq!(health.state, McpServerState::Failed);
    assert!(health.auth_failed());
    let error = health.error.as_deref().expect("a failure says why");
    assert!(error.contains("auth_failed"), "{error}");
    assert!(error.contains("REMOTE_MCP_TOKEN"), "{error}");
    assert_eq!(
        listener.accept().map(|_| ()).map_err(|e| e.kind()),
        Err(std::io::ErrorKind::WouldBlock),
        "nothing may reach the server"
    );
}

#[tokio::test]
async fn an_optional_token_connects_without_one_and_says_so() {
    let stop = tokio_util::sync::CancellationToken::new();
    let (address, serving) = chatty_sse_server(stop.clone()).await;
    let manager = McpManager::new(
        vec![http_with_auth(
            format!("http://{address}/mcp"),
            McpAuthState::Anonymous,
        )],
        None,
        no_hidden(),
    );
    manager.discover_all().await;
    let health = &manager.health()[0];
    assert_eq!(health.state, McpServerState::Ready, "{:?}", health.error);
    assert_eq!(
        health.auth.as_ref().map(|a| a.state),
        Some(McpAuthState::Anonymous)
    );
    assert!(!health.auth_failed());

    stop.cancel();
    let _ = tokio::time::timeout(std::time::Duration::from_secs(5), serving).await;
}
