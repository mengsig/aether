use super::*;
use aether_store::{SqliteStore, Store};

async fn start(store: Arc<SqliteStore>, config: &std::path::Path) -> Arc<Runtime> {
    Runtime::start_with_engines(
        crate::RuntimeConfig {
            max_concurrent_turns: 0,
            default_model: Some(aether_core::model::ModelRef::new(
                "anthropic",
                "claude-sonnet-5-5",
            )),
            ..Default::default()
        },
        store,
        Arc::new(aether_model::Models::load(Some(config.into())).expect("models")),
        Arc::new(aether_tools::ToolRegistry::with_builtins()),
        None,
        Default::default(),
    )
    .await
    .expect("start despite the corrupt turn")
}

fn interrupted(runtime: &Runtime) -> Agent {
    let (mut agent, _) = runtime
        .create_agent(
            crate::CreateAgentSpec {
                role: "worker".into(),
                start: false,
                ..Default::default()
            },
            Actor::user(),
        )
        .expect("agent");
    agent.status = AgentStatus::Running;
    runtime
        .apply(
            Commit::new()
                .mutate(Mutation::UpsertAgent(agent.clone()))
                .mutate(Mutation::SetLease(Lease::new(
                    agent.id,
                    "old-daemon",
                    1,
                    now(),
                    std::time::Duration::ZERO,
                )))
                .event(
                    EventDraft::new(Actor::System, EventKind::AgentStarted { turn: 1 })
                        .agent(agent.id)
                        .run(agent.run),
                ),
        )
        .expect("interrupted agent");
    agent
}

#[tokio::test]
async fn unreadable_turn_is_inspected_without_blocking_other_agents() {
    for payload in [Some("{"), None] {
        let dir = tempfile::tempdir().expect("private state");
        let path = dir.path().join("state.db");
        let store = Arc::new(SqliteStore::open(&path).expect("store"));
        let runtime = start(store.clone(), dir.path()).await;
        runtime.shutdown().await;
        let bad = interrupted(&runtime);
        let good = interrupted(&runtime);
        let key = acquire(&runtime, bad.id);
        request(&runtime, &bad, "Read", SideEffectClass::ReadOnly);
        corrupt_request(&path, bad.id, payload);
        drop(runtime);
        let restarted = start(store.clone(), dir.path()).await;
        let bad = restarted.store.agent(bad.id).unwrap().unwrap();
        let AgentStatus::Blocked {
            reason: BlockReason::Inspection { detail },
        } = bad.status
        else {
            panic!("corrupt history must require inspection");
        };
        assert!(detail.contains("aether doctor"));
        assert_eq!(
            restarted.store.agent(good.id).unwrap().unwrap().status,
            AgentStatus::Ready
        );
        assert_quarantined(&restarted, bad.id, &key);
        restarted.shutdown().await;
        drop(restarted);
        let again = start(store, dir.path()).await;
        assert_quarantined(&again, bad.id, &key);
        again.shutdown().await;
    }
}

fn assert_quarantined(runtime: &Runtime, agent: AgentId, key: &ResourceKey) {
    assert!(
        runtime
            .resources
            .holdings()
            .iter()
            .any(|h| h.agent == agent && h.keys.contains(key))
    );
    assert!(
        runtime
            .store
            .leases()
            .unwrap()
            .iter()
            .all(|l| l.agent_id != agent)
    );
}

fn corrupt_request(path: &std::path::Path, agent: AgentId, payload: Option<&str>) {
    let conn = rusqlite::Connection::open(path).expect("raw store");
    let sql = match payload {
        Some(_) => "UPDATE events SET body = ?1 WHERE agent_id = ?2 AND type = 'tool.requested'",
        None => {
            "UPDATE events SET body = json_set(body, '$.kind.type', ?1) WHERE agent_id = ?2 AND type = 'tool.requested'"
        }
    };
    assert_eq!(
        conn.execute(
            sql,
            rusqlite::params![payload.unwrap_or("future.unknown"), agent.to_string()]
        )
        .expect("corrupt payload"),
        1
    );
    let body: String = conn
        .query_row(
            "SELECT body FROM events WHERE agent_id = ?1 AND type = 'tool.requested'",
            [agent.to_string()],
            |r| r.get(0),
        )
        .expect("body");
    let error =
        serde_json::from_str::<aether_core::event::Event>(&body).expect_err("invalid event");
    assert!(payload.is_some() || error.to_string().contains("unknown variant"));
}

#[tokio::test]
async fn unreadable_resource_history_does_not_clear_the_lease() {
    let dir = tempfile::tempdir().expect("private state");
    let path = dir.path().join("state.db");
    let store = Arc::new(SqliteStore::open(&path).expect("store"));
    let runtime = start(store, dir.path()).await;
    runtime.shutdown().await;
    let bad = interrupted(&runtime);
    acquire(&runtime, bad.id);
    let conn = rusqlite::Connection::open(&path).expect("raw store");
    assert_eq!(
        conn.execute(
            "UPDATE events SET body = '{' WHERE agent_id = ?1 AND type = 'lease.resource_acquired'",
            [bad.id.to_string()],
        )
        .expect("corrupt resource event"),
        1
    );
    recover(&runtime).expect("isolate the corrupt resource history");
    assert!(matches!(
        runtime.store.agent(bad.id).unwrap().unwrap().status,
        AgentStatus::Blocked {
            reason: BlockReason::Inspection { .. }
        }
    ));
    assert!(
        runtime
            .store
            .leases()
            .unwrap()
            .iter()
            .any(|l| l.agent_id == bad.id)
    );
}

fn missing_checkpoint_artifact(agent: AgentId) -> aether_core::checkpoint::Checkpoint {
    use aether_core::checkpoint::{
        ArtifactNote, Checkpoint, CheckpointDraft, CheckpointProvenance,
    };
    Checkpoint {
        id: aether_core::id::CheckpointId::new(),
        agent_id: agent,
        version: 1,
        draft: CheckpointDraft {
            objective: "keep working".into(),
            state: "a result was saved".into(),
            artifacts: vec![ArtifactNote {
                reference: Some(aether_core::id::ArtifactId::new().to_string()),
                note: "required result".into(),
            }],
            ..Default::default()
        },
        provenance: CheckpointProvenance {
            from_seq: 0,
            to_seq: 1,
            source_snapshot: None,
            compactor_model: None,
            previous: None,
            trigger: None,
        },
        created_at: now(),
    }
}

#[tokio::test]
async fn missing_workspace_or_checkpoint_artifact_only_blocks_its_agent() {
    for checkpoint in [false, true] {
        let dir = tempfile::tempdir().expect("private state");
        let store = Arc::new(SqliteStore::open(dir.path().join("state.db")).expect("store"));
        let runtime = start(store.clone(), dir.path()).await;
        runtime.shutdown().await;
        let mut bad = interrupted(&runtime);
        let good = interrupted(&runtime);
        let mutation = if checkpoint {
            Mutation::PutCheckpoint(missing_checkpoint_artifact(bad.id))
        } else {
            bad.scope.workspace_root = Some(dir.path().join("removed-workspace"));
            Mutation::UpsertAgent(bad.clone())
        };
        runtime
            .apply(Commit::new().mutate(mutation))
            .expect("bad reference");
        drop(runtime);
        let restarted = start(store, dir.path()).await;
        let bad = restarted.store.agent(bad.id).unwrap().unwrap();
        assert!(matches!(
            bad.status,
            AgentStatus::Blocked {
                reason: BlockReason::Inspection { .. }
            }
        ));
        assert_eq!(
            restarted.store.agent(good.id).unwrap().unwrap().status,
            AgentStatus::Ready
        );
        assert!(
            !restarted
                .recovery_failed
                .load(std::sync::atomic::Ordering::Relaxed)
        );
        restarted.shutdown().await;
    }
}

#[tokio::test]
async fn unreadable_global_cancellation_journal_keeps_diagnostics_available() {
    let dir = tempfile::tempdir().expect("private state");
    let store = Arc::new(SqliteStore::open(dir.path().join("state.db")).expect("store"));
    let runtime = start(store.clone(), dir.path()).await;
    runtime.shutdown().await;
    runtime
        .apply(Commit::new().mutate(Mutation::SetMeta {
            key: "runtime.pending_cancellations".into(),
            value: "{".into(),
        }))
        .expect("invalid journal");
    drop(runtime);
    let restarted = start(store, dir.path()).await;
    assert!(
        restarted
            .recovery_failed
            .load(std::sync::atomic::Ordering::Relaxed)
    );
    assert!(
        restarted
            .problems()
            .unwrap()
            .iter()
            .any(|p| p.contains("cancellation") && p.contains("aether doctor"))
    );
    crate::scheduler::tick(&restarted).expect("diagnostic mode does not dispatch");
    assert_eq!(
        restarted
            .store
            .meta("runtime.pending_cancellations")
            .unwrap()
            .as_deref(),
        Some("{")
    );
    restarted.shutdown().await;
}

#[tokio::test]
async fn cancellation_keeps_a_corrupt_descendant_in_its_durable_fence() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("state.db");
    let store = Arc::new(SqliteStore::open(&path).unwrap());
    let runtime = start(store.clone(), dir.path()).await;
    runtime.shutdown().await;
    let root = interrupted(&runtime);
    let mut child = interrupted(&runtime);
    child.parent = Some(root.id);
    runtime
        .apply(Commit::new().mutate(Mutation::UpsertAgent(child.clone())))
        .unwrap();
    crate::cancellation::request(
        &runtime,
        root.id,
        Some("stop subtree".into()),
        Actor::user(),
    )
    .unwrap();
    let conn = rusqlite::Connection::open(&path).unwrap();
    conn.execute(
        "UPDATE agents SET body='{' WHERE id=?1",
        [child.id.to_string()],
    )
    .unwrap();
    assert!(crate::cancellation::drain(&runtime).is_err());
    let pending = crate::cancellation::pending_agents(&runtime).unwrap();
    assert!(pending.contains(&root.id) && pending.contains(&child.id));
    assert!(
        store
            .meta("runtime.pending_cancellations")
            .unwrap()
            .unwrap()
            .contains(&root.id.to_string())
    );
}

#[tokio::test]
async fn unreadable_budget_descendant_withholds_only_its_subtree() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("state.db");
    let store = Arc::new(SqliteStore::open(&path).unwrap());
    let runtime = start(store.clone(), dir.path()).await;
    runtime.shutdown().await;
    let mut root = interrupted(&runtime);
    root.config.token_budget = Some(100);
    let mut child = interrupted(&runtime);
    child.parent = Some(root.id);
    let mut good = interrupted(&runtime);
    good.status = AgentStatus::Ready;
    runtime
        .apply(
            Commit::new()
                .mutate(Mutation::UpsertAgent(root.clone()))
                .mutate(Mutation::UpsertAgent(child.clone()))
                .mutate(Mutation::UpsertAgent(good.clone())),
        )
        .unwrap();
    let conn = rusqlite::Connection::open(&path).unwrap();
    conn.execute(
        "UPDATE agents SET body='{' WHERE id=?1",
        [child.id.to_string()],
    )
    .unwrap();
    crate::scheduler::tick(&runtime).expect_err("the affected budget remains visible");
    assert!(
        runtime
            .problems()
            .unwrap()
            .iter()
            .any(|p| p.contains("Budget for agent") && p.contains(&root.id.to_string()))
    );
    assert!(matches!(
        store.agent(good.id).unwrap().unwrap().status,
        AgentStatus::Waiting { .. }
    ));
    assert_eq!(
        store.agent(root.id).unwrap().unwrap().status,
        AgentStatus::Running
    );
    crate::scheduler::tick(&runtime).expect_err("the affected budget remains visible");
    assert!(
        runtime
            .problems()
            .unwrap()
            .iter()
            .any(|p| p.contains("Budget for agent"))
    );
}
