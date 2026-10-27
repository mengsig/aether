use super::*;

#[test]
fn inbox_sequences_are_strict_and_ordered() {
    let store = SqliteStore::open_in_memory().expect("open");
    let a = agent();
    for seq in 1..=3 {
        store
            .commit(Commit::new().mutate(Mutation::DeliverInbox(inbox_item(a.id, seq))))
            .expect("deliver");
    }
    assert_eq!(store.inbox_last_seq(a.id).expect("last"), 3);
    let items = store.inbox(a.id, 1).expect("inbox");
    assert_eq!(items.iter().map(|i| i.seq).collect::<Vec<_>>(), vec![2, 3]);
    assert!(
        store
            .commit(Commit::new().mutate(Mutation::DeliverInbox(inbox_item(a.id, 3))))
            .is_err()
    );
}

#[test]
fn inbox_cursors_beyond_sqlite_sequences_do_not_replay_items() {
    let store = SqliteStore::open_in_memory().expect("open");
    let id = AgentId::new();
    store
        .commit(Commit::new().mutate(Mutation::DeliverInbox(inbox_item(id, 1))))
        .expect("deliver");
    for after in [i64::MAX as u64, i64::MAX as u64 + 1, u64::MAX] {
        assert!(store.inbox(id, after).expect("inbox").is_empty());
    }
}

#[test]
fn event_filters() {
    let store = SqliteStore::open_in_memory().expect("open");
    let a = agent();
    let t = Task::new("x", Actor::user(), a.run, now());
    store
        .commit(
            Commit::new()
                .event(
                    EventDraft::new(Actor::System, EventKind::AgentReady)
                        .agent(a.id)
                        .run(a.run),
                )
                .event(
                    EventDraft::new(Actor::System, EventKind::TaskStarted)
                        .agent(a.id)
                        .task(t.id)
                        .run(a.run),
                )
                .event(EventDraft::new(Actor::System, EventKind::LeaseReleased).agent(a.id)),
        )
        .expect("commit");
    let tasks = store
        .events(
            0,
            10,
            &EventFilter {
                families: vec!["task".into()],
                ..Default::default()
            },
        )
        .expect("events");
    assert_eq!(tasks.len(), 1);
    let by_type = store
        .events(
            0,
            10,
            &EventFilter {
                types: vec!["lease.released".into()],
                ..Default::default()
            },
        )
        .expect("events");
    assert_eq!(by_type.len(), 1);
    let by_run = store
        .events(0, 10, &EventFilter::for_run(a.run))
        .expect("events");
    assert_eq!(by_run.len(), 2);
    let by_task = store
        .events(
            0,
            10,
            &EventFilter {
                task: Some(t.id),
                ..Default::default()
            },
        )
        .expect("e");
    assert_eq!(by_task.len(), 1);
    assert!(EventFilter::for_run(a.run).matches(&by_run[0]));
}

#[test]
fn own_events_leave_out_what_children_emitted() {
    let store = SqliteStore::open_in_memory().expect("open");
    let parent = agent();
    let child = AgentId::new();
    let events = store
        .commit(
            Commit::new()
                .event(EventDraft::new(Actor::System, EventKind::AgentReady).agent(parent.id))
                .event(
                    EventDraft::new(Actor::System, EventKind::AgentReady)
                        .agent(child)
                        .parent(Some(parent.id)),
                ),
        )
        .expect("commit");
    let (own, childs) = (&events[0], &events[1]);

    let family = store
        .events(0, 10, &EventFilter::for_agent(parent.id))
        .expect("events");
    assert_eq!(family.len(), 2, "the default still follows children");
    let only_own = EventFilter::own_events(parent.id);
    let mine = store.events(0, 10, &only_own).expect("events");
    assert_eq!(mine.iter().map(|e| e.seq).collect::<Vec<_>>(), [own.seq]);
    let latest = store.latest_events(10, &only_own).expect("latest");
    assert_eq!(latest.iter().map(|e| e.seq).collect::<Vec<_>>(), [own.seq]);

    // The live-subscription check must agree with the query.
    assert!(only_own.matches(own));
    assert!(!only_own.matches(childs));
    assert!(EventFilter::for_agent(parent.id).matches(childs));
}

/// Connections that may not read `remote.*` events get them renamed after
/// they were filtered, so a filter must never select on a custom name.
#[test]
fn a_filter_cannot_tell_custom_events_apart_by_name() {
    let store = SqliteStore::open_in_memory().expect("open");
    let custom = |name: &str| {
        let kind = EventKind::Custom {
            name: name.into(),
            payload: serde_json::json!({"secret": true}),
        };
        EventDraft::new(Actor::System, kind)
    };
    let commit = Commit::new()
        .event(custom("remote.device_revoked"))
        .event(custom("orchestrator.note"));
    let events = store.commit(commit).expect("commit");
    let selecting = |families: &[&str], types: &[&str]| EventFilter {
        families: families.iter().map(|family| (*family).to_owned()).collect(),
        types: types.iter().map(|kind| (*kind).to_owned()).collect(),
        ..Default::default()
    };

    let by_name = [
        selecting(&["remote"], &[]),
        selecting(&["orchestrator"], &[]),
        selecting(&[], &["remote.device_revoked"]),
        selecting(&[], &["remote.restricted"]),
    ];
    for filter in by_name {
        let found = store.events(0, 10, &filter).expect("events");
        assert!(found.is_empty(), "{filter:?} selected {found:?}");
        assert!(
            events.iter().all(|event| !filter.matches(event)),
            "{filter:?}"
        );
    }
    for filter in [selecting(&["custom"], &[]), selecting(&[], &["custom"])] {
        let found = store.events(0, 10, &filter).expect("events");
        assert_eq!(found.len(), 2, "all of them or none: {filter:?}");
        assert!(
            events.iter().all(|event| filter.matches(event)),
            "{filter:?}"
        );
    }
}

#[test]
fn agent_and_task_filters() {
    let store = SqliteStore::open_in_memory().expect("open");
    let mut parent = agent();
    parent.status = AgentStatus::Ready;
    let mut child = agent();
    child.parent = Some(parent.id);
    child.run = parent.run;
    child.status = AgentStatus::Completed { summary: None };
    let mut task = Task::new("do", Actor::agent(parent.id), parent.run, now());
    task.owner = Some(child.id);
    store
        .commit(
            Commit::new()
                .mutate(Mutation::UpsertAgent(parent.clone()))
                .mutate(Mutation::UpsertAgent(child.clone()))
                .mutate(Mutation::UpsertTask(task.clone())),
        )
        .expect("commit");
    let live = store
        .agents(&AgentFilter {
            live_only: true,
            ..Default::default()
        })
        .expect("agents");
    assert_eq!(live.len(), 1);
    assert_eq!(live[0].id, parent.id);
    let kids = store
        .agents(&AgentFilter {
            parent: Some(parent.id),
            ..Default::default()
        })
        .expect("kids");
    assert_eq!(kids[0].id, child.id);
    let owned = store
        .tasks(&TaskFilter {
            owner: Some(child.id),
            ..Default::default()
        })
        .expect("tasks");
    assert_eq!(owned[0].id, task.id);
    task.transition(aether_core::task::TaskStatus::Assigned, now())
        .expect("assign");
    store
        .commit(Commit::new().mutate(Mutation::UpsertTask(task.clone())))
        .expect("commit");
    assert_eq!(
        store.task(task.id).expect("task").map(|t| t.status.kind()),
        Some(TaskStatusKind::Assigned)
    );
}

use aether_core::task::TaskStatusKind;

#[test]
fn transcript_rebuilds_from_message_events() {
    let store = SqliteStore::open_in_memory().expect("open");
    let a = agent();
    store
        .commit(
            Commit::new()
                .event(
                    EventDraft::new(
                        Actor::user(),
                        EventKind::AgentMessage {
                            role: MessageRole::User,
                            content: vec![ContentBlock::text("hi")],
                            invocation: None,
                        },
                    )
                    .agent(a.id),
                )
                .event(
                    EventDraft::new(Actor::System, EventKind::AgentStarted { turn: 1 }).agent(a.id),
                )
                .event(
                    EventDraft::new(
                        Actor::agent(a.id),
                        EventKind::AgentMessage {
                            role: MessageRole::Assistant,
                            content: vec![ContentBlock::text("hello")],
                            invocation: None,
                        },
                    )
                    .agent(a.id),
                ),
        )
        .expect("commit");
    let transcript = store.transcript(a.id, 0).expect("transcript");
    assert_eq!(transcript.len(), 2);
    assert_eq!(transcript[0].message.text(), "hi");
    assert_eq!(transcript[1].message.role, MessageRole::Assistant);
    assert_eq!(
        store.transcript(a.id, transcript[0].seq).expect("t").len(),
        1
    );
}

#[test]
fn leases_checkpoints_and_approvals_roundtrip() {
    let store = SqliteStore::open_in_memory().expect("open");
    let a = agent();
    let lease = Lease::new(
        a.id,
        "daemon-1",
        1,
        now(),
        std::time::Duration::from_secs(30),
    );
    store
        .commit(Commit::new().mutate(Mutation::SetLease(lease.clone())))
        .expect("lease");
    assert_eq!(store.lease(a.id).expect("lease"), Some(lease));
    assert_eq!(store.leases().expect("leases").len(), 1);
    store
        .commit(Commit::new().mutate(Mutation::ClearLease(a.id)))
        .expect("clear");
    assert!(store.lease(a.id).expect("lease").is_none());

    let task = Task::new("x", Actor::user(), a.run, now());
    let mut done = task.clone();
    done.transition(aether_core::task::TaskStatus::Assigned, now())
        .expect("a");
    done.transition(aether_core::task::TaskStatus::Active, now())
        .expect("b");
    done.transition(
        aether_core::task::TaskStatus::Completed {
            result: TaskResult::summary("ok"),
        },
        now(),
    )
    .expect("c");
    store
        .commit(Commit::new().mutate(Mutation::UpsertTask(done.clone())))
        .expect("task");
    let terminal = store
        .tasks(&TaskFilter {
            statuses: vec![TaskStatusKind::Completed],
            ..Default::default()
        })
        .expect("tasks");
    assert_eq!(terminal.len(), 1);
}

/// The scheduler asks for an agent's latest event of one type several times
/// a tick. Through `events_type` that walks every agent's events of the type:
/// 95 ms at 56k rows on an 844 MB store, and linear in it.
#[test]
fn an_agents_latest_event_of_one_type_is_a_seek() {
    let store = SqliteStore::open_in_memory().expect("open");
    let mut filter = EventFilter::own_events(AgentId::new());
    filter.types = vec!["agent.message".into()];
    let mut args = Vec::new();
    let predicate = super::event_queries::filter_predicate(&filter, &mut args);
    let sql = super::event_queries::page_sql("body", &predicate, 0, None, 1, false, &mut args);
    let refs: Vec<&dyn rusqlite::ToSql> = args.iter().map(|arg| arg.as_ref()).collect();
    let conn = store.conn.lock();
    let plan: Vec<String> = conn
        .prepare(&format!("EXPLAIN QUERY PLAN {sql}"))
        .expect("prepare")
        .query_map(refs.as_slice(), |r| r.get::<_, String>(3))
        .expect("plan")
        .collect::<std::result::Result<_, _>>()
        .expect("rows");
    assert!(
        plan.iter()
            .any(|step| step.contains("INDEX events_agent_type (agent_id=? AND type=?")),
        "seeks the agent's events of that type: {plan:?}"
    );
    assert!(
        !plan.iter().any(|step| step.contains("TEMP B-TREE")),
        "and reads them in seq order: {plan:?}"
    );
}

#[test]
fn before_cursor_returns_bounded_newest_events_in_reverse_order() {
    let store = SqliteStore::open_in_memory().unwrap();
    let parent = AgentId::new();
    let child = AgentId::new();
    for n in 0..8 {
        let draft = EventDraft::new(Actor::System, EventKind::AgentReady);
        let draft = if n % 2 == 0 {
            draft.agent(parent)
        } else {
            draft.agent(child).parent(Some(parent))
        };
        store.commit(Commit::new().event(draft)).unwrap();
    }
    let filter = EventFilter::for_agent(parent);
    let seqs = |before, limit| {
        store
            .events_before(before, limit, &filter)
            .unwrap()
            .into_iter()
            .map(|e| e.seq)
            .collect::<Vec<_>>()
    };
    assert_eq!(seqs(u64::MAX, 3), [8, 7, 6]);
    assert_eq!(seqs(6, 3), [5, 4, 3]);
    assert_eq!(seqs(3, 3), [2, 1]);
    assert!(seqs(1, 3).is_empty());
    assert!(seqs(0, 3).is_empty());
    assert!(seqs(u64::MAX, 0).is_empty());
    for before in [i64::MAX as u64, i64::MAX as u64 + 1, u64::MAX] {
        assert_eq!(seqs(before, 1), [8]);
    }
    let own = store
        .events_before(7, 2, &EventFilter::own_events(parent))
        .unwrap();
    assert_eq!(own.iter().map(|e| e.seq).collect::<Vec<_>>(), [5, 3]);
    assert_eq!(
        store
            .events(0, 2, &filter)
            .unwrap()
            .iter()
            .map(|e| e.seq)
            .collect::<Vec<_>>(),
        [1, 2]
    );
}

/// Seqs 1..=12 cycle through the parent's `agent.ready`, its child's
/// `agent.ready` and `agent.completed`, and an unrelated agent's completion.
fn family_events() -> (SqliteStore, AgentId, Vec<Event>) {
    let store = SqliteStore::open_in_memory().unwrap();
    let (parent, child, stranger) = (AgentId::new(), AgentId::new(), AgentId::new());
    for n in 0..12 {
        let ready = EventDraft::new(Actor::System, EventKind::AgentReady);
        let completed = EventDraft::new(Actor::System, EventKind::AgentCompleted { summary: None });
        let draft = match n % 4 {
            0 => ready.agent(parent),
            1 => ready.agent(child).parent(Some(parent)),
            2 => completed.agent(child).parent(Some(parent)),
            _ => completed.agent(stranger),
        };
        store.commit(Commit::new().event(draft)).unwrap();
    }
    let events = store.events(0, 100, &EventFilter::all()).unwrap();
    assert_eq!(events.len(), 12);
    (store, parent, events)
}

#[test]
fn child_types_narrow_only_the_childrens_events() {
    let (store, parent, events) = family_events();
    let filter = EventFilter {
        agent: Some(parent),
        child_types: vec!["agent.completed".into()],
        ..Default::default()
    };
    // The parent's own `agent.ready` stays; its child's does not.
    let expected: Vec<u64> = events
        .iter()
        .filter(|e| e.seq % 4 == 1 || e.seq % 4 == 3)
        .map(|e| e.seq)
        .collect();
    assert_eq!(expected, [1, 3, 5, 7, 9, 11]);
    let seqs = |events: Vec<Event>| events.into_iter().map(|e| e.seq).collect::<Vec<_>>();
    assert_eq!(seqs(store.events(0, 100, &filter).unwrap()), expected);
    assert_eq!(seqs(store.events(3, 2, &filter).unwrap()), [5, 7]);
    let mut paged = Vec::new();
    let mut before = u64::MAX;
    loop {
        let page = seqs(store.events_before(before, 2, &filter).unwrap());
        assert!(
            page.windows(2).all(|pair| pair[0] > pair[1]),
            "newest first"
        );
        let Some(&oldest) = page.last() else { break };
        paged.extend(page);
        before = oldest;
    }
    paged.reverse();
    assert_eq!(paged, expected, "pages neither skip nor repeat");
    for event in &events {
        assert_eq!(
            filter.matches(event),
            expected.contains(&event.seq),
            "{}",
            event.seq
        );
    }
    let own = EventFilter {
        exclude_children: true,
        ..filter.clone()
    };
    assert_eq!(
        seqs(store.events_before(u64::MAX, 100, &own).unwrap()),
        [9, 5, 1]
    );
}

#[test]
fn empty_child_types_take_every_child_event() {
    let (store, parent, events) = family_events();
    let filter = EventFilter::for_agent(parent);
    assert!(filter.child_types.is_empty());
    let expected: Vec<u64> = events
        .iter()
        .filter(|e| e.seq % 4 != 0)
        .map(|e| e.seq)
        .collect();
    let seqs = |events: Vec<Event>| events.into_iter().map(|e| e.seq).collect::<Vec<_>>();
    assert_eq!(seqs(store.events(0, 100, &filter).unwrap()), expected);
    let mut newest_first = expected.clone();
    newest_first.reverse();
    assert_eq!(
        seqs(store.events_before(u64::MAX, 100, &filter).unwrap()),
        newest_first
    );
    assert!(
        events
            .iter()
            .all(|e| filter.matches(e) == expected.contains(&e.seq))
    );
}

#[test]
fn before_cursor_uses_the_agent_sequence_index() {
    let store = SqliteStore::open_in_memory().unwrap();
    let mut args = Vec::new();
    let predicate =
        super::event_queries::filter_predicate(&EventFilter::own_events(AgentId::new()), &mut args);
    let sql = super::event_queries::page_sql("body", &predicate, 0, Some(123), 5, false, &mut args);
    let refs: Vec<&dyn rusqlite::ToSql> = args.iter().map(|a| a.as_ref()).collect();
    let conn = store.conn.lock();
    let plan: Vec<String> = conn
        .prepare(&format!("EXPLAIN QUERY PLAN {sql}"))
        .unwrap()
        .query_map(refs.as_slice(), |r| r.get(3))
        .unwrap()
        .collect::<std::result::Result<_, _>>()
        .unwrap();
    assert!(
        plan.iter()
            .any(|s| s.contains("INDEX events_agent (agent_id=? AND seq>? AND seq<?)")),
        "{plan:?}"
    );
    assert!(!plan.iter().any(|s| s.contains("TEMP B-TREE")), "{plan:?}");
}

#[test]
fn repository_membership_matches_serialized_scopes_including_archived_agents() {
    let store = SqliteStore::open_in_memory().expect("open");
    assert!(
        !store
            .any_agent_in_repository("aether")
            .expect("empty store")
    );
    let mut a = agent();
    a.scope.repository = Some("aether".into());
    assert_eq!(
        serde_json::to_value(&a).unwrap()["scope"]["repository"],
        "aether"
    );
    for (status, archived) in [
        (AgentStatus::Created, false),
        (AgentStatus::Completed { summary: None }, false),
        (AgentStatus::Completed { summary: None }, true),
    ] {
        a.status = status;
        a.archived = archived;
        store
            .commit(Commit::new().mutate(Mutation::UpsertAgent(a.clone())))
            .unwrap();
        assert!(store.any_agent_in_repository("aether").unwrap());
        for other in ["aeth", "aether.child", "Aether", "other", "' OR 1=1 --"] {
            assert!(!store.any_agent_in_repository(other).unwrap(), "{other}");
        }
    }
}

/// The SQL projection reads the same fields the decoded snapshot holds,
/// in the order asked, and leaves out ids that were never stored.
#[test]
fn snapshot_memories_follow_the_requested_order_and_match_the_stored_snapshots() {
    use aether_core::id::{MemoryId, SnapshotId};
    use aether_core::snapshot::{InvocationSnapshot, SnapshotMemories};

    let store = SqliteStore::open_in_memory().expect("open");
    let a = agent();
    let task = Task::new("x", Actor::user(), a.run, now());
    let stored = |task_id, memory_ids: Vec<MemoryId>| {
        let snapshot = InvocationSnapshot {
            id: SnapshotId::new(),
            agent_id: a.id,
            task_id,
            turn: 3,
            model: a.model.clone(),
            adapter: None,
            capabilities: None,
            memory_ids,
            layers: vec![],
            messages: vec![aether_core::model::Message::user_text("a long tail")],
            tools: vec![],
            params: Default::default(),
            token_estimate: 0,
            created_at: now(),
        };
        store
            .commit(Commit::new().mutate(Mutation::PutSnapshot(snapshot.clone())))
            .expect("snapshot");
        snapshot
    };
    let with = stored(Some(task.id), vec![MemoryId::new(), MemoryId::new()]);
    let without = stored(None, vec![]);
    let missing = SnapshotId::new();

    let found = store
        .snapshot_memories(&[without.id, missing, with.id])
        .expect("memories");
    assert_eq!(
        found,
        vec![SnapshotMemories::of(&without), SnapshotMemories::of(&with)]
    );
    assert_eq!(found[1].memories, with.memory_ids);
    assert_eq!(found[1].task, Some(task.id));
    assert!(store.snapshot_memories(&[]).expect("empty").is_empty());
}

#[test]
fn budget_filter_selects_only_live_budgeted_agents_when_requested() {
    let store = SqliteStore::open_in_memory().expect("store");
    let plain = agent();
    let mut token = agent();
    token.config.token_budget = Some(0);
    let mut cost = agent();
    cost.config.cost_budget = Some(0.0);
    let mut ended = token.clone();
    ended.id = aether_core::id::AgentId::new();
    ended.status = AgentStatus::Completed { summary: None };
    store
        .commit(
            [plain, token.clone(), cost.clone(), ended.clone()]
                .into_iter()
                .fold(Commit::new(), |commit, row| {
                    commit.mutate(Mutation::UpsertAgent(row))
                }),
        )
        .expect("seed");
    let filter = AgentFilter {
        budgeted_only: true,
        live_only: true,
        ..Default::default()
    };
    let rows = store.agents(&filter).expect("budgeted");
    assert_eq!(rows.len(), 2);
    assert!(rows.iter().any(|row| row.id == token.id));
    assert!(rows.iter().any(|row| row.id == cost.id));
    let all = store
        .agents(&AgentFilter {
            live_only: false,
            ..filter
        })
        .expect("including terminal");
    assert_eq!(all.len(), 3);
    assert!(all.iter().any(|row| row.id == ended.id));
}

#[test]
fn the_latest_custom_event_is_the_newest_with_a_wanted_name() {
    let store = SqliteStore::open_in_memory().expect("open");
    let custom = |name: &str, step: u64| {
        let kind = EventKind::Custom {
            name: name.into(),
            payload: serde_json::json!({"step": step}),
        };
        EventDraft::new(Actor::System, kind)
    };
    assert!(
        store
            .latest_custom_event(&["disk.low"])
            .expect("read")
            .is_none()
    );

    let commit = Commit::new()
        .event(custom("disk.low", 1))
        .event(custom("disk.recovered", 2))
        .event(custom("disk.low", 3))
        .event(custom("other.note", 4));
    store.commit(commit).expect("commit");

    let step = |names: &[&str]| {
        let event = store.latest_custom_event(names).expect("read")?;
        match event.kind {
            EventKind::Custom { payload, .. } => payload["step"].as_u64(),
            other => panic!("not a custom event: {other:?}"),
        }
    };
    assert_eq!(step(&["disk.low", "disk.recovered"]), Some(3));
    assert_eq!(step(&["disk.recovered"]), Some(2));
    assert_eq!(step(&["other.note"]), Some(4));
    assert_eq!(step(&["never.written"]), None);
    assert_eq!(step(&[]), None);
}
