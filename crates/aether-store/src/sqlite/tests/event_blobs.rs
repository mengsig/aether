use super::*;

fn big_output(seed: &str) -> String {
    (0..400)
        .map(|line| format!("{seed} line {line:04} of build output\n"))
        .collect()
}

fn succeeded(agent: AgentId, call: ToolCallId, output: &str) -> EventDraft {
    EventDraft::new(
        Actor::System,
        EventKind::ToolSucceeded {
            call,
            outcome: ToolOutcome::ok(output),
        },
    )
    .agent(agent)
}

fn requested(agent: AgentId, call: ToolCallId) -> EventDraft {
    let call = aether_core::tool::ToolCall {
        id: call,
        provider_call_id: "toolu_1".into(),
        name: "bash".into(),
        input: serde_json::json!({"command": "make"}),
        agent_id: agent,
        task_id: None,
        invocation_id: None,
        resources: Vec::new(),
        idempotency_key: None,
        requested_at: now(),
    };
    EventDraft::new(
        Actor::System,
        EventKind::ToolRequested {
            call,
            side_effect: aether_core::tool::SideEffectClass::ReadOnly,
        },
    )
    .agent(agent)
}

fn stored_body(store: &SqliteStore, seq: u64) -> String {
    store
        .conn
        .lock()
        .query_row(
            "SELECT body FROM events WHERE seq = ?1",
            [seq as i64],
            |row| row.get(0),
        )
        .expect("stored body")
}

fn count(store: &SqliteStore, table: &str) -> i64 {
    store
        .conn
        .lock()
        .query_row(&format!("SELECT COUNT(*) FROM {table}"), [], |row| {
            row.get(0)
        })
        .expect("count")
}

fn output_of(event: &Event) -> &str {
    match &event.kind {
        EventKind::ToolSucceeded { outcome, .. } => &outcome.content,
        other => panic!("not a tool result: {other:?}"),
    }
}

#[test]
fn a_large_tool_output_leaves_the_row_and_every_read_returns_it() {
    let store = SqliteStore::open_in_memory().expect("store");
    let (agent, call) = (AgentId::new(), ToolCallId::new());
    let output = big_output("make");
    let committed = store
        .commit(
            Commit::new()
                .event(requested(agent, call))
                .event(succeeded(agent, call, &output)),
        )
        .expect("commit");
    let event = &committed[1];
    assert_eq!(output_of(event), output, "the caller gets the full event");
    let body = stored_body(&store, event.seq);
    assert!(body.len() < 1024, "the row keeps a reference: {body}");
    assert_eq!(count(&store, "event_blobs"), 1);

    let filter = EventFilter::for_agent(agent);
    let reads = [
        store.events(1, 10, &filter).expect("events"),
        store.latest_events(1, &filter).expect("latest"),
        store
            .events_before(event.seq + 1, 1, &filter)
            .expect("before"),
        vec![store.event_by_id(event.id).expect("by id").expect("found")],
        vec![
            store
                .event_by_seq(event.seq)
                .expect("by seq")
                .expect("found"),
        ],
        store
            .tool_events(agent, crate::ToolEventScope::Calls(&[call]))
            .expect("tool events")
            .into_iter()
            .take(1)
            .collect(),
    ];
    for read in reads {
        assert_eq!(read.len(), 1);
        assert_eq!(&read[0], event, "the stored reference is resolved");
    }
}

#[test]
fn identical_outputs_share_one_blob_until_the_last_reference_goes() {
    let store = SqliteStore::open_in_memory().expect("store");
    let agent = AgentId::new();
    let output = big_output("same");
    for _ in 0..3 {
        store
            .commit(Commit::new().event(succeeded(agent, ToolCallId::new(), &output)))
            .expect("commit");
    }
    assert_eq!(count(&store, "event_blobs"), 1, "stored once");
    assert_eq!(count(&store, "event_blob_refs"), 3);

    let conn = store.conn.lock();
    conn.execute("DELETE FROM events WHERE seq IN (1, 2)", [])
        .expect("drop two");
    drop(conn);
    assert_eq!(count(&store, "event_blob_refs"), 1);
    assert_eq!(count(&store, "event_blobs"), 1, "still referenced");
    assert_eq!(output_of(&store.event_by_seq(3).unwrap().unwrap()), output);
    store
        .conn
        .lock()
        .execute("DELETE FROM events WHERE seq = 3", [])
        .expect("drop the last");
    assert_eq!(count(&store, "event_blob_refs"), 0);
    assert_eq!(
        count(&store, "event_blobs"),
        0,
        "collected with its last event"
    );
}

#[test]
fn content_that_looks_like_a_reference_is_kept_verbatim() {
    let store = SqliteStore::open_in_memory().expect("store");
    let agent = AgentId::new();
    let forged = format!("\u{1}aether-blob:{}", "0".repeat(64));
    let committed = store
        .commit(Commit::new().event(succeeded(agent, ToolCallId::new(), &forged)))
        .expect("commit");
    let body = stored_body(&store, committed[0].seq);
    assert!(
        !body.contains(&"0".repeat(64)),
        "moved behind a real digest"
    );
    let read = store.event_by_seq(committed[0].seq).unwrap().unwrap();
    assert_eq!(output_of(&read), forged, "never resolved as a reference");
}

#[test]
fn a_missing_or_damaged_blob_is_reported_not_returned_empty() {
    let store = SqliteStore::open_in_memory().expect("store");
    let agent = AgentId::new();
    let committed = store
        .commit(Commit::new().event(succeeded(agent, ToolCallId::new(), &big_output("x"))))
        .expect("commit");
    let seq = committed[0].seq;
    store
        .conn
        .lock()
        .execute("UPDATE event_blobs SET size = size + 1", [])
        .expect("damage");
    assert!(matches!(
        store.event_by_seq(seq),
        Err(StoreError::Corrupt(_))
    ));
    store
        .conn
        .lock()
        .execute_batch("DROP TRIGGER event_blob_refs_collect; DELETE FROM event_blobs;")
        .expect("lose the blob");
    let read = store.events(0, 10, &EventFilter::for_agent(agent));
    assert!(matches!(read, Err(StoreError::Corrupt(message)) if message.contains("missing")));
}

#[test]
fn writes_are_counted_by_kind_and_a_rollback_counts_nothing() {
    let store = SqliteStore::open_in_memory().expect("store");
    let original = agent();
    store
        .commit(
            Commit::new()
                .event(succeeded(original.id, ToolCallId::new(), "short"))
                .event(succeeded(original.id, ToolCallId::new(), "short"))
                .mutate(Mutation::UpsertAgent(original.clone())),
        )
        .expect("commit");
    let stats = store.write_stats();
    let kind = |name: &str| {
        stats
            .kinds
            .iter()
            .find(|kind| kind.kind == name)
            .cloned()
            .unwrap_or_else(|| panic!("{name} counted: {stats:?}"))
    };
    assert_eq!(kind("tool.succeeded").rows, 2);
    assert!(kind("tool.succeeded").bytes > 0);
    assert_eq!(kind("table:agents").rows, 1);
    assert_eq!(
        stats.total_bytes,
        stats.kinds.iter().map(|kind| kind.bytes).sum::<u64>()
    );

    let failed = store.commit(
        Commit::new()
            .event(succeeded(original.id, ToolCallId::new(), "lost"))
            .mutate(Mutation::DeliverInbox(inbox_item(original.id, 5))),
    );
    assert!(matches!(failed, Err(StoreError::InboxSequence { .. })));
    assert_eq!(store.write_stats().kinds, stats.kinds, "nothing committed");
}
