//! `memory_retrieval_explain` against real turns: what it reports is what
//! the snapshot's memory layer and the holdout log say happened.
use super::*;
use aether_core::id::MemoryId;
use aether_core::memory::{
    MemoryCandidate, MemoryKind, MemoryScope, OperatorMemoryScope, OperatorMemorySpec,
};
use aether_core::memory_v2::TraceOutcome;
use aether_core::memory_v2_reports::RetrievalExplain;
use std::collections::BTreeSet;

#[path = "memory_v2/transfer.rs"]
mod transfer;

const OBJECTIVE: &str = "How is a release build made and signed?";

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

/// Three current memories, a note too long for the budget, and one each of
/// superseded, forgotten and proposed. Returns the three, the long note,
/// then the ones retrieval must never consider.
fn seed(h: &Harness) -> (Vec<MemoryId>, MemoryId, Vec<MemoryId>) {
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
    let long = MemoryRecord::from_candidate(
        MemoryCandidate {
            scope: MemoryScope::Global,
            kind: MemoryKind::Fact,
            text: "Unrelated archive notes about release lunch menus. ".repeat(250),
            structured: None,
            importance: 0.1,
            confidence: 0.1,
            tags: vec![],
            provenance: Default::default(),
            supersedes: None,
        },
        now(),
    );
    let trimmed = long.id;
    h.store
        .commit(Commit::new().mutate(Mutation::PutMemory(long)))
        .expect("seed");
    (
        vec![first, second, third],
        trimmed,
        vec![replaced, forgotten, proposed],
    )
}

fn scripted_task() -> Vec<aether_model::testing::ScriptedResponse> {
    vec![
        ScriptedClient::text("Checking the release notes."),
        ScriptedClient::tool_call("complete_task", json!({"summary": "done"})),
    ]
}

async fn finished_task(h: &Harness) -> (AgentId, TaskId) {
    let (agent, task) = h
        .runtime
        .create_agent(spec(OBJECTIVE), Actor::user())
        .expect("create");
    h.wait_until(agent.id, |s| s.is_terminal()).await;
    (agent.id, task.expect("an objective makes a task").id)
}

fn outcomes(explain: &RetrievalExplain, outcome: TraceOutcome) -> BTreeSet<MemoryId> {
    explain
        .turns
        .iter()
        .flat_map(|turn| turn.candidates.iter())
        .filter(|c| c.outcome == outcome)
        .map(|c| c.memory)
        .collect()
}

/// Every turn's injected list is the snapshot's layer, the trace marks
/// exactly those injected, and the note the budget dropped is trimmed.
#[tokio::test]
async fn explain_reports_what_each_real_turn_was_given_and_why_the_rest_was_not() {
    let h = harness(scripted_task()).await;
    let (kept, trimmed, never) = seed(&h);
    let (agent, task) = finished_task(&h).await;
    let explain = h
        .runtime
        .memory_retrieval_explain(Some(task), None, &Actor::user())
        .expect("explain");
    let context = h
        .runtime
        .memories_in_context(Some(task), None, &Actor::user())
        .expect("context");
    assert!(!explain.turns.is_empty());
    assert_eq!(explain.turns.len(), context.snapshots.len());
    for (turn, entry) in explain.turns.iter().zip(&context.snapshots) {
        let stored = h
            .store
            .snapshot(turn.snapshot)
            .expect("read")
            .expect("stored");
        assert_eq!(
            turn.injected, stored.memory_ids,
            "the layer the model was given"
        );
        assert_eq!(
            (turn.snapshot, &turn.invocations, turn.agent, turn.task),
            (entry.snapshot, &entry.invocations, agent, Some(task))
        );
        assert!(turn.traced, "a G3 turn records its trace");
        let policy = turn.policy.expect("a traced turn has a policy");
        assert_eq!(policy.max_items, 6);
        let injected: BTreeSet<_> = turn
            .candidates
            .iter()
            .filter(|c| c.outcome == TraceOutcome::Injected)
            .map(|c| c.memory)
            .collect();
        assert_eq!(injected, turn.injected.iter().copied().collect());
        assert!(turn.candidates.len() <= turn.pool as usize);
        let ranks: Vec<u32> = turn.candidates.iter().filter_map(|c| c.rank).collect();
        assert!(
            ranks.windows(2).all(|w| w[0] < w[1]),
            "ranked in order: {ranks:?}"
        );
    }
    assert_eq!(
        outcomes(&explain, TraceOutcome::Injected),
        kept.iter().copied().collect()
    );
    assert_eq!(
        outcomes(&explain, TraceOutcome::Trimmed),
        BTreeSet::from([trimmed])
    );
    assert!(outcomes(&explain, TraceOutcome::HeldOut).is_empty());
    for id in kept.iter().chain([&trimmed]) {
        assert!(h.store.memory_holdouts(*id).expect("holdouts").is_empty());
    }
    let considered: BTreeSet<_> = explain
        .turns
        .iter()
        .flat_map(|t| &t.candidates)
        .map(|c| c.memory)
        .collect();
    for id in &never {
        assert!(!considered.contains(id), "{id} is never a candidate");
    }

    let last = context
        .snapshots
        .last()
        .and_then(|s| s.invocations.last())
        .expect("a call");
    let one = h
        .runtime
        .memory_retrieval_explain(None, Some(*last), &Actor::user())
        .expect("by invocation");
    assert_eq!(one.turns.len(), 1);
    assert_eq!(&one.turns[0], explain.turns.last().expect("a turn"));
    let agent_actor = Actor::Agent { id: agent };
    assert!(matches!(
        h.runtime
            .memory_retrieval_explain(Some(task), None, &agent_actor),
        Err(aether_runtime::RuntimeError::Invalid(_))
    ));
    h.runtime.shutdown().await;
}

/// At rate 1 every fitting memory is held out: explain says so for each,
/// and the holdout log holds exactly those.
#[tokio::test]
async fn explain_reports_the_holdout_the_log_recorded() {
    let h = harness_configured(
        scripted_task(),
        |_| {},
        |config| {
            config.memory_retrieval.holdout_rate = 1.0;
        },
    )
    .await;
    let (kept, trimmed, _) = seed(&h);
    let (_, task) = finished_task(&h).await;
    let explain = h
        .runtime
        .memory_retrieval_explain(Some(task), None, &Actor::user())
        .expect("explain");
    assert!(explain.turns.iter().all(|turn| turn.injected.is_empty()));
    assert!(outcomes(&explain, TraceOutcome::Injected).is_empty());
    let held = outcomes(&explain, TraceOutcome::HeldOut);
    assert_eq!(held, kept.iter().copied().collect());
    for id in &held {
        assert_eq!(h.store.memory_holdouts(*id).expect("holdouts").len(), 1);
    }
    assert!(
        h.store
            .memory_holdouts(trimmed)
            .expect("holdouts")
            .is_empty()
    );
    assert_eq!(
        outcomes(&explain, TraceOutcome::Trimmed),
        BTreeSet::from([trimmed])
    );
    h.runtime.shutdown().await;
}
