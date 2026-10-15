use super::*;
use aether_core::id::MemoryFeedbackId;
use aether_runtime::RuntimeError;

#[tokio::test]
async fn feedback_reports_missing_entities_and_validates_reasons_and_limits() {
    let (h, spec, agent) = fixture().await;
    show(&h, agent, spec.task, vec![spec.memory], true);
    for field in ["memory", "task", "invocation"] {
        let mut invalid = spec.clone();
        match field {
            "memory" => invalid.memory = MemoryId::new(),
            "task" => invalid.task = TaskId::new(),
            _ => invalid.invocation = Some(InvocationId::new()),
        }
        let error = h
            .runtime
            .record_memory_feedback(invalid, Actor::user())
            .unwrap_err();
        assert!(matches!(error, RuntimeError::NotFound(_)));
        assert!(error.to_string().contains(field));
    }
    let first = record(&h, &spec);
    let before = h.store.last_seq().unwrap();
    for reason in [" ".into(), "x".repeat(4097)] {
        let mut invalid = spec.clone();
        invalid.reason = reason.clone();
        assert!(matches!(
            h.runtime.record_memory_feedback(invalid, Actor::user()),
            Err(RuntimeError::Invalid(_))
        ));
        assert!(matches!(
            h.runtime
                .retract_memory_feedback(first.id, reason, Actor::user()),
            Err(RuntimeError::Invalid(_))
        ));
    }
    for limit in [0, 101, usize::MAX] {
        assert!(matches!(
            h.runtime
                .memory_feedback(spec.memory, Some(limit), &Actor::user()),
            Err(RuntimeError::Invalid(_))
        ));
    }
    assert!(matches!(
        h.runtime
            .retract_memory_feedback(MemoryFeedbackId::new(), "missing".into(), Actor::user()),
        Err(RuntimeError::NotFound(_))
    ));
    assert!(matches!(
        h.runtime
            .memory_feedback(MemoryId::new(), None, &Actor::user()),
        Err(RuntimeError::NotFound(_))
    ));
    assert_eq!(h.store.last_seq().unwrap(), before);
    h.runtime.shutdown().await;
}

#[tokio::test]
async fn feedback_lists_newest_first_including_retractions_and_reports_truncation() {
    let (h, spec, agent) = fixture().await;
    show(&h, agent, spec.task, vec![spec.memory], true);
    let mut ids = Vec::new();
    for _ in 0..101 {
        ids.push(record(&h, &spec).id);
    }
    let last = *ids.last().unwrap();
    h.runtime
        .retract_memory_feedback(last, "Withdraw latest".into(), Actor::user())
        .unwrap();
    let found = h
        .runtime
        .memory_feedback(spec.memory, None, &Actor::user())
        .unwrap();
    assert_eq!(found.records.len(), 100);
    assert!(found.truncated);
    assert_eq!(found.records[0].id, last);
    assert!(found.records[0].retracted_at.is_some());
    assert!(
        found
            .records
            .is_sorted_by(|a, b| (a.created_at, a.id) >= (b.created_at, b.id))
    );
    let small = h
        .runtime
        .memory_feedback(spec.memory, Some(1), &Actor::user())
        .unwrap();
    assert_eq!(small.records.len(), 1);
    assert!(small.truncated);
    h.runtime.shutdown().await;
}

#[tokio::test]
async fn feedback_does_not_change_retrieval_bytes_or_memory_records() {
    let (h, spec, agent) = fixture().await;
    show(&h, agent, spec.task, vec![spec.memory], true);
    let request = aether_memory::RetrievalRequest {
        scope: None,
        role: Some("worker".into()),
        agent: Some(agent),
        ancestors: vec![],
        query: Some("release build".into()),
        tags: vec![],
        kinds: vec![],
        limit: 6,
    };
    let retrieve = || {
        let results = h.runtime.memory().retrieve(&request).unwrap();
        assert!(!results.is_empty());
        assert_eq!(results[0].record.id, spec.memory);
        results
            .into_iter()
            .map(|r| serde_json::to_vec(&(r.record, r.score)).unwrap())
            .collect::<Vec<_>>()
    };
    let before = retrieve();
    let record = record(&h, &spec);
    assert_eq!(retrieve(), before);
    h.runtime
        .retract_memory_feedback(record.id, "Withdraw".into(), Actor::user())
        .unwrap();
    assert_eq!(retrieve(), before);
    h.runtime.shutdown().await;
}

#[tokio::test]
async fn feedback_accepts_a_memory_superseded_after_it_was_shown() {
    let (h, spec, agent) = fixture().await;
    show(&h, agent, spec.task, vec![spec.memory], true);
    let replacement = h
        .runtime
        .create_memory(
            serde_json::from_value(json!({
                "text":"Always verify release builds with the updated scripts/release.sh",
                "kind":"lesson", "scope":{"scope":"global"}, "supersedes":spec.memory,
            }))
            .unwrap(),
            Actor::user(),
        )
        .unwrap();
    assert_eq!(
        h.store.memory(spec.memory).unwrap().unwrap().superseded_by,
        Some(replacement.id)
    );
    let feedback = record(&h, &spec);
    assert_eq!(feedback.spec.memory, spec.memory);
    h.runtime.shutdown().await;
}
