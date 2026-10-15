use super::*;
use aether_core::model::{ContentBlock, MessageRole};

fn deliver(app: &mut App, event: Event) {
    app.reduce(AppEvent::Frame(SubscriptionFrame::Event(Box::new(event))));
}

#[test]
fn backfill_preserves_live_delta_prefix() {
    let (mut app, id) = app_with_agent();
    app.transcripts.get_mut(&id).unwrap().loaded = false;
    app.select(id);
    let invocation = InvocationId::new();
    app.reduce(AppEvent::Frame(SubscriptionFrame::Delta {
        agent: id,
        invocation,
        delta: aether_protocol::Delta::Text {
            text: "current prefix".into(),
        },
    }));
    app.reduce(AppEvent::Reply {
        then: crate::testing::backfill_then(&app, id),
        result: Ok(Reply::Events(vec![event(
            1,
            id,
            EventKind::AgentMessage {
                role: MessageRole::Assistant,
                content: vec![ContentBlock::text("old answer")],
                invocation: None,
            },
        )])),
    });
    assert_eq!(
        app.transcripts[&id].stream.text, "current prefix",
        "historical assistant messages must not clear the in-flight invocation"
    );
}

#[test]
fn failed_backfill_can_retry_after_reconnect() {
    let (mut app, id) = app_with_agent();
    app.transcripts.get_mut(&id).unwrap().loaded = false;
    app.select(id);
    app.reduce(AppEvent::Reply {
        then: crate::testing::backfill_then(&app, id),
        result: Err(ClientError::Disconnected),
    });
    app.reduce(AppEvent::Disconnected { because: None });
    let effects = app.reduce(AppEvent::Connected);
    assert!(
        effects.iter().any(|e| matches!(
            e,
            Effect::Request {
                then: Then::Backfill { .. },
                ..
            }
        )),
        "failed history load permanently marked as loaded: {effects:?}"
    );
}

#[test]
fn parent_backfill_preserves_child_spawn_link() {
    let (mut app, parent) = app_with_agent();
    app.transcripts.get_mut(&parent).unwrap().loaded = false;
    app.select(parent);
    let child = AgentId::new();
    let mut created = event(
        1,
        child,
        EventKind::AgentCreated {
            name: "child".into(),
            role: Role::new("worker"),
            parent: Some(parent),
            model: ModelRef::new("mock", "m"),
            scope: Scope::default(),
            fallbacks: vec![],
        },
    );
    created.parent_agent_id = Some(parent);
    deliver(&mut app, created.clone());
    app.reduce(AppEvent::Reply {
        then: crate::testing::backfill_then(&app, parent),
        result: Ok(Reply::Events(vec![created])),
    });
    let mut task = event(
        2,
        child,
        EventKind::TaskCreated {
            objective: "review this code".into(),
            parent_task: None,
            dependencies: vec![],
            priority: aether_core::task::Priority::Normal,
        },
    );
    task.parent_agent_id = Some(parent);
    deliver(&mut app, task);
    assert!(app.transcripts[&parent].all_cells().any(|c| matches!(&c.model, CellModel::Spawned { objective, .. } if objective == "review this code")), "spawn link still targets old parent cell id: {:?}", app.transcripts[&parent].cells);
}

fn history_query(effect: Option<Effect>) -> EventQuery {
    match effect {
        Some(Effect::Request {
            command: Command::GetEvents { query, .. },
            then: Then::Backfill { .. },
        }) => query,
        other => panic!("a history request: {other:?}"),
    }
}

#[test]
fn history_pages_ask_only_for_child_events_the_transcript_shows() {
    let (mut app, id) = app_with_agent();
    app.transcripts.get_mut(&id).unwrap().loaded = false;
    let first = history_query(app.backfill_request(id));
    let shown: Vec<String> = crate::app::transcript::PARENT_VISIBLE_CHILD_TYPES
        .map(String::from)
        .into();
    assert_eq!(first.agent, Some(id));
    assert_eq!(first.child_types, shown);
    assert!(!shown.iter().any(|t| t.starts_with("tool.")), "{shown:?}");
    app.reduce(AppEvent::Reply {
        then: crate::testing::backfill_then(&app, id),
        result: Ok(Reply::Events(vec![event(7, id, EventKind::AgentReady)])),
    });
    app.transcripts.get_mut(&id).unwrap().older_available = true;
    let older = history_query(app.older_history_request(id));
    assert_eq!(older.before, Some(7));
    assert_eq!(
        older.child_types, shown,
        "older pages narrow children alike"
    );
}

#[test]
fn latest_window_deduplicates_live_overlap_and_stays_bounded() {
    let (mut app, id) = app_with_agent();
    app.transcripts.get_mut(&id).unwrap().loaded = false;
    let Some(Effect::Request {
        command:
            Command::GetEvents {
                after,
                limit,
                query,
            },
        ..
    }) = app.backfill_request(id)
    else {
        panic!("history request")
    };
    assert_eq!(after, 0);
    assert_eq!(query.before, Some(u64::MAX));
    assert_eq!(limit, 5_000);
    let invocation = InvocationId::new();
    let reply = |seq| answer(seq, id, invocation, &format!("reply {seq}"));
    deliver(&mut app, reply(10_000));
    let page = (1..=10_000)
        .rev()
        .filter(|seq| *seq < query.before.unwrap())
        .take(limit)
        .map(reply)
        .collect();
    let effects = finish(&mut app, id, page);
    assert!(effects.is_empty());
    let t = &app.transcripts[&id];
    assert_eq!(t.cells.len(), 5_000);
    assert!(t.loaded);
    assert!(t.older_available);
    assert!(crate::ui::bottom::footer::hints(&app).contains("earlier history trimmed"));
    assert_eq!(t.history.front().unwrap().seq, 5_001);
    assert_eq!(t.history.back().unwrap().seq, 10_000);
    assert!(
        matches!(&t.cells.last().unwrap().model, CellModel::Assistant { text } if text == "reply 10000")
    );
}

#[test]
fn archived_history_remains_readable_when_requested() {
    let (mut app, id) = app_with_agent();
    app.show_archived = true;
    app.tree.nodes.get_mut(&id).unwrap().agent.archived = true;
    app.transcripts.get_mut(&id).unwrap().loaded = false;
    app.select(id);
    app.reduce(AppEvent::Reply {
        then: crate::testing::backfill_then(&app, id),
        result: Ok(Reply::Events(vec![
            event(
                1,
                id,
                EventKind::AgentMessage {
                    role: MessageRole::Assistant,
                    content: vec![ContentBlock::text("archived answer")],
                    invocation: None,
                },
            ),
            event(2, id, EventKind::AgentArchived { archived: true }),
        ])),
    });
    assert!(
        app.transcripts[&id].all_cells().any(
            |c| matches!(&c.model, CellModel::Assistant { text } if text == "archived answer")
        ),
        "archive event erased history during replay"
    );
}

fn start(app: &mut App, id: AgentId) {
    app.transcripts.entry(id).or_default().loaded = false;
    app.select(id);
}

fn finish(app: &mut App, id: AgentId, history: Vec<Event>) -> Vec<Effect> {
    app.reduce(AppEvent::Reply {
        then: crate::testing::backfill_then(app, id),
        result: Ok(Reply::Events(history)),
    })
}

fn stream(app: &mut App, id: AgentId, invocation: InvocationId, delta: aether_protocol::Delta) {
    app.reduce(AppEvent::Frame(SubscriptionFrame::Delta {
        agent: id,
        invocation,
        delta,
    }));
}

fn text(app: &mut App, id: AgentId, invocation: InvocationId, value: &str) {
    stream(
        app,
        id,
        invocation,
        aether_protocol::Delta::Text { text: value.into() },
    );
}

fn answer(seq: u64, id: AgentId, invocation: InvocationId, text: &str) -> Event {
    event(
        seq,
        id,
        EventKind::AgentMessage {
            role: MessageRole::Assistant,
            content: vec![ContentBlock::text(text)],
            invocation: Some(invocation),
        },
    )
}

#[test]
fn historical_stops_do_not_consume_current_text_or_reasoning() {
    let (mut app, id) = app_with_agent();
    app.reduce(AppEvent::Subscribed {
        id: 1,
        last_seq: 10,
    });
    start(&mut app, id);
    let invocation = InvocationId::new();
    text(&mut app, id, invocation, "prefix");
    stream(
        &mut app,
        id,
        invocation,
        aether_protocol::Delta::Thinking {
            text: "**Planning** carefully".into(),
        },
    );
    let before = app.transcripts[&id].stream.clone();
    finish(
        &mut app,
        id,
        vec![
            answer(
                1,
                id,
                invocation,
                "earlier message in the same engine invocation",
            ),
            event(2, id, EventKind::AgentInterrupted { turn: 1 }),
            event(
                3,
                id,
                EventKind::InvocationFailed {
                    invocation: InvocationId::new(),
                    snapshot: aether_core::id::SnapshotId::new(),
                    error: "old failure".into(),
                    class: None,
                    retryable: false,
                    cancelled: false,
                    partial: vec![],
                },
            ),
            event(
                4,
                id,
                EventKind::AgentFailed {
                    error: "old agent failure".into(),
                },
            ),
        ],
    );
    let after = &app.transcripts[&id].stream;
    assert_eq!(after.text, before.text);
    assert_eq!(after.reasoning, before.reasoning);
    assert_eq!(after.reasoning_title, before.reasoning_title);
    assert_eq!(after.reasoning_since, before.reasoning_since);
    assert!(
        !models(&app)
            .iter()
            .any(|model| matches!(model, CellModel::Assistant { text } if text == "prefix"))
    );
    text(&mut app, id, invocation, " suffix");
    assert_eq!(app.transcripts[&id].stream.text, "prefix suffix");
    deliver(&mut app, answer(11, id, invocation, "prefix suffix"));
    assert!(app.transcripts[&id].stream.is_empty());
}

#[test]
fn stream_before_request_and_live_interruption_are_replayed_once() {
    let (mut app, id) = app_with_agent();
    app.reduce(AppEvent::Subscribed {
        id: 1,
        last_seq: 10,
    });
    let invocation = InvocationId::new();
    text(&mut app, id, invocation, "prefix");
    start(&mut app, id);
    text(&mut app, id, invocation, " suffix");
    let stopped = event(11, id, EventKind::AgentInterrupted { turn: 2 });
    deliver(&mut app, stopped.clone());
    finish(
        &mut app,
        id,
        vec![answer(1, id, invocation, "old"), stopped],
    );
    assert!(app.transcripts[&id].stream.is_empty());
    assert_eq!(
        models(&app)
            .iter()
            .filter(
                |model| matches!(model, CellModel::Assistant { text } if text == "prefix suffix")
            )
            .count(),
        1
    );
}

#[test]
fn overlapping_commit_then_next_message_keeps_only_next_stream() {
    let (mut app, id) = app_with_agent();
    start(&mut app, id);
    let invocation = InvocationId::new();
    text(&mut app, id, invocation, "first");
    let committed = answer(1, id, invocation, "first");
    deliver(&mut app, committed.clone());
    text(&mut app, id, invocation, "second prefix");
    finish(&mut app, id, vec![committed]);
    assert_eq!(app.transcripts[&id].stream.text, "second prefix");
    assert_eq!(
        models(&app),
        vec![CellModel::Assistant {
            text: "first".into()
        }]
    );
    text(&mut app, id, invocation, " suffix");
    deliver(&mut app, answer(2, id, invocation, "second prefix suffix"));
    assert!(app.transcripts[&id].stream.is_empty());
    assert_eq!(models(&app).len(), 2);
}

#[test]
fn unrelated_live_message_and_invocation_failure_keep_current_stream() {
    let (mut app, id) = app_with_agent();
    let invocation = InvocationId::new();
    text(&mut app, id, invocation, "current");
    deliver(
        &mut app,
        answer(1, id, InvocationId::new(), "another invocation"),
    );
    deliver(
        &mut app,
        event(
            2,
            id,
            EventKind::InvocationFailed {
                invocation: InvocationId::new(),
                snapshot: aether_core::id::SnapshotId::new(),
                error: "late failure".into(),
                class: None,
                retryable: false,
                cancelled: false,
                partial: vec![ContentBlock::text("other partial")],
            },
        ),
    );
    assert_eq!(app.transcripts[&id].stream.text, "current");
    deliver(&mut app, answer(3, id, invocation, "current"));
    assert!(app.transcripts[&id].stream.is_empty());
}

#[test]
fn unexpected_reply_keeps_live_cells_and_is_visible_and_retryable() {
    let (mut app, id) = app_with_agent();
    start(&mut app, id);
    let invocation = InvocationId::new();
    deliver(&mut app, answer(1, id, invocation, "live answer"));
    text(&mut app, id, invocation, "next prefix");
    let before = app.transcripts[&id]
        .clone()
        .cells
        .into_iter()
        .map(|c| c.model)
        .collect::<Vec<_>>();
    app.reduce(AppEvent::Reply {
        then: crate::testing::backfill_then(&app, id),
        result: Ok(Reply::Agents(vec![])),
    });
    assert!(!app.transcripts[&id].loaded);
    let mut expected = before;
    expected.push(CellModel::Notice {
        kind: crate::app::transcript::NoticeKind::ClientError,
        text: app.live_flash().expect("error flash").into(),
    });
    assert_eq!(models(&app), expected);
    assert_eq!(app.transcripts[&id].stream.text, "next prefix");
    assert!(app.live_flash().unwrap().contains("unexpected reply"));
    assert!(crate::testing::render_to(&app, 160, 30).contains("unexpected reply"));
    assert!(app.backfill_request(id).is_some());
    assert!(app.backfill_request(id).is_none());
}

#[test]
fn overflow_keeps_live_content_and_explains_retry() {
    let (mut app, id) = app_with_agent();
    start(&mut app, id);
    let invocation = InvocationId::new();
    deliver(&mut app, answer(1, id, invocation, "live answer"));
    for _ in 0..20_001 {
        text(&mut app, id, invocation, "x");
    }
    let before = app.transcripts[&id].clone();
    finish(
        &mut app,
        id,
        vec![answer(1, id, invocation, "historical answer")],
    );
    let mut expected = before;
    expected.client_error(app.now, app.live_flash().expect("error flash").into());
    assert_eq!(app.transcripts[&id], expected);
    assert!(!app.transcripts[&id].loaded);
    let notice = app.live_flash().unwrap();
    assert!(notice.contains("too much live activity"), "{notice}");
    assert!(notice.contains("reselect to retry"), "{notice}");
    assert!(crate::testing::render_to(&app, 160, 30).contains("reselect to retry"));
    assert!(app.backfill_request(id).is_some());
    finish(&mut app, id, vec![answer(1, id, invocation, "live answer")]);
    assert!(app.transcripts[&id].loaded);
}

#[test]
fn parent_replacement_rebinds_child_result_and_child_replay_preserves_it() {
    let (mut app, parent) = app_with_agent();
    start(&mut app, parent);
    let child = AgentId::new();
    let created = event(
        1,
        child,
        EventKind::AgentCreated {
            name: "child".into(),
            role: Role::new("worker"),
            parent: Some(parent),
            model: ModelRef::new("mock", "m"),
            scope: Scope::default(),
            fallbacks: vec![],
        },
    );
    deliver(&mut app, created.clone());
    let completed = event(
        2,
        child,
        EventKind::AgentCompleted {
            summary: Some("done".into()),
        },
    );
    deliver(&mut app, completed.clone());
    finish(&mut app, parent, vec![created.clone(), completed.clone()]);
    start(&mut app, child);
    finish(&mut app, child, vec![created, completed]);
    deliver(
        &mut app,
        event(
            3,
            child,
            EventKind::AgentFailed {
                error: "failure after completion".into(),
            },
        ),
    );
    let results: Vec<_> = app.transcripts[&parent]
        .cells
        .iter()
        .filter_map(|cell| match &cell.model {
            CellModel::ChildResult {
                agent,
                outcome,
                result,
                ..
            } if *agent == child => Some((outcome, result)),
            _ => None,
        })
        .collect();
    assert_eq!(results.len(), 1);
    assert!(
        matches!(results[0].0, transcript::ChildOutcome::Failed { error, .. } if error == "failure after completion")
    );
    assert_eq!(results[0].1, "done");
}

#[test]
fn concurrent_backfills_do_not_repeat_a_second_agents_existing_prefix() {
    let (mut app, first) = app_with_agent();
    start(&mut app, first);
    let second = AgentId::new();
    let invocation = InvocationId::new();
    text(&mut app, second, invocation, "prefix");
    start(&mut app, second);
    text(&mut app, second, invocation, " suffix");
    finish(&mut app, first, vec![]);
    finish(&mut app, second, vec![]);
    assert_eq!(app.transcripts[&second].stream.text, "prefix suffix");
    assert!(app.transcripts[&first].loaded);
    assert!(app.transcripts[&second].loaded);
}

#[test]
fn reconnect_retries_when_an_old_request_finishes_after_connected() {
    let (mut app, id) = app_with_agent();
    let invocation = InvocationId::new();
    text(&mut app, id, invocation, "cut off prefix");
    start(&mut app, id);
    let old_page = crate::testing::backfill_then(&app, id);
    app.reduce(AppEvent::Disconnected { because: None });
    assert!(app.transcripts[&id].stream.is_empty());
    app.reduce(AppEvent::Connected);
    let effects = app.reduce(AppEvent::Reply {
        then: old_page,
        result: Ok(Reply::Events(vec![answer(
            1,
            id,
            invocation,
            "stale answer",
        )])),
    });
    assert!(!app.transcripts[&id].loaded);
    assert!(app.transcripts[&id].stream.is_empty());
    assert!(models(&app).is_empty());
    assert!(effects.iter().any(|effect| matches!(effect, Effect::Request { then: Then::Backfill { agent, .. }, .. } if *agent == id)));
    finish(
        &mut app,
        id,
        vec![answer(1, id, invocation, "fresh answer")],
    );
    assert!(app.transcripts[&id].loaded);
    assert_eq!(
        models(&app),
        vec![CellModel::Assistant {
            text: "fresh answer".into()
        }]
    );
}

#[test]
fn live_archive_during_loading_still_evicts_until_explicitly_reselected() {
    let (mut app, id) = app_with_agent();
    start(&mut app, id);
    let invocation = InvocationId::new();
    let committed = answer(1, id, invocation, "archived answer");
    deliver(&mut app, committed.clone());
    let archived = event(2, id, EventKind::AgentArchived { archived: true });
    deliver(&mut app, archived.clone());
    assert!(!app.transcripts.contains_key(&id));
    finish(&mut app, id, vec![committed.clone(), archived.clone()]);
    assert!(!app.transcripts.contains_key(&id));
    app.show_archived = true;
    app.tree.nodes.get_mut(&id).unwrap().agent.archived = true;
    assert!(app.backfill_request(id).is_some());
    finish(&mut app, id, vec![committed, archived]);
    assert_eq!(
        models(&app),
        vec![CellModel::Assistant {
            text: "archived answer".into()
        }]
    );
}

#[test]
fn older_page_is_requested_only_on_demand_and_empty_page_keeps_window() {
    let (mut app, id) = app_with_agent();
    start(&mut app, id);
    let invocation = InvocationId::new();
    assert!(
        finish(
            &mut app,
            id,
            (1..=5_000)
                .rev()
                .map(|seq| answer(seq, id, invocation, "reply"))
                .collect()
        )
        .is_empty()
    );
    assert!(app.transcripts[&id].loaded);
    let effect = app.older_history_request(id).unwrap();
    assert!(matches!(
        effect,
        Effect::Request {
            command: Command::GetEvents {
                after: 0,
                query: EventQuery {
                    before: Some(1),
                    ..
                },
                ..
            },
            ..
        }
    ));
    assert!(app.older_history_request(id).is_none());
    finish(&mut app, id, vec![]);
    assert_eq!(app.transcripts[&id].cells.len(), 5_000);
    assert!(app.transcripts[&id].loaded);
    assert!(!app.transcripts[&id].older_available);
    assert!(app.older_history_request(id).is_none());
}

#[test]
fn older_page_failure_preserves_current_window_and_can_retry() {
    let (mut app, id) = app_with_agent();
    start(&mut app, id);
    let invocation = InvocationId::new();
    finish(
        &mut app,
        id,
        (5_001..=10_000)
            .rev()
            .map(|seq| answer(seq, id, invocation, "recent reply"))
            .collect(),
    );
    app.older_history_request(id).unwrap();
    app.reduce(AppEvent::Reply {
        then: crate::testing::backfill_then(&app, id),
        result: Err(ClientError::TimedOut {
            command: "GetEvents".into(),
            timeout: std::time::Duration::from_secs(30),
        }),
    });
    assert!(app.transcripts[&id].loaded);
    assert_eq!(app.transcripts[&id].history.front().unwrap().seq, 5_001);
    assert!(app.live_flash().unwrap().contains("did not answer"));
    assert!(app.older_history_request(id).is_some());
}

#[test]
fn evicted_history_reloads_and_live_events_do_not_jump_the_reading_window() {
    let (mut app, id) = app_with_agent();
    let invocation = InvocationId::new();
    for seq in 1..=5_001 {
        deliver(
            &mut app,
            answer(seq, id, invocation, &format!("reply {seq}")),
        );
    }
    let t = &app.transcripts[&id];
    assert_eq!(t.history.len(), 2_500);
    assert_eq!(t.cells.len(), 2_500);
    for (i, cell) in t.cells.iter().enumerate() {
        assert_eq!(t.position(cell.id), Some(i));
        assert_eq!(t.cell(cell.id), Some(cell));
    }
    assert!(matches!(
        app.older_history_request(id).unwrap(),
        Effect::Request {
            command: Command::GetEvents {
                query: EventQuery {
                    before: Some(2_502),
                    ..
                },
                ..
            },
            ..
        }
    ));
    finish(
        &mut app,
        id,
        (1..=2_501)
            .rev()
            .map(|seq| answer(seq, id, invocation, "older reply"))
            .collect(),
    );
    assert_eq!(app.transcripts[&id].cells.len(), 2_501);
    assert!(app.transcripts[&id].live.is_some());
    assert!(crate::ui::bottom::footer::hints(&app).contains("older history"));
    deliver(&mut app, answer(5_002, id, invocation, "new live reply"));
    assert_eq!(app.transcripts[&id].history.back().unwrap().seq, 2_501);
    assert_eq!(app.transcripts[&id].live_state().max_seq, 5_002);
    app.transcripts
        .get_mut(&id)
        .unwrap()
        .queue
        .push("queued while reading".into());
    deliver(
        &mut app,
        answer(5_003, id, invocation, "another live reply"),
    );
    assert_eq!(app.transcripts[&id].queue, ["queued while reading"]);
    app.reduce(AppEvent::Frame(SubscriptionFrame::Delta {
        agent: id,
        invocation,
        delta: aether_protocol::Delta::Text {
            text: "current prefix".into(),
        },
    }));
    assert!(app.transcripts[&id].stream.text.is_empty());
    assert_eq!(
        app.transcripts[&id].live_state().stream.text,
        "current prefix"
    );
    assert!(app.latest_history_request(id).is_none());
    assert!(app.transcripts[&id].live.is_none());
    assert_eq!(app.transcripts[&id].history.back().unwrap().seq, 5_003);
    assert_eq!(app.transcripts[&id].stream.text, "current prefix");
    assert_eq!(app.transcripts[&id].queue, ["queued while reading"]);
}

#[test]
fn trigger_backfill_sanitizes_payloads_and_deduplicates_live_delivery() {
    use crate::app::transcript::NoticeKind;
    use aether_core::inbox::{InboxItem, InboxKind};

    let (mut app, id) = app_with_agent();
    app.transcripts.get_mut(&id).unwrap().loaded = false;
    app.select(id);
    let at = chrono::DateTime::from_timestamp(1_700_000_000, 0).unwrap();
    let delivered = Event::from_draft(
        EventDraft::new(
            Actor::Scheduler,
            EventKind::InboxDelivered {
                item: InboxItem {
                    id: aether_core::id::InboxItemId::new(),
                    agent_id: id,
                    seq: 1,
                    kind: InboxKind::ExternalTrigger {
                        source: "ci\u{1b}]0;hidden title\u{7}".into(),
                        payload: serde_json::json!(format!(
                            "literal a → b · c {}",
                            "x".repeat(10_000)
                        )),
                        operator: false,
                    },
                    delivered_at: at,
                },
            },
        )
        .agent(id),
        EventId::new(),
        1,
        at,
    );
    deliver(&mut app, delivered.clone());
    let live = app.transcripts[&id]
        .cells
        .iter()
        .find(|cell| {
            matches!(
                cell.model,
                CellModel::Notice {
                    kind: NoticeKind::Trigger { .. },
                    ..
                }
            )
        })
        .unwrap()
        .model
        .clone();
    app.reduce(AppEvent::Reply {
        then: crate::testing::backfill_then(&app, id),
        result: Ok(Reply::Events(vec![delivered])),
    });
    assert!(app.transcripts[&id].loaded);
    let triggers: Vec<_> = app.transcripts[&id]
        .cells
        .iter()
        .filter(|cell| {
            matches!(
                cell.model,
                CellModel::Notice {
                    kind: NoticeKind::Trigger { .. },
                    ..
                }
            )
        })
        .collect();
    assert_eq!(triggers.len(), 1);
    assert_eq!(triggers[0].model, live);
    let CellModel::Notice { text, .. } = &live else {
        unreachable!()
    };
    assert!(text.starts_with("Trigger from ci · literal a → b · c "));
    assert!(text.len() <= 4_500);
    assert!(text.ends_with("[truncated]"));
}

#[test]
fn reading_history_keeps_new_questions_in_the_needs_you_strip() {
    let (mut app, id) = app_with_agent();
    start(&mut app, id);
    finish(
        &mut app,
        id,
        vec![answer(10, id, InvocationId::new(), "recent")],
    );
    app.transcripts.get_mut(&id).unwrap().older_available = true;
    app.older_history_request(id).unwrap();
    finish(
        &mut app,
        id,
        vec![answer(1, id, InvocationId::new(), "old")],
    );
    deliver(
        &mut app,
        event(
            11,
            id,
            EventKind::QuestionAsked {
                question: "new question".into(),
            },
        ),
    );
    assert!(app.needs_you().iter().any(|row| row.agent == id));
    assert!(app.reading_history());
    assert_eq!(app.transcripts[&id].history.back().unwrap().seq, 1);
}

#[test]
fn sending_a_message_returns_from_older_history_to_the_reply() {
    let (mut app, id) = app_with_agent();
    start(&mut app, id);
    finish(
        &mut app,
        id,
        vec![answer(10, id, InvocationId::new(), "recent")],
    );
    app.transcripts.get_mut(&id).unwrap().older_available = true;
    app.older_history_request(id).unwrap();
    finish(
        &mut app,
        id,
        vec![answer(1, id, InvocationId::new(), "old")],
    );
    assert!(app.reading_history());
    app.input.set_text("can you see this?");
    let effects = app.reduce(key(KeyCode::Enter));
    assert!(effects.iter().any(|effect| matches!(
        effect,
        Effect::Request {
            command: Command::SendMessage { to, .. },
            ..
        } if *to == id
    )));
    assert!(!app.reading_history());
    let last = app.scroll_log.since(0).last().unwrap();
    assert_eq!(
        (last.agent, last.intent),
        (id, crate::app::transcript::ScrollIntent::Latest)
    );
    deliver(&mut app, answer(11, id, InvocationId::new(), "yes"));
    let shown = &app.transcripts[&id];
    assert!(shown.live.is_none());
    assert!(
        shown
            .all_cells()
            .any(|c| matches!(&c.model, CellModel::Assistant { text } if text == "yes"))
    );
}

#[test]
fn a_busy_childs_events_do_not_trim_the_parents_own_replies() {
    let (mut app, parent) = app_with_agent();
    let child = AgentId::new();
    let of_child = |seq, kind| {
        let mut event = event(seq, child, kind);
        event.parent_agent_id = Some(parent);
        event
    };
    deliver(
        &mut app,
        answer(1, parent, InvocationId::new(), "parent reply"),
    );
    let spawned = of_child(
        2,
        EventKind::AgentCreated {
            name: "child".into(),
            role: Role::new("worker"),
            parent: Some(parent),
            model: ModelRef::new("mock", "m"),
            scope: Scope::default(),
            fallbacks: vec![],
        },
    );
    deliver(&mut app, spawned);
    let invocation = InvocationId::new();
    for seq in 3..=6_002 {
        let chatter = EventKind::AgentMessage {
            role: MessageRole::Assistant,
            content: vec![ContentBlock::text("child progress")],
            invocation: Some(invocation),
        };
        deliver(&mut app, of_child(seq, chatter));
    }
    let t = &app.transcripts[&parent];
    // The reply and the spawn cell mark the window; the chatter does not.
    assert_eq!(t.history.len(), 2);
    assert!(!t.older_available);
    assert!(
        t.all_cells()
            .any(|c| matches!(&c.model, CellModel::Assistant { text } if text == "parent reply"))
    );
    assert!(!crate::ui::bottom::footer::hints(&app).contains("earlier history trimmed"));
    assert!(app.transcripts[&child].older_available);
}

#[test]
fn old_daemon_ascending_cap_is_reported_instead_of_presented_as_recent_history() {
    let (mut app, id) = app_with_agent();
    start(&mut app, id);
    let invocation = InvocationId::new();
    let effects = finish(
        &mut app,
        id,
        (1..=5_000)
            .map(|seq| answer(seq, id, invocation, "oldest reply"))
            .collect(),
    );
    assert!(effects.is_empty());
    assert!(!app.transcripts[&id].loaded);
    assert!(
        app.live_flash()
            .unwrap()
            .contains("does not support recent history windows")
    );
    assert!(
        !app.transcripts[&id]
            .cells
            .iter()
            .any(|cell| matches!(cell.model, CellModel::Assistant { .. }))
    );
}

#[test]
fn latest_intent_supersedes_an_older_page_still_in_flight() {
    let (mut app, id) = app_with_agent();
    start(&mut app, id);
    finish(
        &mut app,
        id,
        vec![answer(10, id, InvocationId::new(), "recent")],
    );
    app.transcripts.get_mut(&id).unwrap().older_available = true;
    app.older_history_request(id).unwrap();
    assert!(app.latest_history_request(id).is_none());
    let effects = finish(
        &mut app,
        id,
        vec![answer(1, id, InvocationId::new(), "old")],
    );
    assert!(!app.reading_history());
    assert!(effects.is_empty());
    assert_eq!(app.transcripts[&id].history.back().unwrap().seq, 10);
}

#[test]
fn expired_older_history_is_not_offered_again() {
    let (mut app, id) = app_with_agent();
    start(&mut app, id);
    finish(
        &mut app,
        id,
        vec![answer(10, id, InvocationId::new(), "recent")],
    );
    app.transcripts.get_mut(&id).unwrap().older_available = true;
    app.older_history_request(id).unwrap();
    let effects = app.reduce(AppEvent::Reply {
        then: crate::testing::backfill_then(&app, id),
        result: Err(ClientError::Protocol(aether_protocol::ErrorReply::new(
            aether_protocol::ErrorCode::NotFound,
            aether_store::StoreError::HistoryExpired {
                first_seq: 1,
                last_seq: 9,
            }
            .to_string(),
        ))),
    });
    assert!(effects.is_empty());
    assert!(!app.transcripts[&id].older_available);
    assert!(app.older_history_request(id).is_none());
    assert!(!app.live_flash().unwrap().contains("retry"));
    assert_eq!(app.transcripts[&id].history.back().unwrap().seq, 10);
}

#[test]
fn eviction_keeps_old_pending_questions_in_the_needs_you_strip() {
    let (mut app, id) = app_with_agent();
    deliver(
        &mut app,
        event(
            1,
            id,
            EventKind::QuestionAsked {
                question: "still pending".into(),
            },
        ),
    );
    let invocation = InvocationId::new();
    for seq in 2..=5_002 {
        deliver(&mut app, answer(seq, id, invocation, "activity"));
    }
    assert!(app.needs_you().iter().any(|row| row.agent == id));
    assert!(app.transcripts[&id].older_available);
}
