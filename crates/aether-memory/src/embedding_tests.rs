use super::*;
use aether_core::memory::MemoryProvenance;
use aether_store::SqliteStore;

/// Facts that share no whole word with the question that needs them, so
/// the lexical and structural passes cannot find the right one.
const CORPUS: [&str; 4] = [
    "Reindexing a shard runs in batches of five hundred rows",
    "The office coffee machine lives on the third floor",
    "Release notes are drafted by the person who cuts the tag",
    "Deployment windows are Tuesday and Thursday afternoons",
];
const QUESTION: &str = "how do I rebuild the index";

fn seeded(index: Option<Arc<dyn MemoryIndex>>) -> MemoryService {
    let service = MemoryService::new(
        Arc::new(SqliteStore::open_in_memory().expect("store")),
        WriteGate::new(),
    );
    let service = match index {
        Some(i) => service.with_index(i),
        None => service,
    };
    // Distinct importance, so ordering is decided by a number and not by
    // `created_at` - an RFC 3339 string whose fractional digits vary with
    // the value, so two rows written in one run can compare lexically out
    // of order. CORPUS[0] is the answer and gets the lowest, so word
    // matching alone cannot put it first.
    for (i, text) in CORPUS.iter().enumerate() {
        service
            .admit(
                MemoryCandidate {
                    scope: MemoryScope::Global,
                    kind: MemoryKind::Fact,
                    text: (*text).into(),
                    structured: None,
                    importance: 0.47 + i as f32 * 0.01,
                    confidence: 0.5,
                    tags: vec![],
                    provenance: MemoryProvenance::default(),
                    supersedes: None,
                },
                Actor::System,
                None,
                None,
            )
            .expect("admit");
    }
    service
}

fn top(service: &MemoryService) -> Option<String> {
    service
        .retrieve(&RetrievalRequest {
            query: Some(QUESTION.into()),
            limit: 4,
            ..Default::default()
        })
        .expect("retrieve")
        .first()
        .map(|found| found.record.text.clone())
}

#[test]
fn the_embedding_pass_finds_what_word_matching_cannot() {
    // Without an index no whole word of the question appears in any of
    // the four, so the relevance cutoff leaves nothing to put first.
    assert_eq!(
        top(&seeded(None)),
        None,
        "if word matching already found it, this test proves nothing"
    );
    assert_eq!(
        top(&seeded(Some(Arc::new(LocalEmbeddingIndex)))).as_deref(),
        Some(CORPUS[0]),
        "the semantic pass connects 'rebuild the index' to 'reindexing'"
    );
}

#[test]
fn the_semantic_pass_never_outranks_being_in_scope() {
    let service = seeded(Some(Arc::new(LocalEmbeddingIndex)));
    let repo = MemoryScope::Repository { id: "mine".into() };
    service
        .admit(
            MemoryCandidate {
                scope: MemoryScope::Repository { id: "other".into() },
                kind: MemoryKind::Fact,
                text: "Reindexing a shard runs in batches of one thousand rows".into(),
                structured: None,
                importance: 0.9,
                confidence: 0.9,
                tags: vec![],
                provenance: MemoryProvenance::default(),
                supersedes: None,
            },
            Actor::System,
            None,
            None,
        )
        .expect("admit");
    let got = service
        .retrieve(&RetrievalRequest {
            scope: Some(repo),
            query: Some(QUESTION.into()),
            limit: 5,
            ..Default::default()
        })
        .expect("retrieve");
    assert!(
        !got.iter().any(|r| r.record.text.contains("one thousand")),
        "another repository's memory stays invisible however close it reads"
    );
}
