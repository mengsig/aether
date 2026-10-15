use super::*;
use aether_runtime::RuntimeError;

#[tokio::test]
async fn feedback_refuses_memories_seen_only_by_a_later_call() {
    let (h, mut spec, agent) = fixture().await;
    let early = show(&h, agent, spec.task, vec![], true);
    let later = show(&h, agent, spec.task, vec![spec.memory], true);
    assert!(early.created_at < later.created_at);
    for failed in [false, true] {
        spec.invocation = Some(call(&h, &early, failed));
        let before = h.store.last_seq().unwrap();
        let error = h
            .runtime
            .record_memory_feedback(spec.clone(), Actor::user())
            .unwrap_err();
        assert!(error.to_string().contains("was not in context"));
        assert_eq!(h.store.last_seq().unwrap(), before);
        spec.invocation = Some(call(&h, &later, failed));
        record(&h, &spec);
    }
    h.runtime.shutdown().await;
}

#[tokio::test]
async fn feedback_failed_call_must_belong_to_the_owner_and_owner_must_exist() {
    let (h, mut spec, agent) = fixture().await;
    let mut snapshot = show(&h, agent, spec.task, vec![spec.memory], true);
    snapshot.agent_id = AgentId::new();
    spec.invocation = Some(call(&h, &snapshot, true));
    let before = h.store.last_seq().unwrap();
    assert!(matches!(
        h.runtime
            .record_memory_feedback(spec.clone(), Actor::user()),
        Err(RuntimeError::NotFound(_))
    ));
    assert_eq!(h.store.last_seq().unwrap(), before);
    snapshot.agent_id = agent;
    spec.invocation = Some(call(&h, &snapshot, true));
    record(&h, &spec);
    let mut task = h.store.task(spec.task).unwrap().unwrap();
    task.owner = None;
    h.store
        .commit(Commit::new().mutate(Mutation::UpsertTask(task)))
        .unwrap();
    assert!(matches!(
        h.runtime
            .record_memory_feedback(spec.clone(), Actor::user()),
        Err(RuntimeError::NotFound(_))
    ));
    spec.invocation = Some(call(&h, &snapshot, false));
    assert!(matches!(
        h.runtime.record_memory_feedback(spec, Actor::user()),
        Err(RuntimeError::NotFound(_))
    ));
    h.runtime.shutdown().await;
}

#[tokio::test]
async fn feedback_refuses_memories_seen_only_by_another_task() {
    refuses_foreign_task(false).await;
}

#[tokio::test]
async fn feedback_refuses_memories_seen_only_by_a_child_task() {
    refuses_foreign_task(true).await;
}

async fn refuses_foreign_task(child: bool) {
    let (h, spec, agent) = fixture().await;
    let parent = h.store.task(spec.task).unwrap().unwrap();
    let mut other = aether_core::task::Task::new("Other task", Actor::user(), parent.run, now());
    other.owner = Some(agent);
    other.parent_task = child.then_some(spec.task);
    let id = other.id;
    h.store
        .commit(Commit::new().mutate(Mutation::UpsertTask(other)))
        .unwrap();
    show(&h, agent, id, vec![spec.memory], true);
    let before = h.store.last_seq().unwrap();
    let error = h
        .runtime
        .record_memory_feedback(spec.clone(), Actor::user())
        .unwrap_err();
    assert!(error.to_string().contains("was not in context"));
    assert_eq!(h.store.last_seq().unwrap(), before);
    h.runtime.shutdown().await;
}

#[tokio::test]
async fn feedback_keyed_retry_with_a_different_body_conflicts_in_the_runtime() {
    let (h, mut spec, agent) = fixture().await;
    show(&h, agent, spec.task, vec![spec.memory], true);
    h.runtime
        .record_memory_feedback_keyed(spec.clone(), Actor::user(), Some(submission(&spec)))
        .unwrap();
    spec.reason = "A different review".into();
    let before = h.store.last_seq().unwrap();
    let error = h
        .runtime
        .record_memory_feedback_keyed(spec.clone(), Actor::user(), Some(submission(&spec)))
        .unwrap_err();
    assert!(
        error
            .to_string()
            .contains("a different request needs a new key")
    );
    assert_eq!(h.store.last_seq().unwrap(), before);
    assert_eq!(h.store.memory_feedback(spec.memory, 100).unwrap().len(), 1);
    h.runtime.shutdown().await;
}

#[tokio::test]
async fn feedback_accepts_the_named_calls_own_stored_memories_without_an_announcement() {
    let (h, mut spec, agent) = fixture().await;
    let snapshot = show(&h, agent, spec.task, vec![spec.memory], false);
    for failed in [false, true] {
        spec.invocation = Some(call(&h, &snapshot, failed));
        let feedback = record(&h, &spec);
        assert_eq!(feedback.spec.invocation, spec.invocation);
        assert_eq!(feedback.spec.memory, spec.memory);
    }
    h.runtime.shutdown().await;
}
