use super::*;
use aether_core::repository::{ALIASED_EVENT, RepositoryAlias};

const CANONICAL: &str = "github.com/mengsig/archon";

fn repo(id: &str) -> MemoryScope {
    MemoryScope::Repository { id: id.into() }
}

fn repo_role(id: &str, role: &str) -> MemoryScope {
    MemoryScope::RepositoryRole {
        id: id.into(),
        role: role.into(),
    }
}

fn file(store: &SqliteStore, scope: MemoryScope, text: &str) -> MemoryRecord {
    let record = MemoryRecord::from_candidate(
        MemoryCandidate {
            scope,
            kind: MemoryKind::Fact,
            text: text.into(),
            structured: None,
            importance: 0.6,
            confidence: 0.8,
            tags: vec!["kept".into()],
            provenance: MemoryProvenance {
                agent: Some(AgentId::new()),
                ..Default::default()
            },
            supersedes: None,
        },
        now(),
    );
    store
        .commit(Commit::new().mutate(Mutation::PutMemory(record.clone())))
        .unwrap();
    record
}

fn alias(old: &str, new: &str) -> RepositoryAlias {
    RepositoryAlias::new(old, new, now()).unwrap()
}

fn operator() -> Actor {
    Actor::User {
        name: Some("op".into()),
        origin: None,
    }
}

fn under(store: &SqliteStore, scope: MemoryScope) -> Vec<MemoryRecord> {
    store
        .memories(&MemoryQuery {
            scope: Some(scope),
            exact_scope: true,
            nested_scopes: true,
            include_forgotten: true,
            include_superseded: true,
            limit: 100,
            ..Default::default()
        })
        .unwrap()
}

/// What the move must leave alone: archon's memories under the alias, one
/// already under the canonical id, and neighbours whose ids merely look alike.
struct Filed {
    plain: MemoryRecord,
    forgotten: MemoryRecord,
    role: MemoryRecord,
    neighbours: Vec<MemoryRecord>,
}

fn filed(store: &SqliteStore) -> Filed {
    let plain = file(store, repo("archon"), "tests run with cargo nextest");
    let mut forgotten = file(store, repo("archon"), "the old release script");
    forgotten.forgotten_at = Some(now());
    store
        .commit(Commit::new().mutate(Mutation::PutMemory(forgotten.clone())))
        .unwrap();
    let role = file(store, repo_role("archon", "worker"), "workers rebase daily");
    file(store, repo(CANONICAL), "tests run with cargo nextest");
    let neighbours = vec![
        file(store, repo("archonite"), "a different project"),
        file(store, repo_role("archon2", "worker"), "another one"),
        file(store, MemoryScope::Global, "global"),
    ];
    let mut old = agent();
    old.scope.repository = Some("archon".into());
    store
        .commit(Commit::new().mutate(Mutation::UpsertAgent(old)))
        .unwrap();
    Filed {
        plain,
        forgotten,
        role,
        neighbours,
    }
}

#[test]
fn a_dry_run_counts_what_the_move_then_moves() {
    let store = SqliteStore::open_in_memory().unwrap();
    let filed = filed(&store);
    let archon = alias("archon", CANONICAL);
    let seq = store.last_seq().unwrap();

    let preview = store.preview_repository_alias(&archon).unwrap();
    assert!(!preview.existing && preview.events.is_empty());
    assert_eq!(
        (preview.counts.repository, preview.counts.repository_role),
        (2, 1)
    );
    assert_eq!((preview.counts.duplicates, preview.counts.agents), (1, 1));
    assert_eq!(store.resolve_repository("archon").unwrap(), "archon");
    assert_eq!(
        under(&store, repo("archon")).len(),
        3,
        "a dry run moves nothing"
    );
    assert_eq!(store.last_seq().unwrap(), seq);

    let gate = crate::WriteGate::new();
    let moved = store
        .alias_repository(&archon, operator(), &gate.hold())
        .unwrap();
    assert_eq!(moved.counts, preview.counts);
    assert!(!moved.existing);
    let [event] = moved.events.as_slice() else {
        panic!("one event records the alias: {:?}", moved.events);
    };
    let EventKind::Custom { name, payload } = &event.kind else {
        panic!("{:?}", event.kind);
    };
    assert_eq!(name, ALIASED_EVENT);
    assert_eq!(payload["moved"]["repository"], 2);
    assert!(matches!(event.actor, Actor::User { .. }));

    assert_eq!(store.resolve_repository("archon").unwrap(), CANONICAL);
    assert_eq!(store.repository_aliases().unwrap(), vec![archon]);
    assert!(under(&store, repo("archon")).is_empty());
    assert_eq!(under(&store, repo(CANONICAL)).len(), 4);
    for kept in &filed.neighbours {
        assert_eq!(store.memory(kept.id).unwrap().as_ref(), Some(kept));
    }
    the_moved_records_keep_everything_but_their_scope(&store, &filed);
}

fn the_moved_records_keep_everything_but_their_scope(store: &SqliteStore, filed: &Filed) {
    for (before, scope) in [
        (&filed.plain, repo(CANONICAL)),
        (&filed.forgotten, repo(CANONICAL)),
        (&filed.role, repo_role(CANONICAL, "worker")),
    ] {
        let after = store.memory(before.id).unwrap().unwrap();
        assert_eq!(
            after,
            MemoryRecord {
                scope,
                ..before.clone()
            }
        );
    }
    // An in-memory store reads on its one connection: release it before querying.
    let rows: Vec<(String, String)> = {
        let conn = store.read_conn();
        let mut statement = conn.prepare("SELECT scope, body FROM memories").unwrap();
        statement
            .query_map([], |row| Ok((row.get(0)?, row.get(1)?)))
            .unwrap()
            .collect::<rusqlite::Result<_>>()
            .unwrap()
    };
    assert_eq!(rows.len(), 7);
    for (key, body) in rows {
        let record: MemoryRecord = serde_json::from_str(&body).unwrap();
        assert_eq!(
            key,
            scope_key(&record.scope),
            "key column and body disagree"
        );
    }
    let searched = store
        .memories(&MemoryQuery {
            scope: Some(repo_role(CANONICAL, "worker")),
            text: Some("rebase".into()),
            limit: 10,
            ..Default::default()
        })
        .unwrap();
    assert_eq!(
        searched.iter().map(|r| r.id).collect::<Vec<_>>(),
        [filed.role.id]
    );
}

#[test]
fn a_duplicate_text_is_moved_beside_its_twin_not_merged() {
    let store = SqliteStore::open_in_memory().unwrap();
    filed(&store);
    store
        .alias_repository(
            &alias("archon", CANONICAL),
            operator(),
            &crate::WriteGate::new().hold(),
        )
        .unwrap();
    let twins: Vec<_> = under(&store, repo(CANONICAL))
        .into_iter()
        .filter(|r| r.text == "tests run with cargo nextest")
        .collect();
    assert_eq!(twins.len(), 2);
    assert!(twins.iter().all(MemoryRecord::is_current));
}

#[test]
fn proposals_at_the_alias_target_do_not_count_as_current_duplicates() {
    for rejected in [false, true] {
        let store = SqliteStore::open_in_memory().unwrap();
        let text = "Use the repository build verification script";
        let active = file(&store, repo("archon"), text);
        let mut pending = file(&store, repo(CANONICAL), text);
        pending.proposed_at = Some(pending.created_at);
        pending.forgotten_at = rejected.then(now);
        store
            .commit(Commit::new().mutate(Mutation::PutMemory(pending.clone())))
            .unwrap();
        let archon = alias("archon", CANONICAL);
        let preview = store.preview_repository_alias(&archon).unwrap();
        assert_eq!(preview.counts.repository, 1);
        assert_eq!(preview.counts.duplicates, 0);
        let moved = store
            .alias_repository(&archon, operator(), &crate::WriteGate::new().hold())
            .unwrap();
        assert_eq!(moved.counts, preview.counts);
        assert!(store.memory(active.id).unwrap().unwrap().is_current());
        assert_eq!(store.memory(pending.id).unwrap(), Some(pending));
    }
}

#[test]
fn registering_the_same_alias_again_moves_only_stragglers() {
    let store = SqliteStore::open_in_memory().unwrap();
    filed(&store);
    let gate = crate::WriteGate::new();
    let archon = alias("archon", CANONICAL);
    store
        .alias_repository(&archon, operator(), &gate.hold())
        .unwrap();
    let seq = store.last_seq().unwrap();

    let again = store
        .alias_repository(&archon, operator(), &gate.hold())
        .unwrap();
    assert!(again.existing && again.events.is_empty());
    assert_eq!(again.counts.memories(), 0);
    assert_eq!(store.last_seq().unwrap(), seq, "nothing to record");

    let straggler = file(&store, repo_role("archon", "reviewer"), "imported late");
    let swept = store
        .alias_repository(&archon, operator(), &gate.hold())
        .unwrap();
    assert!(swept.existing);
    assert_eq!((swept.counts.repository_role, swept.events.len()), (1, 1));
    assert_eq!(
        store.memory(straggler.id).unwrap().unwrap().scope,
        repo_role(CANONICAL, "reviewer")
    );
    assert_eq!(store.repository_aliases().unwrap().len(), 1);
}

#[test]
fn a_second_target_and_either_half_of_a_chain_are_refused() {
    let store = SqliteStore::open_in_memory().unwrap();
    let gate = crate::WriteGate::new();
    store
        .alias_repository(&alias("archon", CANONICAL), operator(), &gate.hold())
        .unwrap();
    let seq = store.last_seq().unwrap();
    for (old, new) in [
        ("archon", "github.com/someone/else"),
        ("older", "archon"),
        (CANONICAL, "github.com/someone/else"),
    ] {
        let refused = store.alias_repository(&alias(old, new), operator(), &gate.hold());
        assert!(
            matches!(refused, Err(StoreError::AliasRefused(_))),
            "{old} -> {new}: {refused:?}"
        );
        let previewed = store.preview_repository_alias(&alias(old, new));
        assert!(matches!(previewed, Err(StoreError::AliasRefused(_))));
    }
    assert_eq!(store.resolve_repository("archon").unwrap(), CANONICAL);
    assert_eq!(store.resolve_repository("older").unwrap(), "older");
    assert_eq!(store.repository_aliases().unwrap().len(), 1);
    assert_eq!(store.last_seq().unwrap(), seq);
}

#[test]
fn a_move_that_fails_partway_leaves_every_memory_where_it_was() {
    let store = SqliteStore::open_in_memory().unwrap();
    let filed = filed(&store);
    let refuse = format!(
        "CREATE TRIGGER refuse_role BEFORE UPDATE ON memories WHEN old.id = '{}'
         BEGIN SELECT RAISE(ABORT, 'disk on fire'); END",
        filed.role.id
    );
    store.conn.lock().execute_batch(&refuse).unwrap();
    let seq = store.last_seq().unwrap();

    let failed = store.alias_repository(
        &alias("archon", CANONICAL),
        operator(),
        &crate::WriteGate::new().hold(),
    );
    assert!(failed.is_err(), "{failed:?}");
    assert_eq!(store.resolve_repository("archon").unwrap(), "archon");
    assert!(store.repository_aliases().unwrap().is_empty());
    assert_eq!(under(&store, repo("archon")).len(), 3);
    assert_eq!(
        store.memory(filed.plain.id).unwrap().unwrap().scope,
        repo("archon")
    );
    assert_eq!(store.last_seq().unwrap(), seq);
}

#[test]
fn a_stored_self_alias_returns_a_corruption_error() {
    let store = SqliteStore::open_in_memory().unwrap();
    store
        .conn
        .lock()
        .execute_batch(
            "PRAGMA ignore_check_constraints=ON;
         INSERT INTO repository_aliases(alias,canonical,created_at,body)
         VALUES ('self','self','2026-01-01T00:00:00Z','{}');
         PRAGMA ignore_check_constraints=OFF;",
        )
        .unwrap();
    let error = store.resolve_repository("self").unwrap_err();
    assert!(matches!(error, StoreError::Corrupt(_)));
    assert!(error.to_string().contains("points to itself"));
}
