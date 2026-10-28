//! commit races scenarios and fixtures.
use super::*;
use aether_core::agent::NetworkPolicy;

#[tokio::test]
async fn dependency_gate_preserves_racing_cancellation_when_blocking() {
    race_dependency_gate(false, true).await;
}

#[tokio::test]
async fn dependency_gate_preserves_racing_pause_when_blocking() {
    race_dependency_gate(false, false).await;
}

#[tokio::test]
async fn dependency_gate_preserves_racing_pause_when_waking() {
    race_dependency_gate(true, false).await;
}

#[tokio::test]
async fn dependency_gate_preserves_racing_cancellation_when_waking() {
    race_dependency_gate(true, true).await;
}

async fn race_dependency_gate(finished: bool, cancel: bool) {
    let inner: Arc<dyn Store> = Arc::new(SqliteStore::open_in_memory().expect("store"));
    let store = Arc::new(CommitsMidPass::new(inner, Hook::DependencyOwnerRead));
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
                role: "worker".into(),
                ..Default::default()
            },
            Actor::user(),
        )
        .expect("agent");
    let id = agent.id;
    let mut dependency =
        aether_core::task::Task::new("dependency", Actor::user(), agent.run, now());
    if finished {
        dependency.status = aether_core::task::TaskStatus::Cancelled { reason: None };
    }
    let mut task = aether_core::task::Task::new("dependent", Actor::user(), agent.run, now());
    task.owner = Some(id);
    task.dependencies = vec![dependency.id];
    agent.current_task = Some(task.id);
    agent.status = if finished {
        AgentStatus::Blocked {
            reason: BlockReason::Dependency { on: dependency.id },
        }
    } else {
        AgentStatus::Ready
    };
    h.runtime
        .apply(
            Commit::new()
                .mutate(Mutation::UpsertAgent(agent))
                .mutate(Mutation::UpsertTask(dependency))
                .mutate(Mutation::UpsertTask(task.clone())),
        )
        .expect("seed dependency gate");

    let racer = race_through_runtime(&store, &h.runtime, move |runtime| {
        if cancel {
            runtime
                .cancel_agent(id, Some("stop".into()), Actor::user())
                .expect("cancel");
        } else {
            runtime
                .pause_agent(id, false, Actor::user())
                .expect("pause");
        }
    });
    aether_runtime::scheduler::tick(&h.runtime).expect("dependency gate");
    join_racer(&racer);
    let row = h.store.agent(id).expect("read").expect("agent");
    if cancel {
        assert_eq!(
            row.status.kind(),
            AgentStatusKind::Cancelled,
            "gate resurrected cancelled owner"
        );
        assert_eq!(
            h.store
                .task(task.id)
                .expect("read")
                .expect("task")
                .status
                .kind(),
            TaskStatusKind::Cancelled
        );
    } else {
        assert!(
            row.paused,
            "gate reverted operator pause (finished={finished})"
        );
    }
}

/// The cap advisory's ancestry read
/// runs after `create_agent` already committed the child, so its failure
/// must never fail the turn - the delegation already happened and must be
/// reported as a success, with the read failure named in the tool result
/// text instead.
#[tokio::test]
async fn a_failed_cap_advisory_read_never_fails_the_delegating_turn() {
    let inner: Arc<dyn Store> = Arc::new(SqliteStore::open_in_memory().expect("store"));
    let store = Arc::new(CommitsMidPass::new(inner, Hook::CapAdvisoryFleetRead));
    let h = harness_full(
        vec![
            // coordinator turn 1: delegate and wait
            ScriptedClient::tool_call(
                "delegate",
                json!({"model":"anthropic/claude-sonnet-5-5", "effort":"low", "role": "worker", "objective": "do the sub task", "wait": true}),
            ),
            // child turn 1: finishes at once - max_concurrent_turns(1) means
            // it does not run until the coordinator's own turn frees the slot
            ScriptedClient::tool_call("complete_task", json!({"summary": "child done"})),
            // coordinator turn 2: woken by the child, only finishes
            ScriptedClient::tool_call("complete_task", json!({"summary": "coordinator done"})),
        ],
        |_| {},
        |_| {},
        None,
        None,
        Some(store.clone()),
        |_| {},
    )
    .await;
    let (coordinator, _) = h
        .runtime
        .create_agent(spec("spawn a worker"), Actor::user())
        .expect("create");
    let id = coordinator.id;

    // Neither the coordinator nor the child ever fails; both reach their own
    // ordinary conclusion, which is itself proof the read's error never
    // escaped the tool call it was advisory to.
    let status = h.wait_until(id, |s| s.is_terminal()).await;
    assert!(store.read_failed(), "the injected read failure never ran");
    assert_eq!(
        status.kind(),
        AgentStatusKind::Completed,
        "an advisory read failing after the child committed must not fail the turn: {status:?}"
    );

    let children = h
        .store
        .agents(&AgentFilter {
            parent: Some(id),
            ..Default::default()
        })
        .expect("agents");
    assert_eq!(children.len(), 1, "the delegation itself still succeeded");

    let outcome_text = h
        .store
        .events(0, 10_000, &EventFilter::for_agent(id))
        .expect("events")
        .into_iter()
        .find_map(|e| match e.kind {
            EventKind::ToolSucceeded { outcome, .. } => Some(outcome.content),
            _ => None,
        })
        .expect("a tool.succeeded event for the delegate call");
    assert!(
        outcome_text.contains("could not be read"),
        "the read failure must be named in the tool result, not swallowed: {outcome_text}"
    );
}

/// The roll-up skips a fleet whose commit count has not moved, so the count it
/// records must be one it actually looked at. Recording the count from after
/// the pass swallows anything committed while the pass ran: the agent that
/// commit put over its budget is marked checked without ever being seen, and
/// with nothing else committing, the pass never runs again.
#[tokio::test]
async fn a_commit_landing_during_the_budget_pass_is_not_marked_checked() {
    let inner: Arc<dyn Store> = Arc::new(SqliteStore::open_in_memory().expect("store"));
    let store = Arc::new(CommitsMidPass::new(inner, Hook::BudgetPassReadsAgents));
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
    let mut s = idle_spec();
    s.config.as_mut().expect("config").cost_budget = Some(1.0);
    let (agent, _) = h.runtime.create_agent(s, Actor::user()).expect("create");
    h.wait_until(agent.id, |s| s.kind() == AgentStatusKind::Waiting)
        .await;
    for _ in 0..3 {
        aether_runtime::scheduler::tick(&h.runtime).expect("tick");
    }
    assert!(
        !h.store
            .agent(agent.id)
            .expect("a")
            .expect("e")
            .status
            .is_terminal()
    );

    // The turn's usage commit lands inside the next pass, after it has read
    // the table it decides from. Something has to un-skip that pass first;
    // any commit does, and this one says nothing about a budget.
    let mut spent = h.store.agent(agent.id).expect("a").expect("e");
    spent.stats.usage.cost_usd = 2.0;
    store.arm(Box::new(move |inner| {
        inner.commit(Commit::new().mutate(Mutation::UpsertAgent(spent)))?;
        Ok(())
    }));
    h.store
        .commit(Commit::new().event(EventDraft::new(
            Actor::System,
            EventKind::DaemonStarted {
                version: "test".into(),
                pid: 1,
            },
        )))
        .expect("an unrelated commit");
    aether_runtime::scheduler::tick(&h.runtime).expect("tick");
    assert!(store.fired(), "the armed commit landed during the pass");

    let status = h.wait_until(agent.id, |s| s.is_terminal()).await;
    assert_eq!(
        status.kind(),
        AgentStatusKind::Failed,
        "the spend that landed mid-pass still stops the agent: {status:?}"
    );
}

/// A pause taken in the window between the scheduler reading `paused` and the
/// turn starting must still hold: the tick does store I/O in between, so the
/// turn re-checks the flag the same way it re-checks Ready and the budget.
#[tokio::test]
async fn a_pause_taken_while_a_turn_is_being_dispatched_runs_no_turn() {
    let inner: Arc<dyn Store> = Arc::new(SqliteStore::open_in_memory().expect("store"));
    let store = Arc::new(CommitsMidPass::new(
        inner,
        Hook::SchedulerAsksIfReadyAgentHasWork,
    ));
    let h = harness_full(
        vec![ScriptedClient::text("should never be asked for")],
        |_| {},
        |_| {},
        None,
        None,
        Some(store.clone()),
        |_| {},
    )
    .await;
    let (agent, _) = h
        .runtime
        .create_agent(idle_spec(), Actor::user())
        .expect("agent");
    let id = agent.id;
    h.wait_until(id, |s| s.kind() == AgentStatusKind::Waiting)
        .await;

    // Work arrives and the operator pauses, both landing while the tick is
    // deciding this agent has something to do - after its own `paused` read,
    // and on the row as it stands rather than a copy taken earlier.
    store.arm(Box::new(move |inner| {
        let mut held = inner.agent(id)?.expect("the agent");
        held.paused = true;
        inner.commit(
            Commit::new()
                .mutate(Mutation::DeliverInbox(aether_core::inbox::InboxItem {
                    id: Default::default(),
                    agent_id: id,
                    seq: 1,
                    kind: aether_core::inbox::InboxKind::Wake {
                        reason: "go".into(),
                    },
                    delivered_at: now(),
                }))
                .mutate(Mutation::UpsertAgent(held)),
        )?;
        Ok(())
    }));
    h.runtime
        .resume_agent(id, false, Actor::user())
        .expect("back to ready");

    for _ in 0..5 {
        aether_runtime::scheduler::tick(&h.runtime).expect("tick");
    }
    tokio::time::sleep(Duration::from_millis(300)).await;
    assert!(store.fired(), "the pause landed inside the dispatch window");
    assert!(paused(h.store.as_ref(), id));
    assert_eq!(
        h.client.remaining(),
        1,
        "a paused agent takes no turn: the model was never called"
    );
    let t = types(h.store.as_ref(), id);
    assert!(!t.contains(&"agent.started".to_owned()), "{t:?}");
}

/// Pause and resume write the whole agent row back, so they must read it under
/// the hold their write takes: a knob changed in between was silently reverted.
#[tokio::test]
async fn pause_and_resume_keep_a_change_committed_while_they_decide() {
    // (what, the read the racer lands on, subtree, pausing)
    let cases = [
        ("pause", Hook::AgentRead, false, true),
        ("subtree pause", Hook::FleetRead, true, true),
        ("resume", Hook::AgentRead, false, false),
        ("subtree resume", Hook::FleetRead, true, false),
    ];
    for (what, hook, subtree, pausing) in cases {
        let inner: Arc<dyn Store> = Arc::new(SqliteStore::open_in_memory().expect("store"));
        let store = Arc::new(CommitsMidPass::new(inner, hook));
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
        let (agent, _) = h
            .runtime
            .create_agent(idle_spec(), Actor::user())
            .expect("agent");
        let id = agent.id;
        h.wait_until(id, |s| s.kind() == AgentStatusKind::Waiting)
            .await;
        if !pausing {
            h.runtime
                .pause_agent(id, subtree, Actor::user())
                .expect("paused first");
        }

        let racer = race_through_runtime(&store, &h.runtime, move |runtime| {
            let update = AgentUpdate {
                max_turns: Some(77),
                ..Default::default()
            };
            runtime
                .update_agent(id, update, Actor::user())
                .expect("the concurrent update");
        });
        if pausing {
            h.runtime
                .pause_agent(id, subtree, Actor::user())
                .expect(what);
        } else {
            h.runtime
                .resume_agent(id, subtree, Actor::user())
                .expect(what);
        }
        join_racer(&racer);

        let row = h.store.agent(id).expect("read").expect("agent");
        assert_eq!(
            row.config.max_turns,
            Some(77),
            "{what} reverted a change committed while it decided"
        );
        assert_eq!(row.paused, pausing, "{what} itself took effect");
    }
}

/// The turn start writes the agent back as Running. Read outside the hold, that
/// write reverted a pause committed in between, and the paused agent ran on.
#[tokio::test]
async fn a_pause_committed_while_a_turn_starts_survives_the_start() {
    let inner: Arc<dyn Store> = Arc::new(SqliteStore::open_in_memory().expect("store"));
    let store = Arc::new(CommitsMidPass::new(inner, Hook::TaskRead));
    let h = harness_full(
        vec![ScriptedClient::text("working on it")],
        |_| {},
        |_| {},
        None,
        None,
        Some(store.clone()),
        |_| {},
    )
    .await;
    let (agent, _) = h
        .runtime
        .create_agent(idle_spec(), Actor::user())
        .expect("agent");
    let id = agent.id;
    h.wait_until(id, |s| s.kind() == AgentStatusKind::Waiting)
        .await;

    // The turn start reads the assigned task between its own agent read and
    // its commit: the operator's pause lands on that read.
    let racer = race_through_runtime(&store, &h.runtime, move |runtime| {
        runtime
            .pause_agent(id, false, Actor::user())
            .expect("the operator's pause");
    });
    let task = aether_runtime::CreateTaskSpec {
        objective: "the work".into(),
        owner: Some(id),
        ..Default::default()
    };
    h.runtime.create_task(task, Actor::user()).expect("task");
    h.runtime
        .send_message(id, vec![ContentBlock::text("go")], Actor::user())
        .expect("message");
    h.wait_for("the turn start to read its task", || store.fired())
        .await;
    join_racer(&racer);

    let t = types(h.store.as_ref(), id);
    assert!(t.contains(&"agent.started".to_owned()), "{t:?}");
    assert!(
        paused(h.store.as_ref(), id),
        "the turn start reverted a pause committed while it started: {t:?}"
    );
}

/// A task boundary is only an automatic fold. If an operator changes models
/// while the turn decides to request it, the required handoff must remain the
/// latest compaction request.
#[tokio::test]
async fn task_boundary_request_does_not_hide_a_racing_model_handoff() {
    let inner: Arc<dyn Store> = Arc::new(SqliteStore::open_in_memory().expect("store"));
    let store = Arc::new(CommitsMidPass::new(inner, Hook::TaskBoundaryTranscriptRead));
    let h = harness_full(
        vec![
            ScriptedClient::text("started"),
            ScriptedClient::text("halfway"),
            ScriptedClient::text("nearly there"),
            ScriptedClient::tool_call("complete_task", json!({"summary": "done"})),
        ],
        |_| {},
        |_| {},
        None,
        None,
        Some(store.clone()),
        |_| {},
    )
    .await;
    let (agent, _) = h
        .runtime
        .create_agent(
            CreateAgentSpec {
                objective: Some("finish the job".into()),
                config: Some(AgentConfig {
                    long_lived: true,
                    approval: ApprovalPolicy::none(),
                    ..Default::default()
                }),
                ..spec("finish the job")
            },
            Actor::user(),
        )
        .expect("create");
    for message in ["continue", "continue again"] {
        h.wait_until(agent.id, |s| s.kind() == AgentStatusKind::Waiting)
            .await;
        let before = h.client.remaining();
        h.runtime
            .send_message(agent.id, vec![ContentBlock::text(message)], Actor::user())
            .expect("send");
        h.wait_for("the next model response", || h.client.remaining() < before)
            .await;
    }
    h.wait_until(agent.id, |s| s.kind() == AgentStatusKind::Waiting)
        .await;

    let id = agent.id;
    let target = ModelRef::new("claude-code", "haiku");
    let for_racer = target.clone();
    let racer = race_through_runtime(&store, &h.runtime, move |runtime| {
        runtime
            .update_agent(
                id,
                AgentUpdate {
                    model: Some(for_racer),
                    ..Default::default()
                },
                Actor::user(),
            )
            .expect("racing model change");
    });
    h.runtime
        .send_message(id, vec![ContentBlock::text("finish")], Actor::user())
        .expect("send final message");
    h.wait_for("task-boundary transcript read", || store.fired())
        .await;
    join_racer(&racer);
    h.wait_until(id, |s| s.kind() == AgentStatusKind::Waiting)
        .await;
    let latest = h
        .store
        .latest_events(
            1,
            &EventFilter {
                types: vec![
                    "compaction.requested".into(),
                    "compaction.completed".into(),
                    "compaction.failed".into(),
                ],
                ..EventFilter::own_events(id)
            },
        )
        .expect("compaction events")
        .pop()
        .expect("latest compaction event");
    assert!(matches!(
        latest.kind,
        EventKind::CompactionRequested {
            trigger: CompactionTrigger::Handoff { to, .. }
        } if to == target
    ));
}

/// When `compete_at_read`'s write ran relative to the reader it raced.
#[derive(Debug, PartialEq, Eq)]
enum Schedule {
    /// The read ran outside the commit gate, so the write landed inside it.
    InsideTheRead,
    /// The reader held the gate, so the write waited for it to return.
    AfterTheReader,
}

type Competing = Box<dyn FnOnce(&Runtime) + Send>;

/// A write competing with the reader at `store`'s hooked read. The schedule
/// is fixed by which side of the commit gate the read is on, never by timing.
struct Competitor {
    schedule: Arc<std::sync::Mutex<Option<Schedule>>>,
    deferred: Arc<std::sync::Mutex<Option<Competing>>>,
}

fn compete_at_read(
    store: &CommitsMidPass,
    runtime: &Arc<Runtime>,
    write: impl FnOnce(&Runtime) + Send + 'static,
) -> Competitor {
    let competitor = Competitor {
        schedule: Arc::default(),
        deferred: Arc::default(),
    };
    let (schedule, deferred) = (competitor.schedule.clone(), competitor.deferred.clone());
    let runtime = Arc::downgrade(runtime);
    store.arm(Box::new(move |_| {
        let runtime = runtime.upgrade().expect("the runtime outlives its race");
        // The background loops are shut down, so a held gate is the reader's.
        let ran = if runtime.commit_gate_held() {
            *deferred.lock().expect("deferred") = Some(Box::new(write));
            Schedule::AfterTheReader
        } else {
            write(&runtime);
            Schedule::InsideTheRead
        };
        *schedule.lock().expect("schedule") = Some(ran);
        Ok(())
    }));
    competitor
}

impl Competitor {
    /// Runs a write the gate held back, now the reader has returned.
    fn finish(self, runtime: &Runtime) -> Schedule {
        if let Some(write) = self.deferred.lock().expect("deferred").take() {
            write(runtime);
        }
        self.schedule
            .lock()
            .expect("schedule")
            .take()
            .expect("the hooked read never ran")
    }
}

/// A harness whose agent reads can be raced, with its background loops shut
/// down so the reader under test is the only one that can hold the gate.
async fn quiet_harness() -> (Harness, Arc<CommitsMidPass>) {
    let inner: Arc<dyn Store> = Arc::new(SqliteStore::open_in_memory().expect("store"));
    let store = Arc::new(CommitsMidPass::new(inner, Hook::AgentRead));
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
    (h, store)
}

fn child_of(parent: AgentId) -> CreateAgentSpec {
    CreateAgentSpec {
        role: "worker".into(),
        parent: Some(parent),
        config: Some(AgentConfig {
            effort: Some(aether_core::model::Effort::Low),
            ..Default::default()
        }),
        ..Default::default()
    }
}

/// A child is derived from its parent before the creation hold. A narrowing
/// committed in between must refuse the create, not leave a wider child.
#[tokio::test]
async fn a_parent_narrowed_while_a_child_is_created_refuses_the_child() {
    let cases = [
        (
            "approval",
            AgentUpdate {
                approval: Some(ApprovalPolicy::default()),
                ..Default::default()
            },
        ),
        (
            "network",
            AgentUpdate {
                network: Some(NetworkPolicy::Denied),
                ..Default::default()
            },
        ),
    ];
    for (what, narrowing) in cases {
        let (h, store) = quiet_harness().await;
        let mut parent_spec = idle_spec();
        parent_spec.scope = Some(Scope {
            workspace_root: Some(h.workspace_root.clone()),
            network: NetworkPolicy::Allowed,
            ..Default::default()
        });
        let (parent, _) = h
            .runtime
            .create_agent(parent_spec, Actor::user())
            .expect("parent");
        let pid = parent.id;

        // Commits on `prepare_agent`'s parent read, before the creation hold.
        let competitor = compete_at_read(&store, &h.runtime, move |runtime| {
            runtime
                .update_agent(pid, narrowing, Actor::user())
                .expect("the concurrent narrowing");
        });
        let created = h.runtime.create_agent(child_of(pid), Actor::user());
        assert_eq!(competitor.finish(&h.runtime), Schedule::InsideTheRead);

        let refusal = created.expect_err(what).to_string();
        assert!(
            refusal.contains("changed while this agent was being created"),
            "{what}: {refusal}"
        );
        let children = h
            .store
            .agents(&AgentFilter {
                parent: Some(pid),
                ..Default::default()
            })
            .expect("agents");
        assert!(children.is_empty(), "{what}: a refused child was committed");
    }
}

/// A subtree pause or resume committed while a child is being created reaches
/// the child: it is born held under a pause and free after a resume.
#[tokio::test]
async fn a_subtree_pause_or_resume_while_a_child_is_created_reaches_the_child() {
    for pausing in [true, false] {
        let (h, store) = quiet_harness().await;
        let (parent, _) = h
            .runtime
            .create_agent(idle_spec(), Actor::user())
            .expect("parent");
        let pid = parent.id;
        if !pausing {
            h.runtime
                .pause_agent(pid, true, Actor::user())
                .expect("paused first");
        }

        // Its subtree walk runs before the child exists, so it cannot see it.
        let competitor = compete_at_read(&store, &h.runtime, move |runtime| {
            if pausing {
                runtime
                    .pause_agent(pid, true, Actor::user())
                    .expect("the concurrent pause");
            } else {
                runtime
                    .resume_agent(pid, true, Actor::user())
                    .expect("the concurrent resume");
            }
        });
        let created = h.runtime.create_agent(child_of(pid), Actor::user());
        assert_eq!(competitor.finish(&h.runtime), Schedule::InsideTheRead);

        let (child, _) = created.expect("child");
        let row = h.store.agent(child.id).expect("read").expect("child");
        assert_eq!(row.paused, pausing, "pausing={pausing}");
        assert_eq!(row.paused_via, pausing.then_some(pid), "pausing={pausing}");
    }
}

/// Two answers to one approval: exactly one lands, and the other is refused
/// with no rule, event or inbox item of its own.
#[tokio::test]
async fn concurrent_answers_to_one_approval_resolve_it_once() {
    let (h, store) = quiet_harness().await;
    let (agent, _) = h
        .runtime
        .create_agent(idle_spec(), Actor::user())
        .expect("agent");
    let approval = ApprovalId::new();
    h.store
        .commit(Commit::new().mutate(Mutation::PutApproval(ApprovalRequest {
            id: approval,
            agent_id: agent.id,
            task_id: None,
            call_id: Default::default(),
            tool: "write_file".into(),
            description: "overwrite a file".into(),
            argument: Some("notes.md".into()),
            resources: Vec::new(),
            side_effect: SideEffectClass::Mutation,
            requested_at: now(),
            status: aether_core::tool::ApprovalStatus::Pending,
        })))
        .expect("seed");

    // The denial competes at the approver's first agent read, after the
    // approval itself was read: unheld, it lands between check and commit.
    let denied = Arc::new(std::sync::Mutex::new(None));
    let slot = denied.clone();
    let competitor = compete_at_read(&store, &h.runtime, move |runtime| {
        let result = runtime.resolve_approval(
            approval,
            ApprovalDecision::Denied,
            Actor::user(),
            None,
            Some(Remember::narrow(None)),
        );
        *slot.lock().expect("slot") = Some(result.map_err(|e| e.to_string()));
    });
    let approved = h.runtime.resolve_approval(
        approval,
        ApprovalDecision::Approved,
        Actor::user(),
        None,
        Some(Remember::narrow(None)),
    );
    let schedule = competitor.finish(&h.runtime);
    let denied = denied.lock().expect("slot").take().expect("denial ran");

    let refusal = match (approved, denied) {
        (Ok(()), Err(refusal)) => refusal,
        (approved, denied) => panic!(
            "the first answer must land and the second be refused: {approved:?} / {denied:?}"
        ),
    };
    assert!(refusal.contains("already resolved"), "{refusal}");
    assert_eq!(
        schedule,
        Schedule::AfterTheReader,
        "the approver reads the approval under the gate, so the denial waits"
    );
    assert_eq!(
        h.runtime.approval_rules(false).expect("rules").len(),
        1,
        "the losing answer remembered a rule"
    );
    let events = h
        .store
        .events(0, 10_000, &EventFilter::default())
        .expect("events");
    let resolutions = events
        .iter()
        .filter(|e| matches!(&e.kind, EventKind::ApprovalResolved { approval: id, .. } if *id == approval))
        .count();
    let deliveries = events
        .iter()
        .filter(|e| {
            matches!(&e.kind, EventKind::InboxDelivered { item }
                if matches!(item.kind, aether_core::inbox::InboxKind::ApprovalResolved { .. }))
        })
        .count();
    assert_eq!((resolutions, deliveries), (1, 1));
}

/// A message sent while its recipient is being cancelled lands before the
/// cancellation, never in the inbox of an ended agent.
#[tokio::test]
async fn a_message_racing_the_recipients_cancellation_never_lands_after_it() {
    let (h, store) = quiet_harness().await;
    let (agent, _) = h
        .runtime
        .create_agent(idle_spec(), Actor::user())
        .expect("agent");
    let id = agent.id;

    // Competes at the sender's liveness read of the recipient.
    let competitor = compete_at_read(&store, &h.runtime, move |runtime| {
        runtime
            .cancel_agent(id, Some("stop".into()), Actor::user())
            .expect("the concurrent cancellation");
    });
    let sent = h
        .runtime
        .send_message(id, vec![ContentBlock::text("hello")], Actor::user());
    let schedule = competitor.finish(&h.runtime);

    let events = h
        .store
        .events(0, 10_000, &EventFilter::for_agent(id))
        .expect("events");
    let seq_of =
        |pred: &dyn Fn(&EventKind) -> bool| events.iter().find(|e| pred(&e.kind)).map(|e| e.seq);
    let cancelled = seq_of(&|k| matches!(k, EventKind::AgentCancelled { .. }))
        .expect("the agent was cancelled");
    let delivered =
        seq_of(&|k| matches!(k, EventKind::MessageSent { .. })).expect("the message was delivered");
    sent.expect("the send succeeded");
    assert!(delivered < cancelled, "delivered after the cancellation");
    assert_eq!(
        schedule,
        Schedule::AfterTheReader,
        "the sender reads liveness under the gate, so the cancellation waits"
    );
}
