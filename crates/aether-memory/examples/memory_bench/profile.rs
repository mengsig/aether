//! Where retrieval spends its time. Each stage `MemoryService::retrieve`
//! runs is repeated through the public store API and timed on its own,
//! beside the whole call, on the dataset store and on the scaled one.
//!
//! The stages mirror the baseline's pipeline (`aether-memory/src/retrieval.rs`):
//! alias resolution, a structural and a lexical page per scope, one feedback
//! read and the semantic pass. What `retrieve` spends beyond their sum is its
//! in-memory scoring and sorting. A change to that pipeline makes the stage
//! split stale; the `retrieve` column stays the authority.

use std::collections::BTreeSet;
use std::path::Path;
use std::time::Instant;

use aether_context::TokenEstimator;
use aether_core::id::{AgentId, MemoryId};
use aether_core::memory::{MemoryRecord, MemoryScope, role_families};
use aether_memory::{LocalEmbeddingIndex, MemoryIndex, MemoryService};
use aether_store::{MemoryQuery, SqliteStore, Store};
use serde::Serialize;

use crate::plan::{Plan, PlanQuery};
use crate::retrieval::request;
use crate::{BenchResult, open};

/// Every sample of every stage of one query, in microseconds.
#[derive(Debug, Default, Serialize)]
pub struct StageTimings {
    pub id: String,
    /// Scopes paged, and store reads one retrieval makes.
    pub scopes: usize,
    pub reads: usize,
    /// Distinct candidates the pages return, and how many `retrieve` kept.
    pub pool: usize,
    pub retrieved: usize,
    pub resolve: Vec<f64>,
    pub structural: Vec<f64>,
    pub lexical: Vec<f64>,
    pub feedback: Vec<f64>,
    pub semantic: Vec<f64>,
    pub layer: Vec<f64>,
    pub retrieve: Vec<f64>,
}

#[derive(Debug, Serialize)]
pub struct Profile {
    pub index: String,
    pub reps: usize,
    pub small: StoreProfile,
    pub scaled: StoreProfile,
}

#[derive(Debug, Serialize)]
pub struct StoreProfile {
    pub memories: usize,
    pub queries: Vec<StageTimings>,
}

pub fn run(plan: &Plan, sandbox: &Path) -> BenchResult<Profile> {
    if !sandbox.is_dir() || std::fs::read_dir(sandbox)?.next().is_some() {
        return Err(format!("sandbox {} must be an empty directory", sandbox.display()).into());
    }
    let small = profile_store(&sandbox.join("main"), plan, 0)?;
    let scaled = profile_store(&sandbox.join("scaled"), plan, plan.latency.filler)?;
    Ok(Profile {
        index: plan.index.clone(),
        reps: plan.latency.reps,
        small,
        scaled,
    })
}

fn profile_store(dir: &Path, plan: &Plan, filler: usize) -> BenchResult<StoreProfile> {
    let (store, service) = open(dir, &plan.index)?;
    let seeded = crate::seed::seed(&store, &service, plan)?;
    crate::retrieval::add_filler(&store, filler)?;
    crate::checkpoint(&store)?;
    let memories = crate::seed::count_current(&store)?;
    let mut queries = Vec::with_capacity(plan.queries.len());
    for query in &plan.queries {
        queries.push(profile_query(&store, &service, plan, query, seeded.agent)?);
    }
    debug_assert_eq!(queries.len(), plan.queries.len());
    Ok(StoreProfile { memories, queries })
}

fn profile_query(
    store: &SqliteStore,
    service: &MemoryService,
    plan: &Plan,
    query: &PlanQuery,
    agent: AgentId,
) -> BenchResult<StageTimings> {
    let req = request(query, agent);
    let local = plan.index == "local";
    let mut row = StageTimings {
        id: query.id.clone(),
        ..Default::default()
    };
    // One untimed pass warms the read pool and SQLite's page cache.
    let pool = stages(
        store,
        service,
        query,
        &req,
        local,
        &mut StageTimings::default(),
    )?;
    let found = service.retrieve(&req)?;
    check_mirror(&pool, found.iter().map(|r| r.record.id), &query.id)?;
    row.scopes = scopes(service, &req)?.len();
    row.reads = 2 + row.scopes * if req.query.is_some() { 2 } else { 1 } + 1;
    row.pool = pool.len();
    row.retrieved = found.len();
    for _ in 0..plan.latency.reps {
        stages(store, service, query, &req, local, &mut row)?;
        let started = Instant::now();
        let found = service.retrieve(&req)?;
        row.retrieve.push(micros(started));
        let records: Vec<MemoryRecord> = found.into_iter().map(|r| r.record).collect();
        let started = Instant::now();
        let layer = aether_context::memory_layer(&records, &TokenEstimator, plan.memory_max);
        row.layer.push(micros(started));
        debug_assert!(layer.1.len() <= records.len());
    }
    debug_assert_eq!(row.retrieve.len(), plan.latency.reps);
    Ok(row)
}

/// One pass of every stage, each timed into `row`; returns the pool.
fn stages(
    store: &SqliteStore,
    service: &MemoryService,
    query: &PlanQuery,
    req: &aether_memory::RetrievalRequest,
    local: bool,
    row: &mut StageTimings,
) -> BenchResult<Vec<MemoryRecord>> {
    let started = Instant::now();
    let scopes = scopes(service, req)?;
    row.resolve.push(micros(started));
    let mut pool = Vec::new();
    let mut seen = BTreeSet::new();
    let started = Instant::now();
    for scope in &scopes {
        keep_new(
            store.memories(&page(scope, None, query.limit))?,
            &mut pool,
            &mut seen,
        );
    }
    row.structural.push(micros(started));
    let started = Instant::now();
    if let Some(text) = req.query.as_deref() {
        for scope in &scopes {
            let lexical = store.memories(&page(scope, Some(text), query.limit))?;
            keep_new(lexical, &mut pool, &mut seen);
        }
    }
    row.lexical.push(micros(started));
    let started = Instant::now();
    let ids: Vec<MemoryId> = pool.iter().map(|r| r.id).collect();
    let tallies = store.memory_feedback_tallies(&ids)?;
    row.feedback.push(micros(started));
    let started = Instant::now();
    if local && let Some(text) = req.query.as_deref() {
        let scores = LocalEmbeddingIndex.similarity(text, &pool)?;
        debug_assert_eq!(scores.len(), pool.len());
    }
    row.semantic.push(micros(started));
    debug_assert!(tallies.len() <= pool.len());
    Ok(pool)
}

/// The scopes `retrieve` pages for a bench request (no ancestors), with the
/// repository resolved twice, as `read_resolved` does.
fn scopes(
    service: &MemoryService,
    req: &aether_memory::RetrievalRequest,
) -> BenchResult<Vec<MemoryScope>> {
    let resolved = req
        .scope
        .clone()
        .map(|s| service.canonical_scope(s))
        .transpose()?;
    let again = req
        .scope
        .clone()
        .map(|s| service.canonical_scope(s))
        .transpose()?;
    debug_assert_eq!(resolved, again, "the bench registers no aliases");
    let mut scopes = Vec::new();
    if let Some(scope) = &resolved {
        scopes.push(scope.clone());
    }
    if let (Some(MemoryScope::Repository { id }), Some(role)) = (&resolved, &req.role) {
        scopes.extend(
            role_families(role)
                .into_iter()
                .map(|family| MemoryScope::RepositoryRole {
                    id: id.clone(),
                    role: family.to_owned(),
                }),
        );
    }
    if let Some(role) = &req.role {
        scopes.push(MemoryScope::Role { role: role.clone() });
    }
    if let Some(agent) = req.agent {
        scopes.push(MemoryScope::Agent { id: agent });
        scopes.push(MemoryScope::Subtree { root: agent });
    }
    Ok(scopes)
}

fn page(scope: &MemoryScope, text: Option<&str>, limit: usize) -> MemoryQuery {
    MemoryQuery {
        scope: Some(scope.clone()),
        text: text.map(str::to_owned),
        limit: limit * 4,
        ..Default::default()
    }
}

fn keep_new(
    records: Vec<MemoryRecord>,
    pool: &mut Vec<MemoryRecord>,
    seen: &mut BTreeSet<MemoryId>,
) {
    for record in records {
        if seen.insert(record.id) {
            pool.push(record);
        }
    }
}

/// The mirrored stages must see every memory `retrieve` returned, or the
/// split would time a different workload than the whole call.
fn check_mirror(
    pool: &[MemoryRecord],
    retrieved: impl Iterator<Item = MemoryId>,
    query: &str,
) -> BenchResult<()> {
    let ids: BTreeSet<MemoryId> = pool.iter().map(|r| r.id).collect();
    for id in retrieved {
        if !ids.contains(&id) {
            return Err(
                format!("{query}: retrieve returned {id}, outside the mirrored pool").into(),
            );
        }
    }
    Ok(())
}

fn micros(started: Instant) -> f64 {
    started.elapsed().as_secs_f64() * 1e6
}
