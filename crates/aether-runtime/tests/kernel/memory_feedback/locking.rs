use super::*;

#[tokio::test]
async fn feedback_context_reads_are_unheld_and_failed_reads_are_owner_filtered() {
    for mode in ["membership", "snapshot", "failed"] {
        let store = Arc::new(CommitsMidPass::new(
            Arc::new(SqliteStore::open_in_memory().unwrap()),
            Hook::FeedbackContextRead,
        ));
        let (h, mut spec, agent) = fixture_on(Some(store.clone())).await;
        let snapshot = show(&h, agent, spec.task, vec![spec.memory], true);
        if mode != "membership" {
            spec.invocation = Some(call(&h, &snapshot, mode == "failed"));
        }
        let runtime = Arc::downgrade(&h.runtime);
        store.arm(Box::new(move |_| {
            let runtime = runtime.upgrade().unwrap();
            assert!(!runtime.commit_gate_held());
            assert!(runtime.store().last_seq()? > 0);
            Ok(())
        }));
        record(&h, &spec);
        assert!(store.fired());
        h.runtime.shutdown().await;
    }
}

#[tokio::test]
async fn feedback_held_replay_catches_a_receipt_committed_during_membership() {
    let store = Arc::new(CommitsMidPass::new(
        Arc::new(SqliteStore::open_in_memory().unwrap()),
        Hook::FeedbackContextRead,
    ));
    let (h, spec, agent) = fixture_on(Some(store.clone())).await;
    show(&h, agent, spec.task, vec![spec.memory], true);
    let key = submission(&spec);
    let runtime = Arc::downgrade(&h.runtime);
    let raced_spec = spec.clone();
    let raced_key = key.clone();
    store.arm(Box::new(move |_| {
        let runtime = runtime.upgrade().unwrap();
        assert!(!runtime.commit_gate_held());
        let first = runtime
            .record_memory_feedback_keyed(raced_spec, Actor::user(), Some(raced_key))
            .unwrap();
        assert!(first.retracted_at.is_none());
        Ok(())
    }));
    let found = h
        .runtime
        .record_memory_feedback_keyed(spec.clone(), Actor::user(), Some(key))
        .unwrap();
    assert!(store.fired());
    assert_eq!(
        h.store.memory_feedback(spec.memory, 100).unwrap(),
        vec![found]
    );
    let events = h.store.events(0, 100, &EventFilter::default()).unwrap();
    assert_eq!(
        events
            .iter()
            .filter(|event| matches!(&event.kind,
        EventKind::Custom { name, .. } if name == "memory.feedback"))
            .count(),
        1
    );
    h.runtime.shutdown().await;
}

#[tokio::test]
async fn feedback_rechecks_memory_and_task_existence_after_membership() {
    for table in ["memories", "tasks"] {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("feedback.db");
        let store = Arc::new(CommitsMidPass::new(
            Arc::new(SqliteStore::open(&path).unwrap()),
            Hook::FeedbackContextRead,
        ));
        let (h, spec, agent) = fixture_on(Some(store.clone())).await;
        show(&h, agent, spec.task, vec![spec.memory], true);
        let id = if table == "memories" {
            spec.memory.to_string()
        } else {
            spec.task.to_string()
        };
        store.arm(Box::new(move |_| {
            let conn = rusqlite::Connection::open(path).unwrap();
            assert_eq!(
                conn.execute(&format!("DELETE FROM {table} WHERE id = ?1"), [id])
                    .unwrap(),
                1
            );
            Ok(())
        }));
        let before = h.store.last_seq().unwrap();
        assert!(matches!(
            h.runtime.record_memory_feedback(spec, Actor::user()),
            Err(aether_runtime::RuntimeError::NotFound(_))
        ));
        assert!(store.fired());
        assert_eq!(h.store.last_seq().unwrap(), before);
        h.runtime.shutdown().await;
    }
}

#[tokio::test]
async fn feedback_keyed_replay_precedes_context_reads_after_task_deletion() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("feedback.db");
    let store = Arc::new(CommitsMidPass::new(
        Arc::new(SqliteStore::open(&path).unwrap()),
        Hook::FeedbackContextRead,
    ));
    let (h, spec, agent) = fixture_on(Some(store.clone())).await;
    show(&h, agent, spec.task, vec![spec.memory], true);
    let key = submission(&spec);
    let first = h
        .runtime
        .record_memory_feedback_keyed(spec.clone(), Actor::user(), Some(key.clone()))
        .unwrap();
    let retracted = h
        .runtime
        .retract_memory_feedback(first.id, "Withdrawn".into(), Actor::user())
        .unwrap();
    let conn = rusqlite::Connection::open(path).unwrap();
    assert_eq!(
        conn.execute("DELETE FROM tasks WHERE id = ?1", [spec.task.to_string()])
            .unwrap(),
        1
    );
    store.arm(Box::new(|_| {
        panic!("a retry must replay before reading context")
    }));
    let before = h.store.last_seq().unwrap();
    assert_eq!(
        h.runtime
            .record_memory_feedback_keyed(spec, Actor::user(), Some(key))
            .unwrap(),
        first
    );
    assert!(!store.fired());
    assert_eq!(h.store.last_seq().unwrap(), before);
    assert_eq!(
        h.store.memory_feedback_record(first.id).unwrap(),
        Some(retracted)
    );
    h.runtime.shutdown().await;
}
