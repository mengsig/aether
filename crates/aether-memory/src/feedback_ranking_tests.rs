use super::*;
use aether_core::id::{MemoryFeedbackId, TaskId};
use aether_core::memory::MemoryProvenance;
use aether_core::memory_feedback::{
    FeedbackTally, MemoryEvidence, MemoryFeedback, MemoryFeedbackSpec, MemoryVerdict,
};
use aether_core::time::Timestamp;
use aether_store::SqliteStore;
use chrono::{TimeZone, Utc};
use std::collections::BTreeMap;

struct Fixture {
    store: Arc<SqliteStore>,
    service: MemoryService,
}

fn fixture(index: bool) -> Fixture {
    let store = Arc::new(SqliteStore::open_in_memory().expect("store"));
    let service = MemoryService::new(store.clone(), WriteGate::new());
    let service = if index {
        service.with_index(Arc::new(LocalEmbeddingIndex))
    } else {
        service
    };
    Fixture { store, service }
}

/// Ahead of any test's clock, so every retrieval sees the same zero age and
/// two retrievals of one memory score it bit for bit alike.
fn future() -> Timestamp {
    Utc.with_ymd_and_hms(2100, 1, 1, 0, 0, 0).unwrap()
}

impl Fixture {
    fn put(&self, text: &str, scope: MemoryScope, importance: f32, pinned: bool) -> MemoryId {
        let mut record = MemoryRecord::from_candidate(
            MemoryCandidate {
                scope,
                kind: MemoryKind::Fact,
                text: text.into(),
                structured: None,
                importance,
                confidence: 0.8,
                tags: vec!["build".into()],
                provenance: MemoryProvenance::default(),
                supersedes: None,
            },
            future(),
        );
        record.pinned = pinned;
        let id = record.id;
        self.store
            .commit(Commit::new().mutate(Mutation::PutMemory(record)))
            .expect("put memory");
        id
    }

    fn vote(
        &self,
        memory: MemoryId,
        verdict: MemoryVerdict,
        evidence: MemoryEvidence,
    ) -> MemoryFeedbackId {
        let record = MemoryFeedback {
            id: MemoryFeedbackId::new(),
            spec: MemoryFeedbackSpec {
                memory,
                verdict,
                evidence,
                task: TaskId::new(),
                invocation: None,
                reason: "Checked".into(),
            },
            actor: Actor::user(),
            created_at: now(),
            retracted_at: None,
            retraction_reason: None,
        };
        let id = record.id;
        self.store
            .commit(Commit::new().mutate(Mutation::RecordMemoryFeedback(record)))
            .expect("record feedback");
        id
    }

    fn votes(&self, memory: MemoryId, verdict: MemoryVerdict, count: usize) {
        for _ in 0..count {
            self.vote(memory, verdict, MemoryEvidence::Review);
        }
    }

    /// Each result as its id and the exact bits of its score, in rank order.
    fn ranked(&self, request: &RetrievalRequest) -> Vec<(MemoryId, u32)> {
        self.service
            .retrieve(request)
            .expect("retrieve")
            .into_iter()
            .map(|r| (r.record.id, r.score.to_bits()))
            .collect()
    }
}

fn everything() -> RetrievalRequest {
    RetrievalRequest {
        limit: 20,
        ..Default::default()
    }
}

fn score_of(ranked: &[(MemoryId, u32)], id: MemoryId) -> f32 {
    let (_, bits) = ranked
        .iter()
        .find(|(found, _)| *found == id)
        .expect("ranked");
    f32::from_bits(*bits)
}

fn position(ranked: &[(MemoryId, u32)], id: MemoryId) -> usize {
    ranked
        .iter()
        .position(|(found, _)| *found == id)
        .expect("ranked")
}

#[test]
fn helped_moves_a_memory_up_and_hurt_down_by_exactly_its_factor() {
    let f = fixture(false);
    let helped = f.put(
        "Release builds run scripts/release.sh first",
        MemoryScope::Global,
        0.55,
        false,
    );
    let plain = f.put(
        "The changelog is edited before tagging",
        MemoryScope::Global,
        0.6,
        false,
    );
    let hurt = f.put(
        "Benchmarks run on the shared runner",
        MemoryScope::Global,
        0.65,
        false,
    );
    let before = f.ranked(&everything());
    assert!(position(&before, helped) > position(&before, plain));
    assert!(position(&before, hurt) < position(&before, plain));
    f.votes(helped, MemoryVerdict::Helped, 2);
    f.votes(hurt, MemoryVerdict::Hurt, 3);
    let after = f.ranked(&everything());
    assert_eq!(
        after.iter().map(|(id, _)| *id).collect::<Vec<_>>(),
        vec![helped, plain, hurt]
    );
    let up = FeedbackTally { helped: 2, hurt: 0 }.factor();
    let down = FeedbackTally { helped: 0, hurt: 3 }.factor();
    assert_eq!(
        score_of(&after, helped).to_bits(),
        (score_of(&before, helped) * up).to_bits()
    );
    assert_eq!(
        score_of(&after, hurt).to_bits(),
        (score_of(&before, hurt) * down).to_bits()
    );
    assert_eq!(
        score_of(&after, plain).to_bits(),
        score_of(&before, plain).to_bits()
    );
}

#[test]
fn feedback_scales_the_whole_score_semantic_similarity_included() {
    let f = fixture(true);
    let id = f.put(
        "Release builds run scripts/release.sh first",
        MemoryScope::Global,
        0.6,
        false,
    );
    let query = "how are release builds run";
    let record = f.store.memory(id).expect("read").expect("stored");
    let semantic = LocalEmbeddingIndex
        .similarity(query, &[record])
        .expect("similarity");
    assert!(semantic[0] > 0.0, "{semantic:?}");
    let request = RetrievalRequest {
        query: Some(query.into()),
        limit: 5,
        ..Default::default()
    };
    let before = f.ranked(&request);
    f.votes(id, MemoryVerdict::Helped, 3);
    let after = f.ranked(&request);
    let up = FeedbackTally { helped: 3, hurt: 0 }.factor();
    assert!(up > 1.0);
    assert_eq!(
        score_of(&after, id).to_bits(),
        (score_of(&before, id) * up).to_bits()
    );
}

#[test]
fn a_pinned_memory_keeps_its_score_and_place_whatever_its_feedback() {
    let f = fixture(false);
    let pinned = f.put(
        "Never force-push the release branch",
        MemoryScope::Global,
        0.4,
        true,
    );
    let twin = f.put(
        "Never rewrite a published tag",
        MemoryScope::Global,
        0.4,
        true,
    );
    let other = f.put(
        "The changelog is edited before tagging",
        MemoryScope::Global,
        0.9,
        false,
    );
    let before = f.ranked(&everything());
    f.votes(pinned, MemoryVerdict::Hurt, 5);
    f.votes(twin, MemoryVerdict::Helped, 5);
    f.votes(other, MemoryVerdict::Helped, 5);
    let after = f.ranked(&everything());
    for id in [pinned, twin] {
        assert_eq!(
            score_of(&after, id).to_bits(),
            score_of(&before, id).to_bits()
        );
        assert_eq!(position(&after, id), position(&before, id));
    }
    assert!(score_of(&after, other) > score_of(&before, other));
    assert!(position(&after, other) > position(&after, twin));
}

#[test]
fn a_memory_without_counted_evidence_keeps_an_identical_score_and_order() {
    let f = fixture(false);
    let ids: Vec<MemoryId> = (0..5)
        .map(|i| {
            let text = format!("Release step number {i} is checked by hand");
            f.put(&text, MemoryScope::Global, 0.5 + i as f32 * 0.05, false)
        })
        .collect();
    let before = f.ranked(&everything());
    f.votes(ids[0], MemoryVerdict::Helped, 1);
    f.votes(ids[1], MemoryVerdict::Neutral, 4);
    f.vote(ids[2], MemoryVerdict::Hurt, MemoryEvidence::Outcome);
    f.vote(ids[2], MemoryVerdict::Hurt, MemoryEvidence::Outcome);
    let retracted = f.vote(ids[3], MemoryVerdict::Hurt, MemoryEvidence::Operator);
    f.vote(ids[3], MemoryVerdict::Hurt, MemoryEvidence::Operator);
    f.store
        .commit(Commit::new().mutate(Mutation::RetractMemoryFeedback {
            id: retracted,
            at: now(),
            reason: "Wrong build".into(),
        }))
        .expect("retract");
    f.votes(ids[4], MemoryVerdict::Helped, 1);
    f.votes(ids[4], MemoryVerdict::Hurt, 1);
    let after = f.ranked(&everything());
    // Two votes make ids[4]'s tally active, but one each way is a factor of 1.
    assert_eq!(after, before);
}

/// Ranks the same corpus as before feedback could move recall: the golden
/// values below were produced by the retrieval this change replaced.
#[test]
fn an_empty_feedback_table_ranks_byte_for_byte_as_before() {
    let f = fixture(true);
    let agent = AgentId::new();
    let repository = || MemoryScope::Repository {
        id: "aether".into(),
    };
    let corpus = [
        (
            "Release builds run scripts/release.sh first",
            MemoryScope::Global,
            0.51,
        ),
        (
            "The release index is rebuilt by cargo xtask",
            repository(),
            0.52,
        ),
        (
            "Workers build with cargo build --locked",
            MemoryScope::Role {
                role: "worker".into(),
            },
            0.53,
        ),
        (
            "Reviewers check the release notes against the index",
            MemoryScope::RepositoryRole {
                id: "aether".into(),
                role: "worker".into(),
            },
            0.54,
        ),
        (
            "This agent keeps its scratch files under target",
            MemoryScope::Agent { id: agent },
            0.55,
        ),
        ("Coffee is on the third floor", MemoryScope::Global, 0.56),
        (
            "Index rebuilds run in batches of five hundred",
            repository(),
            0.57,
        ),
        (
            "Never force-push the release branch",
            MemoryScope::Global,
            0.58,
        ),
    ];
    let mut texts = BTreeMap::new();
    for (i, (text, scope, importance)) in corpus.into_iter().enumerate() {
        texts.insert(f.put(text, scope, importance, i == 7), i);
    }
    let request = RetrievalRequest {
        scope: Some(repository()),
        role: Some("worker".into()),
        agent: Some(agent),
        query: Some("rebuild the release index with cargo".into()),
        tags: vec!["build".into()],
        limit: 6,
        ..Default::default()
    };
    let ranked: Vec<(usize, u32)> = f
        .ranked(&request)
        .into_iter()
        .map(|(id, bits)| (texts[&id], bits))
        .collect();
    assert_eq!(ranked, GOLDEN, "{ranked:?}");
}

const GOLDEN: [(usize, u32); 6] = [
    (7, 1_071_923_617),
    (1, 1_066_623_862),
    (3, 1_065_994_592),
    (6, 1_064_804_309),
    (2, 1_063_921_346),
    (0, 1_062_283_720),
];
