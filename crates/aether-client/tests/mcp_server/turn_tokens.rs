use super::*;

/// The one harness tool the turn double says its agent may use.
const TURN_TOOL: &str = "complete_task";

/// A daemon double that accepts exactly one live per-turn token, the way the
/// real one does through `Runtime::engine_channel`, and answers that turn's
/// tool calls the way `Runtime::engine_tool` does.
#[derive(Debug, Default)]
struct TurnSink {
    agent: Option<aether_core::id::AgentId>,
    seen: Mutex<Vec<Command>>,
}

impl TurnSink {
    fn for_agent(agent: aether_core::id::AgentId) -> Self {
        Self {
            agent: Some(agent),
            seen: Mutex::new(Vec::new()),
        }
    }

    fn seen(&self) -> Vec<Command> {
        self.seen.lock().expect("lock").clone()
    }

    fn live(&self, agent: aether_core::id::AgentId, token: &str) -> bool {
        self.agent == Some(agent) && token == TURN_TOKEN
    }
}

#[async_trait]
impl aether_client::mcp::CommandSink for TurnSink {
    async fn call(&self, command: Command) -> anyhow::Result<Reply> {
        self.seen.lock().expect("lock").push(command.clone());
        match command {
            Command::EngineTools { agent, token } if self.live(agent, &token) => {
                Ok(Reply::EngineTools {
                    tools: vec![aether_core::model::ToolSpec {
                        name: TURN_TOOL.into(),
                        description: "Finish the task this turn was given.".into(),
                        input_schema: json!({"type": "object", "properties": {}}),
                        strict: false,
                    }],
                })
            }
            Command::EngineTools { .. } => anyhow::bail!("no engine turn is running"),
            Command::EngineTool {
                agent, token, name, ..
            } if self.live(agent, &token) => Ok(Reply::EngineTool {
                content: format!("{name} ran for this turn"),
                is_error: false,
            }),
            Command::EngineTool { .. } => anyhow::bail!("no engine turn is running"),
            Command::ListAgents { .. } => Ok(Reply::Agents(Vec::new())),
            other => anyhow::bail!("the double was not asked for {other:?}"),
        }
    }
}

#[tokio::test]
async fn a_live_per_turn_token_authenticates_and_a_stale_one_does_not() {
    // VISION 5.2 asks for a per-turn token here. The turn's token is what the
    // engine bridge already carries, and the daemon is the only thing that
    // can say whether that turn is still running - so it is asked.
    let agent = live_agent();
    let sink: Arc<dyn aether_client::mcp::CommandSink> = Arc::new(TurnSink::for_agent(agent));
    let shutdown = tokio_util::sync::CancellationToken::new();
    let (bound, serving) = serve_http(sink, shutdown.clone()).await;
    let url = format!("http://{bound}/");
    let http = reqwest::Client::new();
    let body = json!({"jsonrpc": "2.0", "id": 1, "method": "tools/list", "params": {}});

    let ok = http
        .post(&url)
        .header("authorization", format!("Bearer {agent}.{TURN_TOKEN}"))
        .json(&body)
        .send()
        .await
        .expect("per-turn token");
    assert_eq!(ok.status(), reqwest::StatusCode::OK);

    // The same token after the turn ends, or another agent's: refused.
    let stale = http
        .post(&url)
        .header(
            "authorization",
            format!("Bearer {}.{TURN_TOKEN}", live_agent()),
        )
        .json(&body)
        .send()
        .await
        .expect("another agent");
    assert_eq!(stale.status(), reqwest::StatusCode::UNAUTHORIZED);

    shutdown.cancel();
    let _ = tokio::time::timeout(std::time::Duration::from_secs(5), serving).await;
}

#[tokio::test]
async fn aethers_own_mcp_client_drives_aethers_own_http_server() {
    // The two halves of streamable HTTP ship together; nothing tested them
    // against each other, so a client that never correlated its replies and
    // a server that answers correctly both looked fine.
    let sink: Arc<dyn aether_client::mcp::CommandSink> = Arc::new(RecordingSink::default());
    let shutdown = tokio_util::sync::CancellationToken::new();
    let (bound, serving) = serve_http(sink, shutdown.clone()).await;

    let manager = aether_tools::mcp::McpManager::new(
        vec![aether_tools::mcp::McpServerSpec {
            name: "self".into(),
            transport: aether_tools::mcp::McpTransportSpec::Http {
                url: format!("http://{bound}/"),
                token: Some(TOKEN.to_owned()),
            },
            classes: std::collections::BTreeMap::new(),
            send_caller_meta: true,
            auth: None,
        }],
        None,
        Vec::new().into(),
    );
    manager.discover_all().await;
    let health = &manager.health()[0];
    assert_eq!(
        health.state,
        aether_core::mcp::McpServerState::Ready,
        "{:?}",
        health.error
    );
    assert!(health.tools.iter().any(|t| t.name == "list_agents"));

    let result = manager
        .call(
            "self",
            "list_agents",
            json!({}),
            &aether_tools::mcp::McpCaller {
                agent_id: aether_core::id::AgentId::new(),
                task_id: None,
            },
        )
        .await
        .expect("call through both halves");
    assert!(!result.is_error, "{result:?}");
    assert!(result.text.contains("agents"), "{}", result.text);

    shutdown.cancel();
    let _ = tokio::time::timeout(std::time::Duration::from_secs(5), serving).await;
}

#[tokio::test]
async fn a_per_turn_token_reaches_its_own_tools_and_not_the_fleet() {
    // The endpoint is a fleet control plane, and the per-turn token travels
    // in a child process's argv. It must therefore authorize one turn, not
    // the fleet: exactly what the daemon listed for that turn (VISION 5.2).
    let agent = live_agent();
    let sink = Arc::new(TurnSink::for_agent(agent));
    let shutdown = tokio_util::sync::CancellationToken::new();
    let (bound, serving) = serve_http(sink.clone(), shutdown.clone()).await;
    let url = format!("http://{bound}/");
    let http = reqwest::Client::new();
    let turn_bearer = format!("Bearer {agent}.{TURN_TOKEN}");

    let listed: Value = http
        .post(&url)
        .header("authorization", &turn_bearer)
        .json(&json!({"jsonrpc": "2.0", "id": 1, "method": "tools/list", "params": {}}))
        .send()
        .await
        .expect("tools/list")
        .json()
        .await
        .expect("json");
    let names: Vec<&str> = listed["result"]["tools"]
        .as_array()
        .expect("tools")
        .iter()
        .filter_map(|t| t["name"].as_str())
        .collect();
    assert_eq!(names, vec![TURN_TOOL], "a turn sees only its own tools");

    for fleet in [
        "create_agent",
        "cancel_agent",
        "send_message",
        "list_events",
    ] {
        let refused = http
            .post(&url)
            .header("authorization", &turn_bearer)
            .json(&json!({
                "jsonrpc": "2.0", "id": 2, "method": "tools/call",
                "params": {"name": fleet, "arguments": {"role": "worker", "agent": agent.to_string(), "content": "hi"}}
            }))
            .send()
            .await
            .expect("fleet call");
        assert_eq!(
            refused.status(),
            reqwest::StatusCode::FORBIDDEN,
            "{fleet} must not be reachable with a per-turn token"
        );
    }
    assert!(
        !sink
            .seen()
            .iter()
            .any(|c| !matches!(c, Command::EngineTools { .. })),
        "a refused fleet call must never reach the daemon: {:?}",
        sink.seen()
    );

    let own: Value = http
        .post(&url)
        .header("authorization", &turn_bearer)
        .json(&json!({
            "jsonrpc": "2.0", "id": 3, "method": "tools/call",
            "params": {"name": TURN_TOOL, "arguments": {}}
        }))
        .send()
        .await
        .expect("own tool")
        .json()
        .await
        .expect("json");
    assert_eq!(own["result"]["isError"], json!(false), "{own}");
    assert!(
        sink.seen().iter().any(|c| matches!(
            c,
            Command::EngineTool { agent: a, name, .. } if *a == agent && name == TURN_TOOL
        )),
        "the turn's own tool goes to the daemon as an engine tool call: {:?}",
        sink.seen()
    );

    shutdown.cancel();
    let _ = tokio::time::timeout(std::time::Duration::from_secs(5), serving).await;
}

#[tokio::test]
async fn the_operator_token_keeps_the_whole_fleet_surface() {
    // The narrowing is on the per-turn token only; nothing about the
    // operator's changed.
    let sink = Arc::new(RecordingSink::default());
    let shutdown = tokio_util::sync::CancellationToken::new();
    let (bound, serving) = serve_http(sink.clone(), shutdown.clone()).await;
    let url = format!("http://{bound}/");
    let http = reqwest::Client::new();

    let listed: Value = http
        .post(&url)
        .header("authorization", format!("Bearer {TOKEN}"))
        .json(&json!({"jsonrpc": "2.0", "id": 1, "method": "tools/list", "params": {}}))
        .send()
        .await
        .expect("tools/list")
        .json()
        .await
        .expect("json");
    let names: Vec<&str> = listed["result"]["tools"]
        .as_array()
        .expect("tools")
        .iter()
        .filter_map(|t| t["name"].as_str())
        .collect();
    for expected in [
        "create_agent",
        "cancel_agent",
        "send_message",
        "list_events",
    ] {
        assert!(
            names.contains(&expected),
            "{expected} missing from {names:?}"
        );
    }

    let called: Value = http
        .post(&url)
        .header("authorization", format!("Bearer {TOKEN}"))
        .json(&json!({
            "jsonrpc": "2.0", "id": 2, "method": "tools/call",
            "params": {"name": "send_message", "arguments": {"agent": live_agent().to_string(), "content": "hi"}}
        }))
        .send()
        .await
        .expect("send_message")
        .json()
        .await
        .expect("json");
    assert_eq!(called["result"]["isError"], json!(false), "{called}");
    assert!(
        sink.seen()
            .iter()
            .any(|c| matches!(c, Command::SendMessage { .. })),
        "the operator's call still becomes the protocol command"
    );

    shutdown.cancel();
    let _ = tokio::time::timeout(std::time::Duration::from_secs(5), serving).await;
}
