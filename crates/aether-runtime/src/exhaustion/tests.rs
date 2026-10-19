use super::*;
use crate::{CreateAgentSpec, RuntimeConfig};
use aether_core::agent::{AgentStatus, WaitCondition};
use aether_core::id::{InvocationId, SnapshotId};
use aether_core::model::ModelErrorClass;
use aether_core::model::{ModelRef, Usage};
use aether_core::snapshot::Invocation;
use aether_core::task::TaskStatus;
use aether_core::time::now;
use aether_model::Models;
use aether_store::{Mutation, SqliteStore, Store};
use aether_tools::ToolRegistry;
use std::sync::Arc;

async fn runtime(store: Arc<dyn Store>, dir: &std::path::Path) -> Arc<Runtime> {
    Runtime::start_with_engines(
        RuntimeConfig {
            max_concurrent_turns: 0,
            default_model: Some(ModelRef::new("anthropic", "claude-sonnet-5-5")),
            workspace_root: Some(dir.to_path_buf()),
            ..Default::default()
        },
        store,
        Arc::new(Models::load(Some(dir.to_path_buf())).unwrap()),
        Arc::new(ToolRegistry::with_builtins()),
        Some(Arc::new(aether_model::testing::ScriptedClient::default())),
        Default::default(),
    )
    .await
    .unwrap()
}

fn create(runtime: &Runtime, parent: Option<AgentId>) -> Agent {
    runtime
        .create_agent(
            CreateAgentSpec {
                role: "worker".into(),
                parent,
                config: Some(aether_core::agent::AgentConfig {
                    effort: Some(aether_core::model::Effort::Low),
                    ..Default::default()
                }),
                objective: Some("retained work".into()),
                start: true,
                ..Default::default()
            },
            parent
                .map(|id| Actor::Agent { id })
                .unwrap_or_else(Actor::user),
        )
        .unwrap()
        .0
}

/// What a turn leaves behind when its link fails: the turn's start and a
/// typed failed invocation, the provenance `prepare` requires.
fn record_turn_failure(
    store: &dyn Store,
    agent: &Agent,
    turn: u32,
    class: ModelErrorClass,
    error: &str,
) {
    let invocation = Invocation {
        id: InvocationId::new(),
        snapshot_id: SnapshotId::new(),
        agent_id: agent.id,
        turn,
        model: agent.model.clone(),
        started_at: now(),
        finished_at: Some(now()),
        content: Vec::new(),
        usage: Usage::default(),
        stop_reason: None,
        error: Some(error.into()),
        provider_message_id: None,
    };
    let draft = |kind| {
        EventDraft::new(Actor::System, kind)
            .agent(agent.id)
            .run(agent.run)
    };
    store
        .commit(
            Commit::new()
                .event(draft(EventKind::AgentStarted { turn }))
                .event(draft(EventKind::InvocationFailed {
                    invocation: invocation.id,
                    snapshot: invocation.snapshot_id,
                    error: error.into(),
                    class: Some(class),
                    retryable: false,
                    cancelled: false,
                    partial: Vec::new(),
                }))
                .mutate(Mutation::PutInvocation(invocation)),
        )
        .unwrap();
}

fn end(runtime: &Runtime, agent: Agent, error: &str) {
    end_by(runtime, agent, error, Actor::System);
}

fn end_by(runtime: &Runtime, agent: Agent, error: &str, by: Actor) {
    let hold = runtime.hold();
    let mut commit = Commit::new();
    let reap = runtime
        .terminate_agent(
            &hold,
            &mut commit,
            agent,
            AgentStatus::Failed {
                error: error.into(),
            },
            by,
        )
        .unwrap();
    runtime.apply_held(hold, commit).unwrap();
    runtime.reap_ended_agent(reap);
}

fn failures(store: &dyn Store, id: AgentId) -> Vec<String> {
    store
        .inbox(id, 0)
        .unwrap()
        .into_iter()
        .filter_map(|item| match item.kind {
            InboxKind::ChildFailed { error, .. } => Some(error),
            _ => None,
        })
        .collect()
}

fn diagnostic(error: &str) -> Value {
    serde_json::from_str(error.strip_prefix("exhaustion: ").expect(error)).unwrap()
}

#[tokio::test]
async fn exhaustion_traverses_terminal_ancestors_and_deduplicates_after_restart() {
    let dir = tempfile::tempdir().unwrap();
    let db = dir.path().join("state.db");
    let store: Arc<dyn Store> = Arc::new(SqliteStore::open(&db).unwrap());
    let rt = runtime(store.clone(), dir.path()).await;
    let root = create(&rt, None);
    let middle = create(&rt, Some(root.id));
    let steward = create(&rt, Some(middle.id));
    let worker = create(&rt, Some(steward.id));
    let descendant = create(&rt, Some(worker.id));
    for (mut agent, status) in [
        (
            root.clone(),
            AgentStatus::Waiting {
                condition: WaitCondition::Deadline {
                    at: now() + chrono::Duration::days(1),
                },
            },
        ),
        (middle.clone(), AgentStatus::Completed { summary: None }),
        (
            steward.clone(),
            AgentStatus::Failed {
                error: "earlier failure".into(),
            },
        ),
    ] {
        agent.status = status;
        store
            .commit(Commit::new().mutate(Mutation::UpsertAgent(agent)))
            .unwrap();
    }
    let original = "claude-code failed: You've hit your limit · resets Sep 18 at 2pm; sk-123456789012345678901234";
    record_turn_failure(
        store.as_ref(),
        &worker,
        1,
        ModelErrorClass::UsageLimit,
        original,
    );
    end(&rt, worker.clone(), original);
    let error = failures(store.as_ref(), root.id).pop().unwrap();
    let payload = diagnostic(&error);
    assert_eq!(payload["category"], "usage_limit");
    assert_eq!(payload["provider"], "anthropic");
    assert_eq!(payload["source_agent"], json!(worker.id));
    assert_eq!(payload["source_task"], json!(worker.current_task.unwrap()));
    assert!(error.contains("resets Sep 18 at 2pm"));
    assert!(!error.contains("sk-123456789012345678901234"));
    assert!(error.contains(&descendant.id.to_string()));
    assert!(error.contains("not verified"));
    assert!(
        matches!(store.task(worker.current_task.unwrap()).unwrap().unwrap().status, TaskStatus::Failed { error: task_error, .. } if task_error == error)
    );
    for agent in [&root, &middle, &steward] {
        assert_eq!(failures(store.as_ref(), agent.id), vec![error.clone()]);
    }
    assert!(
        !store
            .agent(descendant.id)
            .unwrap()
            .unwrap()
            .status
            .is_terminal()
    );
    crate::scheduler::tick(&rt).unwrap();
    assert!(matches!(
        store.agent(root.id).unwrap().unwrap().status,
        AgentStatus::Ready
    ));
    assert!(matches!(
        store.agent(middle.id).unwrap().unwrap().status,
        AgentStatus::Completed { .. }
    ));
    // A racing caller still owns a pre-failure copy.
    end(&rt, worker.clone(), original);
    assert_eq!(failures(store.as_ref(), root.id).len(), 1);
    rt.shutdown().await;
    drop(rt);
    drop(store);
    let store: Arc<dyn Store> = Arc::new(SqliteStore::open(&db).unwrap());
    let rt = runtime(store.clone(), dir.path()).await;
    end(&rt, worker, original);
    crate::scheduler::tick(&rt).unwrap();
    assert_eq!(failures(store.as_ref(), root.id).len(), 1);
    rt.shutdown().await;
}

#[tokio::test]
async fn token_budget_notifies_root_and_keeps_durable_subtree_cancellation() {
    let dir = tempfile::tempdir().unwrap();
    let store: Arc<dyn Store> = Arc::new(SqliteStore::open_in_memory().unwrap());
    let rt = runtime(store.clone(), dir.path()).await;
    let root = create(&rt, None);
    let steward = create(&rt, Some(root.id));
    let worker = create(&rt, Some(steward.id));
    crate::scheduler::stop_for_budget(
        &rt,
        steward.id,
        Usage::default(),
        "token budget of 5: 6 used",
        Actor::Scheduler,
    )
    .unwrap();
    assert!(matches!(
        store.agent(steward.id).unwrap().unwrap().status,
        AgentStatus::Failed { .. }
    ));
    assert!(matches!(
        store.agent(worker.id).unwrap().unwrap().status,
        AgentStatus::Cancelled { .. }
    ));
    let error = failures(store.as_ref(), root.id).pop().unwrap();
    assert!(!error.starts_with("exhaustion:"), "{error}");
    assert!(
        error.starts_with("Stopped by its budget (token budget of 5: 6 used);"),
        "{error}"
    );
    assert!(
        error.contains("its live subtree was cancelled with it"),
        "{error}"
    );
    let AgentStatus::Failed { error: task_error } =
        store.agent(steward.id).unwrap().unwrap().status
    else {
        panic!("the steward must have failed");
    };
    assert_eq!(
        task_error, error,
        "the task error and the inbox say the same"
    );
    let recorded = store
        .latest_events(
            20,
            &EventFilter {
                types: vec!["custom".into()],
                ..EventFilter::own_events(steward.id)
            },
        )
        .unwrap();
    let payload = recorded
        .iter()
        .find_map(|event| match &event.kind {
            EventKind::Custom { name, payload } if name == "exhaustion.failed" => Some(payload),
            _ => None,
        })
        .expect("the structured diagnostic stays in the event");
    assert_eq!(payload["category"], "token_budget");
    assert!(payload.to_string().contains(&worker.id.to_string()));
    assert!(
        payload["descendant_policy"]
            .as_str()
            .unwrap()
            .contains("durable subtree cancellation requested")
    );
    crate::scheduler::stop_for_budget(
        &rt,
        steward.id,
        Usage::default(),
        "token budget of 5: 6 used",
        Actor::Scheduler,
    )
    .unwrap();
    assert_eq!(failures(store.as_ref(), root.id).len(), 1);
    rt.shutdown().await;
}

#[tokio::test]
async fn ordinary_failures_keep_immediate_parent_behavior() {
    let dir = tempfile::tempdir().unwrap();
    let store: Arc<dyn Store> = Arc::new(SqliteStore::open_in_memory().unwrap());
    let rt = runtime(store.clone(), dir.path()).await;
    let root = create(&rt, None);
    let parent = create(&rt, Some(root.id));
    let child = create(&rt, Some(parent.id));
    end(&rt, child, "unknown engine error: quota");
    assert_eq!(
        failures(store.as_ref(), parent.id),
        vec!["unknown engine error: quota"]
    );
    assert!(failures(store.as_ref(), root.id).is_empty());
    rt.shutdown().await;
}

/// Classification needs provenance from the failing turn itself: agent
/// prose, untyped model text and an older turn's exhaustion are ordinary.
#[tokio::test]
async fn failures_without_this_turns_typed_exhaustion_stay_ordinary() {
    let dir = tempfile::tempdir().unwrap();
    let store: Arc<dyn Store> = Arc::new(SqliteStore::open_in_memory().unwrap());
    let rt = runtime(store.clone(), dir.path()).await;
    let root = create(&rt, None);
    let parent = create(&rt, Some(root.id));
    for error in [
        "Blocked: the vendor billing API replied usage limit exceeded for our account",
        "Cannot continue: the embeddings service says insufficient_quota",
        "The generated prompt is too long for the downstream summarizer",
        "model error: QuotaExhausted: insufficient_quota",
        "claude-code failed: You've hit your usage limit.",
    ] {
        let child = create(&rt, Some(parent.id));
        end(&rt, child, error);
        assert_eq!(failures(store.as_ref(), parent.id).pop().unwrap(), error);
        assert!(failures(store.as_ref(), root.id).is_empty(), "{error}");
    }
    // A budget stop's shape, but written by the agent itself.
    for error in [
        "token budget of 100: I cannot finish the migration within it",
        "cost budget of $5 is too small for this job",
    ] {
        let child = create(&rt, Some(parent.id));
        let by = Actor::Agent { id: child.id };
        end_by(&rt, child, error, by);
        assert_eq!(failures(store.as_ref(), parent.id).pop().unwrap(), error);
        assert!(failures(store.as_ref(), root.id).is_empty(), "{error}");
    }
    // Exhausted on turn 1, then woken and failed differently on turn 2.
    let child = create(&rt, Some(parent.id));
    record_turn_failure(
        store.as_ref(),
        &child,
        1,
        ModelErrorClass::QuotaExhausted,
        "QuotaExhausted: credit balance too low",
    );
    store
        .commit(
            Commit::new().event(
                EventDraft::new(Actor::System, EventKind::AgentStarted { turn: 2 })
                    .agent(child.id)
                    .run(child.run),
            ),
        )
        .unwrap();
    end(&rt, child, "model unavailable: no credentials configured");
    assert!(failures(store.as_ref(), root.id).is_empty());
    // A typed class that is not exhaustion keeps its ordinary delivery.
    let child = create(&rt, Some(parent.id));
    record_turn_failure(
        store.as_ref(),
        &child,
        1,
        ModelErrorClass::Auth,
        "Auth: insufficient_quota",
    );
    end(&rt, child, "model error: Auth: insufficient_quota");
    assert!(failures(store.as_ref(), root.id).is_empty());
    rt.shutdown().await;
}

#[tokio::test]
async fn exhaustion_wakes_task_and_deadline_waiters_on_intersecting_creator_paths() {
    let dir = tempfile::tempdir().unwrap();
    let store: Arc<dyn Store> = Arc::new(SqliteStore::open_in_memory().unwrap());
    let rt = runtime(store.clone(), dir.path()).await;
    let mut root = create(&rt, None);
    let mut parent = create(&rt, Some(root.id));
    let creator = create(&rt, Some(root.id));
    let child = create(&rt, Some(parent.id));
    let mut task = store.task(child.current_task.unwrap()).unwrap().unwrap();
    task.creator = Actor::Agent { id: creator.id };
    root.status = AgentStatus::Waiting {
        condition: WaitCondition::Task {
            id: creator.current_task.unwrap(),
        },
    };
    parent.status = AgentStatus::Waiting {
        condition: WaitCondition::Deadline {
            at: now() + chrono::Duration::days(1),
        },
    };
    store
        .commit(
            Commit::new()
                .mutate(Mutation::UpsertAgent(root.clone()))
                .mutate(Mutation::UpsertAgent(parent.clone()))
                .mutate(Mutation::UpsertTask(task)),
        )
        .unwrap();
    let error = "model error from anthropic/claude-sonnet-5-5: ContextLimit: prompt is too long";
    record_turn_failure(
        store.as_ref(),
        &child,
        1,
        ModelErrorClass::ContextLimit,
        "ContextLimit: prompt is too long",
    );
    end(&rt, child, error);
    for id in [root.id, parent.id, creator.id] {
        assert_eq!(failures(store.as_ref(), id).len(), 1);
    }
    crate::scheduler::tick(&rt).unwrap();
    for id in [root.id, parent.id] {
        assert!(matches!(
            store.agent(id).unwrap().unwrap().status,
            AgentStatus::Ready
        ));
    }
    rt.shutdown().await;
}

#[tokio::test]
async fn a_broken_ancestry_is_reported_without_blocking_the_failure() {
    let dir = tempfile::tempdir().unwrap();
    let store: Arc<dyn Store> = Arc::new(SqliteStore::open_in_memory().unwrap());
    let rt = runtime(store.clone(), dir.path()).await;
    let mut root = create(&rt, None);
    let child = create(&rt, Some(root.id));
    root.parent = Some(root.id);
    store
        .commit(Commit::new().mutate(Mutation::UpsertAgent(root.clone())))
        .unwrap();
    let error = "model error from anthropic/claude-sonnet-5-5: QuotaExhausted: insufficient_quota";
    record_turn_failure(
        store.as_ref(),
        &child,
        1,
        ModelErrorClass::QuotaExhausted,
        "QuotaExhausted: insufficient_quota",
    );
    end(&rt, child.clone(), error);
    let AgentStatus::Failed { error } = store.agent(child.id).unwrap().unwrap().status else {
        panic!("the failure must commit")
    };
    let payload = diagnostic(&error);
    assert_eq!(payload["notified_ancestors"], json!([root.id]));
    assert_eq!(
        payload["hierarchy_anomalies"],
        json!([format!("ancestry cycle at {}", root.id)])
    );
    assert_eq!(failures(store.as_ref(), root.id), vec![error]);
    rt.shutdown().await;
}

#[tokio::test]
async fn a_large_live_subtree_is_listed_up_to_a_cap_and_counted_in_full() {
    let dir = tempfile::tempdir().unwrap();
    let store: Arc<dyn Store> = Arc::new(SqliteStore::open_in_memory().unwrap());
    let rt = runtime(store.clone(), dir.path()).await;
    let root = create(&rt, None);
    let steward = create(&rt, Some(root.id));
    let total = MAX_LISTED_DESCENDANTS + 2;
    for _ in 0..total {
        create(&rt, Some(steward.id));
    }
    record_turn_failure(
        store.as_ref(),
        &steward,
        1,
        ModelErrorClass::UsageLimit,
        "UsageLimit: weekly limit reached",
    );
    end(
        &rt,
        steward,
        "model error from anthropic/claude-sonnet-5-5: UsageLimit",
    );
    let payload = diagnostic(&failures(store.as_ref(), root.id).pop().unwrap());
    assert_eq!(
        payload["live_descendants_at_failure"]
            .as_array()
            .unwrap()
            .len(),
        MAX_LISTED_DESCENDANTS
    );
    assert_eq!(payload["live_descendant_count"], json!(total));
    rt.shutdown().await;
}

mod probes;

#[tokio::test]
async fn provider_incidents_deduplicate_across_history_windows_until_that_provider_answers() {
    let dir = tempfile::tempdir().unwrap();
    let store = Arc::new(SqliteStore::open_in_memory().unwrap());
    let runtime = runtime(store.clone(), dir.path()).await;
    let root = create(&runtime, None);
    let child = create(&runtime, Some(root.id));
    let task_before = store
        .task(child.current_task.unwrap())
        .unwrap()
        .unwrap()
        .status;
    let model = ModelRef::new("other-provider", "compactor");
    let error = aether_core::model::ModelError::new(ModelErrorClass::QuotaExhausted, "quota spent");
    for _ in 0..2 {
        let hold = runtime.hold();
        let mut commit = Commit::new();
        record_incident(
            &runtime,
            &hold,
            &mut commit,
            &child,
            &model,
            &error,
            &Actor::System,
        )
        .unwrap();
        for _ in 0..EPISODE_LOOKBACK + 1 {
            commit.push_event(custom(&child, &Actor::System, "unrelated", json!({})));
        }
        runtime.apply_held(hold, commit).unwrap();
    }
    let notices = || {
        store
            .inbox(root.id, 0)
            .unwrap()
            .into_iter()
            .filter(|item| {
                matches!(&item.kind, InboxKind::ExternalTrigger { source, .. }
                    if source == "exhaustion")
            })
            .count()
    };
    assert_eq!(notices(), 1);
    let hold = runtime.hold();
    let mut commit = Commit::new();
    recover_incident(&runtime, &mut commit, &child, &model, &Actor::System).unwrap();
    runtime.apply_held(hold, commit).unwrap();
    let hold = runtime.hold();
    let mut commit = Commit::new();
    record_incident(
        &runtime,
        &hold,
        &mut commit,
        &child,
        &model,
        &error,
        &Actor::System,
    )
    .unwrap();
    runtime.apply_held(hold, commit).unwrap();
    assert_eq!(notices(), 2);
    assert_eq!(
        store
            .task(child.current_task.unwrap())
            .unwrap()
            .unwrap()
            .status,
        task_before
    );
    runtime.shutdown().await;
}
