use super::*;
use std::collections::BTreeSet;

#[test]
fn memory_provenance_filter_precedes_limit_and_preserves_scope_and_status() {
    let store = SqliteStore::open_in_memory().unwrap();
    let author = AgentId::new();
    let other = AgentId::new();
    let mut commit = Commit::new();
    let mut authored = Vec::new();
    for i in 0..208 {
        let mut record = MemoryRecord::from_candidate(
            MemoryCandidate {
                scope: if i == 1 {
                    MemoryScope::Agent { id: other }
                } else {
                    MemoryScope::Global
                },
                kind: MemoryKind::Fact,
                text: format!("record {i}"),
                structured: None,
                importance: if i < 3 { 0.1 } else { 0.9 },
                confidence: 0.8,
                tags: vec![],
                provenance: MemoryProvenance {
                    agent: Some(if i < 3 { author } else { other }),
                    ..Default::default()
                },
                supersedes: None,
            },
            now(),
        );
        if i == 1 {
            record.forgotten_at = Some(now());
        }
        if i == 2 {
            record.superseded_by = Some(MemoryId::new());
        }
        if i < 3 {
            authored.push(record.id);
        }
        commit = commit.mutate(Mutation::PutMemory(record));
    }
    store.commit(commit).unwrap();
    let query = MemoryQuery {
        provenance_agent: Some(author),
        limit: 200,
        include_forgotten: true,
        include_superseded: true,
        ..Default::default()
    };
    let records = store.memories(&query).unwrap();
    assert_eq!(records.len(), 3);
    assert!(records.iter().all(|r| authored.contains(&r.id)));
    assert_eq!(
        store
            .memories(&MemoryQuery {
                provenance_agent: None,
                ..query.clone()
            })
            .unwrap()
            .len(),
        200
    );
    assert_eq!(
        store
            .memories(&MemoryQuery {
                include_forgotten: false,
                include_superseded: false,
                ..query.clone()
            })
            .unwrap()
            .len(),
        1
    );
    let scoped = store
        .memories(&MemoryQuery {
            scope: Some(MemoryScope::Agent { id: other }),
            exact_scope: true,
            ..query
        })
        .unwrap();
    assert_eq!(scoped.len(), 1);
    assert_eq!(scoped[0].id, authored[1]);
}

#[test]
fn memory_search_and_supersession() {
    let store = SqliteStore::open_in_memory().expect("open");
    let repo = MemoryScope::Repository {
        id: "aether".into(),
    };
    let make = |text: &str, scope: MemoryScope, tags: &[&str]| {
        MemoryRecord::from_candidate(
            MemoryCandidate {
                scope,
                kind: MemoryKind::Fact,
                text: text.into(),
                structured: None,
                importance: 0.7,
                confidence: 0.9,
                tags: tags.iter().map(|t| t.to_string()).collect(),
                provenance: MemoryProvenance::default(),
                supersedes: None,
            },
            now(),
        )
    };
    let old = make(
        "The build uses cargo with a local CI script",
        repo.clone(),
        &["build"],
    );
    let global = make(
        "User prefers concise commit messages",
        MemoryScope::Global,
        &["git"],
    );
    let other = make(
        "Unrelated repo uses bazel",
        MemoryScope::Repository { id: "other".into() },
        &[],
    );
    store
        .commit(
            Commit::new()
                .mutate(Mutation::PutMemory(old.clone()))
                .mutate(Mutation::PutMemory(global.clone()))
                .mutate(Mutation::PutMemory(other)),
        )
        .expect("commit");
    let visible = store
        .memories(&MemoryQuery {
            scope: Some(repo.clone()),
            ..Default::default()
        })
        .expect("memories");
    assert_eq!(visible.len(), 2, "repo scope sees its own plus global");
    let found = store
        .memories(&MemoryQuery {
            text: Some("cargo build".into()),
            ..Default::default()
        })
        .expect("search");
    assert_eq!(found.len(), 1);
    assert_eq!(found[0].id, old.id);

    let mut new = make(
        "The build uses cargo; CI runs via scripts/ci.sh",
        repo.clone(),
        &["build"],
    );
    new.supersedes = Some(old.id);
    let mut old_updated = old.clone();
    old_updated.superseded_by = Some(new.id);
    store
        .commit(
            Commit::new()
                .mutate(Mutation::PutMemory(new.clone()))
                .mutate(Mutation::PutMemory(old_updated)),
        )
        .expect("commit");
    let current = store
        .memories(&MemoryQuery {
            text: Some("cargo".into()),
            ..Default::default()
        })
        .expect("search");
    assert_eq!(current.len(), 1);
    assert_eq!(current[0].id, new.id);
    let with_old = store
        .memories(&MemoryQuery {
            text: Some("cargo".into()),
            include_superseded: true,
            ..Default::default()
        })
        .expect("search");
    assert_eq!(with_old.len(), 2);
    let tagged = store
        .memories(&MemoryQuery {
            tags: vec!["git".into()],
            ..Default::default()
        })
        .expect("tags");
    assert_eq!(tagged[0].id, global.id);
}

/// The retraction sweep asks for a repository with every role scope in
/// it, and must get no other repository's: not one whose id is a prefix
/// of this one, and not one whose id and role split differently.
#[test]
fn a_repository_query_reaches_its_nested_role_scopes_only_when_asked() {
    let store = SqliteStore::open_in_memory().expect("open");
    let put = |text: &str, scope: MemoryScope| {
        let record = MemoryRecord::from_candidate(
            MemoryCandidate {
                scope,
                kind: MemoryKind::Fact,
                text: text.into(),
                structured: None,
                importance: 0.7,
                confidence: 0.9,
                tags: Vec::new(),
                provenance: MemoryProvenance::default(),
                supersedes: None,
            },
            now(),
        );
        store
            .commit(Commit::new().mutate(Mutation::PutMemory(record)))
            .expect("commit");
    };
    let repo_role = |id: &str, role: &str| MemoryScope::RepositoryRole {
        id: id.into(),
        role: role.into(),
    };
    put(
        "repository wide",
        MemoryScope::Repository { id: "a".into() },
    );
    put("for stewards", repo_role("a", "team.steward"));
    put("for workers", repo_role("a", "team.worker"));
    put("longer id", repo_role("ab", "team.steward"));
    // `a:x` + `y` and `a` + `x:y` must not share a key.
    put("colon in id", repo_role("a:x", "y"));
    put("colon in role", repo_role("a", "x:y"));
    put("global", MemoryScope::Global);

    let texts = |query: MemoryQuery| -> BTreeSet<String> {
        store
            .memories(&query)
            .expect("memories")
            .into_iter()
            .map(|m| m.text)
            .collect()
    };
    let exact = texts(MemoryQuery {
        scope: Some(MemoryScope::Repository { id: "a".into() }),
        exact_scope: true,
        ..Default::default()
    });
    assert_eq!(exact, BTreeSet::from(["repository wide".to_owned()]));
    let nested = texts(MemoryQuery {
        scope: Some(MemoryScope::Repository { id: "a".into() }),
        exact_scope: true,
        nested_scopes: true,
        ..Default::default()
    });
    assert_eq!(
        nested,
        BTreeSet::from(
            [
                "repository wide",
                "for stewards",
                "for workers",
                "colon in role"
            ]
            .map(str::to_owned)
        )
    );
    let one_role = texts(MemoryQuery {
        scope: Some(repo_role("a", "x:y")),
        exact_scope: true,
        ..Default::default()
    });
    assert_eq!(one_role, BTreeSet::from(["colon in role".to_owned()]));
    // Nothing nests in a role scope; the flag changes nothing there.
    let role_nested = texts(MemoryQuery {
        scope: Some(repo_role("a", "team")),
        exact_scope: true,
        nested_scopes: true,
        ..Default::default()
    });
    assert!(role_nested.is_empty(), "{role_nested:?}");
}

#[test]
fn memory_tag_filters_use_exact_case_sensitive_membership() {
    let store = SqliteStore::open_in_memory().unwrap();
    let tag_sets = [
        vec!["gitlab", "ci"],
        vec!["git", "ci"],
        vec!["axb"],
        vec!["a_b"],
        vec!["%"],
        vec!["GIT"],
        vec!["two words"],
        vec!["two", "words"],
        vec!["quote\"", "back\\slash"],
    ];
    let mut ids = Vec::new();
    let mut commit = Commit::new();
    for tags in tag_sets {
        let record = MemoryRecord::from_candidate(
            MemoryCandidate {
                scope: MemoryScope::Global,
                kind: MemoryKind::Fact,
                text: "durable memory".into(),
                structured: None,
                importance: 0.7,
                confidence: 0.9,
                tags: tags.into_iter().map(str::to_owned).collect(),
                provenance: MemoryProvenance::default(),
                supersedes: None,
            },
            now(),
        );
        ids.push(record.id);
        commit.push_mutation(Mutation::PutMemory(record));
    }
    store.commit(commit).unwrap();
    for (tags, expected) in [
        (vec!["git"], vec![ids[1]]),
        (vec!["git", "ci"], vec![ids[1]]),
        (vec!["a_b"], vec![ids[3]]),
        (vec!["%"], vec![ids[4]]),
        (vec!["GIT"], vec![ids[5]]),
        (vec!["two words"], vec![ids[6]]),
        (vec!["quote\"", "back\\slash"], vec![ids[8]]),
        (vec!["missing"], vec![]),
    ] {
        let found = store
            .memories(&MemoryQuery {
                tags: tags.iter().map(|t| t.to_string()).collect(),
                ..Default::default()
            })
            .unwrap();
        assert_eq!(
            found.iter().map(|r| r.id).collect::<Vec<_>>(),
            expected,
            "{tags:?}"
        );
    }
}

#[test]
fn memory_limits_preserve_zero_small_and_large_requests() {
    let store = SqliteStore::open_in_memory().unwrap();
    let mut commit = Commit::new();
    for i in 0..5001 {
        commit.push_mutation(Mutation::PutMemory(MemoryRecord::from_candidate(
            MemoryCandidate {
                scope: MemoryScope::Global,
                kind: MemoryKind::Fact,
                text: format!("memory {i}"),
                structured: None,
                importance: 0.7,
                confidence: 0.9,
                tags: vec![],
                provenance: MemoryProvenance::default(),
                supersedes: None,
            },
            now(),
        )));
    }
    store.commit(commit).unwrap();
    for (limit, expected) in [(0, 0), (2, 2), (5001, 5001), (usize::MAX, 5001)] {
        assert_eq!(
            store
                .memories(&MemoryQuery {
                    limit,
                    ..Default::default()
                })
                .unwrap()
                .len(),
            expected,
            "limit {limit}"
        );
    }
}

/// One memory of each recorder the operator filter must tell apart.
struct RecorderFixture {
    store: SqliteStore,
    repo: MemoryScope,
    operator_global: MemoryId,
    operator_repo: MemoryId,
    operator_forgotten: MemoryId,
    operator_superseded: MemoryId,
}

fn memory_by(scope: &MemoryScope, kind: MemoryKind, provenance: MemoryProvenance) -> MemoryRecord {
    MemoryRecord::from_candidate(
        MemoryCandidate {
            scope: scope.clone(),
            kind,
            text: "commit with the resolved identity".into(),
            structured: None,
            importance: 0.5,
            confidence: 0.7,
            tags: vec![],
            provenance,
            supersedes: None,
        },
        now(),
    )
}

fn actor(actor: Actor) -> MemoryProvenance {
    MemoryProvenance {
        actor: Some(actor),
        ..Default::default()
    }
}

/// Everything that is not this machine's operator: an import of one, an
/// agent either way it can be recorded, the system, and a repository note.
fn not_by_the_operator(repo: &MemoryScope) -> [MemoryRecord; 5] {
    let agent = AgentId::new();
    let note = aether_core::memory::MemorySource {
        file: ".aether/knowledge/notes.md".into(),
        commit: None,
    };
    [
        memory_by(
            repo,
            MemoryKind::Constraint,
            actor(Actor::user().imported()),
        ),
        memory_by(repo, MemoryKind::Constraint, actor(Actor::agent(agent))),
        memory_by(repo, MemoryKind::Constraint, actor(Actor::System)),
        memory_by(
            repo,
            MemoryKind::Constraint,
            MemoryProvenance {
                agent: Some(agent),
                ..Default::default()
            },
        ),
        memory_by(
            repo,
            MemoryKind::Constraint,
            MemoryProvenance {
                source: Some(note),
                ..Default::default()
            },
        ),
    ]
}

fn recorder_fixture() -> RecorderFixture {
    let store = SqliteStore::open_in_memory().expect("open");
    let repo = MemoryScope::Repository {
        id: "aether".into(),
    };
    let named_operator = Actor::User {
        name: Some("mengsig".into()),
        origin: Some(aether_core::event::OperatorOrigin::LocalSocket {
            connection: "c1".into(),
        }),
    };
    let global = memory_by(
        &MemoryScope::Global,
        MemoryKind::Preference,
        actor(Actor::user()),
    );
    let in_repo = memory_by(&repo, MemoryKind::Constraint, actor(named_operator));
    let mut forgotten = memory_by(&repo, MemoryKind::Constraint, actor(Actor::user()));
    forgotten.forgotten_at = Some(now());
    let mut superseded = memory_by(&repo, MemoryKind::Constraint, actor(Actor::user()));
    superseded.superseded_by = Some(MemoryId::new());
    let fixture = RecorderFixture {
        operator_global: global.id,
        operator_repo: in_repo.id,
        operator_forgotten: forgotten.id,
        operator_superseded: superseded.id,
        repo: repo.clone(),
        store,
    };
    let commit = [global, in_repo, forgotten, superseded]
        .into_iter()
        .chain(not_by_the_operator(&repo))
        .fold(Commit::new(), |c, r| c.mutate(Mutation::PutMemory(r)));
    fixture.store.commit(commit).expect("commit");
    fixture
}

fn recorded_ids(store: &SqliteStore, query: MemoryQuery) -> BTreeSet<MemoryId> {
    store
        .memories(&query)
        .expect("memories")
        .into_iter()
        .map(|r| r.id)
        .collect()
}

fn operator_query() -> MemoryQuery {
    MemoryQuery {
        recorded_by: Some(aether_core::memory::MemoryRecorder::Operator),
        include_forgotten: true,
        include_superseded: true,
        limit: 100,
        ..Default::default()
    }
}

#[test]
fn recorded_by_operator_matches_only_this_daemons_operator() {
    let f = recorder_fixture();
    let raw: Vec<String> = f
        .store
        .conn
        .lock()
        .prepare(
            "SELECT json_extract(body, '$.provenance.actor.actor') FROM memories WHERE id = ?1",
        )
        .expect("prepare")
        .query_map([f.operator_repo.to_string()], |row| row.get(0))
        .expect("query")
        .collect::<std::result::Result<_, _>>()
        .expect("rows");
    assert_eq!(raw, ["user"], "the stored actor tag the filter reads");
    let everything = MemoryQuery {
        recorded_by: None,
        ..operator_query()
    };
    assert_eq!(recorded_ids(&f.store, everything).len(), 9);
    let all = BTreeSet::from([
        f.operator_global,
        f.operator_repo,
        f.operator_forgotten,
        f.operator_superseded,
    ]);
    assert_eq!(recorded_ids(&f.store, operator_query()), all);
}

#[test]
fn recorded_by_keeps_history_out_unless_asked() {
    let f = recorder_fixture();
    let current = MemoryQuery {
        include_forgotten: false,
        include_superseded: false,
        ..operator_query()
    };
    assert_eq!(
        recorded_ids(&f.store, current),
        BTreeSet::from([f.operator_global, f.operator_repo])
    );
    let forgotten = MemoryQuery {
        include_superseded: false,
        ..operator_query()
    };
    assert_eq!(
        recorded_ids(&f.store, forgotten),
        BTreeSet::from([f.operator_global, f.operator_repo, f.operator_forgotten])
    );
}

#[test]
fn recorded_by_composes_with_scope_and_kind() {
    let f = recorder_fixture();
    let current = MemoryQuery {
        include_forgotten: false,
        include_superseded: false,
        ..operator_query()
    };
    let in_repo = MemoryQuery {
        scope: Some(f.repo.clone()),
        exact_scope: true,
        ..current.clone()
    };
    assert_eq!(
        recorded_ids(&f.store, in_repo.clone()),
        BTreeSet::from([f.operator_repo])
    );
    let constraints = MemoryQuery {
        kinds: vec![MemoryKind::Constraint],
        ..in_repo
    };
    assert_eq!(
        recorded_ids(&f.store, constraints),
        BTreeSet::from([f.operator_repo])
    );
    // Without `exact_scope` the repository also sees global memories.
    let visible = MemoryQuery {
        scope: Some(f.repo.clone()),
        kinds: vec![MemoryKind::Preference],
        ..current
    };
    assert_eq!(
        recorded_ids(&f.store, visible),
        BTreeSet::from([f.operator_global])
    );
}

/// A remote operator's memory, ranked below a pinned agent memory that
/// matches the same text and tag.
fn outranked_device_memory(store: &SqliteStore) -> MemoryId {
    let device = Actor::User {
        name: None,
        origin: Some(aether_core::event::OperatorOrigin::Device {
            device: aether_core::id::DeviceId::new(),
            name: "phone".into(),
            connection: "c2".into(),
        }),
    };
    let mut operator = memory_by(&MemoryScope::Global, MemoryKind::Fact, actor(device));
    operator.importance = 0.1;
    operator.tags = vec!["deploy".into()];
    let mut agent = memory_by(
        &MemoryScope::Global,
        MemoryKind::Fact,
        actor(Actor::agent(AgentId::new())),
    );
    agent.pinned = true;
    agent.importance = 1.0;
    agent.tags = vec!["deploy".into()];
    let id = operator.id;
    let commit = Commit::new()
        .mutate(Mutation::PutMemory(agent))
        .mutate(Mutation::PutMemory(operator));
    store.commit(commit).expect("commit");
    id
}

/// The filter sits in the WHERE clause of both the plain and the full-text
/// query, so a better-ranked memory by someone else cannot use up the limit.
#[test]
fn recorded_by_filters_before_the_limit_with_and_without_text() {
    let store = SqliteStore::open_in_memory().expect("open");
    let operator = outranked_device_memory(&store);
    let top = MemoryQuery {
        limit: 1,
        ..Default::default()
    };
    assert_ne!(
        recorded_ids(&store, top.clone()),
        BTreeSet::from([operator]),
        "the agent memory ranks first unfiltered"
    );
    let plain = MemoryQuery {
        recorded_by: Some(aether_core::memory::MemoryRecorder::Operator),
        tags: vec!["deploy".into()],
        ..top
    };
    assert_eq!(
        recorded_ids(&store, plain.clone()),
        BTreeSet::from([operator])
    );
    let searched = MemoryQuery {
        text: Some("resolved identity".into()),
        ..plain
    };
    assert_eq!(recorded_ids(&store, searched), BTreeSet::from([operator]));
}

/// The SQL clause and the Rust predicate name the same records.
#[test]
fn recorded_by_sql_agrees_with_the_predicate() {
    use aether_core::memory::MemoryRecorder;
    let f = recorder_fixture();
    let every = f
        .store
        .memories(&MemoryQuery {
            include_forgotten: true,
            include_superseded: true,
            limit: 100,
            ..Default::default()
        })
        .expect("memories");
    let predicate: BTreeSet<MemoryId> = every
        .iter()
        .filter(|r| MemoryRecorder::Operator.recorded(&r.provenance))
        .map(|r| r.id)
        .collect();
    let sql = recorded_ids(
        &f.store,
        MemoryQuery {
            recorded_by: Some(MemoryRecorder::Operator),
            include_forgotten: true,
            include_superseded: true,
            limit: 100,
            ..Default::default()
        },
    );
    assert_eq!(sql, predicate);
    assert_eq!(sql.len(), 4);
}

fn boundary_memory() -> MemoryRecord {
    MemoryRecord::from_candidate(
        MemoryCandidate {
            scope: MemoryScope::Global,
            kind: MemoryKind::Fact,
            text: "boundary record".into(),
            structured: None,
            importance: 0.5,
            confidence: 0.5,
            tags: vec![],
            provenance: MemoryProvenance::default(),
            supersedes: None,
        },
        now(),
    )
}

#[test]
fn indexed_memory_state_mismatches_return_errors() {
    let store = SqliteStore::open_in_memory().unwrap();
    let record = boundary_memory();
    store
        .commit(Commit::new().mutate(Mutation::PutMemory(record.clone())))
        .unwrap();
    for field in ["proposed_at", "forgotten_at"] {
        let mut body = serde_json::to_value(&record).unwrap();
        body[field] = serde_json::to_value(now()).unwrap();
        store
            .conn
            .lock()
            .execute(
                "UPDATE memories SET body=?1 WHERE id=?2",
                params![body.to_string(), record.id.to_string()],
            )
            .unwrap();
        let error = store.memories(&MemoryQuery::default()).unwrap_err();
        assert!(matches!(error, StoreError::Corrupt(_)));
        assert!(error.to_string().contains(&record.id.to_string()));
    }
}

#[test]
fn invalid_memory_proposals_are_rejected_without_writing() {
    let store = SqliteStore::open_in_memory().unwrap();
    let mut accepted_without_proposal = boundary_memory();
    accepted_without_proposal.accepted_at = Some(now());
    let mut superseded_proposal = boundary_memory();
    superseded_proposal.proposed_at = Some(now());
    superseded_proposal.superseded_by = Some(MemoryId::new());
    for record in [accepted_without_proposal, superseded_proposal] {
        let result = store.commit(Commit::new().mutate(Mutation::PutMemory(record.clone())));
        assert!(matches!(result, Err(StoreError::Corrupt(_))));
        assert!(store.memory(record.id).unwrap().is_none());
    }
}
