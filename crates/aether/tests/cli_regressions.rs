#![allow(clippy::expect_used, clippy::unwrap_used, clippy::panic)]

use std::{process::Output, time::Duration};

use aether_core::{
    agent::{Agent, WaitCondition},
    event::{Actor, Event, EventDraft, EventKind},
    id::{AgentId, EventId, InvocationId, RunId, ToolCallId},
    memory::MemoryRecord,
    task::Task,
    tool::{SideEffectClass, ToolCall},
};
use aether_protocol::{
    ClientFrame, Command, Delta, ErrorCode, ErrorReply, Reply, ServerFrame, Welcome,
};
use serde_json::json;
use tokio::{
    io::{AsyncBufReadExt, AsyncWriteExt, BufReader},
    net::UnixListener,
    process,
};

const AGENT: &str = "agt_01J00000000000000000000001";
const TASK: &str = "tsk_01J00000000000000000000002";
const MEMORY: &str = "mem_01J00000000000000000000003";
const ARTIFACT: &str = "art_01J00000000000000000000005";
const AT: &str = "2026-09-11T03:00:00Z";

fn agent() -> Agent {
    serde_json::from_value(json!({
        "id": AGENT, "name": "assistant", "role": "assistant", "scope": {},
        "model": {"provider": "mock", "model": "m1"}, "status": {"state": "ready"},
        "config": {}, "run": "run_01J00000000000000000000004", "current_task": TASK,
        "stats": {"usage": {"input_tokens": 2, "output_tokens": 1, "cache_read_tokens": 0,
            "cache_write_tokens": 0, "cost_usd": 0.001}, "turns": 1},
        "created_at": AT, "updated_at": AT
    }))
    .unwrap()
}

fn memory() -> MemoryRecord {
    serde_json::from_value(json!({
        "id": MEMORY, "scope": {"scope": "global"}, "kind": "fact", "text": "Useful fact",
        "importance": 0.5, "confidence": 1.0, "provenance": {"agent": AGENT, "task": TASK,
            "source": {"file": "NOTES.md", "commit": "abc1234"}},
        "created_at": AT
    }))
    .unwrap()
}

#[derive(Clone, Copy)]
enum RunEnd {
    Completed,
    Failed(&'static str),
    Cancelled,
    Waiting,
    Disconnected,
    OperatorHints,
}

struct Stub {
    end: RunEnd,
    delegated: bool,
    stream: bool,
    forgotten: bool,
    proposed: bool,
    memory_provenance: Option<aether_core::memory::MemoryProvenance>,
    hostile_text: bool,
    event_count: Option<u64>,
    stale_events: bool,
    event_follow: bool,
    hostile_saved_model: bool,
    reject_models: bool,
    invalid_models: bool,
    features: Vec<String>,
    /// The agent a TUI beside the socket views, for `@viewed`.
    viewed: Option<&'static str>,
}
impl Default for Stub {
    fn default() -> Self {
        Self {
            end: RunEnd::Completed,
            delegated: false,
            stream: false,
            forgotten: false,
            proposed: false,
            memory_provenance: None,
            hostile_text: false,
            event_count: None,
            stale_events: false,
            event_follow: false,
            hostile_saved_model: false,
            reject_models: false,
            invalid_models: false,
            features: Vec::new(),
            viewed: None,
        }
    }
}

/// The CLI exits on a run's last event without reading replies still on the way,
/// so the stub can meet a closed socket; that ends the conversation, like end of stream.
fn hung_up(error: &std::io::Error) -> bool {
    use std::io::ErrorKind::{BrokenPipe, ConnectionReset};
    matches!(error.kind(), BrokenPipe | ConnectionReset)
}

async fn write_frame(write: &mut tokio::net::unix::OwnedWriteHalf, frame: ServerFrame) {
    let frame = aether_protocol::encode(&frame).unwrap();
    if let Err(error) = write.write_all(frame.as_bytes()).await {
        assert!(hung_up(&error), "stub write failed: {error}");
    }
}

async fn next_request(
    lines: &mut tokio::io::Lines<BufReader<tokio::net::unix::OwnedReadHalf>>,
) -> Option<String> {
    match lines.next_line().await {
        Ok(line) => line,
        Err(error) => {
            assert!(hung_up(&error), "stub read failed: {error}");
            None
        }
    }
}

fn event(kind: EventKind) -> ServerFrame {
    ServerFrame::Event {
        sub: 1,
        event: Event::from_draft(
            EventDraft::new(Actor::System, kind).agent(AGENT.parse().unwrap()),
            EventId::new(),
            1,
            AT.parse().unwrap(),
        ),
    }
}

async fn cli(args: &[&str], stub: Stub) -> Output {
    cli_with_socket(args, stub, false).await.0
}

async fn cli_with_socket(
    args: &[&str],
    stub: Stub,
    from_env: bool,
) -> (Output, std::path::PathBuf) {
    let root = tempfile::Builder::new()
        .prefix("hint ' $` ")
        .tempdir()
        .unwrap();
    if stub.hostile_saved_model {
        aether_tui::prefs::UiPrefs {
            model: Some(format!("mock/{HOSTILE}")),
            ..Default::default()
        }
        .save(&root.path().join("config"))
        .unwrap();
    }
    let socket = root.path().join("a.sock");
    // Held until the CLI has run: a TUI's view lasts only while it does.
    let _tui = stub.viewed.map(|agent| {
        let path = aether_tui::viewed::viewed_dir(&socket);
        let mut tui = aether_tui::viewed::Publisher::new(path);
        tui.sync(Some(agent.parse().unwrap())).unwrap();
        tui
    });
    let listener = UnixListener::bind(&socket).unwrap();
    let server = tokio::spawn(async move {
        let (stream, _) = listener.accept().await.unwrap();
        let (read, mut write) = stream.into_split();
        let mut lines = BufReader::new(read).lines();
        // Scripted run frames address sub 1: the agent-scoped subscription
        // `run` opens after creating its agent. Other subscriptions get fresh ids.
        let mut next_sub = 2;
        let mut run_created = false;
        while let Some(line) = next_request(&mut lines).await {
            let frame: ClientFrame = serde_json::from_str(&line).unwrap();
            let result = match &frame.command {
                Command::Hello(_) => Ok(Reply::Welcome(Welcome {
                    protocol_version: aether_protocol::PROTOCOL_VERSION,
                    daemon_version: "stub".into(),
                    instance_id: "stub".into(),
                    schema_version: 1,
                    pid: std::process::id(),
                    features: stub.features.clone(),
                    daemon_protocol_version: None,
                    min_protocol_version: None,
                    idle_timeout_secs: None,
                    device: None,
                    submission_horizon: None,
                })),
                Command::Subscribe { query, .. } if run_created && query.agent.is_some() => {
                    Ok(Reply::Subscribed { id: 1, last_seq: 0 })
                }
                Command::Subscribe { .. } => {
                    next_sub += 1;
                    Ok(Reply::Subscribed {
                        id: next_sub - 1,
                        last_seq: 0,
                    })
                }
                Command::Unsubscribe { .. } => Ok(Reply::Ok),
                Command::CreateAgent(spec) => {
                    if let Some(root) = &spec.repository_root {
                        assert!(root.is_absolute(), "{}", root.display());
                    }
                    let mut created = agent();
                    if spec.objective.is_none() {
                        assert_eq!(spec.role, "assistant");
                        assert!(spec.start);
                        created.current_task = None;
                        created.name = spec.name.clone().unwrap_or(created.name);
                        created.model = spec.model.clone().unwrap_or(created.model);
                        created.fallbacks = spec.fallbacks.clone();
                    }
                    Ok(Reply::Agent(created))
                }
                Command::Compact { .. } => Ok(Reply::Ok),
                Command::GetEvents {
                    after,
                    limit,
                    query,
                } if stub.event_count.is_some() => {
                    assert_eq!(query.agent, Some(AGENT.parse().unwrap()));
                    let after = if stub.stale_events { 0 } else { *after };
                    let count = stub.event_count.unwrap();
                    let seqs: Vec<_> = match query.before {
                        Some(before) => {
                            assert_eq!(after, 0, "before cannot combine with a nonzero after");
                            let before = if stub.stale_events { count + 1 } else { before };
                            (1..before.min(count + 1)).rev().take(*limit).collect()
                        }
                        None => (after + 1..=count).take(*limit).collect(),
                    };
                    Ok(Reply::Events(
                        seqs.into_iter()
                            .map(|seq| {
                                Event::from_draft(
                                    EventDraft::new(
                                        Actor::System,
                                        EventKind::CompactionFailed {
                                            error: if stub.hostile_text {
                                                HOSTILE.into()
                                            } else {
                                                "failed".into()
                                            },
                                            models: vec![],
                                        },
                                    )
                                    .agent(AGENT.parse().unwrap()),
                                    EventId::new(),
                                    seq,
                                    AT.parse().unwrap(),
                                )
                            })
                            .collect(),
                    ))
                }
                Command::GetArtifact { .. } => Ok(Reply::Artifact(hostile_artifact())),
                Command::ReadArtifact { .. } => Ok(Reply::ArtifactBytes {
                    artifact: hostile_artifact(),
                    // HOSTILE encoded for the artifact payload on the wire.
                    base64: "CXNhZmUbWzJKG10wO3RpdGxlBw3igK5lbmQ=".into(),
                    truncated: false,
                }),
                Command::Archive { .. } => {
                    let mut a = agent();
                    a.name = HOSTILE.into();
                    Ok(Reply::Agents(vec![a]))
                }
                Command::AuthStatus => Ok(Reply::Auth(vec![aether_protocol::AuthInfo {
                    provider: HOSTILE.into(), provider_name: HOSTILE.into(), configured: true,
                    source: Some(HOSTILE.into()), methods: vec![HOSTILE.into()], subscription: false,
                    detail: None, program: None,
                }])),
                Command::ListApprovalRules { .. } => Ok(Reply::ApprovalRules(vec![
                    aether_core::approval::ApprovalRule {
                        id: aether_core::id::ApprovalRuleId::new(),
                        decision: aether_core::tool::ApprovalDecision::Approved,
                        matcher: aether_core::approval::RuleMatch::tool(HOSTILE),
                        scope: aether_core::approval::RuleScope::Everywhere,
                        created_at: AT.parse().unwrap(), created_by: Actor::System,
                        note: Some(HOSTILE.into()), revoked: None,
                    }
                ])),
                Command::McpServers { .. } => Ok(Reply::McpServers(vec![hostile_mcp()])),
                Command::Status => Ok(Reply::Status(
                    serde_json::from_value(json!({
                        "instance_id": "stub", "version": "stub", "pid": 1, "started_at": AT,
                        "last_seq": stub.event_count.unwrap_or(0), "agents_live": 0, "agents_running": 0, "agents_waiting": 0,
                        "agents_blocked": 0, "tasks_active": 0, "pending_approvals": 0,
                        "usage_total": aether_core::model::Usage::default(), "mcp": [hostile_mcp()],
                        "providers": [{"provider": HOSTILE, "running": 0, "backoff": {"reason": HOSTILE, "until": AT}}]
                    }))
                    .unwrap(),
                )),
                Command::GetEvents { after, .. } => Ok(Reply::Events(
                    [
                        EventKind::CompactionCompleted {
                            checkpoint: aether_core::id::CheckpointId::new(),
                            version: 1,
                            from_seq: 1,
                            to_seq: 2,
                            before_tokens: Some(100),
                            after_tokens: Some(10),
                        },
                        EventKind::CompactionCompleted {
                            checkpoint: aether_core::id::CheckpointId::new(),
                            version: 2,
                            from_seq: 3,
                            to_seq: 4,
                            before_tokens: None,
                            after_tokens: None,
                        },
                        EventKind::CompactionFailed {
                            error: "Nothing to compact".into(),
                            models: vec![],
                        },
                    ]
                    .into_iter()
                    .enumerate()
                    .map(|(seq, kind)| {
                        Event::from_draft(
                            EventDraft::new(Actor::System, kind).agent(AGENT.parse().unwrap()),
                            EventId::new(),
                            seq as u64 + 1,
                            AT.parse().unwrap(),
                        )
                    })
                    .filter(|event| event.seq > *after)
                    .collect(),
                )),
                Command::GetAgent { id } if *id == agent().id => Ok(Reply::Agent(agent())),
                Command::GetAgent { .. } => {
                    Err(ErrorReply::new(ErrorCode::NotFound, "unknown agent"))
                }
                Command::Replay { .. } => Ok(Reply::Replay(vec![])),
                Command::SendMessage { .. } => {
                    Err(ErrorReply::new(ErrorCode::NotFound, "unknown agent"))
                }
                Command::ListModels if stub.invalid_models => Err(ErrorReply::invalid("model selection refused")),
                Command::ListModels if stub.reject_models => Err(ErrorReply::new(ErrorCode::NotFound, HOSTILE)),
                Command::ListModels => Ok(Reply::Models(vec![aether_protocol::ModelInfo {
                    tier: None,
                    model: aether_core::model::ModelRef::new(
                        "mock",
                        if stub.hostile_text { HOSTILE } else { "m1" },
                    ),
                    aliases: vec![],
                    name: "Mock".into(),
                    provider_name: "Mock".into(),
                    capabilities: Default::default(),
                    configured: true,
                    auth_source: None,
                    is_default: true,
                }])),
                Command::ScheduleHistory { id, .. } => Ok(Reply::ScheduleHistory(vec![
                    aether_core::schedule::ScheduleOccurrence {
                        schedule: *id,
                        due: "2026-09-11T03:00:00.123456789Z".parse().unwrap(),
                        recorded_at: AT.parse().unwrap(),
                        outcome: aether_core::schedule::ScheduleOutcome::Skipped {
                            reason: "target is paused".into(),
                        },
                    },
                ])),
                Command::ListRuns { .. } => {
                    let listing = |usage| aether_protocol::RunListing {
                        run: agent().run,
                        objective: Some("Do work".into()),
                        agents: 1,
                        agents_live: 1,
                        agents_imported: 0,
                        usage,
                        started_at: AT.parse().unwrap(),
                        updated_at: AT.parse().unwrap(),
                    };
                    let unpriced = aether_core::model::Usage {
                        cost_usd: 0.1036,
                        cost_complete: false,
                        ..Default::default()
                    };
                    let unreported = aether_core::model::Usage {
                        cost_complete: false,
                        ..Default::default()
                    };
                    Ok(Reply::Runs(vec![
                        listing(Default::default()),
                        listing(unpriced),
                        listing(unreported),
                    ]))
                }
                Command::ListAgents { .. } => Ok(Reply::Agents(vec![agent()])),
                Command::ListTasks { .. } => {
                    let mut task =
                        Task::new("Do work", Actor::user(), RunId::new(), AT.parse().unwrap());
                    task.id = TASK.parse().unwrap();
                    task.owner = Some(AGENT.parse().unwrap());
                    Ok(Reply::Tasks(vec![task]))
                }
                Command::Cancel { .. } => Ok(Reply::Ok),
                Command::GetTree { .. } => {
                    let a = agent();
                    let mut usage = a.stats.usage;
                    let children = if stub.delegated {
                        usage.cost_usd = 0.059;
                        vec![AgentId::new()]
                    } else {
                        vec![]
                    };
                    let node = aether_protocol::TreeNode {
                        agent: a.clone(),
                        children,
                        task: None,
                        waiting_on: vec![],
                        lease: None,
                        subtree_usage: usage,
                        hidden_usage: None,
                    };
                    Ok(Reply::Tree(aether_protocol::AgentTree {
                        roots: vec![a.id],
                        nodes: [(a.id, node)].into(),
                    }))
                }
                Command::CreateMemory { spec, .. } => {
                    assert!(spec.proposed);
                    assert_eq!(spec.supersedes.map(|id| id.to_string()).as_deref(), Some(MEMORY));
                    let mut record = memory();
                    record.text = spec.text.clone();
                    record.proposed_at = Some(AT.parse().unwrap());
                    Ok(Reply::Memory(record))
                }
                Command::AcceptMemory { id, reason } => {
                    assert_eq!(id.to_string(), MEMORY);
                    assert_eq!(reason.as_deref(), Some("Verified by releases"));
                    let mut record = memory();
                    record.proposed_at = Some(AT.parse().unwrap());
                    record.accepted_at = record.proposed_at;
                    Ok(Reply::Memory(record))
                }
                Command::RejectMemory { id, reason } => {
                    assert_eq!(id.to_string(), MEMORY);
                    assert_eq!(reason, "Insufficient evidence");
                    let mut record = memory();
                    record.proposed_at = Some(AT.parse().unwrap());
                    record.forgotten_at = record.proposed_at;
                    Ok(Reply::Memory(record))
                }
                Command::Memories { query } => {
                    let mut m = memory();
                    if let Some(provenance) = &stub.memory_provenance {
                        m.provenance = provenance.clone();
                    }
                    if stub.hostile_text {
                        m.text = HOSTILE.into();
                        m.tags = vec![HOSTILE.into()];
                        m.scope = aether_core::memory::MemoryScope::Role {
                            role: HOSTILE.into(),
                        };
                        let source = m.provenance.source.as_mut().unwrap();
                        source.file = HOSTILE.into();
                        source.commit = Some(HOSTILE.into());
                    }
                    if stub.forgotten {
                        m.forgotten_at = Some(AT.parse().unwrap());
                    }
                    m.proposed_at = stub.proposed.then(|| AT.parse().unwrap());
                    if let Some(scope) = &query.scope {
                        m.scope = scope.clone();
                    }
                    let matches = query.proposed == stub.proposed && query.text.as_ref().is_none_or(|t| m.text.contains(t))
                        && query
                            .recorded_by
                            .is_none_or(|recorder| recorder.recorded(&m.provenance));
                    Ok(Reply::Memories(
                        if matches && (!stub.forgotten || query.include_forgotten) {
                            vec![m]
                        } else {
                            vec![]
                        },
                    ))
                }
                Command::AliasRepository {
                    alias,
                    root,
                    repository,
                    dry_run,
                } => {
                    let canonical = match root {
                        Some(root) => {
                            assert!(root.is_absolute(), "the CLI sends an absolute root");
                            root.display().to_string()
                        }
                        None => repository.clone().expect("a root or a repository"),
                    };
                    Ok(Reply::RepositoryAliased(
                        aether_core::repository::RepositoryAliasReport {
                            alias: alias.clone(),
                            canonical,
                            dry_run: *dry_run,
                            existing: false,
                            counts: aether_core::repository::RepositoryAliasCounts {
                                repository: 2,
                                repository_role: 1,
                                duplicates: 1,
                                agents: 3,
                            },
                        },
                    ))
                }
                Command::ListRepositoryAliases => Ok(Reply::RepositoryAliases(vec![
                    aether_core::repository::RepositoryAlias::new(
                        "archon",
                        "github.com/mengsig/archon",
                        AT.parse().unwrap(),
                    )
                    .unwrap(),
                ])),
                Command::MemoriesInContext { task, invocation } => {
                    assert_eq!(task.map(|id| id.to_string()).as_deref(), Some(TASK));
                    assert!(invocation.is_none());
                    Ok(Reply::MemoriesInContext(
                        aether_core::snapshot::MemoriesInContext {
                            snapshots: vec![aether_core::snapshot::SnapshotMemories {
                                snapshot: "snp_01J00000000000000000000006".parse().unwrap(),
                                invocations: vec![],
                                agent: AGENT.parse().unwrap(),
                                task: *task,
                                turn: 1,
                                at: AT.parse().unwrap(),
                                memories: vec![MEMORY.parse().unwrap()],
                            }],
                            truncated: false,
                        },
                    ))
                }
                other => panic!("unexpected request: {other:?}"),
            };
            write_frame(
                &mut write,
                ServerFrame::Reply {
                    id: frame.id,
                    result,
                },
            )
            .await;
            if let Command::Subscribe {
                after, ref query, ..
            } = frame.command
                && stub.event_follow
                && query.agent.is_some()
            {
                let count = stub.event_count.unwrap();
                assert_eq!(after, Some(count));
                assert_eq!(query.types, ["compaction.failed"]);
                for (offset, error) in [(1, "absent"), (2, "failed live")] {
                    let event = Event::from_draft(
                        EventDraft::new(
                            Actor::System,
                            EventKind::CompactionFailed {
                                error: error.into(),
                                models: vec![],
                            },
                        )
                        .agent(AGENT.parse().unwrap()),
                        EventId::new(),
                        count + offset,
                        AT.parse().unwrap(),
                    );
                    write_frame(
                        &mut write,
                        ServerFrame::Event {
                            sub: next_sub - 1,
                            event,
                        },
                    )
                    .await;
                }
                break;
            }
            if matches!(frame.command, Command::CreateAgent(ref spec) if spec.objective.is_some()) {
                run_created = true;
                continue;
            }
            if matches!(frame.command, Command::Subscribe { ref query, .. } if run_created && query.agent.is_some())
            {
                if stub.stream {
                    write_frame(
                        &mut write,
                        ServerFrame::Delta {
                            sub: 1,
                            agent: AGENT.parse().unwrap(),
                            invocation: InvocationId::new(),
                            delta: Delta::Text {
                                text: if stub.hostile_text {
                                    HOSTILE.into()
                                } else {
                                    "Preamble".into()
                                },
                            },
                        },
                    )
                    .await;
                    write_frame(
                        &mut write,
                        event(EventKind::ToolRequested {
                            call: ToolCall {
                                id: ToolCallId::new(),
                                provider_call_id: "call_1".into(),
                                agent_id: AGENT.parse().unwrap(),
                                task_id: Some(TASK.parse().unwrap()),
                                invocation_id: None,
                                resources: vec![],
                                idempotency_key: None,
                                requested_at: AT.parse().unwrap(),
                                name: "bash".into(),
                                input: json!({"command":"ls"}),
                            },
                            side_effect: SideEffectClass::ReadOnly,
                        }),
                    )
                    .await;
                    write_frame(
                        &mut write,
                        ServerFrame::Delta {
                            sub: 1,
                            agent: AGENT.parse().unwrap(),
                            invocation: InvocationId::new(),
                            delta: Delta::Text {
                                text: "Reply".into(),
                            },
                        },
                    )
                    .await;
                    write_frame(
                        &mut write,
                        ServerFrame::Delta {
                            sub: 1,
                            agent: AGENT.parse().unwrap(),
                            invocation: InvocationId::new(),
                            delta: Delta::TurnFinished,
                        },
                    )
                    .await;
                }
                if matches!(stub.end, RunEnd::OperatorHints) {
                    write_frame(
                        &mut write,
                        event(EventKind::QuestionAsked {
                            question: "Choose?".into(),
                        }),
                    )
                    .await;
                    write_frame(
                        &mut write,
                        event(EventKind::ApprovalRequested {
                            approval: aether_core::id::ApprovalId::new(),
                            tool: "web_fetch".into(),
                            description: "Fetch".into(),
                            argument: None,
                            side_effect: SideEffectClass::External,
                            call: ToolCallId::new(),
                        }),
                    )
                    .await;
                }
                let kind = match stub.end {
                    RunEnd::Completed => EventKind::AgentCompleted {
                        summary: Some("done".into()),
                    },
                    RunEnd::Failed(error) => EventKind::AgentFailed {
                        error: error.into(),
                    },
                    RunEnd::Cancelled => EventKind::AgentCancelled { reason: None },
                    RunEnd::Waiting | RunEnd::OperatorHints => EventKind::AgentWaiting {
                        condition: WaitCondition::Inbox,
                    },
                    RunEnd::Disconnected => break,
                };
                write_frame(&mut write, event(kind)).await;
            }
        }
    });
    let mut isolated = isolated_command(root.path(), args);
    if from_env {
        isolated = std::process::Command::new(env!("CARGO_BIN_EXE_aether"));
        isolated
            .args(args)
            .env("AETHER_SOCKET", &socket)
            .env("XDG_RUNTIME_DIR", root.path().join("runtime"))
            .env("AETHER_CONFIG_DIR", root.path().join("config"))
            .env("AETHER_DATA_DIR", root.path().join("data"))
            .env_remove("AETHER_TCP_ADDR")
            .env_remove("AETHER_MCP_TOKEN");
    }
    let mut command = process::Command::from(isolated);
    command.kill_on_drop(true);
    let output = tokio::time::timeout(Duration::from_secs(30), command.output())
        .await
        .unwrap()
        .unwrap();
    server.await.unwrap();
    (output, socket)
}

fn isolated_command(root: &std::path::Path, args: &[&str]) -> std::process::Command {
    let mut command = std::process::Command::new(env!("CARGO_BIN_EXE_aether"));
    command
        .args(args)
        .arg("--socket")
        .arg(root.join("a.sock"))
        .env("XDG_RUNTIME_DIR", root.join("runtime"))
        .env("AETHER_CONFIG_DIR", root.join("config"))
        .env("AETHER_DATA_DIR", root.join("data"))
        .env_remove("AETHER_SOCKET")
        .env_remove("AETHER_TCP_ADDR")
        .env_remove("AETHER_MCP_TOKEN");
    command
}

fn stdout(output: &Output) -> String {
    String::from_utf8(output.stdout.clone()).unwrap()
}
fn stderr(output: &Output) -> String {
    String::from_utf8(output.stderr.clone()).unwrap()
}

#[tokio::test]
async fn json_command_errors_preserve_daemon_codes_and_message() {
    for (invalid_models, code, message) in [
        (true, "invalid_argument", "model selection refused"),
        (false, "not_found", HOSTILE),
    ] {
        let (output, socket) = cli_with_socket(
            &["models", "--json"],
            Stub {
                invalid_models,
                reject_models: !invalid_models,
                ..Stub::default()
            },
            true,
        )
        .await;
        assert_eq!(output.status.code(), Some(1));
        let value: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
        assert_eq!(value["error"]["code"], code);
        assert_eq!(value["error"]["socket"], socket.to_string_lossy().as_ref());
        assert!(
            value["error"]["message"]
                .as_str()
                .unwrap()
                .contains(message)
        );
        assert_eq!(stdout(&output).lines().count(), 1);
        assert!(stderr(&output).starts_with("Error: "));
    }
}

#[test]
fn json_daemon_unreachable_reports_the_explicit_socket() {
    let root = tempfile::tempdir().unwrap();
    let output = isolated_command(root.path(), &["status", "--json"])
        .output()
        .unwrap();
    assert_eq!(output.status.code(), Some(1));
    let value: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(value["error"]["code"], "daemon_unreachable");
    assert_eq!(
        value["error"]["socket"],
        root.path().join("a.sock").to_string_lossy().as_ref()
    );
    assert!(
        value["error"]["message"]
            .as_str()
            .unwrap()
            .contains("no daemon at")
    );
    assert!(stderr(&output).starts_with("Error: "));
}

#[test]
fn storage_json_errors_honor_parent_and_leaf_flags() {
    for args in [
        vec!["storage", "--json"],
        vec!["storage", "--json", "gc", "--dry-run"],
        vec!["storage", "gc", "--dry-run", "--json"],
    ] {
        let root = tempfile::tempdir().unwrap();
        let output = isolated_command(root.path(), &args).output().unwrap();
        assert_eq!(output.status.code(), Some(1), "{args:?}");
        let value: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
        assert_eq!(value["error"]["code"], "daemon_unreachable", "{args:?}");
        assert_eq!(stdout(&output).lines().count(), 1);
        assert_eq!(
            value["error"]["socket"],
            root.path().join("a.sock").to_string_lossy().as_ref()
        );
    }
}

/// A daemon that would ignore the flag runs an ordinary sweep, so the client
/// refuses before sending anything.
#[tokio::test]
async fn storage_prune_build_output_needs_a_daemon_that_prunes() {
    let older = Stub {
        features: vec![aether_protocol::features::STORAGE.into()],
        ..Default::default()
    };
    let args = ["storage", "gc", "--prune-build-output", "--dry-run"];
    let output = cli(&args, older).await;
    assert_eq!(output.status.code(), Some(1));
    assert!(
        stderr(&output).contains("cannot prune build output"),
        "{}",
        stderr(&output)
    );
}

#[tokio::test]
async fn nested_json_client_error_uses_the_same_envelope() {
    let output = cli(&["memory", "show", "invalid-id", "--json"], Stub::default()).await;
    assert_eq!(output.status.code(), Some(1));
    let value: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(value["error"]["code"], "internal");
    assert!(
        value["error"]["message"]
            .as_str()
            .unwrap()
            .contains("invalid memory id")
    );
}

#[tokio::test]
async fn literal_json_in_message_does_not_enable_json_errors() {
    let (output, _) =
        cli_with_socket(&["send", AGENT, "--", "--json"], Stub::default(), true).await;
    assert_eq!(output.status.code(), Some(1));
    assert!(output.stdout.is_empty());
    assert!(stderr(&output).starts_with("Error: "));
}

#[test]
fn json_argument_errors_keep_clap_exit_code_and_usage() {
    let root = tempfile::tempdir().unwrap();
    let output = isolated_command(root.path(), &["memory", "show", "--json"])
        .output()
        .unwrap();
    assert_eq!(output.status.code(), Some(2));
    assert!(output.stdout.is_empty());
    assert!(stderr(&output).contains("Usage:"));
}

#[tokio::test]
async fn new_creates_a_taskless_owner_and_preserves_the_model_chain() {
    let output = cli(
        &[
            "new",
            "--name",
            "planner",
            "--model",
            "mock/m1,mock/m2",
            "--json",
        ],
        Stub::default(),
    )
    .await;
    assert!(output.status.success(), "{}", stderr(&output));
    let created: Agent = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(created.current_task, None);
    assert_eq!(created.name, "planner");
    assert_eq!(created.model.to_string(), "mock/m1");
    assert_eq!(created.fallbacks[0].to_string(), "mock/m2");
}

#[tokio::test]
async fn compact_acknowledges_acceptance_and_points_to_observable_failure() {
    let output = cli(&["compact", AGENT], Stub::default()).await;
    assert!(output.status.success(), "{}", stderr(&output));
    assert!(stdout(&output).contains("Compaction request accepted; awaiting outcome."));
    assert!(stdout(&output).contains(&format!("events --agent {AGENT} --follow")));
    assert!(stdout(&output).contains("aether --socket '"));
    let output = cli(&["events", "--agent", AGENT], Stub::default()).await;
    assert!(output.status.success(), "{}", stderr(&output));
    assert!(stdout(&output).contains("Nothing to compact"));
    assert!(stdout(&output).contains("Compaction completed: 100 -> 10 tokens"));
    assert!(
        stdout(&output)
            .lines()
            .any(|line| line.ends_with("Compaction completed"))
    );
    let output = cli(&["compact", AGENT, "--json"], Stub::default()).await;
    assert_eq!(output.status.code(), Some(0));
    assert!(output.stderr.is_empty());
    let value: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(value["compaction_requested"], true);
    assert_eq!(value["outcome"], "accepted");
    assert!(
        value["follow_up"]
            .as_str()
            .unwrap()
            .contains("events --agent")
    );
}

#[test]
fn owner_creation_and_budget_help_are_actionable_without_a_daemon() {
    let root = tempfile::tempdir().unwrap();
    let output = isolated_command(root.path(), &["task", "--help"])
        .output()
        .unwrap();
    assert!(output.status.success());
    let help = stdout(&output);
    for text in [
        "aether new --name planner",
        "jq -r .id",
        "aether task --owner",
        "unowned task stays pending",
        "dependencies have completed",
    ] {
        assert!(help.contains(text), "missing {text}: {help}");
    }
    let output = isolated_command(root.path(), &["models", "--help"])
        .output()
        .unwrap();
    assert!(output.status.success());
    for text in [
        "API providers",
        "Claude Code",
        "provider-reported",
        "Codex",
        "dollar limits are refused",
    ] {
        assert!(
            stdout(&output).contains(text),
            "missing {text}: {}",
            stdout(&output)
        );
    }
    let output = isolated_command(root.path(), &["new", "--model", "invalid"])
        .output()
        .unwrap();
    assert!(!output.status.success());
    assert!(stderr(&output).contains("model must be provider/model"));
    assert!(!root.path().join("a.sock").exists());
}

async fn check_run_exit(end: RunEnd, code: i32, outcome: &str, json: bool) {
    let mut args = vec!["run", "hello", "--model", "mock/m1", "--network=false"];
    if json {
        args.push("--json");
    }
    let out = cli(
        &args,
        Stub {
            end,
            ..Default::default()
        },
    )
    .await;
    assert_eq!(out.status.code(), Some(code), "{}", stderr(&out));
    assert!(stderr(&out).contains("usage:"));
    if json {
        assert_eq!(
            serde_json::from_slice::<serde_json::Value>(&out.stdout).unwrap()["outcome"],
            outcome
        );
    }
}

#[tokio::test]
async fn streaming_failed_runs_exit_nonzero_after_printing_usage() {
    for error in ["provider is not configured", "turn limit exceeded"] {
        check_run_exit(RunEnd::Failed(error), 1, "failed", false).await;
    }
}

#[tokio::test]
async fn json_failed_runs_exit_nonzero_after_printing_result_and_usage() {
    for error in ["provider is not configured", "turn limit exceeded"] {
        check_run_exit(RunEnd::Failed(error), 1, "failed", true).await;
    }
}

#[tokio::test]
async fn streaming_cancelled_run_has_a_distinct_exit_code() {
    check_run_exit(RunEnd::Cancelled, 3, "cancelled", false).await;
}

#[tokio::test]
async fn json_cancelled_run_has_a_distinct_exit_code() {
    check_run_exit(RunEnd::Cancelled, 3, "cancelled", true).await;
}

#[tokio::test]
async fn completed_and_waiting_runs_keep_success_exit_codes() {
    for (end, outcome) in [
        (RunEnd::Completed, "completed"),
        (RunEnd::Waiting, "waiting"),
    ] {
        for json in [false, true] {
            check_run_exit(end, 0, outcome, json).await;
        }
    }
}

#[tokio::test]
async fn run_fails_when_the_daemon_disconnects_before_an_outcome() {
    for json in [false, true] {
        let mut args = vec!["run", "hello", "--model", "mock/m1", "--network=false"];
        if json {
            args.push("--json");
        }
        let out = cli(
            &args,
            Stub {
                end: RunEnd::Disconnected,
                ..Default::default()
            },
        )
        .await;
        assert_eq!(out.status.code(), Some(1), "{}", stderr(&out));
        assert!(
            stderr(&out).contains("ended before an outcome"),
            "{}",
            stderr(&out)
        );
        if json {
            let value: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
            assert_eq!(value["error"]["code"], "internal");
            assert!(
                value["error"]["message"]
                    .as_str()
                    .unwrap()
                    .contains("ended before an outcome")
            );
        } else {
            assert!(out.stdout.is_empty());
        }
    }
}

#[tokio::test]
async fn replay_distinguishes_unknown_agents_from_real_agents_without_turns() {
    let missing = AgentId::new().to_string();
    for json in [false, true] {
        let mut args = vec!["replay", missing.as_str()];
        if json {
            args.push("--json");
        }
        let out = cli(&args, Stub::default()).await;
        assert_eq!(out.status.code(), Some(1));
        assert!(stderr(&out).contains("NotFound"));
        if json {
            let value: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
            assert_eq!(value["error"]["code"], "not_found");
        } else {
            assert!(out.stdout.is_empty());
        }
    }
    let out = cli(&["replay", AGENT], Stub::default()).await;
    assert!(out.status.success());
    assert!(stdout(&out).contains("has taken no turns yet"));
}

#[tokio::test]
async fn memory_human_output_includes_agent_and_task_provenance() {
    for args in [vec!["memory", "list"], vec!["memory", "show", MEMORY]] {
        let out = cli(&args, Stub::default()).await;
        assert!(out.status.success(), "{}", stderr(&out));
        let text = stdout(&out);
        assert!(text.contains(AGENT) && text.contains(TASK), "{text}");
        assert_eq!(text.matches("NOTES.md").count(), 1, "{text}");
    }
}

#[tokio::test]
async fn memory_empty_results_describe_filters_instead_of_claiming_no_history() {
    for (args, forgotten) in [
        (vec!["memory", "list"], true),
        (vec!["memory", "list", "--text", "absent"], false),
    ] {
        let out = cli(
            &args,
            Stub {
                forgotten,
                ..Default::default()
            },
        )
        .await;
        assert!(out.status.success());
        assert!(
            stdout(&out).contains("no memories match"),
            "{}",
            stdout(&out)
        );
        assert!(!stdout(&out).contains("no memories yet"));
        // Only an unfiltered listing explains where memories come from.
        assert_eq!(
            stdout(&out).contains("`remember`"),
            forgotten,
            "{}",
            stdout(&out)
        );
    }
}

/// The filter reaches the daemon, and a daemon that would ignore it is
/// refused rather than trusted to have applied it.
#[tokio::test]
async fn memory_list_by_recorder_needs_a_daemon_that_filters() {
    let args = ["memory", "list", "--recorded-by", "operator"];
    let old = cli(&args, Stub::default()).await;
    assert_eq!(old.status.code(), Some(1));
    assert!(
        stderr(&old).contains("cannot filter memories by recorder"),
        "{}",
        stderr(&old)
    );
    let filtering = || Stub {
        features: vec![aether_protocol::features::MEMORY_RECORDED_BY.into()],
        ..Default::default()
    };
    let by_agent = cli(&args, filtering()).await;
    assert!(by_agent.status.success(), "{}", stderr(&by_agent));
    assert!(stdout(&by_agent).contains("no memories match"));
    assert!(!stdout(&by_agent).contains("`remember`"));
    let by_operator = cli(
        &args,
        Stub {
            memory_provenance: Some(aether_core::memory::MemoryProvenance {
                actor: Some(aether_core::event::Actor::user()),
                ..Default::default()
            }),
            ..filtering()
        },
    )
    .await;
    assert!(by_operator.status.success(), "{}", stderr(&by_operator));
    assert!(
        stdout(&by_operator).contains(MEMORY),
        "{}",
        stdout(&by_operator)
    );
    let root = tempfile::tempdir().unwrap();
    let unknown = isolated_command(root.path(), &["memory", "list", "--recorded-by", "agent"])
        .output()
        .unwrap();
    assert_eq!(unknown.status.code(), Some(2));
    assert!(
        stderr(&unknown).contains("expected `operator`"),
        "{}",
        stderr(&unknown)
    );
}

/// An older daemon cannot alias, and is told so before anything is sent;
/// a dry run says what would move and that nothing did.
#[tokio::test]
async fn memory_alias_needs_a_daemon_that_aliases_and_previews_the_move() {
    let dry = [
        "memory",
        "alias",
        "archon",
        "--repository",
        "github.com/mengsig/archon",
        "--dry-run",
    ];
    let old = cli(&dry, Stub::default()).await;
    assert_eq!(old.status.code(), Some(1));
    assert!(
        stderr(&old).contains("cannot alias repositories"),
        "{}",
        stderr(&old)
    );
    let aliasing = || Stub {
        features: vec![aether_protocol::features::REPOSITORY_ALIASES.into()],
        ..Default::default()
    };
    let preview = cli(&dry, aliasing()).await;
    assert!(preview.status.success(), "{}", stderr(&preview));
    let text = stdout(&preview);
    assert!(
        text.contains("archon -> github.com/mengsig/archon: would move 2 repository and 1 repository-role memories (1 already held by the target)"),
        "{text}"
    );
    assert!(text.contains("3 agent(s) name archon"), "{text}");
    assert!(text.contains("dry run: nothing moved"), "{text}");

    let relative = cli(
        &["memory", "alias", "archon", "--root", "src/archon"],
        aliasing(),
    )
    .await;
    assert!(relative.status.success(), "{}", stderr(&relative));
    assert!(
        stdout(&relative).contains(": moved 2 repository"),
        "{}",
        stdout(&relative)
    );
    assert!(!stdout(&relative).contains("dry run"));

    let listed = cli(&["memory", "aliases"], aliasing()).await;
    assert!(listed.status.success(), "{}", stderr(&listed));
    assert!(stdout(&listed).contains("github.com/mengsig/archon"));
    let root = tempfile::tempdir().unwrap();
    for args in [
        vec!["memory", "alias", "archon"],
        vec![
            "memory",
            "alias",
            "archon",
            "--root",
            "/a",
            "--repository",
            "b",
        ],
    ] {
        let refused = isolated_command(root.path(), &args).output().unwrap();
        assert_eq!(refused.status.code(), Some(2), "{args:?}");
    }
}

/// An older daemon is refused before anything is sent; a current one lists
/// each model call with the memories it had in context.
#[tokio::test]
async fn memory_context_lists_the_memories_each_model_call_saw() {
    let args = ["memory", "context", "--task", TASK];
    let old = cli(&args, Stub::default()).await;
    assert_eq!(old.status.code(), Some(1));
    assert!(
        stderr(&old).contains("which memories were in context"),
        "{}",
        stderr(&old)
    );
    let current = || Stub {
        features: vec![aether_protocol::features::MEMORY_CONTEXT.into()],
        ..Default::default()
    };
    let listed = cli(&args, current()).await;
    assert!(listed.status.success(), "{}", stderr(&listed));
    let text = stdout(&listed);
    assert!(text.contains("snp_01J00000000000000000000006"), "{text}");
    assert!(text.contains(MEMORY), "{text}");
    let json = cli(&["memory", "context", "--task", TASK, "--json"], current()).await;
    let wire: serde_json::Value = serde_json::from_str(&stdout(&json)).unwrap();
    assert_eq!(wire["snapshots"][0]["memories"][0], MEMORY);
    let root = tempfile::tempdir().unwrap();
    for args in [
        vec!["memory", "context"],
        vec!["memory", "context", "--task", TASK, "--invocation", "inv_x"],
    ] {
        let refused = isolated_command(root.path(), &args).output().unwrap();
        assert_eq!(refused.status.code(), Some(2), "{args:?}");
    }
}

#[tokio::test]
async fn table_headers_align_with_agent_and_task_data() {
    for args in [vec!["agents"], vec!["tasks"]] {
        let out = cli(&args, Stub::default()).await;
        assert!(out.status.success());
        let text = stdout(&out);
        let mut lines = text.lines();
        let header = lines.next().unwrap();
        let row = lines.next().unwrap();
        let fields = if args[0] == "agents" {
            vec![
                ("id", AGENT),
                ("name", "assistant"),
                ("status", "ready"),
                ("model", "mock/m1"),
                ("task", TASK),
            ]
        } else {
            vec![
                ("id", TASK),
                ("status", "queued"),
                ("owner", AGENT),
                ("objective", "Do work"),
            ]
        };
        for (heading, value) in fields {
            assert_eq!(header.find(heading), row.find(value), "{text}");
        }
        let spans = |line: &str| {
            let mut spans = Vec::new();
            let mut offset = 0;
            for token in line.split_whitespace() {
                let start = offset + line[offset..].find(token).unwrap();
                spans.push((start, start + token.len()));
                offset = start + token.len();
            }
            spans
        };
        if args[0] == "agents" {
            for index in 4..7 {
                assert_eq!(spans(header)[index].1, spans(row)[index].1, "{text}");
            }
        } else {
            assert_eq!(header.find("waits on"), Some(spans(row)[3].0), "{text}");
        }
    }
}

#[tokio::test]
async fn streaming_tool_notice_starts_on_a_new_line() {
    let out = cli(
        &["run", "hello", "--model", "mock/m1", "--network=false"],
        Stub {
            stream: true,
            ..Default::default()
        },
    )
    .await;
    assert!(out.status.success(), "{}", stderr(&out));
    // stdout must terminate the partial text before stderr prints a tool notice.
    assert_eq!(stdout(&out), "Preamble\nReply\n");
    assert!(stderr(&out).contains("▸ bash"));
}

/// A daemon that would ignore the checkout is refused before the agent is
/// created under the wrong repository.
#[tokio::test]
async fn run_names_a_checkout_only_to_a_daemon_that_derives_its_repository() {
    let checkout = tempfile::tempdir().unwrap();
    let args = [
        "run",
        "hello",
        "--model",
        "mock/m1",
        "--network=false",
        "--repository-root",
        checkout.path().to_str().unwrap(),
    ];
    let old = cli(&args, Stub::default()).await;
    assert_eq!(old.status.code(), Some(1));
    assert!(
        stderr(&old).contains("cannot derive a repository"),
        "{}",
        stderr(&old)
    );
    let deriving = Stub {
        features: vec![aether_protocol::features::REPOSITORY_ROOT.into()],
        ..Default::default()
    };
    let new = cli(&args, deriving).await;
    assert!(new.status.success(), "{}", stderr(&new));
}

#[tokio::test]
async fn daemon_error_is_printed_once() {
    let out = cli(
        &["send", &AgentId::new().to_string(), "hello"],
        Stub::default(),
    )
    .await;
    let text = stderr(&out);
    assert_eq!(text.matches("unknown agent").count(), 1, "{text}");
}

#[tokio::test]
async fn delegated_run_reports_own_usage_and_daemon_subtree_usage() {
    let out = cli(
        &[
            "run",
            "hello",
            "--model",
            "mock/m1",
            "--network=false",
            "--json",
        ],
        Stub {
            delegated: true,
            ..Default::default()
        },
    )
    .await;
    assert!(out.status.success(), "{}", stderr(&out));
    let text = stderr(&out);
    assert!(
        text.contains("$0.0010") && text.contains("subtree") && text.contains("$0.0590"),
        "{text}"
    );
    let value: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(value["usage"]["cost_usd"], 0.001);
    assert_eq!(value["subtree_usage"]["cost_usd"], 0.059);
}

#[tokio::test]
async fn cancel_acknowledgement_explains_subtree_and_protocol_limit() {
    let out = cli(&["cancel", AGENT], Stub::default()).await;
    assert!(out.status.success());
    assert!(
        stdout(&out).contains("descendants") && stdout(&out).contains("daemon"),
        "{}",
        stdout(&out)
    );
}

#[tokio::test]
async fn an_agent_is_named_by_prefix_name_or_last_through_the_daemon() {
    for reference in ["01J0000", "agt_01j0000", "assistant", "@last"] {
        let out = cli(&["cancel", reference, "--json"], Stub::default()).await;
        assert!(out.status.success(), "{reference}: {}", stderr(&out));
        let reply: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
        assert_eq!(reply["agent"], AGENT, "{reference}");
    }
    let out = cli(&["cancel", "nobody"], Stub::default()).await;
    assert_eq!(out.status.code(), Some(1));
    assert!(
        stderr(&out).contains("no agent matches `nobody`"),
        "{}",
        stderr(&out)
    );
}

#[tokio::test]
async fn viewed_names_the_agent_a_tui_beside_the_socket_views() {
    let viewing = Stub {
        viewed: Some(AGENT),
        ..Stub::default()
    };
    let out = cli(&["cancel", "@viewed", "--json"], viewing).await;
    assert!(out.status.success(), "{}", stderr(&out));
    let reply: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(reply["agent"], AGENT);

    let out = cli(&["cancel", "@viewed"], Stub::default()).await;
    assert_eq!(out.status.code(), Some(1));
    assert!(
        stderr(&out).contains("no running TUI views an agent"),
        "{}",
        stderr(&out)
    );
}

#[tokio::test]
async fn a_memory_scope_names_its_agent_like_any_reference() {
    for scope in ["agent:assistant", "agent:01J0000", "agent:@last"] {
        let out = cli(
            &["memory", "list", "--scope", scope, "--json"],
            Stub::default(),
        )
        .await;
        assert!(out.status.success(), "{scope}: {}", stderr(&out));
        let listed: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
        assert_eq!(listed[0]["scope"]["id"], AGENT, "{scope}: {listed}");
    }
}

fn check_help(args: &[&str], phrases: &[&str]) {
    let root = tempfile::tempdir().unwrap();
    let out = isolated_command(root.path(), args).output().unwrap();
    assert!(out.status.success());
    let text = stdout(&out);
    for phrase in phrases {
        assert!(text.contains(phrase), "{args:?} missing {phrase}: {text}");
    }
}

#[test]
fn run_help_documents_exit_codes() {
    check_help(
        &["run", "--help"],
        &["Exit codes", "0", "1", "3", "cancelled"],
    );
}

#[test]
fn run_help_explains_external_only_and_local_mutation_approval() {
    check_help(
        &["run", "--help"],
        &["external", "local", "--approval", "mutation", "/new"],
    );
}

#[test]
fn task_help_explains_owner_eligibility() {
    check_help(&["task", "--help"], &["taskless", "/new", "aether run"]);
}

#[test]
fn watcher_help_documents_automatic_default_and_preserved_dispatch() {
    check_help(
        &["github-watcher", "add", "--help"],
        &["automatic", "unchanged"],
    );
}

#[test]
fn schedule_help_explains_payload_requirement() {
    check_help(
        &["schedule", "create", "--help"],
        &["Required with --target"],
    );
}

#[test]
fn schedule_recurrence_error_requires_one_of_the_two_modes() {
    let root = tempfile::tempdir().unwrap();
    let out = isolated_command(
        root.path(),
        &["schedule", "create", "timer", "--objective", "work"],
    )
    .output()
    .unwrap();
    assert_eq!(out.status.code(), Some(2));
    let text = stderr(&out);
    assert!(text.contains("--every <EVERY>|--cron <CRON>"), "{text}");
    for mode in [vec!["--every", "5"], vec!["--cron", "* * * * *"]] {
        let out = isolated_command(
            root.path(),
            &["schedule", "create", "timer", "--target", AGENT],
        )
        .args(mode)
        .output()
        .unwrap();
        assert_eq!(out.status.code(), Some(2));
        assert!(stderr(&out).contains("--payload"));
    }
}

#[tokio::test]
async fn model_and_run_table_columns_align_with_their_headers() {
    for args in [vec!["models"], vec!["runs", "list"]] {
        let output = cli(&args, Stub::default()).await;
        assert!(output.status.success(), "{}", stderr(&output));
        let text = stdout(&output);
        let lines = text.lines().collect::<Vec<_>>();
        let header = lines[0];
        if args[0] == "models" {
            let row = lines[1];
            assert_eq!(header.find("status"), row.find("ready"), "{text}");
            assert_eq!(row.split_whitespace().count(), 7, "{text}");
            continue;
        }
        // Byte offsets: "≥" is three bytes, so compare character columns.
        let column =
            |line: &str, needle: &str| line.find(needle).map(|at| line[..at].chars().count());
        assert_eq!(lines.len(), 4, "{text}");
        for row in &lines[1..] {
            assert_eq!(
                column(header, "started"),
                column(row, "2026-09-11"),
                "{text}"
            );
            assert_eq!(
                column(header, "objective"),
                column(row, "Do work"),
                "{text}"
            );
        }
    }
}

#[tokio::test]
async fn schedule_history_text_uses_seconds_and_json_keeps_nanoseconds() {
    let schedule = "sch_01J00000000000000000000005";
    let output = cli(&["schedule", "history", schedule], Stub::default()).await;
    assert!(output.status.success(), "{}", stderr(&output));
    assert_eq!(
        stdout(&output),
        "2026-09-11 03:00:00 UTC  skipped · target is paused\n"
    );
    let output = cli(
        &["schedule", "history", schedule, "--json"],
        Stub::default(),
    )
    .await;
    assert!(output.status.success(), "{}", stderr(&output));
    let entries: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(entries[0]["due"], "2026-09-11T03:00:00.123456789Z");
}

#[test]
fn empty_run_objective_example_preserves_the_selected_socket() {
    let root = tempfile::tempdir().unwrap();
    let out = isolated_command(root.path(), &["run", " "])
        .output()
        .unwrap();
    assert_eq!(out.status.code(), Some(1));
    assert!(
        stderr(&out).contains(&format!(
            "aether --socket '{}' run",
            root.path().join("a.sock").display()
        )),
        "{}",
        stderr(&out)
    );
    assert!(!root.path().join("a.sock").exists());
    assert!(!root.path().join("data").exists());
}

#[tokio::test]
async fn continuation_hints_preserve_flag_and_environment_sockets() {
    for from_env in [false, true] {
        let (out, socket) = cli_with_socket(
            &["run", "hello", "--model", "mock/m1", "--network=false"],
            Stub {
                end: RunEnd::OperatorHints,
                ..Default::default()
            },
            from_env,
        )
        .await;
        assert!(out.status.success(), "{}", stderr(&out));
        let prefix = format!(
            "aether --socket '{}'",
            socket.display().to_string().replace('\'', "'\"'\"'")
        );
        let text = stderr(&out);
        assert!(
            text.contains(&format!("run: {prefix} send {AGENT}")),
            "{text}"
        );
        assert!(
            text.contains(&format!("run: {prefix} approve apr_")),
            "{text}"
        );
        assert!(
            text.contains(&format!(
                "continue in `{prefix}` or `{prefix} send {AGENT}`"
            )),
            "{text}"
        );
        // Exercise the printed quoting through the shell, without invoking a daemon.
        let parsed = std::process::Command::new("sh")
            .arg("-c")
            .arg(format!("set -- {prefix}; printf '%s' \"$3\""))
            .output()
            .unwrap();
        assert!(parsed.status.success());
        assert_eq!(stdout(&parsed), socket.to_str().unwrap());
    }
}

#[test]
fn doctor_and_daemon_reject_unwritable_future_paths_with_context() {
    use std::os::unix::fs::PermissionsExt;
    if rustix::process::geteuid().is_root() {
        return;
    }
    let root = tempfile::tempdir().unwrap();
    let data = root.path().join("data");
    std::fs::create_dir(&data).unwrap();
    std::fs::set_permissions(&data, std::fs::Permissions::from_mode(0o500)).unwrap();
    let doctor = isolated_command(root.path(), &["doctor", "--json"])
        .env("PATH", root.path().join("empty-bin"))
        .output()
        .unwrap();
    assert_eq!(doctor.status.code(), Some(2), "{}", stderr(&doctor));
    let report: serde_json::Value = serde_json::from_slice(&doctor.stdout).unwrap();
    for name in ["store", "logs"] {
        let check = report["checks"]
            .as_array()
            .unwrap()
            .iter()
            .find(|check| check["name"] == name)
            .unwrap();
        assert_eq!(check["level"], "fail", "{check}");
        assert!(
            check["detail"]
                .as_str()
                .unwrap()
                .contains(&data.display().to_string()),
            "{check}"
        );
    }
    assert_eq!(std::fs::read_dir(&data).unwrap().count(), 0);
    let out = isolated_command(root.path(), &["daemon", "--foreground"])
        .output()
        .unwrap();
    std::fs::set_permissions(&data, std::fs::Permissions::from_mode(0o700)).unwrap();
    assert_eq!(out.status.code(), Some(1));
    let error = stderr(&out);
    assert!(error.contains("initialize log stream"), "{error}");
    assert!(
        error.contains(
            &data
                .join("logs/aetherd.log/segments-v1/events.0.log")
                .display()
                .to_string()
        ),
        "{error}"
    );
    assert!(error.contains("Permission denied"), "{error}");
    assert_eq!(std::fs::read_dir(data).unwrap().count(), 0);
}

#[tokio::test]
async fn operator_memory_sources_render_and_legacy_empty_sources_still_read() {
    for actor in [
        Some(Actor::connected_user(
            aether_core::event::OperatorOrigin::LocalSocket {
                connection: "operator-connection".into(),
            },
        )),
        None,
    ] {
        for args in [vec!["memory", "list"], vec!["memory", "show", MEMORY]] {
            let out = cli(
                &args,
                Stub {
                    memory_provenance: Some(aether_core::memory::MemoryProvenance {
                        actor: actor.clone(),
                        ..Default::default()
                    }),
                    ..Default::default()
                },
            )
            .await;
            assert!(out.status.success(), "{}", stderr(&out));
            let text = stdout(&out);
            assert!(text.contains("Useful fact"), "{text}");
            if let Some(actor) = &actor {
                assert!(text.contains(&actor.label()), "{text}");
            } else {
                assert!(!text.contains("provenance"), "{text}");
            }
        }
    }
}

const HOSTILE: &str = "\tsafe\x1b[2J\x1b]0;title\x07\r\u{202e}end";

fn hostile_mcp() -> aether_core::mcp::McpServerHealth {
    aether_core::mcp::McpServerHealth {
        state: aether_core::mcp::McpServerState::Failed,
        error: Some(HOSTILE.into()),
        tools: vec![aether_core::mcp::McpToolInfo {
            name: HOSTILE.into(),
            qualified_name: HOSTILE.into(),
            side_effect: SideEffectClass::ReadOnly,
            declared: true,
        }],
        ..aether_core::mcp::McpServerHealth::unknown(HOSTILE, HOSTILE)
    }
}

#[tokio::test]
async fn terminal_output_sanitises_memory_mcp_status_events_and_streams() {
    for args in [
        vec!["memory", "show", MEMORY],
        vec!["mcp", "list"],
        vec!["status"],
        vec!["artifacts", "show", ARTIFACT],
        vec!["archive", AGENT],
        vec!["models"],
        vec!["auth"],
        vec!["rules", "list"],
        vec!["events", "--agent", AGENT],
        vec!["run", "Do work", "--model", "mock/m1"],
    ] {
        let is_events = args[0] == "events";
        let output = cli(
            &args,
            Stub {
                hostile_text: true,
                stream: true,
                event_count: is_events.then_some(1),
                ..Default::default()
            },
        )
        .await;
        assert!(output.status.success(), "{args:?}: {}", stderr(&output));
        for text in [stdout(&output), stderr(&output)] {
            assert!(
                !text.contains(['\x1b', '\x07', '\r', '\u{202e}']),
                "{args:?}: {text:?}"
            );
        }
        assert!(
            stdout(&output).contains("safe"),
            "{args:?}: {}",
            stdout(&output)
        );
    }
}

#[tokio::test]
async fn memory_and_mcp_json_preserve_original_text() {
    for args in [
        vec!["memory", "show", MEMORY, "--json"],
        vec!["mcp", "list", "--json"],
    ] {
        let output = cli(
            &args,
            Stub {
                hostile_text: true,
                ..Default::default()
            },
        )
        .await;
        assert!(output.status.success(), "{}", stderr(&output));
        let value: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
        let text = if args[0] == "memory" {
            &value["text"]
        } else {
            &value[0]["error"]
        };
        assert_eq!(text, HOSTILE);
    }
}

#[tokio::test]
async fn events_page_until_exhausted_with_filters_and_stable_lines() {
    for json in [false, true] {
        let mut args = vec!["events", "--agent", AGENT, "--after", "7"];
        if json {
            args.push("--json");
        }
        let output = cli(
            &args,
            Stub {
                event_count: Some(2012),
                ..Default::default()
            },
        )
        .await;
        assert!(output.status.success(), "{}", stderr(&output));
        let text = stdout(&output);
        let seqs: Vec<u64> = text
            .lines()
            .map(|line| {
                if json {
                    serde_json::from_str::<Event>(line).unwrap().seq
                } else {
                    line.split_whitespace().next().unwrap().parse().unwrap()
                }
            })
            .collect();
        assert_eq!(seqs.len(), 2005);
        assert_eq!(seqs, (8..=2012).collect::<Vec<_>>());
    }
}

#[tokio::test]
async fn events_reject_a_page_that_does_not_advance() {
    let output = cli(
        &["events", "--agent", AGENT],
        Stub {
            event_count: Some(1),
            stale_events: true,
            ..Default::default()
        },
    )
    .await;
    assert!(!output.status.success());
    assert!(stderr(&output).contains("event page did not advance past 1"));
}

fn hostile_artifact() -> aether_core::artifact::Artifact {
    serde_json::from_value(json!({
        "id": ARTIFACT, "kind": {"kind": "other", "label": HOSTILE},
        "name": HOSTILE, "summary": HOSTILE, "size_bytes": HOSTILE.len(),
        "created_by": {"actor": "system"}, "created_at": AT
    }))
    .unwrap()
}

#[tokio::test]
async fn saved_model_warnings_sanitise_labels_and_daemon_errors() {
    for reject_models in [false, true] {
        let output = cli(
            &["run", "Do work", "--network"],
            Stub {
                hostile_saved_model: true,
                reject_models,
                ..Default::default()
            },
        )
        .await;
        assert!(output.status.success(), "{}", stderr(&output));
        let text = stderr(&output);
        assert!(text.contains("saved model mock/ safe"), "{text}");
        assert!(
            !text.contains(['\x1b', '\x07', '\r', '\u{202e}']),
            "{text:?}"
        );
    }
}

#[tokio::test]
async fn document_outputs_preserve_tabs_while_stripping_unsafe_controls() {
    for args in [
        vec!["run", "Do work", "--model", "mock/m1"],
        vec!["artifacts", "show", ARTIFACT],
    ] {
        let output = cli(
            &args,
            Stub {
                hostile_text: true,
                stream: true,
                ..Default::default()
            },
        )
        .await;
        assert!(output.status.success(), "{}", stderr(&output));
        let text = stdout(&output);
        assert!(text.contains("\tsafe[2J]0;titleend"), "{args:?}: {text:?}");
        assert!(!text.contains(['\x1b', '\x07', '\r', '\u{202e}']));
    }
}

#[cfg(unix)]
#[test]
fn json_error_with_a_closed_stdout_pipe_returns_failure_without_panicking() {
    let root = tempfile::tempdir().unwrap();
    let (reader, writer) = std::os::unix::net::UnixStream::pair().unwrap();
    drop(reader);
    let descriptor: std::os::fd::OwnedFd = writer.into();
    let output = isolated_command(root.path(), &["send", "invalid-agent", "hello", "--json"])
        .stdout(std::process::Stdio::from(descriptor))
        .stderr(std::process::Stdio::piped())
        .output()
        .unwrap();
    assert_eq!(output.status.code(), Some(1));
    assert!(stderr(&output).contains("Error:"), "{}", stderr(&output));
    assert!(!stderr(&output).contains("panicked"), "{}", stderr(&output));
}

#[tokio::test]
async fn memory_proposal_cli_checks_support_and_sends_review_commands() {
    let supported = || Stub {
        features: vec![aether_protocol::features::MEMORY_PROPOSALS.into()],
        ..Default::default()
    };
    let commands = [
        vec![
            "memory",
            "add",
            "A durable release verification lesson",
            "--proposed",
            "--kind",
            "lesson",
            "--supersedes",
            MEMORY,
        ],
        vec![
            "memory",
            "accept",
            MEMORY,
            "--reason",
            "Verified by releases",
        ],
        vec![
            "memory",
            "reject",
            MEMORY,
            "--reason",
            "Insufficient evidence",
        ],
    ];
    for args in commands {
        let old = cli(&args, Stub::default()).await;
        assert_eq!(old.status.code(), Some(1));
        assert!(
            stderr(&old).contains("cannot review memory proposals"),
            "{}",
            stderr(&old)
        );
        let current = cli(&args, supported()).await;
        assert!(current.status.success(), "{}", stderr(&current));
        assert!(stdout(&current).contains(MEMORY));
    }
}

#[tokio::test]
async fn memory_proposal_cli_lists_only_with_explicit_filter_and_marks_state() {
    let pending = || Stub {
        proposed: true,
        features: vec![aether_protocol::features::MEMORY_PROPOSALS.into()],
        ..Default::default()
    };
    let ordinary = cli(&["memory", "list", "--all"], pending()).await;
    assert!(ordinary.status.success(), "{}", stderr(&ordinary));
    assert!(!stdout(&ordinary).contains(MEMORY));
    let listed = cli(&["memory", "list", "--proposed"], pending()).await;
    assert!(listed.status.success(), "{}", stderr(&listed));
    assert!(stdout(&listed).contains("proposed") && stdout(&listed).contains(MEMORY));
    let rejected = cli(
        &["memory", "list", "--proposed", "--all"],
        Stub {
            forgotten: true,
            ..pending()
        },
    )
    .await;
    assert!(rejected.status.success(), "{}", stderr(&rejected));
    assert!(stdout(&rejected).contains("rejected"));
    let shown = cli(&["memory", "show", MEMORY], pending()).await;
    assert!(shown.status.success(), "{}", stderr(&shown));
    assert!(stdout(&shown).contains("[proposed]"));
}

#[test]
fn memory_proposal_reject_requires_a_reason_before_connecting() {
    let root = tempfile::tempdir().unwrap();
    let output = isolated_command(root.path(), &["memory", "reject", MEMORY])
        .output()
        .unwrap();
    assert_eq!(output.status.code(), Some(2));
    assert!(stderr(&output).contains("--reason"), "{}", stderr(&output));
}

#[path = "cli_regressions/memory_proposals.rs"]
mod proposal_lookup_tests;

#[path = "cli_regressions/memory_feedback.rs"]
mod feedback_cli_tests;

#[path = "cli_regressions/events.rs"]
mod events;
