use super::*;
use crate::{EngineSessionOwnership, ExpiredEngineSessionOwner};
use aether_core::agent::AgentStatusKind;

fn ownership(engine: &str, agent_id: AgentId) -> EngineSessionOwnership {
    EngineSessionOwnership {
        engine: engine.into(),
        session_id: "new-session".into(),
        agent_id,
        log_paths: Vec::new(),
        created_at: now(),
        expired_owner: None,
    }
}

#[test]
fn ownership_is_explicit_and_first_owner_cannot_be_replaced() {
    let store = SqliteStore::open_in_memory().expect("store");
    let mut legacy = agent();
    legacy.engine_session = Some("legacy-user-session".into());
    store
        .commit(Commit::new().mutate(Mutation::UpsertAgent(legacy.clone())))
        .unwrap();
    assert!(store.engine_session_ownerships().unwrap().is_empty());

    let record = ownership("claude-code", legacy.id);
    let write =
        |record| store.commit(Commit::new().mutate(Mutation::RecordEngineSessionOwnership(record)));
    write(record.clone()).unwrap();
    let mut repeated = record.clone();
    repeated.created_at += chrono::Duration::seconds(1);
    write(repeated).unwrap();
    let mut conflicting = record.clone();
    conflicting.agent_id = AgentId::new();
    assert!(write(conflicting).is_err());
    assert_eq!(store.engine_session_ownerships().unwrap(), vec![record]);
    write(ownership("codex", AgentId::new())).unwrap();
    assert_eq!(store.engine_session_ownerships().unwrap().len(), 2);
}

#[test]
fn invalid_or_unknown_paths_never_expand_ownership() {
    let store = SqliteStore::open_in_memory().unwrap();
    let record = ownership("codex", AgentId::new());
    for path in ["relative/session.jsonl", "/native/../user-session.jsonl"] {
        let mut invalid = record.clone();
        invalid.log_paths.push(path.into());
        assert!(
            store
                .commit(Commit::new().mutate(Mutation::RecordEngineSessionOwnership(invalid)))
                .is_err()
        );
        assert!(store.engine_session_ownerships().unwrap().is_empty());
    }
    store
        .commit(Commit::new().mutate(Mutation::RecordEngineSessionOwnership(record.clone())))
        .unwrap();
    assert_eq!(store.engine_session_ownerships().unwrap(), vec![record]);
}

#[test]
fn v23_migration_does_not_claim_legacy_sessions() {
    let dir = tempfile::tempdir().unwrap();
    let database = dir.path().join("store.sqlite");
    {
        let store = SqliteStore::open(&database).unwrap();
        let mut legacy = agent();
        legacy.engine_session = Some("user-session".into());
        store
            .commit(Commit::new().mutate(Mutation::UpsertAgent(legacy)))
            .unwrap();
        store.conn.lock().execute_batch(
            "DROP TABLE engine_session_ownership; UPDATE meta SET value='23' WHERE key='schema_version';"
        ).unwrap();
    }
    let store = SqliteStore::open(database).unwrap();
    assert!(store.engine_session_ownerships().unwrap().is_empty());
}

#[test]
fn legacy_ownership_without_expired_owner_remains_unknown_after_reopen() {
    let dir = tempfile::tempdir().unwrap();
    let database = dir.path().join("store.sqlite");
    let record = ownership("codex", AgentId::new());
    let body = serde_json::to_value(&record).unwrap();
    assert!(body.get("expired_owner").is_none());
    {
        let store = SqliteStore::open(&database).unwrap();
        store.conn.lock().execute(
            "INSERT INTO engine_session_ownership(engine, session_id, body) VALUES (?1, ?2, ?3)",
            params![record.engine, record.session_id, body.to_string()],
        ).unwrap();
    }
    let store = SqliteStore::open(database).unwrap();
    assert_eq!(store.engine_session_ownerships().unwrap(), vec![record]);
}

#[test]
fn supplied_expired_owner_is_rejected_even_for_idempotent_creation() {
    for existing in [false, true] {
        let store = SqliteStore::open_in_memory().unwrap();
        let record = ownership("codex", AgentId::new());
        if existing {
            store
                .commit(
                    Commit::new().mutate(Mutation::RecordEngineSessionOwnership(record.clone())),
                )
                .unwrap();
        }
        let mut forged = record.clone();
        forged.expired_owner = Some(ExpiredEngineSessionOwner {
            status: AgentStatusKind::Completed,
            updated_at: record.created_at,
        });
        let new_agent = agent();
        assert!(matches!(
            store.commit(
                Commit::new()
                    .mutate(Mutation::UpsertAgent(new_agent.clone()))
                    .mutate(Mutation::RecordEngineSessionOwnership(forged))
            ),
            Err(StoreError::Corrupt(_))
        ));
        assert!(store.agent(new_agent.id).unwrap().is_none());
        let expected = if existing { vec![record] } else { vec![] };
        assert_eq!(store.engine_session_ownerships().unwrap(), expected);
    }
}
