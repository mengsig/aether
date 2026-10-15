use super::*;
use aether_core::memory_feedback::{MemoryFeedback, MemoryFeedbackSpec};

#[path = "memory_feedback/support.rs"]
mod support;
use support::*;

#[tokio::test]
async fn feedback_refuses_memory_outside_task_context_and_unannounced_snapshots() {
    let (h, spec, agent) = fixture().await;
    show(&h, agent, spec.task, vec![spec.memory], false);
    let before = h.store.last_seq().unwrap();
    let error = h
        .runtime
        .record_memory_feedback(spec.clone(), Actor::user())
        .unwrap_err();
    assert!(error.to_string().contains("was not in context"));
    assert_eq!(h.store.last_seq().unwrap(), before);
    assert!(
        h.store
            .memory_feedback(spec.memory, 100)
            .unwrap()
            .is_empty()
    );
    let proposed = h
        .runtime
        .create_memory(
            serde_json::from_value(json!({
                "text":"Always get independent release review", "kind":"lesson",
                "scope":{"scope":"global"}, "proposed":true,
            }))
            .unwrap(),
            Actor::user(),
        )
        .unwrap();
    show(&h, agent, spec.task, vec![spec.memory], true);
    let mut unseen = spec.clone();
    unseen.memory = proposed.id;
    assert!(
        h.runtime
            .record_memory_feedback(unseen, Actor::user())
            .unwrap_err()
            .to_string()
            .contains("was not in context")
    );
    record(&h, &spec);
    h.runtime.shutdown().await;
}

#[tokio::test]
async fn feedback_keyed_retry_is_atomic_and_replays_even_after_retraction() {
    let (h, spec, agent) = fixture().await;
    show(&h, agent, spec.task, vec![spec.memory], true);
    let key = submission(&spec);
    let first = h
        .runtime
        .record_memory_feedback_keyed(spec.clone(), Actor::user(), Some(key.clone()))
        .unwrap();
    let before = h.store.last_seq().unwrap();
    let again = h
        .runtime
        .record_memory_feedback_keyed(spec.clone(), Actor::user(), Some(key.clone()))
        .unwrap();
    assert_eq!(first, again);
    assert_eq!(h.store.last_seq().unwrap(), before);
    assert_eq!(
        h.store.memory_feedback(spec.memory, 100).unwrap(),
        vec![first.clone()]
    );
    h.runtime
        .retract_memory_feedback(first.id, "Review superseded".into(), Actor::user())
        .unwrap();
    let before = h.store.last_seq().unwrap();
    assert_eq!(
        h.runtime
            .record_memory_feedback_keyed(spec, Actor::user(), Some(key))
            .unwrap(),
        first
    );
    assert_eq!(h.store.last_seq().unwrap(), before);
    h.runtime.shutdown().await;
}

#[tokio::test]
async fn feedback_retraction_keeps_the_record_and_retries_without_a_second_event() {
    let (h, spec, agent) = fixture().await;
    show(&h, agent, spec.task, vec![spec.memory], true);
    let first = record(&h, &spec);
    let retracted = h
        .runtime
        .retract_memory_feedback(first.id, "Incorrect review".into(), Actor::user())
        .unwrap();
    assert!(retracted.retracted_at.is_some());
    assert_eq!(
        retracted.retraction_reason.as_deref(),
        Some("Incorrect review")
    );
    assert_eq!(retracted.spec, first.spec);
    assert_eq!(retracted.actor, first.actor);
    assert_eq!(retracted.created_at, first.created_at);
    let before = h.store.last_seq().unwrap();
    let again = h
        .runtime
        .retract_memory_feedback(first.id, "Different retry reason".into(), Actor::user())
        .unwrap();
    assert_eq!(again, retracted);
    assert_eq!(h.store.last_seq().unwrap(), before);
    let found = h
        .runtime
        .memory_feedback(spec.memory, None, &Actor::user())
        .unwrap();
    assert_eq!(found.records, vec![retracted]);
    assert!(!found.truncated);
    h.runtime.shutdown().await;
}

#[tokio::test]
async fn feedback_agent_refusal_precedes_even_keyed_replay_and_reads() {
    let (h, spec, agent) = fixture().await;
    show(&h, agent, spec.task, vec![spec.memory], true);
    let key = submission(&spec);
    let first = h
        .runtime
        .record_memory_feedback_keyed(spec.clone(), Actor::user(), Some(key.clone()))
        .unwrap();
    let before = h.store.last_seq().unwrap();
    let by = Actor::Agent { id: agent };
    let errors = [
        h.runtime
            .record_memory_feedback_keyed(spec.clone(), by.clone(), Some(key))
            .unwrap_err(),
        h.runtime
            .record_memory_feedback(spec.clone(), by.clone())
            .unwrap_err(),
        h.runtime
            .retract_memory_feedback(first.id, "Agent opinion".into(), by.clone())
            .unwrap_err(),
        h.runtime
            .memory_feedback(spec.memory, None, &by)
            .unwrap_err(),
    ];
    assert!(
        errors
            .iter()
            .all(|error| error.to_string().contains("operator"))
    );
    assert_eq!(h.store.last_seq().unwrap(), before);
    assert_eq!(
        h.store.memory_feedback_record(first.id).unwrap(),
        Some(first)
    );
    let tools = h
        .runtime
        .engine_tool_specs(&h.store.agent(agent).unwrap().unwrap());
    for name in [
        "record_memory_feedback",
        "retract_memory_feedback",
        "memory_feedback",
    ] {
        assert!(!tools.iter().any(|tool| tool.name == name));
    }
    h.runtime.shutdown().await;
}

#[tokio::test]
async fn feedback_accepts_earlier_context_beyond_the_query_cap_and_after_forgetting() {
    let (h, mut spec, agent) = fixture().await;
    let first = show(&h, agent, spec.task, vec![], true);
    for _ in 0..256 {
        show(&h, agent, spec.task, vec![], true);
    }
    show(&h, agent, spec.task, vec![spec.memory], true);
    let answer = h
        .runtime
        .memories_in_context(Some(spec.task), None, &Actor::user())
        .unwrap();
    assert!(answer.truncated);
    assert!(answer.snapshots.iter().all(|s| s.memories.is_empty()));
    spec.invocation = Some(call(&h, &first, false));
    assert!(
        h.runtime
            .record_memory_feedback(spec.clone(), Actor::user())
            .unwrap_err()
            .to_string()
            .contains("was not in context")
    );
    let later = show(&h, agent, spec.task, vec![], true);
    spec.invocation = Some(call(&h, &later, false));
    h.runtime
        .forget_memory(
            spec.memory,
            Some("Retired after this task".into()),
            Actor::user(),
        )
        .unwrap();
    record(&h, &spec);
    assert!(
        h.store
            .memory(spec.memory)
            .unwrap()
            .unwrap()
            .forgotten_at
            .is_some()
    );
    h.runtime.shutdown().await;
}

#[tokio::test]
async fn feedback_checks_invocation_rows_and_failed_events_against_the_snapshots_task() {
    let (h, mut spec, agent) = fixture().await;
    let own = show(&h, agent, spec.task, vec![spec.memory], true);
    let other = show(&h, agent, TaskId::new(), vec![spec.memory], true);
    let before = h.store.last_seq().unwrap();
    for failed in [false, true] {
        let count = h.store.memory_feedback(spec.memory, 100).unwrap().len();
        spec.invocation = Some(call(&h, &other, failed));
        let error = h
            .runtime
            .record_memory_feedback(spec.clone(), Actor::user())
            .unwrap_err();
        assert!(error.to_string().contains("not a call of task"));
        assert_eq!(
            h.store.memory_feedback(spec.memory, 100).unwrap().len(),
            count
        );
        spec.invocation = Some(call(&h, &own, failed));
        let found = record(&h, &spec);
        assert_eq!(found.spec.invocation, spec.invocation);
    }
    assert!(h.store.last_seq().unwrap() > before);
    h.runtime.shutdown().await;
}

#[path = "memory_feedback/validation.rs"]
mod validation;

#[path = "memory_feedback/transfer.rs"]
mod transfer;

#[path = "memory_feedback/eligibility.rs"]
mod eligibility;

#[path = "memory_feedback/event_membership.rs"]
mod event_membership;

#[path = "memory_feedback/locking.rs"]
mod locking;
