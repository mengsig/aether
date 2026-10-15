use super::*;

#[tokio::test]
async fn feedback_uses_the_event_memory_list_and_only_falls_back_for_legacy_layers() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("feedback.db");
    let (h, spec, agent) = fixture_on(Some(Arc::new(SqliteStore::open(&path).unwrap()))).await;
    let snapshot = show(&h, agent, spec.task, vec![spec.memory], true);
    let conn = rusqlite::Connection::open(path).unwrap();
    assert_eq!(
        conn.execute(
            "UPDATE snapshots SET body = json_set(body, '$.memory_ids', json('[]')) WHERE id = ?1",
            [snapshot.id.to_string()]
        )
        .unwrap(),
        1
    );
    record(&h, &spec);
    assert_eq!(
        conn.execute(
            "UPDATE snapshots SET body = json_set(body, '$.memory_ids', json(?1)) WHERE id = ?2",
            rusqlite::params![
                serde_json::to_string(&vec![spec.memory]).unwrap(),
                snapshot.id.to_string()
            ]
        )
        .unwrap(),
        1
    );
    assert_eq!(conn.execute("UPDATE events SET body = json_remove(body, '$.kind.layers[0].memories') WHERE type = 'snapshot.created'", []).unwrap(), 1);
    record(&h, &spec);
    assert_eq!(conn.execute("UPDATE events SET body = json_set(body, '$.kind.layers[0].items', 0) WHERE type = 'snapshot.created'", []).unwrap(), 1);
    let error = h
        .runtime
        .record_memory_feedback(spec.clone(), Actor::user())
        .unwrap_err();
    assert!(error.to_string().contains("was not in context"));
    assert_eq!(h.store.memory_feedback(spec.memory, 100).unwrap().len(), 2);
    h.runtime.shutdown().await;
}

#[tokio::test]
async fn feedback_legacy_event_reads_the_stored_snapshot_memory_ids() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("feedback.db");
    let (h, spec, agent) = fixture_on(Some(Arc::new(SqliteStore::open(&path).unwrap()))).await;
    show(&h, agent, spec.task, vec![spec.memory], true);
    let conn = rusqlite::Connection::open(path).unwrap();
    assert_eq!(conn.execute("UPDATE events SET body = json_remove(body, '$.kind.layers[0].memories') WHERE type = 'snapshot.created'", []).unwrap(), 1);
    let feedback = record(&h, &spec);
    assert_eq!(feedback.spec.memory, spec.memory);
    assert_eq!(h.store.memory_feedback(spec.memory, 100).unwrap().len(), 1);
    h.runtime.shutdown().await;
}

#[tokio::test]
async fn feedback_refuses_announced_but_unstored_snapshots() {
    let (h, spec, agent) = fixture().await;
    let mut snapshot = show(&h, agent, spec.task, vec![spec.memory], false);
    snapshot.id = SnapshotId::new();
    h.store
        .commit(Commit::new().event(announcement(&snapshot)))
        .unwrap();
    assert!(h.store.snapshot(snapshot.id).unwrap().is_none());
    let before = h.store.last_seq().unwrap();
    let error = h
        .runtime
        .record_memory_feedback(spec, Actor::user())
        .unwrap_err();
    assert!(error.to_string().contains("was not in context"));
    assert_eq!(h.store.last_seq().unwrap(), before);
    h.runtime.shutdown().await;
}

#[tokio::test]
async fn feedback_retraction_after_task_deletion_has_no_task_or_run_envelope() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("feedback.db");
    let (h, spec, agent) = fixture_on(Some(Arc::new(SqliteStore::open(&path).unwrap()))).await;
    show(&h, agent, spec.task, vec![spec.memory], true);
    let feedback = record(&h, &spec);
    let conn = rusqlite::Connection::open(path).unwrap();
    assert_eq!(
        conn.execute("DELETE FROM tasks WHERE id = ?1", [spec.task.to_string()])
            .unwrap(),
        1
    );
    assert!(h.store.task(spec.task).unwrap().is_none());
    let retracted = h
        .runtime
        .retract_memory_feedback(feedback.id, "History expired".into(), Actor::user())
        .unwrap();
    assert!(retracted.retracted_at.is_some());
    let event = h
        .store
        .event_by_seq(h.store.last_seq().unwrap())
        .unwrap()
        .unwrap();
    assert_eq!(event.task_id, None);
    assert_eq!(event.run, None);
    h.runtime.shutdown().await;
}

#[tokio::test]
async fn feedback_does_not_fall_back_when_the_event_has_an_empty_memories_key() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("feedback.db");
    let (h, spec, agent) = fixture_on(Some(Arc::new(SqliteStore::open(&path).unwrap()))).await;
    show(&h, agent, spec.task, vec![spec.memory], true);
    let conn = rusqlite::Connection::open(path).unwrap();
    assert_eq!(conn.execute("UPDATE events SET body = json_set(body, '$.kind.layers[0].memories', json('[]')) WHERE type = 'snapshot.created'", []).unwrap(), 1);
    let before = h.store.last_seq().unwrap();
    let error = h
        .runtime
        .record_memory_feedback(spec.clone(), Actor::user())
        .unwrap_err();
    assert!(error.to_string().contains("was not in context"));
    assert_eq!(h.store.last_seq().unwrap(), before);
    h.runtime.shutdown().await;
}
