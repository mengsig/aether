//! Ranking each query as a turn does, the layer it puts in front of the
//! model, and how long retrieval takes on a small and a large store.

use std::path::Path;
use std::time::Instant;

use aether_context::{TokenEstimator, render};
use aether_core::id::AgentId;
use aether_core::memory::{MemoryCandidate, MemoryKind, MemoryRecord, MemoryScope};
use aether_core::time::now;
use aether_memory::{MemoryService, RetrievalRequest};
use aether_store::{Commit, Mutation, SqliteStore, Store};
use serde::Serialize;

use crate::plan::{Plan, PlanQuery};
use crate::seed::{Keys, Seeded};
use crate::{BenchResult, open};

#[derive(Debug, Serialize, PartialEq)]
pub struct Ranked {
    pub key: String,
    pub score: f32,
    /// Estimated tokens of this memory's line in the layer.
    pub tokens: u32,
    /// Whether the line fit under the layer's cap and reached the model.
    pub injected: bool,
}

#[derive(Debug, Serialize)]
pub struct QueryRow {
    pub id: String,
    pub ranked: Vec<Ranked>,
    /// Estimated tokens of the whole layer, header included; 0 with no layer.
    pub layer_tokens: u32,
    pub layer: Option<String>,
}

#[derive(Debug, Serialize)]
pub struct LatencyRows {
    pub small: Vec<Timings>,
    pub scaled: Scaled,
}

/// Wall time of each repetition of one query's retrieval, in microseconds.
#[derive(Debug, Serialize)]
pub struct Timings {
    pub id: String,
    pub micros: Vec<f64>,
}

#[derive(Debug, Serialize)]
pub struct Scaled {
    pub memories: usize,
    pub footprint: crate::Footprint,
    pub timings: Vec<Timings>,
}

/// The request `aether-runtime`'s `retrieve_memories` builds for a turn: the
/// agent's repository, role and lineage, ranked on the objective.
pub fn request(query: &PlanQuery, agent: AgentId) -> RetrievalRequest {
    RetrievalRequest {
        scope: query
            .repository
            .clone()
            .map(|id| MemoryScope::Repository { id }),
        role: Some(query.role.clone()),
        agent: Some(agent),
        ancestors: Vec::new(),
        query: Some(query.query.clone()).filter(|q| !q.trim().is_empty()),
        tags: Vec::new(),
        kinds: Vec::new(),
        limit: query.limit,
    }
}

pub fn run_queries(
    service: &MemoryService,
    plan: &Plan,
    seeded: &Seeded,
) -> BenchResult<Vec<QueryRow>> {
    plan.queries
        .iter()
        .map(|query| run_query(service, plan, query, seeded.agent, &seeded.keys))
        .collect()
}

fn run_query(
    service: &MemoryService,
    plan: &Plan,
    query: &PlanQuery,
    agent: AgentId,
    keys: &Keys,
) -> BenchResult<QueryRow> {
    let retrieved = service.retrieve(&request(query, agent))?;
    let records: Vec<MemoryRecord> = retrieved.iter().map(|r| r.record.clone()).collect();
    let (layer, injected) =
        aether_context::memory_layer(&records, &TokenEstimator, plan.memory_max);
    let estimator = TokenEstimator;
    let mut ranked = Vec::with_capacity(retrieved.len());
    for item in &retrieved {
        ranked.push(Ranked {
            key: keys.key(item.record.id)?.to_owned(),
            score: item.score,
            tokens: estimator.text(&render::memory_line(&item.record)),
            injected: injected.contains(&item.record.id),
        });
    }
    let layer_tokens = layer.as_deref().map_or(0, |text| estimator.text(text));
    debug_assert!(ranked.len() <= query.limit);
    debug_assert!(layer_tokens <= plan.memory_max);
    Ok(QueryRow {
        id: query.id.clone(),
        ranked,
        layer_tokens,
        layer,
    })
}

pub fn time_queries(
    service: &MemoryService,
    plan: &Plan,
    seeded: &Seeded,
) -> BenchResult<Vec<Timings>> {
    time_with(service, plan, seeded.agent)
}

fn time_with(service: &MemoryService, plan: &Plan, agent: AgentId) -> BenchResult<Vec<Timings>> {
    let mut rows = Vec::with_capacity(plan.queries.len());
    for query in &plan.queries {
        let request = request(query, agent);
        service.retrieve(&request)?;
        let mut micros = Vec::with_capacity(plan.latency.reps);
        for _ in 0..plan.latency.reps {
            let started = Instant::now();
            let found = service.retrieve(&request)?;
            micros.push(started.elapsed().as_secs_f64() * 1e6);
            debug_assert!(found.len() <= query.limit);
        }
        rows.push(Timings {
            id: query.id.clone(),
            micros,
        });
    }
    Ok(rows)
}

/// The plan's memories, admitted as before, among `filler` synthetic ones in
/// other repositories and roles; then the same queries, timed.
pub fn scaled_latency(dir: &Path, plan: &Plan) -> BenchResult<Scaled> {
    let (store, service) = open(dir, &plan.index)?;
    let seeded = crate::seed::seed(&store, &service, plan)?;
    add_filler(&store, plan.latency.filler)?;
    crate::checkpoint(&store)?;
    let timings = time_with(&service, plan, seeded.agent)?;
    let memories = crate::seed::count_current(&store)?;
    drop(service);
    drop(store);
    debug_assert!(memories >= plan.latency.filler);
    Ok(Scaled {
        memories,
        footprint: crate::footprint(dir)?,
        timings,
    })
}

/// Words the filler draws from: engineering prose, so lexical retrieval has
/// real competition rather than gibberish it ignores.
const WORDS: [&str; 32] = [
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
    "config",
    "feedback",
    "scheduler",
    "engine",
    "workspace",
    "tests",
    "deploy",
    "cache",
    "lockfile",
    "container",
    "timeout",
    "retry",
    "branch",
    "commit",
    "pipeline",
];

/// Committed directly, without admission or its events: filler is the load,
/// not the system under test.
pub fn add_filler(store: &SqliteStore, count: usize) -> BenchResult<()> {
    let mut rng = Lcg(0x5eed);
    let at = now();
    let mut commit = Commit::new();
    for i in 0..count {
        let record = MemoryRecord::from_candidate(filler(&mut rng, i), at);
        commit.push_mutation(Mutation::PutMemory(record));
        if commit.mutations.len() == 1_000 {
            store.commit(std::mem::take(&mut commit))?;
        }
    }
    if !commit.is_empty() {
        store.commit(commit)?;
    }
    Ok(())
}

fn filler(rng: &mut Lcg, i: usize) -> MemoryCandidate {
    let scope = match rng.below(20) {
        0 => MemoryScope::Global,
        1..=3 => MemoryScope::Role {
            role: ["worker", "reviewer", "build"][rng.below(3)].to_owned(),
        },
        n => MemoryScope::Repository {
            id: format!("github.com/example-org/filler-{}", n % 40),
        },
    };
    let words: Vec<&str> = (0..12 + rng.below(12))
        .map(|_| WORDS[rng.below(WORDS.len())])
        .collect();
    MemoryCandidate {
        scope,
        kind: MemoryKind::Fact,
        text: format!("Filler note {i}: {}.", words.join(" ")),
        structured: None,
        importance: 0.3 + 0.6 * rng.below(100) as f32 / 100.0,
        confidence: 0.5 + 0.4 * rng.below(100) as f32 / 100.0,
        tags: vec![WORDS[rng.below(WORDS.len())].to_owned()],
        provenance: Default::default(),
        supersedes: None,
    }
}

/// A linear congruential generator: the filler must be the same every run.
struct Lcg(u64);

impl Lcg {
    fn below(&mut self, bound: usize) -> usize {
        debug_assert!(bound > 0);
        self.0 = self
            .0
            .wrapping_mul(6_364_136_223_846_793_005)
            .wrapping_add(1_442_695_040_888_963_407);
        ((self.0 >> 33) % bound as u64) as usize
    }
}
