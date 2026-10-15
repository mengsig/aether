//! Which memories each model call saw: `snapshot.created` and the host query.
use super::*;
use aether_core::id::MemoryId;
use aether_core::memory::{
    MemoryCandidate, MemoryKind, MemoryScope, OperatorMemoryScope, OperatorMemorySpec,
};

fn fact(text: &str, supersedes: Option<MemoryId>, proposed: bool) -> OperatorMemorySpec {
    OperatorMemorySpec {
        text: text.into(),
        kind: MemoryKind::Fact,
        scope: OperatorMemoryScope::Global,
        importance: Some(0.9),
        confidence: Some(0.9),
        tags: vec!["release".into()],
        supersedes,
        proposed,
    }
}

/// Stored past admission, which caps length: a note too long for the memory
/// budget, ranked last so it is the one the budget trims.
fn oversized_note(h: &Harness) -> MemoryId {
    let record = MemoryRecord::from_candidate(
        MemoryCandidate {
            scope: MemoryScope::Global,
            kind: MemoryKind::Fact,
            text: "Unrelated archive notes about lunch menus. ".repeat(250),
            structured: None,
            importance: 0.1,
            confidence: 0.1,
            tags: vec![],
            provenance: Default::default(),
            supersedes: None,
        },
        now(),
    );
    let id = record.id;
    h.store
        .commit(aether_store::Commit::new().mutate(aether_store::Mutation::PutMemory(record)))
        .expect("seed");
    id
}

/// Three current memories, one the budget trims, and one each of forgotten,
/// superseded and proposed. Returns the three, then the ones never shown.
fn seed_memories(h: &Harness) -> (Vec<MemoryId>, Vec<MemoryId>) {
    let create = |spec| {
        h.runtime
            .create_memory(spec, Actor::user())
            .expect("memory")
            .id
    };
    let first = create(fact(
        "The release build runs through scripts/release.sh",
        None,
        false,
    ));
    let second = create(fact(
        "Release notes live in docs/releases for every build",
        None,
        false,
    ));
    let replaced = create(fact(
        "Release builds are signed with the old key",
        None,
        false,
    ));
    let third = create(fact(
        "Release builds are signed with the 2026 key",
        Some(replaced),
        false,
    ));
    let forgotten = create(fact(
        "Release builds go through the jenkins pipeline",
        None,
        false,
    ));
    h.runtime
        .forget_memory(forgotten, Some("retired".into()), Actor::user())
        .expect("forget");
    let proposed = create(fact("Release builds need two approvals", None, true));
    let trimmed = oversized_note(h);
    (
        vec![first, second, third],
        vec![replaced, forgotten, proposed, trimmed],
    )
}

/// The memory ids each `snapshot.created` of `task` names, with the system
/// prompt its stored snapshot holds.
fn announced(h: &Harness, task: TaskId) -> Vec<(Vec<MemoryId>, String)> {
    let filter = EventFilter {
        task: Some(task),
        types: vec!["snapshot.created".into()],
        ..Default::default()
    };
    h.store
        .events(0, 1_000, &filter)
        .expect("events")
        .into_iter()
        .map(|event| {
            let EventKind::SnapshotCreated {
                snapshot, layers, ..
            } = event.kind
            else {
                panic!("filtered to snapshot.created");
            };
            let layer = layers
                .into_iter()
                .find(|layer| layer.kind == LayerKind::RetrievedMemory)
                .expect("memory layer");
            let stored = h.store.snapshot(snapshot).expect("read").expect("stored");
            assert_eq!(layer.memories, stored.memory_ids, "one list, one source");
            assert_eq!(layer.items as usize, layer.memories.len());
            (layer.memories, stored.system_text())
        })
        .collect()
}

/// What the turn's retrieval offered context assembly, before the budget.
fn retrieved(h: &Harness, agent: AgentId, objective: &str) -> Vec<MemoryId> {
    let service =
        aether_memory::MemoryService::new(h.store.clone(), aether_store::WriteGate::new());
    let request = aether_memory::RetrievalRequest {
        scope: None,
        role: Some("worker".into()),
        agent: Some(agent),
        ancestors: vec![],
        query: Some(objective.into()),
        tags: vec![],
        kinds: vec![],
        limit: 6,
    };
    let found = service.retrieve(&request).expect("retrieve");
    found.into_iter().map(|hit| hit.record.id).collect()
}

async fn finished_task(h: &Harness, objective: &str) -> (AgentId, TaskId) {
    let (agent, task) = h
        .runtime
        .create_agent(spec(objective), Actor::user())
        .expect("create");
    h.wait_until(agent.id, |s| s.is_terminal()).await;
    (agent.id, task.expect("an objective makes a task").id)
}

fn scripted_task() -> Vec<aether_model::testing::ScriptedResponse> {
    vec![
        ScriptedClient::text("Checking the release notes."),
        ScriptedClient::tool_call("complete_task", json!({"summary": "done"})),
    ]
}

#[tokio::test]
async fn snapshot_created_names_exactly_the_rendered_memories_in_render_order() {
    let h = harness(scripted_task()).await;
    let (kept, never) = seed_memories(&h);
    let objective = "How is a release build made and signed?";
    let (agent, task) = finished_task(&h, objective).await;
    let trimmed = *never.last().expect("the oversized note");
    let candidates = retrieved(&h, agent, objective);
    assert_eq!(
        candidates.last(),
        Some(&trimmed),
        "a candidate the budget trims"
    );

    let snapshots = announced(&h, task);
    assert!(!snapshots.is_empty());
    for (memories, system) in snapshots {
        let mut sorted = memories.clone();
        sorted.sort();
        let mut expected = kept.clone();
        expected.sort();
        assert_eq!(sorted, expected, "the three current memories, nothing else");
        let at: Vec<usize> = memories
            .iter()
            .map(|id| system.find(&format!("[{id}]")).expect("rendered"))
            .collect();
        assert!(at.is_sorted(), "listed in the order the prompt shows them");
        for id in &never {
            assert!(!memories.contains(id));
            assert!(!system.contains(&id.to_string()), "{id} never rendered");
        }
    }
    h.runtime.shutdown().await;
}

/// By task: every snapshot of the task, oldest first, with the invocations
/// answered from it. By invocation: that one call's snapshot.
#[tokio::test]
async fn the_host_query_answers_by_task_and_by_invocation() {
    let h = harness(scripted_task()).await;
    let (kept, _) = seed_memories(&h);
    let (agent, task) = finished_task(&h, "How is a release build made and signed?").await;
    let by_task = h
        .runtime
        .memories_in_context(Some(task), None, &Actor::user())
        .expect("by task");
    let invocations = h.store.invocations_for_agent(agent).expect("invocations");
    assert!(!by_task.truncated);
    assert_eq!(by_task.snapshots.len(), invocations.len());
    let announced = announced(&h, task);
    for ((entry, call), (memories, _)) in by_task.snapshots.iter().zip(&invocations).zip(announced)
    {
        assert_eq!(entry.snapshot, call.snapshot_id);
        assert_eq!(entry.invocations, vec![call.id]);
        assert_eq!((entry.agent, entry.task), (agent, Some(task)));
        assert_eq!(entry.memories, memories);
        assert_eq!(entry.memories.len(), kept.len());
    }
    assert!(by_task.snapshots.is_sorted_by_key(|entry| entry.at));

    let last = invocations.last().expect("a call");
    let by_call = h
        .runtime
        .memories_in_context(None, Some(last.id), &Actor::user())
        .expect("by invocation");
    assert_eq!(by_call.snapshots.len(), 1);
    assert_eq!(
        &by_call.snapshots[0],
        by_task.snapshots.last().expect("one")
    );
    h.runtime.shutdown().await;
}

/// Daemon recovery and imported history record a failed call by its
/// `invocation.failed` event alone, with no invocation row to look up.
#[tokio::test]
async fn a_failed_call_with_no_invocation_row_is_found_by_its_event() {
    let h = harness(scripted_task()).await;
    seed_memories(&h);
    let (agent, task) = finished_task(&h, "How is a release build made and signed?").await;
    let before = h
        .runtime
        .memories_in_context(Some(task), None, &Actor::user())
        .expect("by task");
    let first = before.snapshots.first().expect("a snapshot").clone();
    assert!(!first.memories.is_empty());
    let failed = |task: Option<TaskId>| {
        let invocation = InvocationId::new();
        let mut draft = EventDraft::new(
            Actor::System,
            EventKind::InvocationFailed {
                class: None,
                invocation,
                snapshot: first.snapshot,
                error: "daemon restarted during the turn".into(),
                retryable: true,
                cancelled: false,
                partial: Vec::new(),
            },
        )
        .agent(agent);
        draft.task_id = task;
        h.store.commit(Commit::new().event(draft)).expect("commit");
        assert!(h.store.invocation(invocation).expect("read").is_none());
        invocation
    };
    let recovered = failed(None);
    let imported = failed(Some(task));

    for call in [recovered, imported] {
        let found = h
            .runtime
            .memories_in_context(None, Some(call), &Actor::user())
            .expect("found by its event");
        assert_eq!(found.snapshots.len(), 1);
        let entry = &found.snapshots[0];
        assert_eq!(
            (entry.snapshot, &entry.memories),
            (first.snapshot, &first.memories)
        );
        assert_eq!(entry.invocations, vec![call]);
    }
    let after = h
        .runtime
        .memories_in_context(Some(task), None, &Actor::user())
        .expect("by task");
    assert!(after.snapshots[0].invocations.contains(&imported));
    h.runtime.shutdown().await;
}

#[tokio::test]
async fn a_task_that_saw_no_memories_lists_its_snapshots_with_none() {
    let h = harness(scripted_task()).await;
    let (_, task) = finished_task(&h, "Summarise the repository layout").await;
    let found = h
        .runtime
        .memories_in_context(Some(task), None, &Actor::user())
        .expect("by task");
    assert_eq!(found.snapshots.len(), 2, "one per scripted model call");
    assert!(
        found
            .snapshots
            .iter()
            .all(|entry| entry.memories.is_empty())
    );
    let wire = serde_json::to_value(&found).expect("json");
    assert_eq!(wire["snapshots"][0]["memories"], json!([]));
    h.runtime.shutdown().await;
}

#[tokio::test]
async fn the_query_takes_exactly_one_scope_from_an_operator() {
    let h = harness(scripted_task()).await;
    let (agent, task) = finished_task(&h, "Summarise the repository layout").await;
    let rt = &h.runtime;
    let agent_actor = Actor::Agent { id: agent };
    let refused = rt.memories_in_context(Some(task), None, &agent_actor);
    assert!(refused.unwrap_err().to_string().contains("operator"));
    let call = InvocationId::new();
    for (task, invocation) in [(None, None), (Some(task), Some(call))] {
        let error = rt.memories_in_context(task, invocation, &Actor::user());
        assert!(error.unwrap_err().to_string().contains("exactly one"));
    }
    let missing = rt.memories_in_context(None, Some(call), &Actor::user());
    assert!(matches!(
        missing,
        Err(aether_runtime::RuntimeError::NotFound(_))
    ));
    let missing = rt.memories_in_context(Some(TaskId::new()), None, &Actor::user());
    assert!(matches!(
        missing,
        Err(aether_runtime::RuntimeError::NotFound(_))
    ));
    h.runtime.shutdown().await;
}
