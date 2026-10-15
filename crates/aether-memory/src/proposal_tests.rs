use super::*;
use aether_core::memory::MemoryProvenance;
use aether_store::SqliteStore;

fn service() -> MemoryService {
    MemoryService::new(
        Arc::new(SqliteStore::open_in_memory().unwrap()),
        WriteGate::new(),
    )
}

fn candidate(text: &str) -> MemoryCandidate {
    MemoryCandidate {
        scope: MemoryScope::Global,
        kind: MemoryKind::Lesson,
        text: text.into(),
        structured: None,
        importance: 0.7,
        confidence: 0.9,
        tags: vec!["build".into()],
        provenance: MemoryProvenance {
            actor: Some(Actor::user()),
            ..Default::default()
        },
        supersedes: None,
    }
}

fn propose(service: &MemoryService, candidate: MemoryCandidate) -> MemoryRecord {
    let hold = service.gate.hold();
    let (admission, commit) = service
        .evaluate_operator(&hold, candidate, Actor::user(), true)
        .unwrap();
    let record = admission.record().expect("admitted proposal").clone();
    assert!(record.is_proposed());
    assert!(!record.is_current());
    service.store.commit(commit).unwrap();
    record
}

fn admit(service: &MemoryService, candidate: MemoryCandidate) -> MemoryRecord {
    let admission = service.admit(candidate, Actor::user(), None, None).unwrap();
    let record = admission.record().expect("admitted memory").clone();
    assert!(record.is_current());
    assert!(!record.is_proposed());
    record
}

fn events(service: &MemoryService) -> Vec<aether_core::event::Event> {
    service.store.events(0, 1000, &Default::default()).unwrap()
}

#[test]
fn proposal_recall_exclusion_covers_structural_lexical_pinned_and_history_queries() {
    let service = service();
    let pending = propose(
        &service,
        candidate("Use the repository build verification script"),
    );
    service.pin(pending.id, true, Actor::user()).unwrap();
    for query in [None, Some("build".into())] {
        assert!(
            service
                .retrieve(&RetrievalRequest {
                    query,
                    ..Default::default()
                })
                .unwrap()
                .is_empty()
        );
    }
    assert!(
        service
            .store
            .memories(&MemoryQuery {
                include_superseded: true,
                include_forgotten: true,
                ..Default::default()
            })
            .unwrap()
            .is_empty()
    );
    let found = service
        .store
        .memories(&MemoryQuery {
            proposed: true,
            ..Default::default()
        })
        .unwrap();
    assert_eq!(found.len(), 1);
    assert_eq!(found[0].id, pending.id);
    assert_eq!(found[0].provenance.actor, Some(Actor::user()));
    let history = events(&service);
    let event = history
        .iter()
        .find(|e| matches!(&e.kind, EventKind::Custom { name, .. } if name == "memory.proposed"))
        .unwrap();
    let EventKind::Custom { payload, .. } = &event.kind else {
        unreachable!();
    };
    assert_eq!(event.actor, Actor::user());
    assert_eq!(
        payload,
        &serde_json::json!({"memory":pending.id,"scope":{"scope":"global"},"tags":["build"]})
    );
}

#[test]
fn proposal_accept_refuses_new_active_duplicate_without_mutations_or_events() {
    for replace in [false, true] {
        let service = service();
        let old =
            replace.then(|| admit(&service, candidate("Use the old build verification script")));
        let mut next = candidate("Use the new build verification script");
        next.supersedes = old.as_ref().map(|record| record.id);
        let pending = propose(&service, next);
        let existing = admit(&service, candidate(&pending.text));
        let history = events(&service);
        let error = service.accept(pending.id, None, Actor::user()).unwrap_err();
        assert!(
            error.to_string().contains(&existing.id.to_string()),
            "{error}"
        );
        assert!(error.to_string().contains("duplicates"), "{error}");
        assert_eq!(events(&service), history);
        assert_eq!(service.store.memory(pending.id).unwrap(), Some(pending));
        assert_eq!(service.store.memory(existing.id).unwrap(), Some(existing));
        if let Some(old) = old {
            assert_eq!(service.store.memory(old.id).unwrap(), Some(old));
        }
    }
}

#[derive(Debug, Default)]
struct FailOnceIndex(std::sync::atomic::AtomicUsize);

impl MemoryIndex for FailOnceIndex {
    fn name(&self) -> &'static str {
        "fail-once"
    }

    fn index(&self, record: &MemoryRecord) -> Result<()> {
        assert!(record.is_current());
        assert!(record.accepted_at.is_some());
        if self.0.fetch_add(1, std::sync::atomic::Ordering::SeqCst) == 0 {
            return Err(MemoryError::Index {
                index: self.name().into(),
                detail: "injected indexing failure".into(),
            });
        }
        Ok(())
    }

    fn forget(&self, id: MemoryId) -> Result<()> {
        LocalEmbeddingIndex.forget(id)
    }

    fn similarity(&self, query: &str, records: &[MemoryRecord]) -> Result<Vec<f32>> {
        LocalEmbeddingIndex.similarity(query, records)
    }
}

#[test]
fn proposal_accept_retry_repairs_a_failed_index_without_events() {
    let index = Arc::new(FailOnceIndex::default());
    let service = service().with_index(index.clone());
    let pending = propose(
        &service,
        candidate("Build changes require independent verification"),
    );
    assert!(matches!(
        service.accept(pending.id, None, Actor::user()),
        Err(MemoryError::Index { .. })
    ));
    let committed = service.store.memory(pending.id).unwrap().unwrap();
    assert!(committed.is_current());
    let history = events(&service);
    assert_eq!(
        service.accept(pending.id, None, Actor::user()).unwrap(),
        committed
    );
    assert_eq!(index.0.load(std::sync::atomic::Ordering::SeqCst), 2);
    assert_eq!(events(&service), history);
}

#[test]
fn proposal_admission_preserves_policy_and_does_not_count_pending_as_active() {
    let service = service();
    let hold = service.gate.hold();
    let (rejected, commit) = service
        .evaluate_operator(&hold, candidate("short"), Actor::user(), true)
        .unwrap();
    assert!(matches!(rejected, Admission::Rejected { .. }));
    assert!(commit.is_empty());
    drop(hold);
    let pending = propose(
        &service,
        candidate("Build changes require a repository verification run"),
    );
    let active = service
        .admit(candidate(&pending.text), Actor::user(), None, None)
        .unwrap();
    assert!(matches!(active, Admission::Accepted { .. }));
    assert!(active.record().unwrap().supersedes.is_none());
    let hold = service.gate.hold();
    let (duplicate, commit) = service
        .evaluate_operator(&hold, candidate(&pending.text), Actor::user(), true)
        .unwrap();
    assert!(
        matches!(duplicate, Admission::Rejected { ref reason } if reason.contains("redundant"))
    );
    assert!(commit.is_empty());
}

#[test]
fn proposal_accept_defers_supersession_and_is_idempotent_without_events() {
    let service = service();
    let old = admit(&service, candidate("Use the old build verification script"));
    let mut replacement = candidate("Use the new build verification script");
    replacement.supersedes = Some(old.id);
    let pending = propose(&service, replacement);
    assert!(service.store.memory(old.id).unwrap().unwrap().is_current());
    assert_eq!(
        service.retrieve(&RetrievalRequest::default()).unwrap()[0]
            .record
            .id,
        old.id
    );
    let accepted = service
        .accept(
            pending.id,
            Some("Confirmed by three clean runs".into()),
            Actor::user(),
        )
        .unwrap();
    assert!(accepted.is_current());
    assert!(accepted.accepted_at.is_some() && accepted.proposed_at.is_some());
    assert_eq!(
        service.store.memory(old.id).unwrap().unwrap().superseded_by,
        Some(accepted.id)
    );
    let history = events(&service);
    assert!(history.iter().any(|e| matches!(&e.kind, EventKind::MemoryAccepted { memory, reason: Some(reason) } if *memory == pending.id && reason == "Confirmed by three clean runs")));
    assert!(history.iter().any(|e| matches!(&e.kind, EventKind::MemorySuperseded { old: id, new, reason: Some(reason) } if *id == old.id && *new == accepted.id && reason == "Confirmed by three clean runs")));
    assert_eq!(
        service.accept(pending.id, None, Actor::user()).unwrap(),
        accepted
    );
    assert_eq!(events(&service), history);
    assert_eq!(
        service.retrieve(&RetrievalRequest::default()).unwrap()[0]
            .record
            .id,
        accepted.id
    );
    assert!(
        service
            .store
            .memories(&MemoryQuery {
                proposed: true,
                ..Default::default()
            })
            .unwrap()
            .is_empty()
    );
}

#[test]
fn proposal_implicit_duplicate_replacement_is_also_deferred() {
    let service = service();
    let text = "Build conventions are checked by the verification script";
    let old = service
        .admit(candidate(text), Actor::user(), None, None)
        .unwrap()
        .record()
        .unwrap()
        .clone();
    let mut stronger = candidate(text);
    stronger.importance = 0.9;
    let pending = propose(&service, stronger);
    assert_eq!(pending.supersedes, Some(old.id));
    assert!(events(&service).iter().any(|event| {
        matches!(&event.kind, EventKind::Custom { name, payload }
        if name == "memory.proposed" && *payload == serde_json::json!({
            "memory":pending.id, "scope":{"scope":"global"},
            "supersedes":old.id, "tags":["build"],
        }))
    }));
    assert!(service.store.memory(old.id).unwrap().unwrap().is_current());
    service.accept(pending.id, None, Actor::user()).unwrap();
    assert_eq!(
        service.store.memory(old.id).unwrap().unwrap().superseded_by,
        Some(pending.id)
    );
}

#[test]
fn proposal_reject_is_idempotent_and_restore_reopens_review_without_recall() {
    let service = service();
    let pending = propose(
        &service,
        candidate("Every build must use the verification script"),
    );
    let rejected = service
        .reject(pending.id, "Insufficient evidence".into(), Actor::user())
        .unwrap();
    assert!(rejected.forgotten_at.is_some() && rejected.is_proposed());
    let history = events(&service);
    assert!(history.iter().any(|e| matches!(&e.kind, EventKind::Custom { name, payload } if name == "memory.proposal_rejected" && payload["memory"] == pending.id.to_string() && payload["reason"] == "Insufficient evidence")));
    assert_eq!(
        service
            .reject(pending.id, "Retry".into(), Actor::user())
            .unwrap(),
        rejected
    );
    assert_eq!(events(&service), history);
    assert!(service.accept(pending.id, None, Actor::user()).is_err());
    assert_eq!(events(&service), history);
    assert!(
        service
            .store
            .memories(&MemoryQuery {
                proposed: true,
                ..Default::default()
            })
            .unwrap()
            .is_empty()
    );
    assert_eq!(
        service
            .store
            .memories(&MemoryQuery {
                proposed: true,
                include_forgotten: true,
                ..Default::default()
            })
            .unwrap(),
        vec![rejected]
    );
    let restored = service.restore(pending.id, Actor::user()).unwrap();
    assert!(restored.is_proposed() && restored.forgotten_at.is_none());
    assert!(!restored.is_current());
    assert!(
        service
            .retrieve(&RetrievalRequest::default())
            .unwrap()
            .is_empty()
    );
    assert!(
        service
            .accept(pending.id, None, Actor::user())
            .unwrap()
            .is_current()
    );
}

#[test]
fn proposal_stale_supersession_target_fails_without_mutations_or_events() {
    for forget in [true, false] {
        let service = service();
        let old = service
            .admit(
                candidate("Every build runs the old verification script"),
                Actor::user(),
                None,
                None,
            )
            .unwrap()
            .record()
            .unwrap()
            .clone();
        let mut next = candidate("Every build runs the new verification script");
        next.supersedes = Some(old.id);
        let pending = propose(&service, next);
        if forget {
            service
                .forget(old.id, Some("Retired".into()), Actor::user())
                .unwrap();
        } else {
            let mut alternate = candidate("Every build runs an alternate verification script");
            alternate.supersedes = Some(old.id);
            service.admit(alternate, Actor::user(), None, None).unwrap();
        }
        let history = events(&service);
        let predecessor = service.store.memory(old.id).unwrap();
        let error = service.accept(pending.id, None, Actor::user()).unwrap_err();
        assert!(error.to_string().contains("current"), "{error}");
        assert_eq!(service.store.memory(pending.id).unwrap(), Some(pending));
        assert_eq!(service.store.memory(old.id).unwrap(), predecessor);
        assert_eq!(events(&service), history);
    }
}

#[test]
fn proposal_review_refuses_agents_and_preserves_the_pending_record() {
    let service = service();
    let pending = propose(
        &service,
        candidate("Build changes always need an independent review"),
    );
    let by = Actor::Agent { id: AgentId::new() };
    let history = events(&service);
    assert!(
        service
            .accept(pending.id, None, by.clone())
            .unwrap_err()
            .to_string()
            .contains("operator")
    );
    assert!(
        service
            .reject(pending.id, "Agent decision".into(), by.clone())
            .unwrap_err()
            .to_string()
            .contains("operator")
    );
    let hold = service.gate.hold();
    assert!(
        service
            .evaluate_operator(
                &hold,
                candidate("Agents cannot submit operator proposals"),
                by,
                true
            )
            .is_err()
    );
    assert_eq!(service.store.memory(pending.id).unwrap(), Some(pending));
    assert_eq!(events(&service), history);
}

#[test]
fn proposal_review_errors_are_explicit_for_missing_nonproposal_and_curated_ids() {
    let service = service();
    let missing = MemoryId::new();
    assert!(
        matches!(service.accept(missing, None, Actor::user()), Err(MemoryError::NotFound(id)) if id == missing)
    );
    assert!(
        matches!(service.reject(missing, "Missing".into(), Actor::user()), Err(MemoryError::NotFound(id)) if id == missing)
    );
    let ordinary = service
        .admit(
            candidate("An ordinary operator memory is admitted immediately"),
            Actor::user(),
            None,
            None,
        )
        .unwrap()
        .record()
        .unwrap()
        .id;
    assert!(
        service
            .accept(ordinary, None, Actor::user())
            .unwrap_err()
            .to_string()
            .contains("never proposed")
    );
    assert!(
        service
            .reject(ordinary, "Invalid".into(), Actor::user())
            .is_err()
    );
    let pending = propose(
        &service,
        candidate("A proposal requires a human decision before recall"),
    );
    assert!(
        service
            .reject(pending.id, "  ".into(), Actor::user())
            .is_err()
    );
    assert!(
        service
            .accept(pending.id, Some(" ".into()), Actor::user())
            .is_err()
    );
    service.accept(pending.id, None, Actor::user()).unwrap();
    assert!(
        service
            .reject(pending.id, "Too late".into(), Actor::user())
            .is_err()
    );
    service.forget(pending.id, None, Actor::user()).unwrap();
    assert!(service.accept(pending.id, None, Actor::user()).is_err());
}
