//! Consolidation as the runtime offers it: an operator's command, dry run or not.
use super::*;
use aether_core::memory::{MemoryCandidate, MemoryKind, MemoryRecord, MemoryScope};
use aether_memory::ClusterOutcome;

const RESTATED: [&str; 2] = [
    "Release procedure: bump VERSION, move the Unreleased section of CHANGELOG.md under the new version, then push a vX.Y.Z tag; the release workflow builds and publishes the archives.",
    "To cut a release edit VERSION and CHANGELOG.md, then push a tag like v2.3.0; CI then builds the release archives and publishes them.",
];

/// Agent-written memories, put straight into the store as admission would.
fn seed(store: &dyn Store) -> Vec<MemoryRecord> {
    let records: Vec<MemoryRecord> = RESTATED
        .iter()
        .zip([0.8, 0.6])
        .map(|(text, importance)| {
            let candidate = MemoryCandidate {
                scope: MemoryScope::Global,
                kind: MemoryKind::Fact,
                text: (*text).into(),
                structured: None,
                importance,
                confidence: 0.8,
                tags: Vec::new(),
                provenance: Default::default(),
                supersedes: None,
            };
            MemoryRecord::from_candidate(candidate, now())
        })
        .collect();
    let mut commit = Commit::new();
    records
        .iter()
        .for_each(|r| commit.push_mutation(Mutation::PutMemory(r.clone())));
    store.commit(commit).unwrap();
    records
}

#[tokio::test]
async fn an_operator_consolidates_and_an_agent_cannot() {
    let h = harness(Vec::new()).await;
    let records = seed(h.store.as_ref());
    let (survivor, member) = (records[0].id, records[1].id);
    let agent = Actor::agent(AgentId::new());
    assert!(h.runtime.consolidate_memories(None, false, agent).is_err());

    let planned = h
        .runtime
        .consolidate_memories(None, true, Actor::user())
        .unwrap();
    assert_eq!(planned.clusters.len(), 1);
    assert_eq!(planned.clusters[0].outcome, ClusterOutcome::Planned);
    assert!(
        h.store.memory(member).unwrap().unwrap().is_current(),
        "a dry run writes nothing"
    );

    let merged = h
        .runtime
        .consolidate_memories(None, false, Actor::user())
        .unwrap();
    assert_eq!(merged.clusters[0].survivor, survivor);
    assert_eq!(merged.clusters[0].members, [member]);
    let retired = h.store.memory(member).unwrap().unwrap();
    assert_eq!(
        retired.superseded_by,
        Some(survivor),
        "kept, pointing at its survivor"
    );
    assert!(
        h.runtime
            .rollback_memory_abstraction(survivor, Actor::user())
            .is_err()
    );
}
