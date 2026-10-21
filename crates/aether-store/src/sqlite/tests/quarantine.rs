use super::*;

struct Record {
    table: &'static str,
    key: &'static str,
    id: String,
    body: String,
}

fn seed(store: &SqliteStore) -> Vec<Record> {
    let agent = agent();
    let task = Task::new("task", Actor::System, agent.run, now());
    let lease = Lease::new(
        agent.id,
        "holder",
        1,
        now(),
        std::time::Duration::from_secs(60),
    );
    store
        .commit(
            Commit::new()
                .mutate(Mutation::UpsertAgent(agent.clone()))
                .mutate(Mutation::UpsertTask(task.clone()))
                .mutate(Mutation::SetLease(lease.clone())),
        )
        .unwrap();
    vec![
        Record {
            table: "agents",
            key: "id",
            id: agent.id.to_string(),
            body: serde_json::to_string(&agent).unwrap(),
        },
        Record {
            table: "tasks",
            key: "id",
            id: task.id.to_string(),
            body: serde_json::to_string(&task).unwrap(),
        },
        Record {
            table: "leases",
            key: "agent_id",
            id: lease.agent_id.to_string(),
            body: serde_json::to_string(&lease).unwrap(),
        },
    ]
}

impl Record {
    fn replace_body(&self, store: &SqliteStore, body: &dyn rusqlite::ToSql) {
        let changed = store
            .conn
            .lock()
            .execute(
                &format!("UPDATE {} SET body=?1 WHERE {}=?2", self.table, self.key),
                params![body, self.id],
            )
            .unwrap();
        assert_eq!(changed, 1);
    }

    fn raw_body(&self, store: &SqliteStore) -> rusqlite::types::Value {
        store
            .read_conn()
            .query_row(
                &format!("SELECT body FROM {} WHERE {}=?1", self.table, self.key),
                [&self.id],
                |row| row.get(0),
            )
            .unwrap()
    }

    fn read(&self, store: &SqliteStore) -> Result<bool> {
        match self.table {
            "agents" => store
                .agent(self.id.parse().unwrap())
                .map(|row| row.is_some()),
            "tasks" => store
                .task(self.id.parse().unwrap())
                .map(|row| row.is_some()),
            "leases" => store
                .lease(self.id.parse().unwrap())
                .map(|row| row.is_some()),
            _ => unreachable!(),
        }
    }

    fn list(&self, store: &SqliteStore) -> Vec<String> {
        match self.table {
            "agents" => store
                .agents(&AgentFilter::default())
                .unwrap()
                .into_iter()
                .map(|a| a.id.to_string())
                .collect(),
            "tasks" => store
                .tasks(&TaskFilter::default())
                .unwrap()
                .into_iter()
                .map(|t| t.id.to_string())
                .collect(),
            "leases" => store
                .leases()
                .unwrap()
                .into_iter()
                .map(|l| l.agent_id.to_string())
                .collect(),
            _ => unreachable!(),
        }
    }
}

#[test]
fn unidentifiable_quarantines_are_not_assumed_to_have_disappeared() {
    let store = SqliteStore::open_in_memory().unwrap();
    for key in [
        "quarantine.agents.unreadable-key",
        "quarantine.unknown.record",
    ] {
        store
            .commit(Commit::new().mutate(Mutation::SetMeta {
                key: key.into(),
                value: "{broken".into(),
            }))
            .unwrap();
    }
    let notices = store.quarantines().unwrap();
    assert_eq!(notices.len(), 2);
    assert!(
        notices
            .iter()
            .all(|notice| notice.reason.contains("unreadable"))
    );
}

#[test]
fn deleting_a_quarantined_record_removes_its_notice() {
    let store = SqliteStore::open_in_memory().unwrap();
    for record in seed(&store) {
        record.replace_body(&store, &"{broken");
        assert!(record.read(&store).is_err());
        assert_eq!(store.quarantines().unwrap().len(), 1);
        assert_eq!(
            store
                .conn
                .lock()
                .execute(
                    &format!("DELETE FROM {} WHERE {}=?1", record.table, record.key),
                    [&record.id],
                )
                .unwrap(),
            1
        );
        assert!(store.quarantines().unwrap().is_empty());
        assert!(
            store
                .meta(&format!("quarantine.{}.{}", record.table, record.id))
                .unwrap()
                .is_none()
        );
    }
}

#[test]
fn malformed_records_keep_healthy_siblings_and_preserve_the_raw_body() {
    let store = SqliteStore::open_in_memory().unwrap();
    let damaged = seed(&store);
    let healthy = seed(&store);
    for (bad, good) in damaged.iter().zip(&healthy) {
        for body in [
            rusqlite::types::Value::Text("{broken".into()),
            rusqlite::types::Value::Text("{}".into()),
            rusqlite::types::Value::Blob(vec![0xff, 0x00]),
        ] {
            bad.replace_body(&store, &body);
            assert_eq!(bad.list(&store), vec![good.id.clone()]);
            assert_eq!(bad.raw_body(&store), body);
            let error = bad.read(&store).unwrap_err();
            assert!(matches!(error, StoreError::Corrupt(_)));
            let message = error.to_string();
            for expected in [bad.table, &bad.id, "aether doctor", "aether diagnose"] {
                assert!(message.contains(expected), "{message}");
            }
            let notices = store.quarantines().unwrap();
            assert!(
                notices
                    .iter()
                    .any(|n| n.table == bad.table && n.id == bad.id)
            );
            assert!(good.read(&store).unwrap());
        }
    }
    assert_eq!(store.quarantines().unwrap().len(), 3);
}

#[test]
fn unknown_statuses_are_quarantined_even_in_budgeted_agent_lists() {
    let store = SqliteStore::open_in_memory().unwrap();
    let records = seed(&store);
    for record in records.iter().filter(|r| r.table != "leases") {
        let mut body: serde_json::Value = serde_json::from_str(&record.body).unwrap();
        body["status"] = serde_json::json!({"state": "future_state"});
        record.replace_body(&store, &body.to_string());
        assert!(record.list(&store).is_empty());
        assert!(matches!(record.read(&store), Err(StoreError::Corrupt(_))));
    }
    records[0].replace_body(&store, &"{broken");
    assert!(
        store
            .agents(&AgentFilter {
                budgeted_only: true,
                ..Default::default()
            })
            .unwrap()
            .is_empty()
    );
    assert_eq!(store.quarantines().unwrap().len(), 2);
}

#[test]
fn both_read_paths_reject_mismatched_ids_and_clear_notices_after_repair() {
    let store = SqliteStore::open_in_memory().unwrap();
    let damaged = seed(&store);
    let healthy = seed(&store);
    for (bad, good) in damaged.iter().zip(&healthy) {
        bad.replace_body(&store, &good.body);
        assert!(matches!(bad.read(&store), Err(StoreError::Corrupt(_))));
        assert_eq!(bad.list(&store), vec![good.id.clone()]);
        assert_eq!(
            bad.raw_body(&store),
            rusqlite::types::Value::Text(good.body.clone())
        );
        bad.replace_body(&store, &bad.body);
        assert!(bad.read(&store).unwrap());
        assert!(store.quarantines().unwrap().is_empty());
        bad.replace_body(&store, &"{broken");
        assert_eq!(bad.list(&store), vec![good.id.clone()]);
        bad.replace_body(&store, &bad.body);
        assert_eq!(bad.list(&store).len(), 2);
        assert!(store.quarantines().unwrap().is_empty());
    }
}

#[test]
fn notices_and_raw_rows_survive_reopening_a_file_store() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("store.db");
    let store = SqliteStore::open(&path).unwrap();
    let records = seed(&store);
    for record in &records {
        record.replace_body(&store, &"{broken");
        assert!(record.list(&store).is_empty());
    }
    let notices = store.quarantines().unwrap();
    assert_eq!(notices.len(), 3);
    drop(store);
    let reopened = SqliteStore::open(&path).unwrap();
    assert_eq!(reopened.quarantines().unwrap(), notices);
    for record in &records {
        assert_eq!(
            record.raw_body(&reopened),
            rusqlite::types::Value::Text("{broken".into())
        );
        record.replace_body(&reopened, &record.body);
        assert_eq!(record.list(&reopened), vec![record.id.clone()]);
    }
    drop(reopened);
    assert!(
        SqliteStore::open(&path)
            .unwrap()
            .quarantines()
            .unwrap()
            .is_empty()
    );
}

#[test]
fn corrupt_event_history_fails_instead_of_silently_dropping_events() {
    let store = SqliteStore::open_in_memory().unwrap();
    let damaged = AgentId::new();
    let healthy = AgentId::new();
    let events = store
        .commit(
            Commit::new()
                .event(EventDraft::new(Actor::System, EventKind::AgentReady).agent(damaged))
                .event(EventDraft::new(Actor::System, EventKind::AgentReady).agent(damaged))
                .event(EventDraft::new(Actor::System, EventKind::AgentReady).agent(healthy)),
        )
        .unwrap();
    store
        .conn
        .lock()
        .execute(
            "UPDATE events SET body=?1 WHERE seq=?2",
            params!["{broken", events[0].seq as i64],
        )
        .unwrap();
    assert!(
        store
            .events(0, 100, &EventFilter::own_events(damaged))
            .is_err()
    );
    assert!(
        store
            .events_before(u64::MAX, 100, &EventFilter::own_events(damaged))
            .is_err()
    );
    assert!(store.event_by_id(events[0].id).is_err());
    assert!(store.event_by_seq(events[0].seq).is_err());
    assert_eq!(
        store
            .events(0, 100, &EventFilter::own_events(healthy))
            .unwrap(),
        vec![events[2].clone()]
    );
    let body: String = store
        .read_conn()
        .query_row(
            "SELECT body FROM events WHERE seq=?1",
            [events[0].seq as i64],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(body, "{broken");
}

#[test]
fn fleet_usage_and_repository_queries_skip_corrupt_agents() {
    let store = SqliteStore::open_in_memory().unwrap();
    let damaged = seed(&store);
    let mut healthy = agent();
    healthy.stats.usage.input_tokens = 42;
    healthy.config.token_budget = Some(100);
    healthy.scope.repository = Some("example/repo".into());
    store
        .commit(Commit::new().mutate(Mutation::UpsertAgent(healthy.clone())))
        .unwrap();
    for body in [
        "{broken".to_owned(),
        serde_json::to_string(&healthy).unwrap(),
    ] {
        damaged[0].replace_body(&store, &body);
        assert_eq!(store.fleet_usage().unwrap(), healthy.stats.usage);
        assert!(store.any_agent_in_repository("example/repo").unwrap());
        assert!(!store.any_agent_in_repository("missing").unwrap());
        let budgeted = store
            .agents(&AgentFilter {
                budgeted_only: true,
                ..Default::default()
            })
            .unwrap();
        assert_eq!(budgeted, vec![healthy.clone()]);
        assert_eq!(store.quarantines().unwrap().len(), 1);
        assert_eq!(
            damaged[0].raw_body(&store),
            rusqlite::types::Value::Text(body)
        );
    }
}

#[test]
fn invalid_utf8_text_is_quarantined_without_rewriting_its_bytes() {
    let store = SqliteStore::open_in_memory().unwrap();
    for record in seed(&store) {
        store
            .conn
            .lock()
            .execute(
                &format!(
                    "UPDATE {} SET body=CAST(x'ff00' AS TEXT) WHERE {}=?1",
                    record.table, record.key
                ),
                [&record.id],
            )
            .unwrap();
        assert!(record.list(&store).is_empty());
        assert!(matches!(record.read(&store), Err(StoreError::Corrupt(_))));
        let bytes: Vec<u8> = store
            .read_conn()
            .query_row(
                &format!(
                    "SELECT CAST(body AS BLOB) FROM {} WHERE {}=?1",
                    record.table, record.key
                ),
                [&record.id],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(bytes, vec![0xff, 0]);
    }
    assert_eq!(store.quarantines().unwrap().len(), 3);
}

#[test]
fn unknown_row_keys_do_not_hide_healthy_siblings() {
    let store = SqliteStore::open_in_memory().unwrap();
    let damaged = seed(&store);
    let healthy = seed(&store);
    for (bad, good) in damaged.iter().zip(&healthy) {
        store
            .conn
            .lock()
            .execute(
                &format!(
                    "UPDATE {} SET {}='unknown-id' WHERE {}=?1",
                    bad.table, bad.key, bad.key
                ),
                [&bad.id],
            )
            .unwrap();
        assert_eq!(bad.list(&store), vec![good.id.clone()]);
        assert!(
            store
                .quarantines()
                .unwrap()
                .iter()
                .any(|n| n.table == bad.table && n.id == "unknown-id")
        );
        assert!(!bad.read(&store).unwrap());
    }
}

#[test]
fn quarantine_persistence_failure_is_returned_to_the_caller() {
    let store = SqliteStore::open_in_memory().unwrap();
    let records = seed(&store);
    records[0].replace_body(&store, &"{broken");
    store
        .conn
        .lock()
        .execute_batch(
            "CREATE TRIGGER refuse_quarantine BEFORE INSERT ON meta
         WHEN NEW.key LIKE 'quarantine.%'
         BEGIN SELECT RAISE(FAIL, 'quarantine unavailable'); END;",
        )
        .unwrap();
    let error = store.agents(&AgentFilter::default()).unwrap_err();
    assert!(error.to_string().contains("quarantine unavailable"));
    assert!(store.quarantines().unwrap().is_empty());
    assert_eq!(
        records[0].raw_body(&store),
        rusqlite::types::Value::Text("{broken".into())
    );
}

#[test]
fn event_envelope_must_match_its_indexed_identity() {
    let store = SqliteStore::open_in_memory().unwrap();
    let owner = AgentId::new();
    let event = store
        .commit(
            Commit::new().event(EventDraft::new(Actor::System, EventKind::AgentReady).agent(owner)),
        )
        .unwrap()
        .remove(0);
    let original = serde_json::to_value(&event).unwrap();
    for (key, value) in [
        ("seq", serde_json::json!(event.seq + 100)),
        ("id", serde_json::json!(aether_core::id::EventId::new())),
        ("agent_id", serde_json::json!(AgentId::new())),
        (
            "kind",
            serde_json::to_value(EventKind::AgentStarted { turn: 1 }).unwrap(),
        ),
        ("parent_agent_id", serde_json::json!(AgentId::new())),
        ("run", serde_json::json!(aether_core::id::RunId::new())),
    ] {
        let mut body = original.clone();
        body[key] = value;
        assert_eq!(
            store
                .conn
                .lock()
                .execute(
                    "UPDATE events SET body=?1 WHERE seq=?2",
                    params![body.to_string(), event.seq as i64]
                )
                .unwrap(),
            1
        );
        let error = store
            .events(0, 100, &EventFilter::own_events(owner))
            .unwrap_err();
        assert!(error.to_string().contains("aether doctor"), "{error}");
        assert!(
            store
                .latest_events(1, &EventFilter::own_events(owner))
                .is_err()
        );
    }
}

#[test]
fn damaged_quarantine_notices_do_not_poison_record_lists() {
    let store = SqliteStore::open_in_memory().unwrap();
    let records = seed(&store);
    records[0].replace_body(&store, &"{");
    assert!(records[0].list(&store).is_empty());
    assert_eq!(
        store
            .conn
            .lock()
            .execute(
                "UPDATE meta SET value='{' WHERE key LIKE 'quarantine.%'",
                []
            )
            .unwrap(),
        1
    );
    assert!(
        store.quarantines().unwrap()[0]
            .reason
            .contains("aether doctor")
    );
    records[0].replace_body(&store, &records[0].body);
    assert_eq!(records[0].list(&store), vec![records[0].id.clone()]);
    assert!(store.quarantines().unwrap().is_empty());
}
