use super::*;
use aether_core::memory_feedback::{MemoryEvidence, MemoryVerdict};

pub(super) async fn fixture() -> (Harness, MemoryFeedbackSpec, AgentId) {
    fixture_on(None).await
}

pub(super) async fn fixture_on(
    store: Option<Arc<dyn Store>>,
) -> (Harness, MemoryFeedbackSpec, AgentId) {
    let h = harness_full(Vec::new(), |_| {}, |_| {}, None, None, store, |_| {}).await;
    let mut agent_spec = spec("Verify feedback evidence");
    agent_spec.start = false;
    let (agent, task) = h.runtime.create_agent(agent_spec, Actor::user()).unwrap();
    let memory = h
        .runtime
        .create_memory(
            serde_json::from_value(json!({
                "text":"Always verify release builds with scripts/release.sh",
                "kind":"lesson", "scope":{"scope":"global"},
            }))
            .unwrap(),
            Actor::user(),
        )
        .unwrap();
    let task = task.unwrap();
    assert_eq!(task.owner, Some(agent.id));
    assert!(h.store.snapshot_memories(&[]).unwrap().is_empty());
    let feedback = MemoryFeedbackSpec {
        memory: memory.id,
        verdict: MemoryVerdict::Helped,
        evidence: MemoryEvidence::Review,
        task: task.id,
        invocation: None,
        reason: "Release check passed".into(),
    };
    (h, feedback, agent.id)
}

pub(super) fn show(
    h: &Harness,
    agent: AgentId,
    task: TaskId,
    memories: Vec<MemoryId>,
    announce: bool,
) -> InvocationSnapshot {
    let snapshot = InvocationSnapshot {
        id: SnapshotId::new(),
        agent_id: agent,
        task_id: Some(task),
        turn: 1,
        model: ModelRef::new("mock", "m"),
        adapter: None,
        capabilities: None,
        memory_ids: memories,
        layers: vec![],
        messages: vec![],
        tools: vec![],
        params: Default::default(),
        token_estimate: 0,
        created_at: now(),
    };
    assert_eq!(snapshot.task_id, Some(task));
    let mut commit = Commit::new().mutate(Mutation::PutSnapshot(snapshot.clone()));
    if announce {
        commit.push_event(announcement(&snapshot));
    }
    h.store.commit(commit).unwrap();
    assert_eq!(
        h.store.snapshot(snapshot.id).unwrap(),
        Some(snapshot.clone())
    );
    snapshot
}

pub(super) fn call(h: &Harness, snapshot: &InvocationSnapshot, failed: bool) -> InvocationId {
    let id = InvocationId::new();
    let commit = if failed {
        Commit::new().event(
            EventDraft::new(
                Actor::System,
                EventKind::InvocationFailed {
                    invocation: id,
                    snapshot: snapshot.id,
                    class: None,
                    error: "recovered".into(),
                    retryable: false,
                    cancelled: false,
                    partial: vec![],
                },
            )
            .agent(snapshot.agent_id),
        )
    } else {
        Commit::new().mutate(Mutation::PutInvocation(Invocation {
            id,
            snapshot_id: snapshot.id,
            agent_id: snapshot.agent_id,
            turn: snapshot.turn,
            model: snapshot.model.clone(),
            started_at: now(),
            finished_at: Some(now()),
            content: vec![],
            usage: Default::default(),
            stop_reason: None,
            error: None,
            provider_message_id: None,
        }))
    };
    h.store.commit(commit).unwrap();
    assert_eq!(h.store.invocation(id).unwrap().is_none(), failed);
    assert!(h.store.snapshot(snapshot.id).unwrap().is_some());
    id
}

pub(super) fn submission(spec: &MemoryFeedbackSpec) -> aether_core::idempotency::Submission {
    aether_core::idempotency::Submission::new(
        "feedback-retry",
        &serde_json::to_value(spec).unwrap(),
    )
    .unwrap()
}

pub(super) fn record(h: &Harness, spec: &MemoryFeedbackSpec) -> MemoryFeedback {
    let found = h
        .runtime
        .record_memory_feedback(spec.clone(), Actor::user())
        .unwrap();
    assert_eq!(&found.spec, spec);
    assert!(found.retracted_at.is_none());
    found
}

pub(super) fn announcement(snapshot: &InvocationSnapshot) -> EventDraft {
    use aether_core::snapshot::{ContextLayer, LayerPlacement};
    assert!(snapshot.task_id.is_some());
    assert_eq!(
        snapshot
            .memory_ids
            .iter()
            .collect::<std::collections::HashSet<_>>()
            .len(),
        snapshot.memory_ids.len()
    );
    EventDraft::new(
        Actor::System,
        EventKind::SnapshotCreated {
            snapshot: snapshot.id,
            turn: snapshot.turn,
            model: snapshot.model.clone(),
            token_estimate: snapshot.token_estimate,
            layers: vec![ContextLayer {
                kind: LayerKind::RetrievedMemory,
                text: None,
                tokens: 0,
                items: snapshot.memory_ids.len() as u32,
                placement: LayerPlacement::System,
                memories: snapshot.memory_ids.clone(),
            }],
        },
    )
    .agent(snapshot.agent_id)
    .task(snapshot.task_id.unwrap())
}
