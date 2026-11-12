use super::*;
use crate::{MemoryHoldout, MemoryStaleMark};
use aether_core::id::{AgentId, MemoryId, TaskId};

fn stored_memory(store: &SqliteStore) -> MemoryId {
    let record = MemoryRecord::from_candidate(
        MemoryCandidate {
            scope: MemoryScope::Global,
            kind: MemoryKind::Fact,
            text: "Run make ci before pushing.".into(),
            structured: None,
            importance: 0.8,
            confidence: 0.9,
            tags: vec![],
            provenance: MemoryProvenance::default(),
            supersedes: None,
        },
        now(),
    );
    let id = record.id;
    store
        .commit(Commit::new().mutate(Mutation::PutMemory(record)))
        .unwrap();
    id
}

fn holdout(memory: MemoryId, agent: AgentId, task: Option<TaskId>, rate: f32) -> MemoryHoldout {
    MemoryHoldout {
        memory,
        agent,
        task,
        at: now(),
        rate,
        score: 0.6,
    }
}

#[test]
fn a_stale_mark_is_read_for_the_asked_memories_replaced_and_cleared() {
    let store = SqliteStore::open_in_memory().unwrap();
    let (marked, plain) = (stored_memory(&store), stored_memory(&store));
    let mark = |reason: &str, revision: Option<&str>| {
        Mutation::MarkMemoryStale(MemoryStaleMark {
            memory: marked,
            reason: reason.into(),
            since: now(),
            revision: revision.map(str::to_owned),
        })
    };
    store
        .commit(Commit::new().mutate(mark("first", Some("c2"))))
        .unwrap();
    assert_eq!(
        store.memory_stale_marks(&[marked]).unwrap()[&marked]
            .revision
            .as_deref(),
        Some("c2")
    );
    store
        .commit(Commit::new().mutate(mark("second", None)))
        .unwrap();
    let marks = store.memory_stale_marks(&[marked, plain]).unwrap();
    assert_eq!(marks.len(), 1);
    assert_eq!(
        (
            marks[&marked].reason.as_str(),
            marks[&marked].revision.as_deref()
        ),
        ("second", None),
        "a mark is replaced, not stacked"
    );
    assert!(store.memory_stale_marks(&[]).unwrap().is_empty());

    store
        .commit(Commit::new().mutate(Mutation::ClearMemoryStale(marked)))
        .unwrap();
    store
        .commit(Commit::new().mutate(Mutation::ClearMemoryStale(plain)))
        .unwrap();
    assert!(
        store
            .memory_stale_marks(&[marked, plain])
            .unwrap()
            .is_empty()
    );
}

#[test]
fn a_stale_mark_needs_a_stored_memory_and_a_reason() {
    let store = SqliteStore::open_in_memory().unwrap();
    let stored = stored_memory(&store);
    for (memory, reason, revision) in [
        (MemoryId::new(), "gone", None),
        (stored, "  ", None),
        (stored, "gone", Some(" ")),
    ] {
        let refused = store.commit(Commit::new().mutate(Mutation::MarkMemoryStale(
            MemoryStaleMark {
                memory,
                reason: reason.into(),
                since: now(),
                revision: revision.map(str::to_owned),
            },
        )));
        assert!(
            matches!(refused, Err(StoreError::Corrupt(_))),
            "{refused:?}"
        );
    }
    assert!(store.memory_stale_marks(&[stored]).unwrap().is_empty());
}

#[test]
fn a_holdout_is_logged_once_per_unit_and_read_back_in_order() {
    let store = SqliteStore::open_in_memory().unwrap();
    let memory = stored_memory(&store);
    let agent = AgentId::new();
    let (task, other_task) = (TaskId::new(), TaskId::new());
    for entry in [
        holdout(memory, agent, Some(task), 0.05),
        holdout(memory, agent, Some(task), 0.05),
        holdout(memory, agent, Some(other_task), 0.05),
        holdout(memory, agent, None, 1.0),
    ] {
        store
            .commit(Commit::new().mutate(Mutation::LogMemoryHoldout(entry)))
            .unwrap();
    }
    let logged = store.memory_holdouts(memory).unwrap();
    let tasks: Vec<Option<TaskId>> = logged.iter().map(|h| h.task).collect();
    assert_eq!(tasks, [Some(task), Some(other_task), None]);
    assert!(
        logged
            .iter()
            .all(|h| h.memory == memory && h.agent == agent)
    );
    assert_eq!(logged[2].rate, 1.0);
    assert_eq!(logged[0].score, 0.6);
}

#[test]
fn a_holdout_needs_a_stored_memory_and_a_rate_in_range() {
    let store = SqliteStore::open_in_memory().unwrap();
    let stored = stored_memory(&store);
    let agent = AgentId::new();
    for entry in [
        holdout(MemoryId::new(), agent, None, 0.05),
        holdout(stored, agent, None, 1.5),
        holdout(stored, agent, None, f32::NAN),
    ] {
        let refused = store.commit(Commit::new().mutate(Mutation::LogMemoryHoldout(entry)));
        assert!(
            matches!(refused, Err(StoreError::Corrupt(_))),
            "{refused:?}"
        );
    }
    assert!(store.memory_holdouts(stored).unwrap().is_empty());
}
