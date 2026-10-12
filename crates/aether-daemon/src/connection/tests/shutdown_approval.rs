use super::*;
use aether_core::agent::ApprovalPolicy;
use aether_core::id::AgentId;
use aether_core::model::ModelRef;
use aether_core::tool::SideEffectClass;
use aether_engine::{Engine, EngineError, EngineKind, EngineRun, EngineTurn};
use aether_runtime::{BridgeConfig, RuntimeConfig};
use aether_store::{SqliteStore, Store as _};
use tokio::io::AsyncWriteExt;

#[derive(Debug)]
struct ApprovalEngine {
    ready: mpsc::UnboundedSender<(AgentId, String)>,
}

#[async_trait::async_trait]
impl Engine for ApprovalEngine {
    fn kind(&self) -> EngineKind {
        EngineKind::CLAUDE_CODE
    }

    fn adapter(&self) -> String {
        "approval-fixture".into()
    }

    async fn run(
        &self,
        turn: EngineTurn,
        cancel: CancellationToken,
    ) -> Result<EngineRun, EngineError> {
        let args = &turn.mcp.unwrap().args;
        let after = |flag| {
            let index = args.iter().position(|arg| arg == flag).unwrap();
            args[index + 1].clone()
        };
        self.ready
            .send((after("--agent").parse().unwrap(), after("--token")))
            .unwrap();
        let (events, rx) = mpsc::channel(1);
        tokio::spawn(async move {
            cancel.cancelled().await;
            drop(events);
        });
        Ok(EngineRun {
            events: rx,
            input: None,
            decisions: None,
            stop: None,
        })
    }
}

#[tokio::test]
async fn daemon_shutdown_releases_an_engine_tool_waiting_for_approval() {
    let workspace = tempfile::tempdir().unwrap();
    let config = tempfile::tempdir().unwrap();
    let store = Arc::new(SqliteStore::open_in_memory().unwrap());
    let models = Arc::new(aether_model::Models::load(Some(config.path().to_path_buf())).unwrap());
    let (ready, mut started) = mpsc::unbounded_channel();
    let engine: Arc<dyn Engine> = Arc::new(ApprovalEngine { ready });
    let runtime = aether_runtime::Runtime::start_with_engines(
        RuntimeConfig {
            holder: "approval-shutdown-test".into(),
            default_model: Some(ModelRef::new("claude-code", "haiku")),
            workspace_root: Some(workspace.path().to_path_buf()),
            bridge: Some(BridgeConfig {
                command: "/bin/true".into(),
                socket: workspace.path().join("aetherd.sock"),
            }),
            ..Default::default()
        },
        store.clone(),
        models.clone(),
        Arc::new(aether_tools::ToolRegistry::with_builtins()),
        None,
        [(EngineKind::CLAUDE_CODE, engine)].into_iter().collect(),
    )
    .await
    .unwrap();
    runtime
        .create_agent(
            CreateAgentSpec {
                role: "worker".into(),
                objective: Some("write evidence".into()),
                approval: Some(ApprovalPolicy {
                    require_for: [SideEffectClass::IdempotentMutation].into_iter().collect(),
                }),
                start: true,
                ..Default::default()
            },
            Actor::user(),
        )
        .unwrap();
    let (agent, token) = tokio::time::timeout(Duration::from_secs(3), started.recv())
        .await
        .unwrap()
        .unwrap();
    let daemon = Arc::new(Daemon {
        runtime: runtime.clone(),
        models,
        instance_id: "inst".into(),
        version: "test".into(),
        push: None,
        remote: Default::default(),
    });
    let shutdown = CancellationToken::new();
    let (client, server) = tokio::io::duplex(16 * 1024);
    let (read, mut write) = tokio::io::split(client);
    let mut input = FramedRead::new(read, LinesCodec::new());
    let mut connections = tokio::task::JoinSet::new();
    let stopping = shutdown.clone();
    connections.spawn(async move {
        serve(
            daemon,
            server,
            ConnectionPolicy::default(),
            None,
            None,
            stopping,
        )
        .await
        .unwrap();
    });
    for (id, command) in [
        (
            1,
            Command::Hello(aether_protocol::Hello::new(
                "approval-shutdown-test",
                "test",
            )),
        ),
        (
            2,
            Command::EngineTool {
                agent,
                token,
                name: "permission".into(),
                input: serde_json::json!({
                    "tool_name": "Write",
                    "input": {"file_path": "evidence.txt", "content": "must not run"}
                }),
            },
        ),
    ] {
        let line = aether_protocol::encode(&ClientFrame { id, command }).unwrap();
        write.write_all(line.as_bytes()).await.unwrap();
        if id == 1 {
            tokio::time::timeout(Duration::from_secs(3), input.next())
                .await
                .unwrap()
                .unwrap()
                .unwrap();
        }
    }
    tokio::time::timeout(Duration::from_secs(3), async {
        while store.pending_approvals().unwrap().is_empty() {
            tokio::select! {
                reply = input.next() => panic!("permission prompt ended without approval: {reply:?}"),
                _ = tokio::task::yield_now() => {}
            }
        }
    })
    .await
    .expect("the engine tool is waiting for approval");
    shutdown.cancel();
    let stopped = tokio::time::timeout(
        Duration::from_secs(3),
        crate::shutdown_connections(&runtime, &mut connections),
    )
    .await;
    if stopped.is_err() {
        runtime.shutdown().await;
        connections.abort_all();
    }
    stopped.expect("runtime cancellation must unblock connection draining");
    assert!(!workspace.path().join("evidence.txt").exists());
}
