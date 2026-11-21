#![allow(clippy::expect_used, clippy::unwrap_used, clippy::panic)]

//! An MCP server's tool is an ordinary Aether tool: the same approval gate,
//! the same idempotency key, the same events. Run against the in-repo mock
//! server, so no network and no real MCP server are involved.

use std::collections::BTreeMap;
use std::sync::Arc;
use std::time::Duration;

use aether_core::agent::{
    AgentConfig, AgentStatus, AgentStatusKind, ApprovalPolicy, NetworkPolicy, Scope,
};
use aether_core::approval::{ApprovalRule, RuleMatch, RuleScope};
use aether_core::event::{Actor, EventKind};
use aether_core::id::AgentId;
use aether_core::id::ApprovalRuleId;
use aether_core::mcp::McpServerState;
use aether_core::model::ModelRef;
use aether_core::tool::{ApprovalDecision, SideEffectClass};
use aether_model::Models;
use aether_model::testing::ScriptedClient;
use aether_runtime::approval::Remember;
use aether_runtime::{CreateAgentSpec, Runtime, RuntimeConfig};
use aether_store::{Commit, EventFilter, Mutation, SqliteStore, Store};
use aether_tools::ToolRegistry;
use aether_tools::mcp::{McpManager, McpServerSpec, McpTransportSpec};
use serde_json::json;

const MOCK: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../../scripts/mock_mcp_server.sh"
);

struct Harness {
    runtime: Arc<Runtime>,
    store: Arc<dyn Store>,
    _workspace: tempfile::TempDir,
}

/// A runtime whose registry also serves the mock MCP server's tools.
async fn harness(
    responses: Vec<aether_model::testing::ScriptedResponse>,
) -> (Harness, Arc<McpManager>) {
    let manager = McpManager::new(
        vec![McpServerSpec {
            name: "mock".into(),
            transport: McpTransportSpec::Stdio {
                command: "bash".into(),
                args: vec![MOCK.to_owned()],
                env: BTreeMap::new(),
                cwd: None,
            },
            // `write_note` is deliberately left unclassified: an undeclared
            // remote tool must be gated.
            classes: BTreeMap::from([("read_note".to_owned(), SideEffectClass::ReadOnly)]),
            send_caller_meta: true,
            auth: None,
        }],
        None,
        Vec::new().into(),
    );
    manager.discover_all().await;
    assert_eq!(manager.health()[0].state, McpServerState::Ready);

    let workspace = tempfile::tempdir().expect("tempdir");
    let config_dir = tempfile::tempdir().expect("config dir");
    let store: Arc<dyn Store> = Arc::new(SqliteStore::open_in_memory().expect("store"));
    let models = Arc::new(Models::load(Some(config_dir.path().to_path_buf())).expect("models"));
    std::mem::forget(config_dir);
    let runtime = Runtime::start_with(
        RuntimeConfig {
            holder: "test".into(),
            max_concurrent_turns: 1,
            lease_ttl: Duration::from_secs(30),
            default_model: Some(ModelRef::new("anthropic", "claude-sonnet-5-5")),
            workspace_root: Some(workspace.path().to_path_buf()),
            ..Default::default()
        },
        store.clone(),
        models,
        Arc::new(ToolRegistry::with_builtins().with_mcp(manager.clone())),
        Some(Arc::new(ScriptedClient::with_responses(responses))),
    )
    .await
    .expect("runtime");
    (
        Harness {
            runtime,
            store,
            _workspace: workspace,
        },
        manager,
    )
}

/// The scope an agent needs to reach an MCP server at all: every MCP server
/// can reach the network, so a network-denied agent is offered none.
fn networked() -> Scope {
    Scope {
        network: NetworkPolicy::Allowed,
        ..Default::default()
    }
}

async fn wait_until(
    store: &dyn Store,
    id: AgentId,
    pred: impl Fn(&AgentStatus) -> bool,
) -> AgentStatus {
    let deadline = tokio::time::Instant::now() + Duration::from_secs(15);
    loop {
        let agent = store.agent(id).expect("agent").expect("exists");
        if pred(&agent.status) {
            return agent.status;
        }
        assert!(
            tokio::time::Instant::now() < deadline,
            "timed out waiting; status {:?}",
            agent.status
        );
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
}

fn events(store: &dyn Store, id: AgentId) -> Vec<aether_core::event::Event> {
    store
        .events(0, 10_000, &EventFilter::for_agent(id))
        .expect("events")
}

#[tokio::test]
async fn an_mcp_tool_is_gated_keyed_and_recorded_like_a_builtin() {
    let (h, _manager) = harness(vec![
        ScriptedClient::tool_call("mcp__mock__write_note", json!({"key": "k", "text": "hi"})),
        ScriptedClient::tool_call("complete_task", json!({"summary": "wrote the note"})),
    ])
    .await;
    let (agent, _) = h
        .runtime
        .create_agent(
            CreateAgentSpec {
                role: "worker".into(),
                objective: Some("write the note".into()),
                start: true,
                scope: Some(networked()),
                config: Some(AgentConfig::default()),
                // The gate is the subject here, so it is asked for outright
                // rather than left to the shipped default (yolo) - see
                // `an_mcp_call_runs_unapproved_under_the_shipped_yolo_default`
                // below for that case.
                approval: Some(ApprovalPolicy::default()),
                ..Default::default()
            },
            Actor::user(),
        )
        .expect("create");

    // The call is held: an undeclared MCP tool is an outside service, which
    // an `ask` policy holds for a person.
    wait_until(h.store.as_ref(), agent.id, |s| {
        s.kind() == AgentStatusKind::Waiting
    })
    .await;
    let pending = h.store.pending_approvals().expect("pending");
    assert_eq!(pending.len(), 1);
    assert_eq!(pending[0].tool, "mcp__mock__write_note");
    assert_eq!(pending[0].side_effect, SideEffectClass::External);

    // The recorded call carries a key, so a recovered turn can replay it.
    let requested = events(h.store.as_ref(), agent.id)
        .into_iter()
        .find_map(|e| match e.kind {
            EventKind::ToolRequested { call, .. } => Some(call),
            _ => None,
        })
        .expect("tool.requested");
    assert!(requested.idempotency_key.is_some(), "{requested:?}");
    assert_eq!(
        requested.resources,
        vec![aether_core::tool::ResourceKey("mcp:mock".into())]
    );

    h.runtime
        .resolve_approval(
            pending[0].id,
            ApprovalDecision::Approved,
            Actor::user(),
            None,
            None,
        )
        .expect("approve");
    let status = wait_until(h.store.as_ref(), agent.id, |s| s.is_terminal()).await;
    assert_eq!(status.kind(), AgentStatusKind::Completed);
    let succeeded = events(h.store.as_ref(), agent.id)
        .into_iter()
        .find_map(|e| match e.kind {
            EventKind::ToolSucceeded { outcome, .. } if outcome.content.contains("written") => {
                Some(outcome)
            }
            _ => None,
        })
        .expect("the server's answer reaches the model");
    assert!(!succeeded.is_error);
}

/// The readiness-matrix evidence must show what the yolo
/// default actually changed: `bash` (`Mutation`) was never gated, under
/// either default. An MCP tool is `External`, the class the default did
/// move for - so, unlike the test above, this one asks for nothing and
/// leaves `approval` unset, taking the daemon's own default (yolo, as
/// `harness` configures it) exactly the way a fresh root would.
#[tokio::test]
async fn an_mcp_call_runs_unapproved_under_the_shipped_yolo_default() {
    let (h, _manager) = harness(vec![
        ScriptedClient::tool_call("mcp__mock__write_note", json!({"key": "k", "text": "hi"})),
        ScriptedClient::tool_call("complete_task", json!({"summary": "wrote the note"})),
    ])
    .await;
    let (agent, _) = h
        .runtime
        .create_agent(
            CreateAgentSpec {
                role: "worker".into(),
                objective: Some("write the note".into()),
                start: true,
                scope: Some(networked()),
                ..Default::default()
            },
            Actor::user(),
        )
        .expect("create");
    assert!(
        agent.config.approval.is_yolo(),
        "a fresh root takes the daemon's default"
    );

    let status = wait_until(h.store.as_ref(), agent.id, |s| s.is_terminal()).await;
    assert_eq!(
        status.kind(),
        AgentStatusKind::Completed,
        "an External-class MCP call must not wait for a person under yolo"
    );
    assert!(
        h.store.pending_approvals().expect("pending").is_empty(),
        "yolo must not raise an approval for an MCP tool"
    );
    let succeeded = events(h.store.as_ref(), agent.id)
        .into_iter()
        .find_map(|e| match e.kind {
            EventKind::ToolSucceeded { outcome, .. } if outcome.content.contains("written") => {
                Some(outcome)
            }
            _ => None,
        })
        .expect("the call actually ran, not just skipped the gate");
    assert!(!succeeded.is_error);
}

#[tokio::test]
async fn a_turns_mcp_call_reaches_the_server_as_its_own_agent() {
    let (h, _manager) = harness(vec![
        ScriptedClient::tool_call("mcp__mock__echo_request", json!({"q": 1})),
        ScriptedClient::tool_call("complete_task", json!({"summary": "asked"})),
    ])
    .await;
    let (agent, task) = h
        .runtime
        .create_agent(
            CreateAgentSpec {
                role: "worker".into(),
                objective: Some("ask the server".into()),
                start: true,
                scope: Some(networked()),
                ..Default::default()
            },
            Actor::user(),
        )
        .expect("create");
    let task = task.expect("an objective is a task");

    let status = wait_until(h.store.as_ref(), agent.id, |s| s.is_terminal()).await;
    assert_eq!(status.kind(), AgentStatusKind::Completed);
    let echoed = events(h.store.as_ref(), agent.id)
        .into_iter()
        .find_map(|e| match e.kind {
            EventKind::ToolSucceeded { outcome, .. } if outcome.content.contains("tools/call") => {
                Some(outcome.content)
            }
            _ => None,
        })
        .expect("the server's echo reaches the model");
    let request: serde_json::Value = serde_json::from_str(&echoed).expect("echoed request");
    assert_eq!(
        request["params"]["_meta"],
        json!({"aether/agent_id": agent.id.to_string(), "aether/task_id": task.id.to_string()})
    );
}

#[tokio::test]
async fn a_servers_state_is_a_durable_event() {
    let (h, manager) = harness(vec![]).await;
    let health = manager.health();
    h.runtime
        .record_mcp_state(&health[0])
        .expect("record ready");
    assert_eq!(h.runtime.mcp_health(), health);

    let failed = aether_core::mcp::McpServerHealth {
        state: McpServerState::Failed,
        error: Some("pipe closed".into()),
        ..health[0].clone()
    };
    h.runtime.record_mcp_state(&failed).expect("record failure");

    let recorded: Vec<EventKind> = h
        .store
        .events(0, 10_000, &EventFilter::default())
        .expect("events")
        .into_iter()
        .map(|e| e.kind)
        .filter(|k| k.family() == "mcp")
        .collect();
    assert_eq!(
        recorded,
        vec![
            EventKind::McpServerConnected {
                server: "mock".into(),
                tools: vec![
                    "read_note".into(),
                    "write_note".into(),
                    "read_env".into(),
                    "echo_request".into(),
                ],
            },
            EventKind::McpServerFailed {
                server: "mock".into(),
                error: "pipe closed".into(),
            },
        ]
    );
}

/// A rule in force for every agent this daemon runs.
fn everywhere(decision: ApprovalDecision, matcher: RuleMatch) -> ApprovalRule {
    ApprovalRule {
        id: ApprovalRuleId::new(),
        decision,
        matcher,
        scope: RuleScope::Everywhere,
        created_at: aether_core::time::now(),
        created_by: Actor::user(),
        note: None,
        revoked: None,
    }
}

/// Run the scripted `write_note` call under `rules` and report whether a
/// person was asked.
async fn write_note_is_held(rules: Vec<ApprovalRule>) -> bool {
    note_call_is_held(json!({"key": "k", "text": "hi"}), rules).await
}

/// The same, for a chosen argument.
async fn note_call_is_held(arguments: serde_json::Value, rules: Vec<ApprovalRule>) -> bool {
    let (h, _manager) = harness(vec![
        ScriptedClient::tool_call("mcp__mock__write_note", arguments),
        ScriptedClient::tool_call("complete_task", json!({"summary": "wrote the note"})),
    ])
    .await;
    let mut commit = Commit::new();
    for rule in rules {
        commit = commit.mutate(Mutation::PutApprovalRule(rule));
    }
    h.runtime.apply(commit).expect("rules");
    let (agent, _) = h
        .runtime
        .create_agent(
            CreateAgentSpec {
                role: "worker".into(),
                objective: Some("write the note".into()),
                start: true,
                scope: Some(networked()),
                config: Some(AgentConfig::default()),
                // The gate is the subject here, so it is asked for outright
                // rather than left to the shipped default (yolo).
                approval: Some(ApprovalPolicy::default()),
                ..Default::default()
            },
            Actor::user(),
        )
        .expect("create");
    wait_until(h.store.as_ref(), agent.id, |s| {
        s.is_terminal() || s.kind() == AgentStatusKind::Waiting
    })
    .await;
    !h.store.pending_approvals().expect("pending").is_empty()
}

#[tokio::test]
async fn a_remembered_allow_decides_an_mcp_call() {
    let allow = everywhere(
        ApprovalDecision::Approved,
        RuleMatch {
            tool: Some("mcp__mock__write_note".into()),
            ..Default::default()
        },
    );
    assert!(
        !write_note_is_held(vec![allow]).await,
        "a rule about the server's tool must decide it"
    );
}

#[tokio::test]
async fn a_deny_the_server_makes_unevaluable_holds_a_call_the_allow_would_pass() {
    // The server runs the call, so Aether cannot say it never touches
    // /etc: the deny cannot be evaluated, and no allow may decide it.
    let allow = everywhere(
        ApprovalDecision::Approved,
        RuleMatch {
            tool: Some("mcp__mock__write_note".into()),
            ..Default::default()
        },
    );
    let deny = everywhere(
        ApprovalDecision::Denied,
        RuleMatch {
            resource_prefix: Some("file:/etc".into()),
            ..Default::default()
        },
    );
    assert!(
        write_note_is_held(vec![allow, deny]).await,
        "an unevaluable deny must fail the call closed, not be silently inert"
    );
}

#[tokio::test]
async fn remembering_an_mcp_approval_covers_that_call_and_not_the_whole_tool() {
    // An MCP call's only resource key is the constant `mcp:<server>`, so
    // without an argument to remember, one "always allow" would cover every
    // future call of the tool with any arguments at all.
    let answered = json!({"key": "k", "text": "hi"});
    let (h, _manager) = harness(vec![
        ScriptedClient::tool_call("mcp__mock__write_note", answered.clone()),
        ScriptedClient::tool_call("complete_task", json!({"summary": "wrote the note"})),
    ])
    .await;
    let (agent, _) = h
        .runtime
        .create_agent(
            CreateAgentSpec {
                role: "worker".into(),
                objective: Some("write the note".into()),
                start: true,
                scope: Some(networked()),
                config: Some(AgentConfig::default()),
                // The gate is the subject here, so it is asked for outright
                // rather than left to the shipped default (yolo).
                approval: Some(ApprovalPolicy::default()),
                ..Default::default()
            },
            Actor::user(),
        )
        .expect("create");
    wait_until(h.store.as_ref(), agent.id, |s| {
        s.kind() == AgentStatusKind::Waiting
    })
    .await;
    let pending = h.store.pending_approvals().expect("pending");
    assert_eq!(pending.len(), 1);
    assert_eq!(
        pending[0].argument.as_deref(),
        Some(answered.to_string().as_str()),
        "the card carries the call a rule would be written about"
    );

    h.runtime
        .resolve_approval(
            pending[0].id,
            ApprovalDecision::Approved,
            Actor::user(),
            None,
            Some(Remember::narrow(None)),
        )
        .expect("approve and remember");
    wait_until(h.store.as_ref(), agent.id, |s| s.is_terminal()).await;

    let rules = h.store.approval_rules(false).expect("rules");
    assert_eq!(rules.len(), 1);
    let matcher = &rules[0].matcher;
    assert_eq!(matcher.tool.as_deref(), Some("mcp__mock__write_note"));
    assert!(
        matcher.argument.is_some(),
        "a remembered MCP answer is pinned to its argument: {matcher:?}"
    );
    assert_eq!(
        matcher.resource_prefix, None,
        "not to `mcp:mock`, which every call of the tool carries"
    );

    // The remembered answer decides the call it was written about, and only
    // that one: a different argument still reaches a person.
    let remembered = ApprovalRule {
        scope: RuleScope::Everywhere,
        ..rules[0].clone()
    };
    assert!(
        !note_call_is_held(answered, vec![remembered.clone()]).await,
        "the answered call is decided"
    );
    assert!(
        note_call_is_held(json!({"key": "other", "text": "hi"}), vec![remembered]).await,
        "a different call is not"
    );
}
