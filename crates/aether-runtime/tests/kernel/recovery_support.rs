//! recovery support scenarios and fixtures.
use super::*;

/// A seeded agent left mid-turn by a dead daemon, for the recovery paths.
pub(super) fn seed_interrupted(
    store: &dyn Store,
    id: AgentId,
    status: AgentStatus,
    open_tool: Option<&str>,
) {
    let at = now();
    let mut agent = aether_core::agent::Agent {
        archived: false,
        id,
        parent: None,
        role: "worker".into(),
        name: "w".into(),
        scope: Scope::default(),
        model: ModelRef::new("anthropic", "claude-sonnet-5-5"),
        fallbacks: Vec::new(),
        status: AgentStatus::Created,
        paused: false,
        paused_via: None,
        current_task: None,
        inbox_cursor: 0,
        checkpoint: None,
        engine_session: None,
        engine_usage: None,
        engine_sent: None,
        status_since: None,
        resume_wait: None,
        restarted_from: None,
        restarted_as: None,
        engine_alive: false,
        last_event_at: None,
        config: AgentConfig::default(),
        stats: Default::default(),
        run: Default::default(),
        created_at: at,
        updated_at: at,
    };
    agent.transition(AgentStatus::Ready, at).expect("ready");
    agent.transition(AgentStatus::Running, at).expect("running");
    if status != AgentStatus::Running {
        agent.transition(status, at).expect("seeded status");
    }
    let mut commit = Commit::new()
        .event(EventDraft::new(Actor::System, EventKind::AgentStarted { turn: 1 }).agent(id));
    if let Some(tool) = open_tool {
        let call = ToolCall {
            id: Default::default(),
            provider_call_id: "p".into(),
            name: tool.into(),
            input: json!({}),
            agent_id: id,
            task_id: None,
            invocation_id: None,
            resources: vec![],
            idempotency_key: None,
            requested_at: at,
        };
        commit.push_event(
            EventDraft::new(
                Actor::System,
                EventKind::ToolRequested {
                    call: call.clone(),
                    side_effect: aether_engine::side_effect_of(tool),
                },
            )
            .agent(id),
        );
        commit.push_event(
            EventDraft::new(Actor::System, EventKind::ToolStarted { call: call.id }).agent(id),
        );
    }
    store
        .commit(
            commit
                .mutate(Mutation::UpsertAgent(agent))
                .mutate(Mutation::SetLease(Lease::new(
                    id,
                    "dead-daemon",
                    1,
                    at,
                    Duration::from_secs(1),
                ))),
        )
        .expect("seed");
}

// ---------------------------------------------------------------------------
// VISION §3.9 / §17 line 7: recovery drills. The daemon dies at each of the
// four moments that can lose something - mid-turn, mid-tool, mid-approval,
// mid-steer - and a second daemon over the same store has to come back with
// nothing lost and the interrupted turn classified honestly.
// ---------------------------------------------------------------------------

/// A second daemon over the same store, as a restart is. The old harness stays
/// alive so its workspace directory outlives it, the way a real one does.
pub(super) async fn restart_over(
    h: &Harness,
    responses: Vec<aether_model::testing::ScriptedResponse>,
) -> Harness {
    h.runtime.shutdown().await;
    harness_full(
        responses,
        |_| {},
        |_| {},
        None,
        None,
        Some(h.store.clone()),
        |_| {},
    )
    .await
}

pub(super) fn recovery_class(
    store: &dyn Store,
    id: AgentId,
) -> Option<aether_core::event::RecoveryClass> {
    store
        .events(0, 10_000, &EventFilter::for_agent(id))
        .expect("events")
        .into_iter()
        .find_map(|e| match e.kind {
            EventKind::RecoveryClassified { class, .. } => Some(class),
            _ => None,
        })
}

/// Seed the records a kill leaves behind: an agent Running, an invocation
/// sent and never settled, optionally a tool call started and never settled
/// with the keys it took never released, and the lease of a daemon that is
/// gone. `tool` decides how recovery must classify it.
pub(super) fn seed_crashed_turn(
    store: &dyn Store,
    id: AgentId,
    tool: Option<&str>,
    keys: Vec<aether_core::tool::ResourceKey>,
    snapshot: bool,
) {
    let at = now();
    let mut agent = aether_core::agent::Agent {
        archived: false,
        id,
        parent: None,
        role: "worker".into(),
        name: "w".into(),
        scope: Scope::default(),
        model: ModelRef::new("anthropic", "claude-sonnet-5-5"),
        fallbacks: Vec::new(),
        status: AgentStatus::Created,
        current_task: None,
        inbox_cursor: 0,
        checkpoint: None,
        engine_session: None,
        engine_usage: None,
        engine_sent: None,
        status_since: None,
        paused: false,
        paused_via: None,
        resume_wait: None,
        restarted_from: None,
        restarted_as: None,
        engine_alive: false,
        last_event_at: None,
        config: AgentConfig::default(),
        stats: Default::default(),
        run: Default::default(),
        created_at: at,
        updated_at: at,
    };
    agent.transition(AgentStatus::Ready, at).expect("ready");
    agent.transition(AgentStatus::Running, at).expect("running");
    let draft = |kind: EventKind| EventDraft::new(Actor::System, kind).agent(id);
    let mut commit = Commit::new().event(draft(EventKind::AgentStarted { turn: 1 }));
    if snapshot {
        commit.push_event(draft(EventKind::SnapshotCreated {
            snapshot: aether_core::id::SnapshotId::new(),
            turn: 1,
            model: ModelRef::new("anthropic", "claude-sonnet-5-5"),
            token_estimate: 100,
            layers: Vec::new(),
        }));
    }
    if let Some(tool) = tool {
        let call = ToolCall {
            id: Default::default(),
            provider_call_id: "p".into(),
            name: tool.into(),
            input: json!({}),
            agent_id: id,
            task_id: None,
            invocation_id: None,
            resources: keys.clone(),
            idempotency_key: None,
            requested_at: at,
        };
        commit.push_event(draft(EventKind::ToolRequested {
            call: call.clone(),
            side_effect: aether_engine::side_effect_of(tool),
        }));
        if !keys.is_empty() {
            commit.push_event(draft(EventKind::ResourceAcquired { keys }));
        }
        commit.push_event(draft(EventKind::ToolStarted { call: call.id }));
    }
    commit.push_mutation(Mutation::UpsertAgent(agent));
    commit.push_mutation(Mutation::SetLease(Lease::new(
        id,
        "dead-daemon",
        1,
        at,
        Duration::from_secs(1),
    )));
    store.commit(commit).expect("seed");
}

/// A concurrent writer's thread, taken and joined by `join_racer`.
pub(super) type Racer = Arc<std::sync::Mutex<Option<std::thread::JoinHandle<()>>>>;

/// How long a hooked read waits for its racer. A caller holding the commit
/// gate keeps the racer out for the whole window; one that does not lets it land.
pub(super) const RACE_WINDOW: Duration = Duration::from_millis(500);

/// Arms `store` so its hooked read hands `write` to another thread, which goes
/// through `runtime` like a second request would, and waits for it to land.
pub(super) fn race_through_runtime(
    store: &CommitsMidPass,
    runtime: &Arc<Runtime>,
    write: impl FnOnce(&Runtime) + Send + 'static,
) -> Racer {
    let racer = Racer::default();
    let slot = racer.clone();
    // Weak: an armed write that never fires must not keep the runtime alive.
    let runtime = Arc::downgrade(runtime);
    store.arm(Box::new(move |_| {
        let (landed, waited) = std::sync::mpsc::channel();
        let handle = std::thread::spawn(move || {
            let runtime = runtime.upgrade().expect("the runtime outlives its race");
            write(&runtime);
            // The reader may have stopped waiting; `join_racer` is the check.
            let _ = landed.send(());
        });
        *slot.lock().expect("racer slot") = Some(handle);
        // Landing and timing out are both expected: which one is the result.
        let _ = waited.recv_timeout(RACE_WINDOW);
        Ok(())
    }));
    racer
}

/// Waits for the racing write to finish; a panic inside it fails the test.
pub(super) fn join_racer(racer: &Racer) {
    let handle = racer
        .lock()
        .expect("racer slot")
        .take()
        .expect("the hooked read never fired");
    handle.join().expect("the racing write panicked");
}

/// A second daemon over the same store, with the first one left exactly where
/// it stood. It is deliberately not shut down: `shutdown` cancels the turn,
/// records the cancellation and ends the agent cleanly, which is the opposite
/// of a kill. Parked on a call that never returns, the first daemon writes
/// nothing more - so what the store holds is what a real turn actually wrote.
pub(super) async fn second_daemon_over(
    h: &Harness,
    responses: Vec<aether_model::testing::ScriptedResponse>,
) -> Harness {
    harness_full(
        responses,
        |_| {},
        |_| {},
        None,
        None,
        Some(h.store.clone()),
        |_| {},
    )
    .await
}

/// The agent's own events, once `pred` holds over them.
pub(super) async fn wait_until_events(
    store: &dyn Store,
    id: AgentId,
    pred: impl Fn(&[aether_core::event::Event]) -> bool,
) -> Vec<aether_core::event::Event> {
    let deadline = tokio::time::Instant::now() + Duration::from_secs(15);
    loop {
        let events: Vec<_> = store
            .events(0, 10_000, &EventFilter::for_agent(id))
            .expect("events")
            .into_iter()
            .filter(|e| e.agent_id == Some(id))
            .collect();
        if pred(&events) {
            return events;
        }
        assert!(
            tokio::time::Instant::now() < deadline,
            "timed out; saw {:?}",
            events.iter().map(|e| e.type_name()).collect::<Vec<_>>()
        );
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
}

pub(super) async fn wait_for_event(store: &dyn Store, id: AgentId, kind: &str) {
    wait_until_events(store, id, |events| {
        events.iter().any(|e| e.type_name() == kind)
    })
    .await;
}
