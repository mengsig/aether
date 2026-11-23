use super::*;

#[tokio::test]
async fn task_pages_include_only_task_ancestry_usage() {
    use aether_core::event::{EventDraft, EventKind};
    use aether_core::model::{CostGap, Usage};
    let (runtime, _config) = runtime().await;
    let (root, parent) = create(&runtime, None);
    let (child, mut delegated) = create(&runtime, Some(root.id));
    let (_, unrelated) = create(&runtime, Some(root.id));
    delegated.parent_task = Some(parent.id);
    runtime
        .store
        .commit(Commit::new().mutate(Mutation::UpsertTask(delegated.clone())))
        .unwrap();
    let draft = |agent, task, kind| {
        EventDraft::new(Actor::agent(agent), kind)
            .agent(agent)
            .task(task)
    };
    let usage = |tokens, estimate| Usage {
        input_tokens: tokens,
        estimated_cost_usd: estimate,
        ..Usage::default()
    };
    let snapshot = |usage| EventKind::AgentUsageUpdated {
        total: usage,
        turn_usage: usage,
        billing: None,
    };
    let invocation = |usage| EventKind::InvocationCompleted {
        invocation: aether_core::id::InvocationId::new(),
        snapshot: aether_core::id::SnapshotId::new(),
        model: ModelRef::new("example", "model"),
        usage,
        stop_reason: None,
        duration_ms: 1,
    };
    let mut unknown = usage(7, None);
    unknown.mark_cost_gap(CostGap::Unreported);
    let mut unreported = usage(0, None);
    unreported.mark_cost_gap(CostGap::Unreported);
    let commit = Commit::new()
        .event(draft(
            root.id,
            parent.id,
            EventKind::AgentStarted { turn: 1 },
        ))
        .event(draft(root.id, parent.id, snapshot(usage(10, Some(0.1)))))
        .event(draft(root.id, parent.id, invocation(usage(5, None))))
        .event(draft(root.id, parent.id, snapshot(usage(20, Some(0.2)))))
        .event(draft(root.id, parent.id, invocation(usage(2, None))))
        .event(draft(root.id, parent.id, invocation(Usage::default())))
        .event(draft(
            root.id,
            unrelated.id,
            EventKind::AgentStarted { turn: 2 },
        ))
        .event(draft(root.id, unrelated.id, snapshot(usage(1000, None))))
        .event(draft(
            root.id,
            parent.id,
            EventKind::AgentStarted { turn: 3 },
        ))
        .event(draft(root.id, parent.id, invocation(unreported)))
        .event(draft(
            child.id,
            delegated.id,
            EventKind::AgentStarted { turn: 1 },
        ))
        .event(draft(child.id, delegated.id, snapshot(unknown)))
        .event(draft(
            child.id,
            delegated.id,
            EventKind::AgentStarted { turn: 2 },
        ))
        .event(draft(child.id, delegated.id, snapshot(usage(3, Some(0.3)))));
    runtime.store.commit(commit).unwrap();
    let projected = runtime.get_task_with_usage(parent.id).unwrap().unwrap();
    assert_eq!(projected.own_usage.unwrap().input_tokens, 22);
    assert!(!projected.own_usage.unwrap().cost_complete);
    let delegated_usage = projected.delegated_usage.unwrap();
    assert_eq!(delegated_usage.input_tokens, 10);
    assert_eq!(delegated_usage.estimated_cost_usd, Some(0.3));
    assert!(!delegated_usage.cost_complete);
    let page = task(&runtime, root.id, parent.id, None, 0, MAX_TASK_CHARS)
        .unwrap()
        .structured
        .unwrap();
    let record: Task = serde_json::from_str(page["text"].as_str().unwrap()).unwrap();
    assert_eq!(record, projected);
    runtime.shutdown().await;
}

#[tokio::test]
async fn task_usage_stops_at_an_invisible_descendant() {
    use aether_core::event::{EventDraft, EventKind};
    use aether_core::model::Usage;
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
async fn expired_history_omits_only_affected_usage() {
    use aether_core::event::{EventDraft, EventKind};
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
