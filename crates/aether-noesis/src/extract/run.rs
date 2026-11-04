//! Running one extraction job: unit by unit (a batch of a session's prose or
//! of a source's chunks), each ending in one transaction that writes the
//! merge, its coverage and the job's progress. A cancel or a failure between
//! units keeps every unit committed before it.

use std::collections::{BTreeMap, BTreeSet};
use std::sync::Arc;

use aether_core::noesis::{Job, JobKind, JobState, JobStats};
use parking_lot::Mutex;
use rusqlite::{Connection, TransactionBehavior};

use super::batch::{self, Message};
use super::candidates::{Index, PER_FINDING};
use super::findings::{self, Submission};
use super::prompt::{self, AuditItem};
use super::{
    BackgroundBudget, CancelFlag, Corpus, ExtractError, ExtractModel, ModelError, Role, jobs,
    spend_of,
};
use crate::budget::Spend;
use crate::graph::store::{self as graph_store, now_text};
use crate::graph::{Extraction, Graph, merge};

/// Existing propositions offered to the extractor as chain endpoints.
const EXISTING_FOR_EXTRACTOR: usize = 20;

/// What a job reports as it goes, for `noesis.job_updated` and
/// `noesis.graph_merged`.
#[derive(Debug, Clone, PartialEq)]
pub enum JobEvent {
    Updated(Job),
    Merged { job: String, stats: JobStats },
}

/// Everything outside `noesis.db` a job uses.
pub struct JobPorts<'a> {
    pub model: &'a mut dyn ExtractModel,
    pub budget: &'a mut dyn BackgroundBudget,
    pub corpus: &'a dyn Corpus,
    pub cancel: &'a CancelFlag,
    pub events: &'a mut dyn FnMut(JobEvent),
}

impl std::fmt::Debug for JobPorts<'_> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("JobPorts")
            .field("cancelled", &self.cancel.is_cancelled())
            .finish_non_exhaustive()
    }
}

/// Why a job stopped before its last unit.
enum Stop {
    Cancelled,
    Failed(String),
    Store(ExtractError),
}

impl From<ExtractError> for Stop {
    fn from(error: ExtractError) -> Self {
        Stop::Store(error)
    }
}

impl From<crate::graph::store::GraphError> for Stop {
    fn from(error: crate::graph::store::GraphError) -> Self {
        Stop::Store(error.into())
    }
}

impl From<rusqlite::Error> for Stop {
    fn from(error: rusqlite::Error) -> Self {
        Stop::Store(error.into())
    }
}

/// Run a job [`claim_next`] moved to running, to its end. Store errors are
/// returned after the job is marked failed where the store still allows it.
pub fn run_job(conn: &Connection, job: Job, mut ports: JobPorts<'_>) -> Result<Job, ExtractError> {
    assert_eq!(job.state, JobState::Running, "run a claimed job");
    let mut job = job;
    let outcome = run_units(conn, &mut job, &mut ports);
    let (state, error) = match outcome {
        Ok(()) => (JobState::Succeeded, None),
        Err(Stop::Cancelled) => (JobState::Cancelled, None),
        Err(Stop::Failed(why)) => (JobState::Failed, Some(why)),
        Err(Stop::Store(error)) => (JobState::Failed, Some(error.to_string())),
    };
    job.state = state;
    job.error = error;
    let ended = jobs::update(conn, &job)?;
    debug_assert!(ended.state.is_terminal());
    (ports.events)(JobEvent::Updated(ended.clone()));
    Ok(ended)
}

fn run_units(conn: &Connection, job: &mut Job, ports: &mut JobPorts<'_>) -> Result<(), Stop> {
    let messages = material(conn, job, ports.corpus)?;
    let coverage = graph_store::coverage_of(conn, &job.target)?;
    let units = batch::batches(batch::uncovered(messages, &coverage), batch::BATCH_TOKENS);
    for unit in units {
        if ports.cancel.is_cancelled() {
            return Err(Stop::Cancelled);
        }
        let extraction = extract_unit(conn, job, &unit, ports)?;
        // A cancel during the unit's calls drops the unit rather than commit it late.
        if ports.cancel.is_cancelled() {
            return Err(Stop::Cancelled);
        }
        commit_unit(conn, job, &unit, &extraction, ports)?;
    }
    Ok(())
}

/// The messages the job reads: a session's teacher prose or a source's chunks.
fn material(conn: &Connection, job: &Job, corpus: &dyn Corpus) -> Result<Vec<Message>, Stop> {
    match job.kind {
        JobKind::ExtractSession => corpus.session_prose(&job.target).map_err(Stop::Failed),
        JobKind::ExtractSource => {
            let Some(row) = graph_store::source(conn, &job.target)? else {
                return Err(Stop::Failed(format!("no source `{}`", job.target)));
            };
            let text = corpus.source_text(&row.artifact).map_err(Stop::Failed)?;
            Ok(batch::chunks(&text))
        }
        JobKind::ExtractEngineering => Err(Stop::Failed(
            "engineering extraction is not part of this service".into(),
        )),
    }
}

/// One model call, admitted by the budget first and charged after.
fn call(job: &mut Job, ports: &mut JobPorts<'_>, role: Role, prompt: &str) -> Result<String, Stop> {
    if ports.cancel.is_cancelled() {
        return Err(Stop::Cancelled);
    }
    let estimate = Spend {
        usd: 0.0,
        tokens: batch::tokens(prompt),
    };
    if let Some(reason) = ports.budget.before_invocation(estimate) {
        return Err(Stop::Failed(format!(
            "refused before the model call: {reason}"
        )));
    }
    match ports.model.invoke(role, prompt) {
        Ok(reply) => {
            ports.budget.settle(spend_of(&reply.usage));
            job.usage += reply.usage;
            Ok(reply.text)
        }
        Err(error @ ModelError::NotSent(_)) => {
            ports.budget.settle(Spend::default());
            Err(Stop::Failed(error.to_string()))
        }
        Err(_) if ports.cancel.is_cancelled() => {
            ports.budget.settle(estimate);
            Err(Stop::Cancelled)
        }
        Err(error @ ModelError::Failed(_)) => {
            ports.budget.settle(estimate);
            Err(Stop::Failed(error.to_string()))
        }
    }
}

/// The extractor's findings for `unit`, linked by the auditor.
fn extract_unit(
    conn: &Connection,
    job: &mut Job,
    unit: &[Message],
    ports: &mut JobPorts<'_>,
) -> Result<Extraction, Stop> {
    let graph = graph_store::load(conn)?;
    let index = Index::new(&graph);
    let all_text: String = unit
        .iter()
        .map(|m| m.text.as_str())
        .collect::<Vec<_>>()
        .join("\n");
    let existing = with_statements(&graph, index.nearest(&all_text, EXISTING_FOR_EXTRACTOR));
    let reply = call(
        job,
        ports,
        Role::Extractor,
        &prompt::extractor(unit, &existing),
    )?;
    let refs: BTreeSet<String> = unit.iter().map(|m| m.message_ref.clone()).collect();
    let submission =
        findings::submission(&reply, &job.target, &refs, &graph).map_err(Stop::Failed)?;
    let links = audit(job, ports, &graph, &index, &submission)?;
    let mut extraction = Extraction {
        findings: submission.findings,
        chains: submission.chains,
    };
    for finding in &mut extraction.findings {
        finding.link = links.get(&finding.key).cloned();
    }
    Ok(extraction)
}

fn with_statements(graph: &Graph, ids: Vec<String>) -> Vec<(String, String)> {
    ids.into_iter()
        .map(|id| {
            let statement = graph.nodes[&id].statement.clone();
            (id, statement)
        })
        .collect()
}

/// The auditor's links; no call when nothing was found or nothing is like it.
fn audit(
    job: &mut Job,
    ports: &mut JobPorts<'_>,
    graph: &Graph,
    index: &Index,
    submission: &Submission,
) -> Result<BTreeMap<String, String>, Stop> {
    let mut candidates = BTreeMap::new();
    let mut items = Vec::new();
    for finding in &submission.findings {
        let text = format!("{} {}", finding.name, finding.statement);
        let near = index.nearest(&text, PER_FINDING);
        if near.is_empty() {
            continue;
        }
        items.push(AuditItem {
            key: finding.key.clone(),
            name: finding.name.clone(),
            statement: finding.statement.clone(),
            candidates: with_statements(graph, near.clone()),
        });
        candidates.insert(finding.key.clone(), near);
    }
    if items.is_empty() {
        return Ok(BTreeMap::new());
    }
    let reply = call(job, ports, Role::Auditor, &prompt::auditor(&items))?;
    findings::links(&reply, &candidates).map_err(Stop::Failed)
}

/// Merge the unit, then write the graph, its coverage (last) and the job's
/// progress in one immediate transaction: all of it lands, or none.
fn commit_unit(
    conn: &Connection,
    job: &mut Job,
    unit: &[Message],
    extraction: &Extraction,
    ports: &mut JobPorts<'_>,
) -> Result<(), Stop> {
    job.state = JobState::Merging;
    *job = jobs::update(conn, job)?;
    (ports.events)(JobEvent::Updated(job.clone()));
    let tx = rusqlite::Transaction::new_unchecked(conn, TransactionBehavior::Immediate)?;
    let before = graph_store::load(&tx)?;
    let outcome = merge(&before, extraction);
    let now = now_text();
    graph_store::write_diff(&tx, &before, &outcome.graph, &now)?;
    let covered: Vec<_> = unit
        .iter()
        .map(|m| batch::covered(&job.target, m))
        .collect();
    graph_store::write_coverage(&tx, &covered, &now)?;
    let unit_stats = JobStats {
        nodes: outcome.nodes,
        chains: outcome.chains,
        dropped_chains: outcome.dropped_chains,
    };
    let total = job.stats.unwrap_or(JobStats {
        nodes: 0,
        chains: 0,
        dropped_chains: 0,
    });
    job.stats = Some(JobStats {
        nodes: total.nodes.saturating_add(unit_stats.nodes),
        chains: total.chains.saturating_add(unit_stats.chains),
        dropped_chains: total
            .dropped_chains
            .saturating_add(unit_stats.dropped_chains),
    });
    job.state = JobState::Running;
    *job = jobs::update(&tx, job)?;
    tx.commit()?;
    (ports.events)(JobEvent::Merged {
        job: job.id.clone(),
        stats: unit_stats,
    });
    (ports.events)(JobEvent::Updated(job.clone()));
    Ok(())
}

/// The background concurrency cap over jobs: at most `cap` run at once, and
/// a cap of 0 runs none.
#[derive(Debug, Clone)]
pub struct Slots {
    cap: u8,
    running: Arc<Mutex<u8>>,
}

/// One running job's slot, freed when dropped.
#[derive(Debug)]
pub struct Slot {
    running: Arc<Mutex<u8>>,
}

impl Drop for Slot {
    fn drop(&mut self) {
        let mut running = self.running.lock();
        debug_assert!(*running > 0, "a slot was taken");
        *running = running.saturating_sub(1);
    }
}

impl Slots {
    pub fn new(cap: u8) -> Self {
        Self {
            cap,
            running: Arc::new(Mutex::new(0)),
        }
    }

    pub fn running(&self) -> u8 {
        *self.running.lock()
    }

    fn try_take(&self) -> Option<Slot> {
        let mut running = self.running.lock();
        if *running >= self.cap {
            return None;
        }
        *running += 1;
        Some(Slot {
            running: Arc::clone(&self.running),
        })
    }
}

/// Take a slot and the oldest queued job, moving it to running; `None` when
/// the cap is reached or nothing is queued.
pub fn claim_next(conn: &Connection, slots: &Slots) -> Result<Option<(Slot, Job)>, ExtractError> {
    let Some(slot) = slots.try_take() else {
        return Ok(None);
    };
    let tx = rusqlite::Transaction::new_unchecked(conn, TransactionBehavior::Immediate)?;
    let next: Option<String> = graph_store::optional_text(
        &tx,
        "SELECT id FROM jobs WHERE state = 'queued' AND ?1 = ?1 ORDER BY created_at, id LIMIT 1",
        "",
    )?;
    let Some(id) = next else {
        return Ok(None);
    };
    let mut job = jobs::get(&tx, &id)?;
    job.state = JobState::Running;
    let job = jobs::update(&tx, &job)?;
    tx.commit()?;
    assert!(slots.running() <= slots.cap, "the cap holds");
    Ok(Some((slot, job)))
}
