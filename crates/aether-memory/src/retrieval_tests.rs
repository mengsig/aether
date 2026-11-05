use super::*;
use aether_core::id::TaskId;
use aether_core::memory::MemoryProvenance;
use aether_core::time::Timestamp;
use aether_store::SqliteStore;
use chrono::{TimeZone, Utc};

struct Fixture {
    store: Arc<SqliteStore>,
    service: MemoryService,
}

fn fixture(policy: RetrievalPolicy) -> Fixture {
    let store = Arc::new(SqliteStore::open_in_memory().expect("store"));
    let service = MemoryService::new(store.clone(), WriteGate::new()).with_retrieval(policy);
    Fixture { store, service }
}

/// Ahead of any test's clock, so every memory has the same zero age.
fn future() -> Timestamp {
    Utc.with_ymd_and_hms(2100, 1, 1, 0, 0, 0).unwrap()
}

const REPO: &str = "github.com/example/widget";

fn repo() -> MemoryScope {
    MemoryScope::Repository { id: REPO.into() }
}

impl Fixture {
    fn put(&self, text: &str, pinned: bool) -> MemoryId {
        let mut record = MemoryRecord::from_candidate(
            MemoryCandidate {
                scope: repo(),
                kind: MemoryKind::Fact,
                text: text.into(),
                structured: None,
                importance: 0.7,
                confidence: 0.9,
                tags: vec![],
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

    fn ask(&self, query: Option<&str>) -> Vec<Retrieved> {
        self.service
            .retrieve(&RetrievalRequest {
                scope: Some(repo()),
                role: Some("worker".into()),
                query: query.map(str::to_owned),
                limit: 6,
                ..Default::default()
            })
            .expect("retrieve")
    }
}

fn ids(found: &[Retrieved]) -> Vec<MemoryId> {
    found.iter().map(|r| r.record.id).collect()
}

const CI: &str = "Run make ci before pushing; it runs clippy and the whole test suite.";
const DOCKER: &str = "Integration tests need a running Docker daemon on the host.";
const DOCS: &str = "API documentation is generated with make docs into target/doc.";

#[test]
fn a_query_sharing_no_word_with_a_memory_retrieves_nothing_but_the_pinned() {
    let f = fixture(RetrievalPolicy::default());
    let ci = f.put(CI, false);
    f.put(DOCKER, false);
    let pinned = f.put(DOCS, true);
    let unrelated = f.ask(Some("Translate the introduction into French."));
    assert_eq!(
        ids(&unrelated),
        [pinned],
        "only the pinned memory survives the cutoff"
    );
    let related = f.ask(Some("Make sure clippy passes before pushing the branch."));
    assert_eq!(ids(&related)[..2], [pinned, ci], "{related:?}");
    assert_eq!(
        related.len(),
        2,
        "the Docker note shares no word: {related:?}"
    );
}

#[test]
fn without_a_query_or_with_only_function_words_nothing_is_cut() {
    let f = fixture(RetrievalPolicy::default());
    for text in [CI, DOCKER, DOCS] {
        f.put(text, false);
    }
    assert_eq!(f.ask(None).len(), 3);
    assert_eq!(
        f.ask(Some("what is the")).len(),
        3,
        "no meaningful word, no cutoff"
    );
}

#[test]
fn min_score_is_the_cutoff_zero_keeps_all_and_one_keeps_only_an_exact_match() {
    let open = fixture(RetrievalPolicy {
        min_score: 0.0,
        ..Default::default()
    });
    for text in [CI, DOCKER, DOCS] {
        open.put(text, false);
    }
    assert_eq!(
        open.ask(Some("Translate the introduction into French."))
            .len(),
        3
    );

    let strict = fixture(RetrievalPolicy {
        min_score: 1.0,
        ..Default::default()
    });
    let docker = strict.put(DOCKER, false);
    strict.put(CI, false);
    assert_eq!(
        ids(&strict.ask(Some(DOCKER))),
        [docker],
        "the same words are a cosine of 1"
    );
    assert!(strict.ask(Some("Docker tests on the host")).is_empty());
}

#[test]
fn a_stale_memory_is_demoted_to_half_its_score_and_restored_when_cleared() {
    let f = fixture(RetrievalPolicy::default());
    let stale = f.put(CI, false);
    let fresh = f.put(
        "Run make ci before pushing; clippy and the test suite run in it.",
        false,
    );
    let query = Some("run make ci before pushing");
    let before = f.ask(query);
    let score_of =
        |found: &[Retrieved], id| found.iter().find(|r| r.record.id == id).unwrap().score;

    assert!(
        f.service
            .set_stale(stale, Some("ci.md no longer states it".into()))
            .unwrap()
    );
    assert!(
        !f.service
            .set_stale(stale, Some("ci.md no longer states it".into()))
            .unwrap()
    );
    let demoted = f.ask(query);
    assert_eq!(ids(&demoted), [fresh, stale], "demoted, never hidden");
    assert_eq!(
        score_of(&demoted, stale),
        score_of(&before, stale) * retrieval::STALE_FACTOR
    );
    assert_eq!(score_of(&demoted, fresh), score_of(&before, fresh));

    assert!(f.service.set_stale(stale, None).unwrap());
    assert!(!f.service.set_stale(stale, None).unwrap());
    assert_eq!(score_of(&f.ask(query), stale), score_of(&before, stale));
}

#[test]
fn a_stale_mark_needs_a_known_memory_and_a_reason() {
    let f = fixture(RetrievalPolicy::default());
    let known = f.put(CI, false);
    assert!(matches!(
        f.service.set_stale(MemoryId::new(), Some("gone".into())),
        Err(MemoryError::NotFound(_))
    ));
    assert!(matches!(
        f.service.set_stale(known, Some(" ".into())),
        Err(MemoryError::Invalid(_))
    ));
    assert!(f.store.memory_stale_marks(&[known]).unwrap().is_empty());
}

fn holdout_fixture(rate: f32, seed: u64) -> (Fixture, Vec<Retrieved>) {
    let f = fixture(RetrievalPolicy {
        holdout_rate: rate,
        holdout_seed: seed,
        ..Default::default()
    });
    for i in 0..40 {
        f.put(
            &format!("Release note {i}: the build host runs one heavy job."),
            i == 0,
        );
    }
    let found = f
        .service
        .retrieve(&RetrievalRequest {
            scope: Some(repo()),
            query: Some("heavy job on the build host".into()),
            limit: 40,
            ..Default::default()
        })
        .unwrap();
    assert_eq!(found.len(), 40);
    (f, found)
}

fn unit() -> HoldoutUnit {
    HoldoutUnit {
        agent: AgentId::new(),
        task: Some(TaskId::new()),
    }
}

#[test]
fn rate_zero_never_holds_out_and_logs_nothing() {
    let (f, found) = holdout_fixture(0.0, 1);
    let unit = unit();
    let split = f.service.hold_out(found.clone(), unit);
    assert_eq!(split.injected, found);
    assert!(split.held_out.is_empty());
    assert!(
        f.service
            .holdout_log(&split.held_out, unit, now())
            .is_empty()
    );
}

#[test]
fn rate_one_holds_out_every_unpinned_memory_and_logs_each_once() {
    let (f, found) = holdout_fixture(1.0, 1);
    let unit = unit();
    let split = f.service.hold_out(found, unit);
    assert_eq!(split.injected.len(), 1);
    assert!(
        split.injected[0].record.pinned,
        "a person's pin is never withheld"
    );
    assert_eq!(split.held_out.len(), 39);
    let log = f.service.holdout_log(&split.held_out, unit, now());
    // Logged twice, as two turns of one task would: kept once per memory.
    for _turn in 0..2 {
        let commit = log.iter().cloned().fold(Commit::new(), Commit::mutate);
        f.store.commit(commit).unwrap();
    }
    for item in &split.held_out {
        let logged = f.store.memory_holdouts(item.record.id).unwrap();
        assert_eq!(logged.len(), 1, "{logged:?}");
        assert_eq!((logged[0].agent, logged[0].task), (unit.agent, unit.task));
        assert_eq!((logged[0].rate, logged[0].score), (1.0, item.score));
    }
}

#[test]
fn a_fixed_seed_draws_the_same_holdout_every_time_and_another_seed_redraws() {
    let (f, found) = holdout_fixture(0.5, 42);
    let unit = unit();
    let first = f.service.hold_out(found.clone(), unit);
    assert_eq!(
        f.service.hold_out(found.clone(), unit),
        first,
        "the same service draws alike"
    );
    let same_seed = f.service.clone().with_retrieval(RetrievalPolicy {
        holdout_rate: 0.5,
        holdout_seed: 42,
        ..Default::default()
    });
    assert_eq!(
        same_seed.hold_out(found.clone(), unit),
        first,
        "and so does a fresh one"
    );
    let reseeded = f.service.clone().with_retrieval(RetrievalPolicy {
        holdout_rate: 0.5,
        holdout_seed: 43,
        ..Default::default()
    });
    assert_ne!(
        reseeded.hold_out(found.clone(), unit).held_out,
        first.held_out
    );
    // About half of 39 eligible memories, in the ranking's order.
    assert!(
        (8..=31).contains(&first.held_out.len()),
        "{}",
        first.held_out.len()
    );
    let order: Vec<MemoryId> = first.injected.iter().map(|r| r.record.id).collect();
    let ranked: Vec<MemoryId> = ids(&found)
        .into_iter()
        .filter(|id| order.contains(id))
        .collect();
    assert_eq!(order, ranked);
}

#[test]
fn a_policy_out_of_range_is_refused_by_name() {
    let bad = [
        RetrievalPolicy {
            max_items: 0,
            ..Default::default()
        },
        RetrievalPolicy {
            min_score: -0.1,
            ..Default::default()
        },
        RetrievalPolicy {
            min_score: f32::NAN,
            ..Default::default()
        },
        RetrievalPolicy {
            holdout_rate: 1.01,
            ..Default::default()
        },
    ];
    let names = ["max_items", "min_score", "min_score", "holdout_rate"];
    for (policy, name) in bad.iter().zip(names) {
        let err = policy.validate().expect_err("out of range").to_string();
        assert!(err.contains(name), "{err}");
    }
    assert!(RetrievalPolicy::default().validate().is_ok());
}
