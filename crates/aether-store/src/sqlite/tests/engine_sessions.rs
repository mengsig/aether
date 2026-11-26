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
    let refused = write(conflicting).unwrap_err();
    assert!(
        matches!(refused, StoreError::OwnershipRefused(_)),
        "{refused}"
    );
    assert!(refused.to_string().contains("already owned"), "{refused}");
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
            Err(StoreError::OwnershipRefused(_))
        ));
        assert!(store.agent(new_agent.id).unwrap().is_none());
        let expected = if existing { vec![record] } else { vec![] };
        assert_eq!(store.engine_session_ownerships().unwrap(), expected);
    }
}

/// The session id a real `agy acp` 1.5.18 returned from `session/new`.
const AGY_SESSION: &str = "e595a646-998a-44b5-b978-759aa2cfabf1";

#[test]
fn antigravity_ownership_accepts_the_real_agy_session_shape() {
    let store = SqliteStore::open_in_memory().unwrap();
    let mut record = ownership("antigravity", AgentId::new());
    record.session_id = AGY_SESSION.into();
    store
        .commit(Commit::new().mutate(Mutation::RecordEngineSessionOwnership(record.clone())))
        .unwrap();
    assert_eq!(store.engine_session_ownerships().unwrap(), vec![record]);
}

#[test]
fn antigravity_ownership_refuses_other_forms_without_claiming_corruption() {
    let store = SqliteStore::open_in_memory().unwrap();
    let valid = || {
        let mut record = ownership("antigravity", AgentId::new());
        record.session_id = AGY_SESSION.into();
        record
    };
    let mut with_path = valid();
    with_path
        .log_paths
        .push(format!("/home/user/.gemini/antigravity-cli/conversations/{AGY_SESSION}.db").into());
    let cases = [
        ("new-session", "not a lowercase UUID"),
        (
            "E595A646-998A-44B5-B978-759AA2CFABF1",
            "not a lowercase UUID",
        ),
        ("e595a646998a44b5b978759aa2cfabf1", "not a lowercase UUID"),
        (
            "e595a646-998a-44b5-b978-759aa2cfabf1\n",
            "not a lowercase UUID",
        ),
        (
            "../e595a646-998a-44b5-b978-759aa2cf",
            "not a lowercase UUID",
        ),
    ];
    let mut refused: Vec<_> = cases
        .iter()
        .map(|(id, why)| {
            let mut record = valid();
            record.session_id = (*id).into();
            (record, *why)
        })
        .collect();
    refused.push((with_path, "no verifiable log paths"));
    let mut unknown = valid();
    unknown.engine = "unowned-engine".into();
    refused.push((unknown, "has no ownership evidence form"));
    for (record, why) in refused {
        let error = store
            .commit(Commit::new().mutate(Mutation::RecordEngineSessionOwnership(record)))
            .unwrap_err();
        assert!(matches!(error, StoreError::OwnershipRefused(_)), "{error}");
        let message = error.to_string();
        assert!(message.contains(why), "{message}");
        assert!(!message.contains("corrupt"), "{message}");
    }
    assert!(store.engine_session_ownerships().unwrap().is_empty());
}
