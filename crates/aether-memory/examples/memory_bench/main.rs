//! Driver of the operational-memory benchmark. `evals/memory_bench.py` writes
//! a plan, runs this on it in a private sandbox and computes the metrics:
//!
//! ```sh
//! cargo run --release -p aether-memory --example memory_bench -- <plan.json> <empty-sandbox-dir>
//! cargo run --release -p aether-memory --example memory_bench -- --profile <plan.json> <empty-sandbox-dir>
//! ```
//!
//! `--profile` times each stage of retrieval instead (`profile.rs`).
//!
//! It drives the code agents use: `MemoryService::admit` (the `remember`
//! path), `retrieve` with the request a turn builds, and the context layer a
//! model is sent. It prints one JSON document; it never calls a model.

mod plan;
mod profile;
mod recovery;
mod retrieval;
mod seed;

#[cfg(test)]
mod tests;

use std::error::Error;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use aether_memory::{LocalEmbeddingIndex, MemoryService};
use aether_store::{SqliteStore, WriteGate};
use serde::Serialize;

use plan::Plan;

pub type BenchResult<T> = Result<T, Box<dyn Error>>;

/// The store file inside each sandbox directory, as the daemon names its own.
const STORE_FILE: &str = "aether.db";

/// Everything one run measured, as the harness reads it.
#[derive(Debug, Serialize)]
pub struct Report {
    pub index: String,
    pub admissions: Vec<seed::AdmissionRow>,
    /// Clusters consolidation merged; zero when the plan does not consolidate.
    pub consolidated: usize,
    /// Keys the store holds current after seeding, read back from it.
    pub current: Vec<String>,
    pub queries: Vec<retrieval::QueryRow>,
    pub latency: retrieval::LatencyRows,
    pub storage: seed::StorageRow,
    pub restart: recovery::RestartRow,
    pub corruption: Vec<recovery::CorruptionRow>,
}

fn main() -> BenchResult<()> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let (profiling, plan_path, sandbox) = match args.as_slice() {
        [flag, plan, sandbox] if flag == "--profile" => (true, plan, sandbox),
        [plan, sandbox] => (false, plan, sandbox),
        _ => return Err("usage: memory_bench [--profile] <plan.json> <empty-sandbox-dir>".into()),
    };
    let plan = Plan::read(Path::new(plan_path))?;
    let json = if profiling {
        serde_json::to_string(&profile::run(&plan, Path::new(sandbox))?)?
    } else {
        serde_json::to_string(&run(&plan, Path::new(sandbox))?)?
    };
    println!("{json}");
    Ok(())
}

/// The whole benchmark against a sandbox the caller created empty and removes.
pub fn run(plan: &Plan, sandbox: &Path) -> BenchResult<Report> {
    if !sandbox.is_dir() || std::fs::read_dir(sandbox)?.next().is_some() {
        return Err(format!("sandbox {} must be an empty directory", sandbox.display()).into());
    }
    let empty = seed::empty_store(&sandbox.join("empty"))?;
    let main_dir = sandbox.join("main");
    let (store, service) = open(&main_dir, &plan.index)?;
    let mut seeded = seed::seed(&store, &service, plan)?;
    let consolidated = match plan.consolidate {
        true => seed::consolidate(&store, &service, &mut seeded)?,
        false => 0,
    };
    checkpoint(&store)?;
    let queries = retrieval::run_queries(&service, plan, &seeded)?;
    let small = retrieval::time_queries(&service, plan, &seeded)?;
    drop(service);
    drop(store);
    let storage = seed::storage_row(empty, &main_dir, &seeded)?;
    let restart = recovery::restart(&main_dir, plan, &seeded, &queries)?;
    let corruption = recovery::corruption_cases(&main_dir, sandbox, plan, &seeded)?;
    let scaled = retrieval::scaled_latency(&sandbox.join("scaled"), plan)?;
    debug_assert_eq!(queries.len(), plan.queries.len());
    Ok(Report {
        index: plan.index.clone(),
        admissions: seeded.admissions,
        consolidated,
        current: seeded.current_keys,
        queries,
        latency: retrieval::LatencyRows { small, scaled },
        storage,
        restart,
        corruption,
    })
}

/// A store in `dir` and the memory service over it, configured as the daemon
/// configures it for `index` (`none`, the default, or `local`).
pub fn open(dir: &Path, index: &str) -> BenchResult<(Arc<SqliteStore>, MemoryService)> {
    let store = Arc::new(SqliteStore::open(store_path(dir))?);
    let service = MemoryService::new(store.clone(), WriteGate::new());
    let service = match index {
        "none" => service,
        "local" => service.with_index(Arc::new(LocalEmbeddingIndex)),
        other => return Err(format!("unknown memory index `{other}`").into()),
    };
    Ok((store, service))
}

pub fn store_path(dir: &Path) -> PathBuf {
    dir.join(STORE_FILE)
}

/// Fold the WAL into the database file, so a copy of that file alone holds
/// every commit. A reader can hold frames back for a moment; it is retried.
pub fn checkpoint(store: &SqliteStore) -> BenchResult<()> {
    for _ in 0..100 {
        if store.sync()? {
            return Ok(());
        }
        std::thread::sleep(std::time::Duration::from_millis(20));
    }
    Err("the WAL checkpoint was held back for two seconds".into())
}

/// A closed store's size on disk.
#[derive(Debug, Clone, Copy, Serialize)]
pub struct Footprint {
    /// The checkpointed database file alone.
    pub database: u64,
    /// Everything in its directory: database, WAL, shared memory, artifacts.
    pub total: u64,
}

pub fn footprint(dir: &Path) -> BenchResult<Footprint> {
    let database = std::fs::metadata(store_path(dir))?.len();
    let total = dir_bytes(dir)?;
    debug_assert!(database <= total);
    Ok(Footprint { database, total })
}

/// Bytes of every file under `dir`: the database, its WAL and side files.
fn dir_bytes(dir: &Path) -> BenchResult<u64> {
    let mut total = 0;
    for entry in std::fs::read_dir(dir)? {
        let entry = entry?;
        let kind = entry.file_type()?;
        if kind.is_dir() {
            total += dir_bytes(&entry.path())?;
        } else if kind.is_file() {
            total += entry.metadata()?.len();
        }
    }
    Ok(total)
}
