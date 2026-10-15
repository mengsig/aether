use super::*;
use aether_core::id::MemoryFeedbackId;
use aether_core::memory_feedback::{FeedbackRanking, FeedbackTally};
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

fn ranking_request(agent: AgentId) -> aether_memory::RetrievalRequest {
    aether_memory::RetrievalRequest {
        scope: None,
        role: Some("worker".into()),
        agent: Some(agent),
        ancestors: vec![],
        query: Some("release build".into()),
        tags: vec![],
        kinds: vec![],
        limit: 6,
    }
}

/// Each result's record and score, serialized: the bytes recall hands on.
fn retrieved(h: &Harness, agent: AgentId) -> Vec<(Vec<u8>, f32)> {
    let results = h
        .runtime
        .memory()
        .retrieve(&ranking_request(agent))
        .unwrap();
    assert!(!results.is_empty());
    results
        .into_iter()
        .map(|r| (serde_json::to_vec(&r.record).unwrap(), r.score))
        .collect()
}

#[tokio::test]
async fn feedback_moves_only_the_score_and_only_once_two_tasks_agree() {
    let (h, spec, agent) = fixture().await;
    show(&h, agent, spec.task, vec![spec.memory], true);
    let before = retrieved(&h, agent);
    let first = record(&h, &spec);
    let latest = record(&h, &spec);
    assert_eq!(
        retrieved(&h, agent),
        before,
        "one task is below the threshold"
    );
    let (second_agent, second_task) = h
        .runtime
        .create_agent(
            CreateAgentSpec {
                start: false,
                ..super::spec("Check the release again")
            },
            Actor::user(),
        )
        .unwrap();
    let second = MemoryFeedbackSpec {
        task: second_task.unwrap().id,
        ..spec.clone()
    };
    show(&h, second_agent.id, second.task, vec![spec.memory], true);
    record(&h, &second);
    let after = retrieved(&h, agent);
    let factor = FeedbackTally { helped: 2, hurt: 0 }.factor();
    assert_eq!(after.len(), before.len());
    assert_eq!(after[0].0, before[0].0, "the memory record is unchanged");
    assert_eq!(after[0].1.to_bits(), (before[0].1 * factor).to_bits());
    let listed = h
        .runtime
        .memory_feedback(spec.memory, None, &Actor::user())
        .unwrap();
    assert_eq!(
        listed.ranking,
        FeedbackRanking {
            helped: 2,
            hurt: 0,
            factor
        }
    );
    let retract = |id| {
        h.runtime
            .retract_memory_feedback(id, "Withdraw".into(), Actor::user())
            .unwrap()
    };
    retract(latest.id);
    assert_eq!(
        retrieved(&h, agent),
        after,
        "the task's earlier vote counts"
    );
    retract(first.id);
    assert_eq!(retrieved(&h, agent), before);
    let listed = h
        .runtime
        .memory_feedback(spec.memory, None, &Actor::user())
        .unwrap();
    assert_eq!(
        listed.ranking,
        FeedbackRanking {
            helped: 1,
            hurt: 0,
            factor: 1.0
        }
    );
    h.runtime.shutdown().await;
}

#[tokio::test]
async fn a_failed_feedback_read_fails_retrieval_instead_of_ranking_without_it() {
    let inner: Arc<dyn Store> = Arc::new(SqliteStore::open_in_memory().unwrap());
    let store = Arc::new(CommitsMidPass::new(inner, Hook::FeedbackTallyRead));
    let (h, spec, agent) = fixture_on(Some(store.clone())).await;
    show(&h, agent, spec.task, vec![spec.memory], true);
    store.arm(Box::new(|_| {
        Err(aether_store::StoreError::Corrupt(
            "simulated feedback read failure".into(),
        ))
    }));
    let error = h
        .runtime
        .memory()
        .retrieve(&ranking_request(agent))
        .unwrap_err();
    assert!(store.fired());
    assert!(
        error
            .to_string()
            .contains("simulated feedback read failure"),
        "{error}"
    );
    assert!(!retrieved(&h, agent).is_empty());
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
