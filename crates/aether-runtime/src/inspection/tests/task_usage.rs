use super::*;
use aether_core::event::{EventDraft, EventKind};
use aether_core::id::{InvocationId, SnapshotId};
use aether_core::model::{CostGap, Usage};
use aether_core::snapshot::Invocation;

fn event(agent: AgentId, task: TaskId, kind: EventKind) -> EventDraft {
    EventDraft::new(Actor::agent(agent), kind)
        .agent(agent)
        .task(task)
}

fn usage_snapshot(usage: Usage) -> EventKind {
    EventKind::AgentUsageUpdated {
        total: usage,
        turn_usage: usage,
        billing: None,
    }
}

fn invocation(usage: Usage) -> EventKind {
    EventKind::InvocationCompleted {
        invocation: InvocationId::new(),
        snapshot: SnapshotId::new(),
        model: ModelRef::new("example", "model"),
        usage,
        stop_reason: None,
        duration_ms: 1,
    }
}

#[tokio::test]
async fn task_pages_include_only_task_ancestry_usage() {
    let (runtime, _config) = runtime().await;
    let (root, parent) = create(&runtime, None);
    let (child, mut delegated) = create(&runtime, Some(root.id));
    let (_, unrelated) = create(&runtime, Some(root.id));
    delegated.parent_task = Some(parent.id);
    let own = |kind| event(root.id, parent.id, kind);
    let descendant = |kind| event(child.id, delegated.id, kind);
    let other = |kind| event(root.id, unrelated.id, kind);
    let usage = |input_tokens, estimated_cost_usd| Usage {
        input_tokens,
        estimated_cost_usd,
        ..Usage::default()
    };
    let unreported = |tokens| {
        let mut usage = usage(tokens, None);
        usage.mark_cost_gap(CostGap::Unreported);
        usage
    };
    let start = |turn| EventKind::AgentStarted { turn };
    let commit = Commit::new()
        .mutate(Mutation::UpsertTask(delegated.clone()))
        .event(own(start(1)))
        .event(own(usage_snapshot(usage(10, Some(0.1)))))
        .event(own(invocation(usage(5, None))))
        .event(own(usage_snapshot(usage(20, Some(0.2)))))
        .event(own(invocation(usage(2, None))))
        .event(own(invocation(unreported(0))))
        .event(other(start(2)))
        .event(other(usage_snapshot(usage(1000, None))))
        .event(own(start(3)))
        .event(own(invocation(unreported(0))))
        .event(descendant(start(1)))
        .event(descendant(usage_snapshot(unreported(7))))
        .event(descendant(start(2)))
        .event(descendant(usage_snapshot(usage(3, Some(0.3)))));
    runtime.store.commit(commit).unwrap();
    assert_task_page_usage(&runtime, root.id, parent.id);
    runtime.shutdown().await;
}

fn assert_task_page_usage(runtime: &Runtime, caller: AgentId, parent: TaskId) {
    let projected = runtime.get_task_with_usage(parent).unwrap().unwrap();
    let own = projected.own_usage.unwrap();
    assert_eq!(own.input_tokens, 22);
    assert_eq!(own.estimated_cost_usd, Some(0.2));
    assert!(!own.cost_complete);
    let delegated_usage = projected.delegated_usage.unwrap();
    assert_eq!(delegated_usage.input_tokens, 10);
    assert_eq!(delegated_usage.estimated_cost_usd, Some(0.3));
    assert!(!delegated_usage.cost_complete);
    let page = task(runtime, caller, parent, None, 0, MAX_TASK_CHARS)
        .unwrap()
        .structured
        .unwrap();
    let record: Task = serde_json::from_str(page["text"].as_str().unwrap()).unwrap();
    assert_eq!(record, projected);
}

#[tokio::test]
async fn task_usage_stops_at_an_invisible_descendant() {
    let (runtime, _config) = runtime().await;
    let (root, parent) = create(&runtime, None);
    let (foreign, mut hidden) = create(&runtime, None);
    let (child, mut visible_child) = create(&runtime, Some(root.id));
    hidden.parent_task = Some(parent.id);
    visible_child.parent_task = Some(hidden.id);
    let start = |agent, task| {
        EventDraft::new(Actor::agent(agent), EventKind::AgentStarted { turn: 1 })
            .agent(agent)
            .task(task)
    };
    let snapshot = |agent, task, tokens| {
        let usage = Usage {
            input_tokens: tokens,
            ..Usage::default()
        };
        EventDraft::new(
            Actor::agent(agent),
            EventKind::AgentUsageUpdated {
                total: usage,
                turn_usage: usage,
                billing: None,
            },
        )
        .agent(agent)
        .task(task)
    };
    runtime
        .store
        .commit(
            Commit::new()
                .mutate(Mutation::UpsertTask(hidden.clone()))
                .mutate(Mutation::UpsertTask(visible_child.clone()))
                .event(start(foreign.id, hidden.id))
                .event(snapshot(foreign.id, hidden.id, 3))
                .event(start(child.id, visible_child.id))
                .event(snapshot(child.id, visible_child.id, 5)),
        )
        .unwrap();
    let unrestricted = runtime.get_task_with_usage(parent.id).unwrap().unwrap();
    assert_eq!(unrestricted.delegated_usage.unwrap().input_tokens, 8);
    let page = task(&runtime, root.id, parent.id, None, 0, MAX_TASK_CHARS)
        .unwrap()
        .structured
        .unwrap();
    let scoped: Task = serde_json::from_str(page["text"].as_str().unwrap()).unwrap();
    assert_eq!(scoped.delegated_usage.unwrap().input_tokens, 0);
    runtime.shutdown().await;
}

#[tokio::test]
async fn earlier_task_snapshot_cannot_cover_a_later_unreported_invocation() {
    let (runtime, _config) = runtime().await;
    let (agent, record) = create(&runtime, None);
    let draft = |kind| event(agent.id, record.id, kind);
    let mut placeholder = Usage::default();
    placeholder.mark_cost_gap(CostGap::Unreported);
    let first = Usage::cost_only(Some(0.25));
    runtime
        .store
        .commit(
            Commit::new()
                .event(draft(EventKind::AgentStarted { turn: 1 }))
                .event(draft(usage_snapshot(first)))
                .event(draft(invocation(placeholder))),
        )
        .unwrap();

    let own = || {
        runtime
            .get_task_with_usage(record.id)
            .unwrap()
            .unwrap()
            .own_usage
            .unwrap()
    };
    let uncovered = own();
    assert_eq!(uncovered.cost_usd, 0.25);
    assert_eq!(uncovered.cost_gap, Some(CostGap::Unreported));
    assert!(!uncovered.cost_complete);

    let settled = Usage::cost_only(Some(0.75));
    runtime
        .store
        .commit(Commit::new().event(draft(usage_snapshot(settled))))
        .unwrap();
    assert_eq!(own(), settled);
    runtime.shutdown().await;
}

#[tokio::test]
async fn task_usage_keeps_the_cost_of_a_failed_request_after_a_snapshot() {
    let (runtime, _config) = runtime().await;
    let (agent, task_record) = create(&runtime, None);
    let at = aether_core::time::now();
    let mut partial = Usage::cost_only(Some(0.25));
    partial.input_tokens = 20;
    partial.mark_cost_gap(CostGap::Interrupted);
    let failed = Invocation {
        id: InvocationId::new(),
        snapshot_id: SnapshotId::new(),
        agent_id: agent.id,
        turn: 1,
        model: agent.model.clone(),
        started_at: at,
        finished_at: Some(at),
        content: Vec::new(),
        usage: partial,
        stop_reason: None,
        error: Some("interrupted".into()),
        provider_message_id: None,
    };
    let draft = |kind| event(agent.id, task_record.id, kind);
    let first = Usage::cost_only(Some(0.5));
    runtime
        .store
        .commit(
            Commit::new()
                .event(draft(EventKind::AgentStarted { turn: 1 }))
                .event(draft(usage_snapshot(first)))
                .event(draft(EventKind::InvocationFailed {
                    invocation: failed.id,
                    snapshot: failed.snapshot_id,
                    error: "interrupted".into(),
                    class: None,
                    retryable: false,
                    cancelled: true,
                    partial: Vec::new(),
                }))
                .mutate(Mutation::PutInvocation(failed)),
        )
        .unwrap();

    let projected = runtime
        .get_task_with_usage(task_record.id)
        .unwrap()
        .unwrap();
    let own = projected.own_usage.unwrap();
    assert_eq!(own.input_tokens, 20);
    assert_eq!(own.cost_usd, 0.75);
    assert_eq!(own.cost_gap, Some(CostGap::Interrupted));
    assert!(!own.cost_complete);
    runtime.shutdown().await;
}

#[tokio::test]
async fn expired_history_omits_only_affected_usage() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("store.db");
    let runtime = Runtime::start_with_engines(
        RuntimeConfig {
            default_model: Some(ModelRef::new("anthropic", "claude-sonnet-5-5")),
            ..Default::default()
        },
        Arc::new(SqliteStore::open(&path).unwrap()),
        Arc::new(Models::load(Some(dir.path().into())).unwrap()),
        Arc::new(ToolRegistry::with_builtins()),
        Some(Arc::new(ScriptedClient::new())),
        Default::default(),
    )
    .await
    .unwrap();
    let (root, parent) = create(&runtime, None);
    let (child, mut descendant) = create(&runtime, Some(root.id));
    descendant.parent_task = Some(parent.id);
    runtime
        .store
        .commit(
            Commit::new()
                .mutate(Mutation::UpsertTask(descendant.clone()))
                .event(
                    EventDraft::new(Actor::agent(child.id), EventKind::AgentStarted { turn: 1 })
                        .agent(child.id)
                        .task(descendant.id)
                        .run(child.run),
                ),
        )
        .unwrap();
    let conn = rusqlite::Connection::open(path).unwrap();
    conn.execute("INSERT INTO expired_event_spans SELECT run, COALESCE(agent_id,''), COALESCE(parent_agent_id,''), COALESCE(task_id,''), type, family, MIN(seq), MAX(seq) FROM events WHERE task_id = ?1 AND type = 'agent.started' GROUP BY run, agent_id, parent_agent_id, task_id, type, family", [descendant.id.to_string()]).unwrap();
    conn.execute(
        "DELETE FROM events WHERE task_id = ?1 AND type = 'agent.started'",
        [descendant.id.to_string()],
    )
    .unwrap();
    let projected = runtime.get_task_with_usage(parent.id).unwrap().unwrap();
    assert!(projected.own_usage.is_some());
    assert!(projected.delegated_usage.is_none());
    let projected_child = runtime.get_task_with_usage(descendant.id).unwrap().unwrap();
    assert!(projected_child.own_usage.is_none());
    assert!(projected_child.delegated_usage.is_some());
    let page = task(&runtime, root.id, parent.id, None, 0, MAX_TASK_CHARS).unwrap();
    assert!(!page.is_error);
    runtime.shutdown().await;
}
