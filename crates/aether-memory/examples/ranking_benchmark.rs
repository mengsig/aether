//! How far feedback moves recall, and what it costs. Not part of any test run:
//!
//! ```sh
//! cargo run --release -p aether-memory --example ranking_benchmark -- fixture
//! cargo run --release -p aether-memory --example ranking_benchmark -- latency <rows> <batches> <reps>
//! ```
//!
//! It uses only API that predates ranking by feedback, so the same file builds
//! against the code before the change and measures both.

#![allow(clippy::expect_used, clippy::unwrap_used)]

use std::collections::BTreeMap;
use std::sync::Arc;
use std::time::Instant;

use aether_core::event::Actor;
use aether_core::id::{AgentId, MemoryFeedbackId, MemoryId, TaskId};
use aether_core::memory::{
    MemoryCandidate, MemoryKind, MemoryProvenance, MemoryRecord, MemoryScope,
};
use aether_core::memory_feedback::{
    MemoryEvidence, MemoryFeedback, MemoryFeedbackSpec, MemoryVerdict,
};
use aether_core::time::{Timestamp, now};
use aether_memory::{MemoryService, RetrievalRequest};
use aether_store::{Commit, Mutation, SqliteStore, Store, WriteGate};
use chrono::{Duration, TimeZone, Utc};

const WORDS: [&str; 24] = [
    "release",
    "build",
    "cargo",
    "index",
    "review",
    "schema",
    "migration",
    "daemon",
    "socket",
    "token",
    "budget",
    "lease",
    "snapshot",
    "retention",
    "export",
    "import",
    "approval",
    "memory",
    "feedback",
    "scheduler",
    "engine",
    "codex",
    "claude",
    "workspace",
];
const TAGS: [&str; 6] = ["build", "release", "review", "store", "ops", "docs"];
const ROLES: [&str; 2] = ["worker", "reviewer"];

/// A small linear congruential generator: the fixture must not depend on a seed file.
struct Lcg(u64);

impl Lcg {
    fn next(&mut self, below: usize) -> usize {
        debug_assert!(below > 0);
        self.0 = self
            .0
            .wrapping_mul(6_364_136_223_846_793_005)
            .wrapping_add(1_442_695_040_888_963_407);
        ((self.0 >> 33) % below as u64) as usize
    }
}

/// The agents and their lineage every scope and request draws from.
struct World {
    agents: Vec<AgentId>,
}

impl World {
    fn new() -> Self {
        Self {
            agents: (0..8).map(|_| AgentId::new()).collect(),
        }
    }

    fn scope(&self, rng: &mut Lcg) -> MemoryScope {
        let repo = format!("repo-{}", rng.next(4));
        match rng.next(10) {
            0 | 1 => MemoryScope::Global,
            2..=4 => MemoryScope::Repository { id: repo },
            5 => MemoryScope::RepositoryRole {
                id: repo,
                role: ROLES[rng.next(2)].into(),
            },
            6 | 7 => MemoryScope::Role {
                role: ROLES[rng.next(2)].into(),
            },
            8 => MemoryScope::Agent {
                id: self.agents[rng.next(self.agents.len())],
            },
            _ => MemoryScope::Subtree {
                root: self.agents[rng.next(self.agents.len())],
            },
        }
    }

    fn request(&self, rng: &mut Lcg, limit: usize) -> RetrievalRequest {
        let agent = rng.next(self.agents.len());
        let query: Vec<&str> = (0..3).map(|_| WORDS[rng.next(WORDS.len())]).collect();
        RetrievalRequest {
            scope: Some(MemoryScope::Repository {
                id: format!("repo-{}", rng.next(4)),
            }),
            role: Some(ROLES[rng.next(2)].into()),
            agent: Some(self.agents[agent]),
            ancestors: self.agents[..agent].iter().rev().copied().collect(),
            query: Some(query.join(" ")),
            tags: if rng.next(3) == 0 {
                vec![TAGS[rng.next(TAGS.len())].into()]
            } else {
                vec![]
            },
            kinds: vec![],
            limit,
        }
    }
}

/// `count` memories with distinct importance, committed in pages. Returns ids
/// in creation order, so index `i` names the same memory in every process.
fn seed_memories(
    store: &SqliteStore,
    world: &World,
    count: usize,
    at: impl Fn(usize) -> Timestamp,
) -> Vec<MemoryId> {
    let mut rng = Lcg(7);
    let mut ids = Vec::with_capacity(count);
    let mut commit = Commit::new();
    for i in 0..count {
        let text: Vec<&str> = (0..8).map(|_| WORDS[rng.next(WORDS.len())]).collect();
        let mut record = MemoryRecord::from_candidate(
            MemoryCandidate {
                scope: world.scope(&mut rng),
                kind: MemoryKind::Fact,
                text: format!("Note {i}: {}", text.join(" ")),
                structured: None,
                importance: 0.3 + 0.6 * i as f32 / count as f32,
                confidence: 0.5 + 0.4 * rng.next(100) as f32 / 100.0,
                tags: vec![TAGS[rng.next(TAGS.len())].into()],
                provenance: MemoryProvenance::default(),
                supersedes: None,
            },
            at(i),
        );
        record.pinned = i % 97 == 0;
        ids.push(record.id);
        commit.push_mutation(Mutation::PutMemory(record));
        if commit.mutations.len() == 500 {
            store
                .commit(std::mem::take(&mut commit))
                .expect("seed memories");
        }
    }
    store.commit(commit).expect("seed memories");
    debug_assert_eq!(ids.len(), count);
    ids
}

fn feedback(
    memory: MemoryId,
    task: TaskId,
    verdict: MemoryVerdict,
    evidence: MemoryEvidence,
    at: Timestamp,
) -> MemoryFeedback {
    MemoryFeedback {
        id: MemoryFeedbackId::new(),
        spec: MemoryFeedbackSpec {
            memory,
            verdict,
            evidence,
            task,
            invocation: None,
            reason: "Benchmark".into(),
        },
        actor: Actor::user(),
        created_at: at,
        retracted_at: None,
        retraction_reason: None,
    }
}

/// Commit `records`, then retract those `retract` names, in pages.
fn record_feedback(store: &SqliteStore, records: Vec<MemoryFeedback>, retract: &[bool]) {
    debug_assert_eq!(records.len(), retract.len());
    let retracted: Vec<MemoryFeedbackId> = records
        .iter()
        .zip(retract)
        .filter(|(_, r)| **r)
        .map(|(f, _)| f.id)
        .collect();
    for page in records.chunks(1000) {
        let commit = page.iter().cloned().fold(Commit::new(), |c, f| {
            c.mutate(Mutation::RecordMemoryFeedback(f))
        });
        store.commit(commit).expect("record feedback");
    }
    for page in retracted.chunks(1000) {
        let commit = page.iter().fold(Commit::new(), |c, id| {
            c.mutate(Mutation::RetractMemoryFeedback {
                id: *id,
                at: now(),
                reason: "Benchmark".into(),
            })
        });
        store.commit(commit).expect("retract feedback");
    }
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    match args.first().map(String::as_str) {
        Some("fixture") => fixture(),
        Some("latency") => {
            let number = |i: usize| -> usize {
                args.get(i)
                    .and_then(|a| a.parse().ok())
                    .expect("latency <rows> <batches> <reps>")
            };
            latency(number(1), number(2), number(3));
        }
        _ => eprintln!("usage: ranking_benchmark fixture | latency <rows> <batches> <reps>"),
    }
}

// ---------------------------------------------------------------- fixture

/// Feedback by memory index: helped x5, hurt x5, 3 helped and 2 hurt, one helped,
/// outcome only, retracted, neutral, helped x2, hurt x2, and none for the rest.
fn fixture_feedback(ids: &[MemoryId]) -> (Vec<MemoryFeedback>, Vec<bool>) {
    let mut records = Vec::new();
    let mut retract = Vec::new();
    let start = Utc
        .with_ymd_and_hms(2026, 10, 1, 0, 0, 0)
        .single()
        .expect("start");
    let mut push = |memory, verdict, evidence, retracted| {
        let at = start + Duration::minutes(records.len() as i64);
        records.push(feedback(memory, TaskId::new(), verdict, evidence, at));
        retract.push(retracted);
    };
    use MemoryEvidence::{Operator, Outcome, Review};
    use MemoryVerdict::{Helped, Hurt, Neutral};
    for (i, &memory) in ids.iter().enumerate() {
        let votes: &[(MemoryVerdict, MemoryEvidence, bool)] = match i % 20 {
            0 => &[(Helped, Review, false); 5],
            1 => &[(Hurt, Operator, false); 5],
            2 => &[
                (Helped, Review, false),
                (Helped, Review, false),
                (Helped, Operator, false),
                (Hurt, Review, false),
                (Hurt, Review, false),
            ],
            3 => &[(Helped, Review, false)],
            4 => &[(Hurt, Outcome, false); 4],
            5 => &[(Hurt, Review, true); 2],
            6 => &[(Neutral, Review, false); 4],
            7 => &[(Helped, Operator, false); 2],
            8 => &[(Hurt, Review, false); 2],
            _ => &[],
        };
        for &(verdict, evidence, retracted) in votes {
            push(memory, verdict, evidence, retracted);
        }
    }
    (records, retract)
}

/// Each request's results as (memory index, score bits), in rank order.
type Rankings = Vec<Vec<(usize, u32)>>;

fn rank_all(
    service: &MemoryService,
    requests: &[RetrievalRequest],
    index: &BTreeMap<MemoryId, usize>,
) -> Rankings {
    requests
        .iter()
        .map(|req| {
            let found = service.retrieve(req).expect("retrieve");
            debug_assert!(found.len() <= req.limit);
            found
                .into_iter()
                .map(|r| (index[&r.record.id], r.score.to_bits()))
                .collect()
        })
        .collect()
}

fn fixture() {
    let dir = tempfile::tempdir().expect("tempdir");
    let store = Arc::new(SqliteStore::open(dir.path().join("fixture.db")).expect("store"));
    let world = World::new();
    // Ahead of the clock, so recency is equal everywhere and the scores are deterministic.
    let future = Utc
        .with_ymd_and_hms(2100, 1, 1, 0, 0, 0)
        .single()
        .expect("future");
    let ids = seed_memories(&store, &world, 200, |i| {
        future + Duration::minutes(i as i64)
    });
    let index: BTreeMap<MemoryId, usize> = ids.iter().enumerate().map(|(i, id)| (*id, i)).collect();
    let mut rng = Lcg(11);
    let requests: Vec<RetrievalRequest> = (0..60).map(|_| world.request(&mut rng, 10)).collect();
    let service = MemoryService::new(store.clone(), WriteGate::new());
    let before = rank_all(&service, &requests, &index);
    let (records, retract) = fixture_feedback(&ids);
    let rows = records.len();
    record_feedback(&store, records, &retract);
    let after = rank_all(&service, &requests, &index);
    debug_assert_eq!(before.len(), requests.len());
    debug_assert_eq!(after.len(), requests.len());
    for (i, ranked) in before.iter().enumerate() {
        println!("baseline {i}: {ranked:?}");
    }
    for (i, ranked) in after.iter().enumerate() {
        println!("ranked {i}: {ranked:?}");
    }
    report_fixture(&before, &after, rows);
}

fn top(ranked: &[(usize, u32)], k: usize) -> Vec<usize> {
    ranked.iter().take(k).map(|(i, _)| *i).collect()
}

/// Memories whose feedback can move them: an active tally and not pinned.
fn moved_by_feedback(i: usize) -> bool {
    matches!(i % 20, 0 | 1 | 2 | 7 | 8) && !i.is_multiple_of(97)
}

fn report_fixture(before: &Rankings, after: &Rankings, rows: usize) {
    debug_assert_eq!(before.len(), after.len());
    let changed = |k: usize, set: bool| {
        before
            .iter()
            .zip(after)
            .filter(|(b, a)| {
                let (mut b, mut a) = (top(b, k), top(a, k));
                if set {
                    b.sort_unstable();
                    a.sort_unstable();
                }
                b != a
            })
            .count()
    };
    let mut unmoved_checked = 0;
    let mut unmoved_differ = 0;
    for (b, a) in before.iter().zip(after) {
        let scores: BTreeMap<usize, u32> = b.iter().copied().collect();
        for (i, bits) in a.iter().filter(|(i, _)| !moved_by_feedback(*i)) {
            if let Some(old) = scores.get(i) {
                unmoved_checked += 1;
                unmoved_differ += usize::from(old != bits);
            }
        }
    }
    let untouched: Vec<usize> = (0..before.len())
        .filter(|r| {
            before[*r]
                .iter()
                .chain(&after[*r])
                .all(|(i, _)| !moved_by_feedback(*i))
        })
        .collect();
    let untouched_identical = untouched
        .iter()
        .filter(|r| before[**r] == after[**r])
        .count();
    println!(
        "fixture: 200 memories, {} requests (limit 10), {rows} feedback rows",
        before.len()
    );
    for k in [5, 10] {
        println!(
            "top-{k}: order changed in {} requests, membership changed in {}",
            changed(k, false),
            changed(k, true)
        );
    }
    println!(
        "results of memories without an active tally (or pinned): {unmoved_checked} compared, {unmoved_differ} with a different score"
    );
    println!(
        "requests whose before and after results hold no movable memory: {}, identical: {untouched_identical}",
        untouched.len()
    );
}

// ---------------------------------------------------------------- latency

/// 5,000 memories and `rows` feedback rows, half of them on the 500 most
/// important memories, which the structural pages pull into most pools.
fn latency_store(dir: &std::path::Path, world: &World, rows: usize) -> Arc<SqliteStore> {
    let store = Arc::new(SqliteStore::open(dir.join("latency.db")).expect("store"));
    let start = now() - Duration::days(400);
    let ids = seed_memories(&store, world, 5_000, |i| {
        start + Duration::minutes(i as i64 * 100)
    });
    let tasks: Vec<TaskId> = (0..3_000).map(|_| TaskId::new()).collect();
    let mut rng = Lcg(23);
    let verdicts = [
        MemoryVerdict::Helped,
        MemoryVerdict::Helped,
        MemoryVerdict::Hurt,
        MemoryVerdict::Neutral,
    ];
    let evidence = [
        MemoryEvidence::Review,
        MemoryEvidence::Operator,
        MemoryEvidence::Outcome,
    ];
    let (mut records, mut retract) = (Vec::with_capacity(rows), Vec::with_capacity(rows));
    for n in 0..rows {
        let memory = if n % 2 == 0 {
            ids[4_500 + rng.next(500)]
        } else {
            ids[rng.next(ids.len())]
        };
        let at = start + Duration::seconds(n as i64 * 60);
        records.push(feedback(
            memory,
            tasks[rng.next(tasks.len())],
            verdicts[rng.next(4)],
            evidence[rng.next(3)],
            at,
        ));
        retract.push(rng.next(10) == 0);
    }
    record_feedback(&store, records, &retract);
    debug_assert_eq!(ids.len(), 5_000);
    store
}

fn median(values: &mut [f64]) -> f64 {
    debug_assert!(!values.is_empty());
    values.sort_by(f64::total_cmp);
    values[values.len() / 2]
}

fn latency(rows: usize, batches: usize, reps: usize) {
    let dir = tempfile::tempdir().expect("tempdir");
    let world = World::new();
    let store = latency_store(dir.path(), &world, rows);
    let service = MemoryService::new(store, WriteGate::new());
    let mut rng = Lcg(31);
    let requests: Vec<RetrievalRequest> = (0..32).map(|_| world.request(&mut rng, 6)).collect();
    for req in &requests {
        assert!(!service.retrieve(req).expect("warm up").is_empty());
    }
    let mut batch_medians = Vec::with_capacity(batches);
    for _ in 0..batches {
        let mut samples: Vec<f64> = Vec::with_capacity(reps);
        for n in 0..reps {
            let started = Instant::now();
            let found = service
                .retrieve(&requests[n % requests.len()])
                .expect("retrieve");
            samples.push(started.elapsed().as_secs_f64() * 1e6);
            std::hint::black_box(found);
        }
        batch_medians.push(median(&mut samples));
    }
    let rounded: Vec<String> = batch_medians.iter().map(|m| format!("{m:.1}")).collect();
    println!(
        "rows={rows} median_us={:.1} batch_medians_us=[{}]",
        median(&mut batch_medians.clone()),
        rounded.join(",")
    );
}
