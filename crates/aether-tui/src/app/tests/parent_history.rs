//! A parent's history pages, served by a real store: its children's activity
//! must not crowd its own conversation off the page it opens on.
use super::*;
use aether_core::id::TaskId;
use aether_core::model::{ContentBlock, MessageRole};
use aether_store::{Commit, SqliteStore, Store};

/// Commit `drafts` in one transaction; the store stamps their sequence.
fn commit(store: &SqliteStore, drafts: Vec<EventDraft>) -> Vec<Event> {
    let count = drafts.len();
    let events = store
        .commit(Commit {
            events: drafts,
            mutations: Vec::new(),
        })
        .expect("commit");
    assert_eq!(events.len(), count);
    events
}

fn own(parent: AgentId, kind: EventKind) -> EventDraft {
    EventDraft::new(Actor::System, kind).agent(parent)
}

fn of_child(parent: AgentId, child: AgentId, kind: EventKind) -> EventDraft {
    EventDraft::new(Actor::System, kind)
        .agent(child)
        .parent(Some(parent))
}

fn reply(text: &str) -> EventKind {
    EventKind::AgentMessage {
        role: MessageRole::Assistant,
        content: vec![ContentBlock::text(text)],
        invocation: None,
    }
}

fn tool_call(agent: AgentId, name: &str) -> ToolCall {
    ToolCall {
        id: ToolCallId::new(),
        provider_call_id: "toolu".into(),
        name: name.into(),
        input: serde_json::json!({ "command": "true" }),
        agent_id: agent,
        task_id: None,
        invocation_id: None,
        resources: vec![],
        idempotency_key: None,
        requested_at: now(),
    }
}

/// `calls` settled tool calls by `child`: two events each, none of which
/// draws anything in the parent.
fn tool_chatter(parent: AgentId, child: AgentId, calls: usize) -> Vec<EventDraft> {
    let mut drafts = Vec::with_capacity(calls * 2);
    for _ in 0..calls {
        let call = tool_call(child, "bash");
        let id = call.id;
        drafts.push(of_child(
            parent,
            child,
            EventKind::ToolRequested {
                call,
                side_effect: SideEffectClass::ReadOnly,
            },
        ));
        drafts.push(of_child(
            parent,
            child,
            EventKind::ToolSucceeded {
                call: id,
                outcome: ToolOutcome::ok("ok"),
            },
        ));
    }
    drafts
}

/// Answer a history request the way the daemon's `GetEvents` does.
fn serve(store: &SqliteStore, effect: Effect) -> Reply {
    let Effect::Request {
        command:
            Command::GetEvents {
                after,
                limit,
                query,
            },
        ..
    } = effect
    else {
        panic!("a history request: {effect:?}")
    };
    assert_eq!(after, 0, "history pages read backwards");
    let before = query.before.expect("a history page has a cursor");
    let events = store
        .events_before(before, limit.clamp(1, 5_000), &query.into())
        .expect("page");
    Reply::Events(events)
}

/// Open `agent`'s transcript afresh, from the store.
fn open(app: &mut App, store: &SqliteStore, agent: AgentId) {
    app.transcripts.get_mut(&agent).expect("transcript").loaded = false;
    let request = app.backfill_request(agent).expect("history request");
    let reply = serve(store, request);
    app.reduce(AppEvent::Reply {
        then: crate::testing::backfill_then(app, agent),
        result: Ok(reply),
    });
    assert!(app.transcripts[&agent].loaded);
}

fn replies(app: &App, agent: AgentId) -> usize {
    app.transcripts[&agent]
        .all_cells()
        .filter(|cell| matches!(cell.model, CellModel::Assistant { .. }))
        .count()
}

/// Issue #16: a child that spawned and then ran thousands of tool calls
/// pushed every one of its parent's 300 turns off the page the parent opened on.
#[test]
fn a_chatty_child_leaves_its_parents_turns_on_the_first_page() {
    let (mut app, parent) = app_with_agent();
    let child = AgentId::new();
    let store = SqliteStore::open_in_memory().expect("store");
    let spawn = EventKind::AgentCreated {
        name: "busy".into(),
        role: Role::new("worker"),
        parent: Some(parent),
        model: ModelRef::new("mock", "m"),
        scope: Scope::default(),
        fallbacks: vec![],
    };
    let mut drafts = vec![of_child(parent, child, spawn)];
    drafts.extend((0..300).map(|n| own(parent, reply(&format!("turn {n}")))));
    drafts.extend(tool_chatter(parent, child, 2_600));
    drafts.push(of_child(
        parent,
        child,
        EventKind::AgentCompleted { summary: None },
    ));
    let committed = commit(&store, drafts);
    assert_eq!(committed.len(), 5_502);

    open(&mut app, &store, parent);
    let shown = replies(&app, parent);
    eprintln!("parent turns on the first page: {shown} of 300");
    assert_eq!(shown, 300, "every parent turn is on its first page");
    assert!(
        app.transcripts[&parent]
            .all_cells()
            .any(|cell| matches!(cell.model, CellModel::Spawned { .. })),
        "and the spawn that started the child"
    );
    assert!(!app.earlier_history_available(), "nothing older remains");
}

/// A parent, the child it spawned, and that child's own child.
struct Family {
    parent: AgentId,
    child: AgentId,
    grandchild: AgentId,
    first: TaskId,
    second: TaskId,
}

fn spawn(name: &str, parent: AgentId) -> EventKind {
    EventKind::AgentCreated {
        name: name.into(),
        role: Role::new("worker"),
        parent: Some(parent),
        model: ModelRef::new("mock", "m"),
        scope: Scope::default(),
        fallbacks: vec![],
    }
}

fn spend(usd: f64) -> EventKind {
    let usage = |usd| Usage {
        cost_usd: usd,
        cost_complete: true,
        ..Usage::default()
    };
    EventKind::AgentUsageUpdated {
        total: usage(usd),
        turn_usage: usage(0.25),
    }
}

impl Family {
    fn new(parent: AgentId) -> Self {
        Self {
            parent,
            child: AgentId::new(),
            grandchild: AgentId::new(),
            first: TaskId::new(),
            second: TaskId::new(),
        }
    }

    fn child(&self, kind: EventKind) -> EventDraft {
        of_child(self.parent, self.child, kind)
    }

    /// Each child outcome lands as a task event, then a spend, then the agent
    /// event, so leaving out any one of them shows in the parent.
    fn history(&self) -> Vec<EventDraft> {
        let mut drafts = vec![
            own(self.parent, EventKind::AgentStarted { turn: 1 }),
            own(self.parent, task("fix the build")),
        ];
        drafts.extend(self.failed_run());
        drafts.extend(self.cancelled_run());
        drafts.extend(self.finished_run());
        drafts
    }

    fn failed_run(&self) -> Vec<EventDraft> {
        let mut drafts = vec![
            self.child(spawn("helper", self.parent)),
            self.child(task("find the bug")).task(self.first),
            self.child(EventKind::AgentStarted { turn: 1 }),
        ];
        drafts.extend(tool_chatter(self.parent, self.child, 3));
        drafts.extend([
            self.child(spend(0.25)),
            self.child(EventKind::InvocationFailed {
                invocation: InvocationId::new(),
                snapshot: aether_core::id::SnapshotId::new(),
                error: "limit".into(),
                class: Some(aether_core::model::ModelErrorClass::UsageLimit),
                retryable: false,
                cancelled: false,
                partial: vec![],
            }),
            self.child(EventKind::TaskFailed {
                error: "task broke".into(),
                structured: None,
            })
            .task(self.first),
            self.child(spend(0.5)),
            self.child(EventKind::AgentFailed {
                error: "agent broke".into(),
            }),
        ]);
        drafts
    }

    fn cancelled_run(&self) -> Vec<EventDraft> {
        let message = EventKind::MessageSent {
            message: aether_core::id::MessageId::new(),
            from: Actor::Agent { id: self.parent },
            to: self.child,
            content: vec![ContentBlock::text("try another way")],
        };
        let wait = EventKind::AgentWaiting {
            condition: aether_core::agent::WaitCondition::Task { id: self.second },
        };
        let mut drafts = vec![
            own(self.parent, reply("the helper failed; renaming it")),
            self.child(EventKind::AgentConfigUpdated {
                name: Some("renamed".into()),
                model: Some(ModelRef::new("other", "m2")),
                config: AgentConfig::default(),
                scope: None,
            }),
            self.child(message),
            self.child(EventKind::AgentStarted { turn: 2 }),
            self.child(EventKind::TaskAssigned { owner: self.child })
                .task(self.second),
            own(self.parent, wait),
        ];
        drafts.extend(tool_chatter(self.parent, self.child, 2));
        drafts.extend([
            self.child(EventKind::TaskCancelled { reason: None })
                .task(self.second),
            self.child(spend(0.75)),
            self.child(EventKind::AgentCancelled { reason: None }),
        ]);
        drafts
    }

    fn finished_run(&self) -> Vec<EventDraft> {
        let grandchild = |kind| of_child(self.child, self.grandchild, kind);
        vec![
            self.child(EventKind::AgentStarted { turn: 3 }),
            grandchild(spawn("deeper", self.child)),
            grandchild(spend(2.0)),
            grandchild(EventKind::AgentCompleted { summary: None }),
            self.child(EventKind::TaskCompleted {
                result: aether_core::task::TaskResult::summary("task done"),
            }),
            self.child(spend(1.0)),
            self.child(EventKind::AgentCompleted {
                summary: Some("agent done".into()),
            }),
            own(self.parent, reply("all done")),
            own(
                self.parent,
                EventKind::AgentTurnCompleted {
                    turn: 1,
                    invocation: None,
                    next: aether_core::agent::AgentStatusKind::Completed,
                },
            ),
        ]
    }
}

fn task(objective: &str) -> EventKind {
    EventKind::TaskCreated {
        objective: objective.into(),
        parent_task: None,
        dependencies: vec![],
        priority: aether_core::task::Priority::Normal,
    }
}

/// The parent's cells as a reader sees them, and where its windows begin.
type ParentView = (Vec<Cell>, Option<Cell>, Vec<transcript::HistoryPoint>);

/// Ingest `events` in order, as they arrive live or replay from history.
fn ingest(events: &[Event], tree: &AgentTree, live: bool) -> transcript::Transcripts {
    let ctx = transcript::IngestCtx { tree, now: now() };
    let mut transcripts = transcript::Transcripts::new();
    for event in events {
        if live {
            transcript::ingest_event(&mut transcripts, event, &ctx);
        } else {
            transcript::ingest_replay_event(&mut transcripts, event, &ctx);
        }
    }
    transcripts
}

fn parent_view(transcripts: &transcript::Transcripts, parent: AgentId) -> ParentView {
    let t = &transcripts[&parent];
    (
        t.cells.clone(),
        t.active.clone(),
        t.history.iter().copied().collect(),
    )
}

/// Every history page of `parent` taking `child_types` from its children,
/// fetched newest first as the terminal pages up, returned oldest first.
fn paged(store: &SqliteStore, parent: AgentId, child_types: &[&str], limit: usize) -> Vec<Event> {
    let filter = aether_store::EventFilter {
        agent: Some(parent),
        child_types: child_types.iter().map(|t| t.to_string()).collect(),
        ..Default::default()
    };
    let (mut events, mut before) = (Vec::new(), u64::MAX);
    loop {
        let page = store.events_before(before, limit, &filter).expect("page");
        assert!(page.len() <= limit);
        assert!(page.windows(2).all(|pair| pair[0].seq > pair[1].seq));
        let full = page.len() == limit;
        before = page.last().map_or(0, |event| event.seq);
        events.extend(page);
        if !full {
            break;
        }
    }
    events.reverse();
    events
}

fn seqs(events: &[Event]) -> Vec<u64> {
    events.iter().map(|event| event.seq).collect()
}

/// The family's history, and the parent's view of it as it happened live.
fn family_history() -> (SqliteStore, Family, AgentTree, ParentView) {
    let parent = AgentId::new();
    let family = Family::new(parent);
    let store = SqliteStore::open_in_memory().expect("store");
    let events = commit(&store, family.history());
    let tree = tree(agent(parent, "root"));
    // Today's pages: the parent's own events and all of its children's.
    let unfiltered: Vec<Event> = events
        .iter()
        .filter(|e| e.agent_id == Some(parent) || e.parent_agent_id == Some(parent))
        .cloned()
        .collect();
    assert!(
        unfiltered.len() < events.len(),
        "no page holds the grandchild"
    );
    assert_eq!(seqs(&paged(&store, parent, &[], 4)), seqs(&unfiltered));
    let live = parent_view(&ingest(&unfiltered, &tree, true), parent);
    // Grandchild events too, so the child-type guard sees every kind here.
    ingest(&events, &tree, true);
    (store, family, tree, live)
}

#[test]
fn filtered_history_pages_draw_the_parent_exactly_as_live_events_did() {
    let (store, family, tree, live) = family_history();
    let models: Vec<_> = live.0.iter().map(|cell| &cell.model).collect();
    let results = models
        .iter()
        .filter(|m| matches!(m, CellModel::ChildResult { .. }))
        .count();
    assert_eq!(results, 3, "one result per child run: {models:#?}");
    for drawn in [
        |m: &CellModel| matches!(m, CellModel::Spawned { objective, .. } if objective == "find the bug"),
        |m: &CellModel| matches!(m, CellModel::Sent { to, .. } if to == "renamed"),
        |m: &CellModel| matches!(m, CellModel::Waiting { .. }),
        |m: &CellModel| matches!(m, CellModel::Separator { cost: Some(_), .. }),
    ] {
        assert!(models.iter().any(|m| drawn(m)), "{models:#?}");
    }

    let types = transcript::PARENT_VISIBLE_CHILD_TYPES;
    let pages = paged(&store, family.parent, &types, 4);
    let unfiltered = paged(&store, family.parent, &[], 4);
    assert!(
        pages.len() < unfiltered.len(),
        "child tool chatter is left out"
    );
    let ascending = store
        .events(
            0,
            1_000,
            &aether_store::EventFilter {
                agent: Some(family.parent),
                child_types: types.map(String::from).into(),
                ..Default::default()
            },
        )
        .expect("ascending");
    assert_eq!(seqs(&pages), seqs(&ascending), "no page skips or repeats");
    assert_eq!(
        parent_view(&ingest(&pages, &tree, false), family.parent),
        live
    );
}

/// Each listed type draws something here, so the test above covers them all;
/// a type that draws but is missing trips `route_event`'s assertion.
#[test]
fn every_parent_visible_child_type_changes_what_the_parent_shows() {
    let (store, family, tree, live) = family_history();
    for left_out in transcript::PARENT_VISIBLE_CHILD_TYPES {
        let types: Vec<&str> = transcript::PARENT_VISIBLE_CHILD_TYPES
            .into_iter()
            .filter(|t| *t != left_out)
            .collect();
        let pages = paged(&store, family.parent, &types, 4);
        let shown = parent_view(&ingest(&pages, &tree, false), family.parent);
        assert_ne!(shown, live, "history without the child's {left_out} events");
    }
}

/// Real provider notices carry no agent. One stamped with a child's shows in
/// the parent without being a listed type, which the guard must refuse.
#[cfg(debug_assertions)]
#[test]
#[should_panic(expected = "changed its parent's transcript")]
fn a_child_event_type_that_draws_in_the_parent_must_be_listed() {
    let parent = AgentId::new();
    let child = AgentId::new();
    let tree = tree(agent(parent, "root"));
    let backoff = EventKind::ProviderBackoff {
        provider: "mock".into(),
        reason: "busy".into(),
        until: now(),
    };
    let events = [
        own(parent, EventKind::AgentStarted { turn: 1 }),
        of_child(parent, child, backoff),
    ];
    let events: Vec<Event> = events
        .into_iter()
        .enumerate()
        .map(|(n, draft)| Event::from_draft(draft, EventId::new(), n as u64 + 1, now()))
        .collect();
    ingest(&events, &tree, true);
}
