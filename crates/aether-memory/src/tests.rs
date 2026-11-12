use super::*;
use aether_core::memory::MemoryProvenance;
use aether_store::SqliteStore;

fn service() -> MemoryService {
    MemoryService::new(
        Arc::new(SqliteStore::open_in_memory().expect("store")),
        WriteGate::new(),
    )
}

fn candidate(text: &str, scope: MemoryScope) -> MemoryCandidate {
    MemoryCandidate {
        scope,
        kind: MemoryKind::Fact,
        text: text.into(),
        structured: None,
        importance: 0.7,
        confidence: 0.9,
        tags: vec!["build".into()],
        provenance: MemoryProvenance::default(),
        supersedes: None,
    }
}

fn repo() -> MemoryScope {
    MemoryScope::Repository {
        id: "aether".into(),
    }
}

#[test]
fn rejects_transient_and_trivial_facts() {
    let s = service();
    let a = s
        .admit(candidate("short", repo()), Actor::System, None, None)
        .expect("admit");
    assert!(matches!(a, Admission::Rejected { .. }));
    let a = s
        .admit(
            candidate("The tests are failing right now because of a typo", repo()),
            Actor::System,
            None,
            None,
        )
        .expect("admit");
    assert!(matches!(a, Admission::Rejected { ref reason } if reason.contains("transient")));
    let mut low = candidate(
        "Repository uses cargo workspaces with a local ci script",
        repo(),
    );
    low.importance = 0.1;
    let a = s.admit(low, Actor::System, None, None).expect("admit");
    assert!(matches!(a, Admission::Rejected { ref reason } if reason.contains("importance")));
}

#[test]
fn accepts_then_rejects_duplicate_then_supersedes_stronger() {
    let s = service();
    let first = s
        .admit(
            candidate(
                "The repository builds with cargo and runs scripts/ci.sh",
                repo(),
            ),
            Actor::System,
            None,
            None,
        )
        .expect("admit");
    let first_id = first.record().expect("record").id;
    assert!(matches!(first, Admission::Accepted { .. }));

    let dup = s
        .admit(
            candidate(
                "The repository builds with cargo and runs scripts/ci.sh",
                repo(),
            ),
            Actor::System,
            None,
            None,
        )
        .expect("admit");
    assert!(matches!(dup, Admission::Rejected { ref reason } if reason.contains("redundant")));

    let mut stronger = candidate(
        "The repository builds with cargo and runs scripts/ci.sh",
        repo(),
    );
    stronger.importance = 0.95;
    let sup = s.admit(stronger, Actor::System, None, None).expect("admit");
    match sup {
        Admission::Superseded { record, old } => {
            assert_eq!(old, first_id);
            assert_eq!(record.supersedes, Some(first_id));
            let old_rec = s.store.memory(first_id).expect("get").expect("exists");
            assert_eq!(old_rec.superseded_by, Some(record.id));
        }
        other => panic!("expected supersession, got {other:?}"),
    }
    let current = s
        .retrieve(&RetrievalRequest {
            scope: Some(repo()),
            query: Some("cargo".into()),
            limit: 5,
            ..Default::default()
        })
        .expect("retrieve");
    assert_eq!(current.len(), 1, "superseded memory must not be retrieved");
    assert_eq!(current[0].record.importance, 0.95);
}

#[test]
fn retrieval_respects_scope_and_prefers_relevance() {
    let s = service();
    s.admit(
        candidate(
            "Global rule: every unsafe block carries a SAFETY comment",
            MemoryScope::Global,
        ),
        Actor::System,
        None,
        None,
    )
    .expect("g");
    s.admit(
        candidate(
            "This repository pins rust toolchain stable and forbids unsafe",
            repo(),
        ),
        Actor::System,
        None,
        None,
    )
    .expect("r");
    s.admit(
        candidate(
            "Other repository uses bazel for its rust builds",
            MemoryScope::Repository { id: "other".into() },
        ),
        Actor::System,
        None,
        None,
    )
    .expect("o");
    let got = s
        .retrieve(&RetrievalRequest {
            scope: Some(repo()),
            query: Some("rust toolchain unsafe".into()),
            limit: 5,
            ..Default::default()
        })
        .expect("retrieve");
    let texts: Vec<&str> = got.iter().map(|r| r.record.text.as_str()).collect();
    assert!(texts[0].contains("pins rust toolchain"));
    assert!(texts.iter().any(|t| t.contains("Global rule")));
    assert!(
        !texts.iter().any(|t| t.contains("bazel")),
        "other repo's memories are invisible"
    );
}

#[test]
fn evaluate_does_not_write_and_touch_counts() {
    let s = service();
    let hold = s.gate.hold();
    let (adm, commit) = s
        .evaluate(
            &hold,
            candidate(
                "Users prefer the sidebar collapsed by default in the tui",
                repo(),
            ),
            Actor::System,
            None,
            None,
        )
        .expect("evaluate");
    let id = adm.record().expect("rec").id;
    assert!(s.store.memory(id).expect("get").is_none());
    s.store.commit(commit).expect("commit");
    drop(hold);
    assert!(s.store.memory(id).expect("get").is_some());
    let touch = s.touch(&s.gate.hold(), &[id]).expect("touch");
    s.store.commit(touch).expect("commit");
    assert_eq!(
        s.store
            .memory(id)
            .expect("get")
            .expect("rec")
            .retrieval_count,
        1
    );
}

fn visible_texts(s: &MemoryService, agent: AgentId, ancestors: Vec<AgentId>) -> Vec<String> {
    s.retrieve(&RetrievalRequest {
        scope: Some(repo()),
        role: Some("worker".into()),
        agent: Some(agent),
        ancestors,
        limit: 50,
        ..Default::default()
    })
    .expect("retrieve")
    .into_iter()
    .map(|r| r.record.text)
    .collect()
}

#[test]
fn a_subtree_note_reaches_its_descendants_within_the_lineage_bound_only() {
    let s = service();
    let root = AgentId::new();
    let shared = "Reviews in this tree always run clippy before the tests";
    let private = "The root agent keeps its own review checklist in an artifact";
    s.admit(
        candidate(shared, MemoryScope::Subtree { root }),
        Actor::System,
        None,
        None,
    )
    .expect("subtree");
    s.admit(
        candidate(private, MemoryScope::Agent { id: root }),
        Actor::System,
        None,
        None,
    )
    .expect("agent");
    let sees = |texts: &[String], text: &str| texts.iter().any(|t| t == text);

    let at_root = visible_texts(&s, root, vec![]);
    assert!(
        sees(&at_root, shared) && sees(&at_root, private),
        "{at_root:?}"
    );

    let child = AgentId::new();
    let at_child = visible_texts(&s, child, vec![root]);
    assert!(sees(&at_child, shared), "{at_child:?}");
    assert!(
        !sees(&at_child, private),
        "an agent's own scope stays its own"
    );

    let sibling = visible_texts(&s, AgentId::new(), vec![AgentId::new()]);
    assert!(!sees(&sibling, shared), "another tree: {sibling:?}");

    // `root` as the furthest ancestor still in reach, then one past it.
    let mut within: Vec<AgentId> = (1..MAX_LINEAGE).map(|_| AgentId::new()).collect();
    within.push(root);
    assert!(sees(
        &visible_texts(&s, AgentId::new(), within.clone()),
        shared
    ));
    let mut beyond = within;
    beyond.insert(0, AgentId::new());
    assert_eq!(beyond.len(), MAX_LINEAGE + 1);
    assert!(!sees(&visible_texts(&s, AgentId::new(), beyond), shared));
}

#[test]
fn agent_cannot_supersede_an_unpinned_operator_memory() {
    let service = service();
    let mut authored = candidate(
        "The operator requires reviews before production releases",
        repo(),
    );
    authored.provenance.actor = Some(Actor::user());
    let original = service
        .admit(authored, Actor::user(), None, None)
        .unwrap()
        .record()
        .unwrap()
        .id;
    let mut replacement = candidate(
        "The repository permits production releases without review",
        repo(),
    );
    replacement.supersedes = Some(original);
    let result = service
        .admit(
            replacement.clone(),
            Actor::agent(AgentId::new()),
            None,
            None,
        )
        .unwrap();
    assert!(matches!(result, Admission::Rejected { ref reason } if reason.contains("operator")));
    assert!(
        service
            .store
            .memory(original)
            .unwrap()
            .unwrap()
            .is_current()
    );
    assert!(matches!(
        service
            .admit(replacement, Actor::user(), None, None)
            .unwrap(),
        Admission::Superseded { .. }
    ));
}

#[test]
fn two_candidates_cannot_supersede_the_same_record_in_one_commit() {
    for explicit in [false, true] {
        let service = service();
        let text = "The repository builds with cargo and runs scripts/ci.sh";
        let old = service
            .admit(candidate(text, repo()), Actor::System, None, None)
            .unwrap()
            .record()
            .unwrap()
            .id;
        let mut replacement = candidate(text, repo());
        replacement.importance = 0.95;
        replacement.supersedes = explicit.then_some(old);
        let hold = service.gate.hold();
        let (first, mut commit) = service
            .evaluate_batch(
                &hold,
                replacement.clone(),
                Actor::System,
                None,
                None,
                &BTreeSet::new(),
            )
            .unwrap();
        assert!(matches!(first, Admission::Superseded { old: id, .. } if id == old));
        let (second, second_commit) = service
            .evaluate_batch(
                &hold,
                replacement,
                Actor::System,
                None,
                None,
                &BTreeSet::from([old]),
            )
            .unwrap();
        assert!(matches!(second, Admission::Rejected { ref reason } if reason.contains("batch")));
        commit.events.extend(second_commit.events);
        commit.mutations.extend(second_commit.mutations);
        service.store.commit(commit).unwrap();
        assert_eq!(
            service.store.memory(old).unwrap().unwrap().superseded_by,
            Some(first.record().unwrap().id)
        );
    }
}
