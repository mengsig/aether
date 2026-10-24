//! held waits scenarios and fixtures.
use super::*;

/// VISION §3.6: an approval wait is never bounded or interrupted, so the
/// dependency gate must not overwrite it. The owner keeps the wait it chose
/// and hears about the gate through its inbox.
#[tokio::test]
async fn the_gate_keeps_a_wait_the_owner_chose() {
    let waiting_on_a_person = AgentId::new();
    let approval = aether_core::id::ApprovalId::new();
    let condition = aether_core::agent::WaitCondition::Approval { id: approval };
    let parked = condition.clone();
    let h = harness_with(vec![slow_turn("dependency done")], move |store| {
        let at = now();
        let mut agent = aether_core::agent::Agent {
            archived: false,
            id: waiting_on_a_person,
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
        agent
            .transition(
                AgentStatus::Waiting {
                    condition: parked.clone(),
                },
                at,
            )
            .expect("waiting on a person");
        store
            .commit(
                Commit::new()
                    .mutate(Mutation::PutApproval(aether_core::tool::ApprovalRequest {
                        id: approval,
                        agent_id: waiting_on_a_person,
                        task_id: None,
                        call_id: Default::default(),
                        tool: "write_file".into(),
                        description: "overwrite a file".into(),
                        argument: None,
                        resources: Vec::new(),
                        side_effect: SideEffectClass::Mutation,
                        requested_at: at,
                        status: aether_core::tool::ApprovalStatus::Pending,
                    }))
                    .mutate(Mutation::UpsertAgent(agent)),
            )
            .expect("seed");
    })
    .await;

    let (_, dependency) = h
        .runtime
        .create_agent(spec("the thing it waits for"), Actor::user())
        .expect("owner");
    let dependency = dependency.expect("task");
    h.runtime
        .create_task(
            aether_runtime::CreateTaskSpec {
                objective: "run after it".into(),
                owner: Some(waiting_on_a_person),
                dependencies: vec![dependency.id],
                ..Default::default()
            },
            Actor::user(),
        )
        .expect("gated task");
    // Several gate passes run while the dependency streams.
    let deadline = tokio::time::Instant::now() + Duration::from_secs(15);
    while !h
        .store
        .task(dependency.id)
        .expect("task")
        .expect("exists")
        .status
        .is_terminal()
    {
        assert_eq!(
            h.store
                .agent(waiting_on_a_person)
                .expect("a")
                .expect("exists")
                .status,
            AgentStatus::Waiting {
                condition: condition.clone()
            },
            "the gate discarded a wait on a person"
        );
        assert!(tokio::time::Instant::now() < deadline, "dependency stalled");
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    tokio::time::sleep(Duration::from_millis(200)).await;
    let after = h
        .store
        .agent(waiting_on_a_person)
        .expect("a")
        .expect("exists");
    assert_eq!(
        after.status,
        AgentStatus::Waiting { condition },
        "an approval wait outlives the gate clearing"
    );
    let woken = h
        .store
        .inbox(waiting_on_a_person, 0)
        .expect("inbox")
        .into_iter()
        .any(|i| matches!(i.kind, aether_core::inbox::InboxKind::Wake { .. }));
    assert!(
        woken,
        "the owner is told the gate cleared, not forced Ready"
    );
}

/// VISION §3.8 with §1.3: pausing an agent mid-turn does not stop that turn,
/// so the projection must keep saying Running and the caller must be told the
/// pause lands after it. The pause then holds: no further turn is dispatched.
#[tokio::test]
async fn pausing_a_running_turn_says_when_it_takes_effect() {
    let h = harness(vec![
        ScriptedClient::tool_call("bash", json!({"command": "sleep 0.4"})),
        ScriptedClient::text("that is done"),
    ])
    .await;
    let (agent, _) = h
        .runtime
        .create_agent(spec("keep working"), Actor::user())
        .expect("agent");
    h.wait_until(agent.id, |s| matches!(s, AgentStatus::Running))
        .await;
    let outcome = h
        .runtime
        .pause_agent(agent.id, false, Actor::user())
        .expect("pause");
    assert_eq!(
        outcome.mid_turn,
        vec![agent.id],
        "a pause on a running turn must report that the turn finishes first"
    );
    let mid = h.store.agent(agent.id).expect("a").expect("exists");
    assert_eq!(
        mid.status,
        AgentStatus::Running,
        "the turn is still running and the status must say so"
    );
    assert!(
        mid.paused,
        "the flag is durable and separate from the status"
    );

    // The turn it was in the middle of runs to the end...
    h.wait_until(agent.id, |s| !matches!(s, AgentStatus::Running))
        .await;
    // ...and nothing after it: the second response is never asked for.
    tokio::time::sleep(Duration::from_millis(400)).await;
    let after = h.store.agent(agent.id).expect("a").expect("exists");
    assert_eq!(after.stats.turns, 1, "{:?}", after.status);
    assert!(after.paused);
    assert!(!after.status.is_terminal(), "{:?}", after.status);
    assert_eq!(
        h.client.remaining(),
        1,
        "the paused agent was dispatched again"
    );
}

/// VISION §3.6: limit 1 must not cost the agent limit 4. The chain that
/// exposed it: a failed child wakes an agent early from the wait it chose,
/// the global limit holds it Ready, the gate blocks it, and a person then
/// messages it. Answering the person must not discard that wait.
#[tokio::test]
async fn answering_a_person_keeps_the_wait_a_blocked_agent_chose() {
    let worker = AgentId::new();
    let failed_child = aether_core::id::TaskId::new();
    let still_running = aether_core::id::TaskId::new();
    let condition = aether_core::agent::WaitCondition::all_tasks([failed_child, still_running]);
    let parked = condition.clone();
    let h = harness_configured(
        vec![
            // Long enough to hold the only turn slot while the gate blocks
            // the worker.
            turn_of_length(400, "slot held"),
            ScriptedClient::text("I hear you."),
        ],
        move |store| {
            let at = now();
            let mut agent = aether_core::agent::Agent {
                archived: false,
                id: worker,
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
                config: AgentConfig {
                    approval: ApprovalPolicy::none(),
                    ..Default::default()
                },
                stats: Default::default(),
                run: Default::default(),
                created_at: at,
                updated_at: at,
            };
            agent.transition(AgentStatus::Ready, at).expect("ready");
            agent.transition(AgentStatus::Running, at).expect("running");
            agent
                .transition(
                    AgentStatus::Waiting {
                        condition: parked.clone(),
                    },
                    at,
                )
                .expect("waiting for its children");
            let seeded = |id, objective: &str| {
                let mut t =
                    aether_core::task::Task::new(objective, Actor::user(), Default::default(), at);
                t.id = id;
                t
            };
            let mut failed = seeded(failed_child, "the child that failed");
            failed.status = aether_core::task::TaskStatus::Failed {
                error: "boom".into(),
                structured: None,
            };
            let running = seeded(still_running, "the child still going");
            let mut gated = seeded(aether_core::id::TaskId::new(), "runs after the child");
            gated.owner = Some(worker);
            gated.dependencies = vec![still_running];
            store
                .commit(
                    Commit::new()
                        .mutate(Mutation::UpsertTask(failed))
                        .mutate(Mutation::UpsertTask(running))
                        .mutate(Mutation::UpsertTask(gated))
                        .mutate(Mutation::UpsertAgent(agent)),
                )
                .expect("seed");
        },
        |c| c.max_concurrent_turns = 1,
    )
    .await;
    h.client.set_delay(Duration::from_millis(20));
    let (hog, _) = h
        .runtime
        .create_agent(spec("hold the only slot"), Actor::user())
        .expect("hog");
    h.wait_until(hog.id, |s| matches!(s, AgentStatus::Running))
        .await;

    // The failed child reaches it while it is still waiting for the other.
    h.store
        .commit(
            Commit::new().mutate(Mutation::DeliverInbox(aether_core::inbox::InboxItem {
                id: Default::default(),
                agent_id: worker,
                seq: 1,
                kind: aether_core::inbox::InboxKind::TaskFailed {
                    task: failed_child,
                    error: "boom".into(),
                    structured: None,
                },
                delivered_at: now(),
            })),
        )
        .expect("deliver");
    // Woken early, held back by the limit, then blocked by the gate.
    h.wait_until(worker, |s| {
        matches!(
            s,
            AgentStatus::Blocked {
                reason: BlockReason::Dependency { .. }
            }
        )
    })
    .await;
    assert_eq!(
        h.store
            .agent(worker)
            .expect("a")
            .expect("exists")
            .resume_wait
            .as_ref(),
        Some(&condition),
        "the early wake must record the wait it interrupted"
    );

    h.runtime
        .send_message(
            worker,
            vec![ContentBlock::text("what is going on?")],
            Actor::user(),
        )
        .expect("send");
    h.wait_until(worker, |s| matches!(s, AgentStatus::Waiting { .. }))
        .await;
    let after = h.store.agent(worker).expect("a").expect("exists");
    let AgentStatus::Waiting { condition: back } = &after.status else {
        panic!("{:?}", after.status);
    };
    let expected: std::collections::BTreeSet<_> = [failed_child, still_running].into();
    assert_eq!(
        back.task_ids(),
        expected,
        "answering the person discarded the wait the agent chose: {back:?}"
    );
}
