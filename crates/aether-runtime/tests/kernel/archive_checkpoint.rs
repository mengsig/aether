//! archive checkpoint scenarios and fixtures.
use super::*;

/// Parent relationships survive restart without assigning workflow policy.
#[tokio::test]
async fn a_root_agent_reports_its_parent_relationship_across_restart() {
    let h = harness(vec![]).await;
    let (root, _) = h
        .runtime
        .create_agent(
            CreateAgentSpec {
                role: aether_core::agent::Role::COORDINATOR.into(),
                start: false,
                ..Default::default()
            },
            Actor::user(),
        )
        .expect("root");
    assert_eq!(root.parent, None);
    let prompt = aether_context::render::role_task(&root, None, None);
    assert!(prompt.contains("Parent: none."), "{prompt}");

    let (child, _) = h
        .runtime
        .create_agent(
            CreateAgentSpec {
                role: "worker".into(),
                parent: Some(root.id),
                config: Some(aether_core::agent::AgentConfig {
                    effort: Some(aether_core::model::Effort::Low),
                    ..Default::default()
                }),
                start: false,
                ..Default::default()
            },
            Actor::user(),
        )
        .expect("child");
    let prompt = aether_context::render::role_task(&child, None, None);
    assert!(
        prompt.contains(&format!("Parent: {}.", root.id)),
        "{prompt}"
    );
    assert!(!prompt.contains("root coordinator"), "{prompt}");

    h.runtime
        .cancel_agent(root.id, Some("done".into()), Actor::user())
        .expect("cancel");
    let restarted = h
        .runtime
        .restart_agent(root.id, Actor::user())
        .expect("restart");
    assert_eq!(restarted.parent, None, "a restarted root is still a root");
    assert_eq!(restarted.role, root.role);
}

/// `/clear` takes finished work out of the view without taking it
/// out of the store. Live work is never cleared, the record and its events
/// stay put, and `--all` / `/show archived` brings it back.
#[tokio::test]
async fn archiving_hides_finished_work_from_the_view_and_nothing_else() {
    let h = harness(vec![]).await;
    let make = |role: &str| {
        h.runtime
            .create_agent(
                CreateAgentSpec {
                    role: role.into(),
                    start: false,
                    ..Default::default()
                },
                Actor::user(),
            )
            .expect("create")
            .0
    };
    let live = make("live");
    let done = make("done");
    h.runtime
        .cancel_agent(done.id, Some("finished".into()), Actor::user())
        .expect("cancel");

    // A live agent is never cleared, whether named or swept.
    let err = h
        .runtime
        .archive_agents(ArchiveTarget::Agent(live.id), true, Actor::user())
        .expect_err("a live agent cannot be cleared");
    assert!(err.to_string().contains("only finished work"), "{err}");

    let cleared = h
        .runtime
        .archive_agents(ArchiveTarget::AllFinished, true, Actor::user())
        .expect("sweep");
    assert_eq!(
        cleared.iter().map(|a| a.id).collect::<Vec<_>>(),
        vec![done.id],
        "the sweep passes over live work"
    );

    // Out of the view...
    let view = |include_archived: bool| {
        h.store
            .agents(&aether_store::AgentFilter {
                exclude_archived: !include_archived,
                ..Default::default()
            })
            .expect("agents")
            .into_iter()
            .map(|a| a.id)
            .collect::<Vec<_>>()
    };
    assert_eq!(view(false), vec![live.id]);
    // ...still in the store, and still itself.
    assert!(view(true).contains(&done.id));
    let stored = h.store.agent(done.id).expect("agent").expect("exists");
    assert!(stored.archived);
    assert_eq!(
        stored.role, done.role,
        "clearing changes nothing but the mark"
    );

    // The event log - what replay and reports read - still carries the whole
    // agent, plus one event saying the view mark moved.
    let events = h
        .store
        .events(0, 10_000, &EventFilter::for_agent(done.id))
        .expect("events");
    assert!(
        events
            .iter()
            .any(|e| matches!(e.kind, EventKind::AgentArchived { archived: true })),
        "the mark is not an event"
    );
    assert!(
        events
            .iter()
            .any(|e| matches!(e.kind, EventKind::AgentCreated { .. })),
        "clearing must not touch the history"
    );

    // And it comes back.
    let restored = h
        .runtime
        .archive_agents(ArchiveTarget::Agent(done.id), false, Actor::user())
        .expect("undo");
    assert_eq!(restored.len(), 1);
    assert_eq!(view(false).len(), 2);
}

// ----- VISION §6.3: compaction at every trigger ----------------------------

/// Finishing records a boundary without spending on a checkpoint while idle.
/// The next real input folds the old episode before answering the new message.
#[tokio::test]
async fn closing_a_task_defers_its_checkpoint_until_new_work_arrives() {
    let checkpoint_json = serde_json::json!({
        "checkpoint": {
            "objective": "index the shard",
            "state": "indexed and verified",
            "decisions": ["batch of 500 rows"],
            "constraints": [],
            "open_loops": []
        },
        "memory_candidates": [{
            "scope": "repository", "kind": "fact",
            "text": "Indexing this repository runs in batches of five hundred rows",
            "importance": 0.8, "confidence": 0.9, "tags": ["indexing"]
        }]
    })
    .to_string();
    let h = harness(vec![
        ScriptedClient::text("started"),
        ScriptedClient::text("halfway"),
        ScriptedClient::text("nearly there"),
        ScriptedClient::tool_call("complete_task", json!({"summary": "indexed"})),
        // The deferred checkpoint, then the response to the next real input.
        ScriptedClient::text(&checkpoint_json),
        ScriptedClient::text("ready for another shard"),
    ])
    .await;
    let (agent, _) = h
        .runtime
        .create_agent(
            CreateAgentSpec {
                objective: Some("index the shard".into()),
                config: Some(AgentConfig {
                    long_lived: true,
                    approval: ApprovalPolicy::none(),
                    ..Default::default()
                }),
                ..spec("index the shard")
            },
            Actor::user(),
        )
        .expect("create");
    // Three exchanges make an episode worth folding, then the task closes.
    for text in ["how far?", "and now?", "finish it"] {
        h.wait_until(agent.id, |s| s.kind() == AgentStatusKind::Waiting)
            .await;
        let before = h.client.remaining();
        h.runtime
            .send_message(agent.id, vec![ContentBlock::text(text)], Actor::user())
            .expect("send");
        let deadline = tokio::time::Instant::now() + Duration::from_secs(10);
        while h.client.remaining() == before {
            assert!(tokio::time::Instant::now() < deadline, "turn did not run");
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    }
    h.wait_until(agent.id, |s| s.kind() == AgentStatusKind::Waiting)
        .await;
    let requests_before_idle = h.client.requests().len();
    tokio::time::sleep(Duration::from_millis(1100)).await;
    assert_eq!(h.client.requests().len(), requests_before_idle);
    assert_eq!(
        h.client.remaining(),
        2,
        "idle compaction consumed a response"
    );
    assert!(h.store.latest_checkpoint(agent.id).unwrap().is_none());
    let completed_task = h
        .store
        .agent(agent.id)
        .unwrap()
        .unwrap()
        .current_task
        .unwrap();
    h.runtime
        .send_message(
            agent.id,
            vec![ContentBlock::text("prepare a different shard")],
            Actor::user(),
        )
        .expect("new work");
    let deadline = tokio::time::Instant::now() + Duration::from_secs(10);
    while h.client.remaining() > 0 {
        assert!(
            tokio::time::Instant::now() < deadline,
            "task-boundary compaction did not run"
        );
        tokio::time::sleep(Duration::from_millis(20)).await;
    }

    let checkpoint = h
        .store
        .latest_checkpoint(agent.id)
        .expect("cp")
        .expect("a closed task leaves a checkpoint");
    assert_eq!(checkpoint.draft.state, "indexed and verified");
    let requests = h.client.requests();
    let fold_input = requests[requests.len() - 2].messages[0].text();
    assert!(fold_input.contains("index the shard"));
    assert!(!fold_input.contains("prepare a different shard"));
    assert_eq!(
        checkpoint.provenance.trigger,
        Some(CompactionTrigger::TaskBoundary {
            task: completed_task
        }),
        "the checkpoint records what asked for it"
    );
    let t = types(h.store.as_ref(), agent.id);
    assert!(t.contains(&"compaction.requested".to_string()), "{t:?}");
    assert!(t.contains(&"compaction.completed".to_string()), "{t:?}");
    assert!(
        t.contains(&"memory.accepted".to_string()),
        "a closed episode proposes what it learned: {t:?}"
    );
}

/// Engine agents can request the same validated checkpoint as API agents.
#[tokio::test]
async fn an_engine_agent_can_request_a_checkpoint() {
    let h = harness(vec![
        ScriptedClient::tool_call("request_checkpoint", json!({"reason": "long conversation"})),
        ScriptedClient::text("carrying on then"),
    ])
    .await;
    let (agent, _) = h
        .runtime
        .create_agent(
            CreateAgentSpec {
                role: "assistant".into(),
                start: true,
                model: Some(ModelRef::new("claude-code", "haiku")),
                config: Some(AgentConfig {
                    long_lived: true,
                    approval: ApprovalPolicy::none(),
                    ..Default::default()
                }),
                ..Default::default()
            },
            Actor::user(),
        )
        .expect("create");
    h.wait_until(agent.id, |s| s.kind() == AgentStatusKind::Waiting)
        .await;
    h.runtime
        .send_message(
            agent.id,
            vec![ContentBlock::text("this is getting long")],
            Actor::user(),
        )
        .expect("send");
    let deadline = tokio::time::Instant::now() + Duration::from_secs(10);
    while h.client.remaining() > 0 {
        assert!(tokio::time::Instant::now() < deadline, "turn did not run");
        tokio::time::sleep(Duration::from_millis(20)).await;
    }

    let t = types(h.store.as_ref(), agent.id);
    assert!(
        t.contains(&"compaction.requested".to_string()),
        "checkpoint request was not recorded: {t:?}"
    );
    let told = h
        .store
        .events(0, 10_000, &EventFilter::for_agent(agent.id))
        .expect("events")
        .into_iter()
        .find_map(|e| match e.kind {
            EventKind::ToolSucceeded { outcome, .. } => Some(outcome.content),
            EventKind::ToolFailed { error, .. } => Some(error),
            _ => None,
        })
        .expect("the tool answered");
    assert!(
        told.contains("Checkpoint requested"),
        "the agent was told something else: {told}"
    );
}

#[tokio::test]
async fn archive_sweep_preserves_a_wake_after_its_snapshot() {
    archive_after_concurrent_change(false).await;
}

#[tokio::test]
async fn archive_sweep_preserves_a_restart_after_its_snapshot() {
    archive_after_concurrent_change(true).await;
}

async fn archive_after_concurrent_change(restart: bool) {
    let inner: Arc<dyn Store> = Arc::new(SqliteStore::open_in_memory().expect("store"));
    let store = Arc::new(CommitsMidPass::new(inner, Hook::FleetRead));
    let h = harness_full(
        vec![],
        |_| {},
        |_| {},
        None,
        None,
        Some(store.clone()),
        |_| {},
    )
    .await;
    h.runtime.shutdown().await;
    let (mut agent, _) = h
        .runtime
        .create_agent(
            CreateAgentSpec {
                start: false,
                ..Default::default()
            },
            Actor::user(),
        )
        .expect("agent");
    agent.status = AgentStatus::Failed {
        error: "retryable failure".into(),
    };
    let id = agent.id;
    h.runtime
        .apply(Commit::new().mutate(Mutation::UpsertAgent(agent)))
        .expect("failed agent");

    let runtime = Arc::downgrade(&h.runtime);
    store.arm(Box::new(move |_| {
        let runtime = runtime.upgrade().expect("runtime");
        if restart {
            runtime
                .restart_agent(id, Actor::user())
                .expect("concurrent restart");
        } else {
            runtime
                .wake_agent(id, Actor::user())
                .expect("concurrent wake");
        }
        Ok(())
    }));
    let changed = h
        .runtime
        .archive_agents(ArchiveTarget::AllFinished, true, Actor::user())
        .expect("archive");
    assert!(store.fired());
    let row = h.store.agent(id).expect("read agent").expect("agent");
    if restart {
        let successor = row.restarted_as.expect("archive preserved successor link");
        assert_eq!(
            h.store.agent(successor).unwrap().unwrap().restarted_from,
            Some(id)
        );
        assert!(row.archived);
        assert_eq!(changed.len(), 1);
        assert_eq!(changed[0].restarted_as, Some(successor));
        assert!(h.runtime.restart_agent(id, Actor::user()).is_err());
    } else {
        assert_eq!(row.status.kind(), AgentStatusKind::Ready);
        assert!(!row.archived);
        assert!(changed.is_empty());
        assert!(!types(h.store.as_ref(), id).contains(&"agent.archived".to_owned()));
    }
}

#[tokio::test]
async fn archive_sweep_skips_history_expired_after_its_snapshot() {
    let inner: Arc<dyn Store> = Arc::new(SqliteStore::open_in_memory().expect("store"));
    let store = Arc::new(CommitsMidPass::new(inner, Hook::FleetRead));
    let h = harness_full(
        vec![],
        |_| {},
        |_| {},
        None,
        None,
        Some(store.clone()),
        |_| {},
    )
    .await;
    h.runtime.shutdown().await;
    let create_finished = || {
        let (mut agent, _) = h
            .runtime
            .create_agent(
                CreateAgentSpec {
                    start: false,
                    ..Default::default()
                },
                Actor::user(),
            )
            .expect("agent");
        agent.status = AgentStatus::Completed { summary: None };
        h.runtime
            .apply(Commit::new().mutate(Mutation::UpsertAgent(agent.clone())))
            .expect("finished agent");
        agent
    };
    let expired = create_finished();
    let retained = create_finished();
    let run = expired.run;
    let runtime = Arc::downgrade(&h.runtime);
    store.arm(Box::new(move |_| {
        let result = runtime
            .upgrade()
            .expect("runtime")
            .expire_run_history(run, now() + chrono::Duration::days(1))
            .expect("concurrent history expiry");
        assert!(
            matches!(result, aether_store::RunExpiry::Expired { .. }),
            "{result:?}"
        );
        Ok(())
    }));

    let changed = h
        .runtime
        .archive_agents(ArchiveTarget::AllFinished, true, Actor::user())
        .expect("archive surviving rows");
    assert!(store.fired());
    assert_eq!(
        changed.iter().map(|agent| agent.id).collect::<Vec<_>>(),
        vec![retained.id]
    );
    assert!(h.store.agent(expired.id).unwrap().is_none());
    assert!(h.store.agent(retained.id).unwrap().unwrap().archived);
    let error = h
        .runtime
        .archive_agents(ArchiveTarget::Agent(expired.id), true, Actor::user())
        .expect_err("a named missing agent still fails");
    assert!(matches!(error, aether_runtime::RuntimeError::NotFound(_)));
}
