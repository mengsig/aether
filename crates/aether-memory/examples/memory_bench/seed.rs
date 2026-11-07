//! Seeding a store through admission, as agents fill it, and what it costs.

use std::collections::BTreeMap;
use std::path::Path;

use aether_core::event::Actor;
use aether_core::id::{AgentId, MemoryFeedbackId, MemoryId, TaskId};
use aether_core::memory::{MemoryCandidate, MemoryProvenance, MemoryRecord};
use aether_core::memory_feedback::{
    MemoryEvidence, MemoryFeedback, MemoryFeedbackSpec, MemoryVerdict,
};
use aether_core::time::now;
use aether_memory::{Admission, ClusterOutcome, MemoryService};
use aether_store::{Commit, MemoryQuery, Mutation, SqliteStore, Store};
use chrono::Duration;
use serde::Serialize;

use crate::plan::{Plan, PlanMemory};
use crate::{BenchResult, Footprint, checkpoint, footprint};

/// What admission made of one candidate.
#[derive(Debug, Serialize)]
pub struct AdmissionRow {
    pub key: String,
    /// `accepted`, `superseded` (accepted, replacing another) or `rejected`.
    pub outcome: &'static str,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
    /// The key admission retired, for `superseded`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub replaced: Option<String>,
}

#[derive(Debug, Serialize)]
pub struct StorageRow {
    pub empty: Footprint,
    pub seeded: Footprint,
    /// Admitted records, current or superseded: every row the store keeps.
    pub records: usize,
    pub current: usize,
}

/// The seeded store's key map and admission log.
#[derive(Debug)]
pub struct Seeded {
    /// The agent every candidate was remembered by; every query runs as it.
    pub agent: AgentId,
    pub keys: Keys,
    pub admissions: Vec<AdmissionRow>,
    pub records: usize,
    /// Keys of the memories the store holds current after seeding, sorted.
    pub current_keys: Vec<String>,
    pub current: usize,
}

/// Dataset keys by memory id and back, for every admitted candidate.
#[derive(Debug, Default, Clone)]
pub struct Keys {
    pub by_id: BTreeMap<MemoryId, String>,
    pub by_key: BTreeMap<String, MemoryId>,
}

impl Keys {
    fn insert(&mut self, key: &str, id: MemoryId) {
        let fresh_id = self.by_id.insert(id, key.to_owned()).is_none();
        let fresh_key = self.by_key.insert(key.to_owned(), id).is_none();
        debug_assert!(fresh_id && fresh_key, "{key} admitted twice");
    }

    pub fn key(&self, id: MemoryId) -> BenchResult<&str> {
        self.by_id
            .get(&id)
            .map(String::as_str)
            .ok_or_else(|| format!("retrieved {id}, which the plan never admitted").into())
    }
}

/// Admit the plan in order, then backdate and record feedback.
pub fn seed(store: &SqliteStore, service: &MemoryService, plan: &Plan) -> BenchResult<Seeded> {
    let agent = AgentId::new();
    let mut keys = Keys::default();
    let mut admissions = Vec::with_capacity(plan.memories.len());
    for memory in &plan.memories {
        let candidate = candidate(memory, agent, &keys)?;
        let actor = actor(memory, agent);
        let admission = service.admit(candidate, actor, Some(agent), None)?;
        if let Some(record) = admission.record() {
            keys.insert(&memory.key, record.id);
        }
        admissions.push(admission_row(&memory.key, admission, &keys)?);
    }
    backdate(store, plan, &keys)?;
    record_feedback(store, plan, &keys)?;
    let current_keys = current_keys(store, &keys)?;
    debug_assert_eq!(admissions.len(), plan.memories.len());
    debug_assert!(current_keys.len() <= keys.by_id.len());
    Ok(Seeded {
        agent,
        records: keys.by_id.len(),
        current: current_keys.len(),
        current_keys,
        keys,
        admissions,
    })
}

/// Merge near-duplicates as an operator's `consolidate_memories` does, then
/// read back what stays current. Returns the clusters merged.
pub fn consolidate(
    store: &SqliteStore,
    service: &MemoryService,
    seeded: &mut Seeded,
) -> BenchResult<usize> {
    let report = service.consolidate(None, Actor::user(), Default::default())?;
    let merged = report
        .clusters
        .iter()
        .filter(|c| matches!(c.outcome, ClusterOutcome::Merged { .. }))
        .count();
    if merged != report.clusters.len() {
        return Err(format!("{} clusters skipped", report.clusters.len() - merged).into());
    }
    let before = seeded.current;
    seeded.current_keys = current_keys(store, &seeded.keys)?;
    seeded.current = seeded.current_keys.len();
    debug_assert!(seeded.current <= before);
    Ok(merged)
}

fn actor(memory: &PlanMemory, agent: AgentId) -> Actor {
    if memory.operator {
        Actor::user()
    } else {
        Actor::agent(agent)
    }
}

/// The candidate as the runtime hands it over: the `remember` tool sets the
/// agent; an operator's carries the authenticated actor.
fn candidate(memory: &PlanMemory, agent: AgentId, keys: &Keys) -> BenchResult<MemoryCandidate> {
    let supersedes =
        match &memory.supersedes {
            Some(old) => Some(*keys.by_key.get(old).ok_or_else(|| {
                format!("{} supersedes {old}, which admission rejected", memory.key)
            })?),
            None => None,
        };
    let provenance = MemoryProvenance {
        actor: memory.operator.then(Actor::user),
        agent: (!memory.operator).then_some(agent),
        ..Default::default()
    };
    Ok(MemoryCandidate {
        scope: memory.scope.clone(),
        kind: memory.kind,
        text: memory.text.clone(),
        structured: None,
        importance: memory.importance,
        confidence: memory.confidence,
        tags: memory.tags.clone(),
        provenance,
        supersedes,
    })
}

fn admission_row(key: &str, admission: Admission, keys: &Keys) -> BenchResult<AdmissionRow> {
    let (outcome, reason, replaced) = match admission {
        Admission::Accepted { .. } => ("accepted", None, None),
        Admission::Superseded { old, .. } => ("superseded", None, Some(keys.key(old)?.to_owned())),
        Admission::Rejected { reason } => ("rejected", Some(reason), None),
        Admission::Reinforced { .. } => ("reinforced", None, None),
    };
    debug_assert!(replaced.as_deref() != Some(key), "{key} replaced itself");
    Ok(AdmissionRow {
        key: key.to_owned(),
        outcome,
        reason,
        replaced,
    })
}

/// Move `created_at` back by each memory's `age_days`, written directly: no
/// public path records an old memory. See BASELINE.md's fixture exceptions.
fn backdate(store: &SqliteStore, plan: &Plan, keys: &Keys) -> BenchResult<()> {
    let at = now();
    let mut commit = Commit::new();
    for memory in plan.memories.iter().filter(|m| m.age_days > 0.0) {
        let Some(id) = keys.by_key.get(&memory.key) else {
            continue;
        };
        let mut record = store.memory(*id)?.ok_or("an admitted memory is missing")?;
        let seconds = (f64::from(memory.age_days) * 86_400.0).round() as i64;
        record.created_at = at - Duration::seconds(seconds);
        debug_assert!(record.created_at < at);
        commit.push_mutation(Mutation::PutMemory(record));
    }
    if !commit.is_empty() {
        store.commit(commit)?;
    }
    Ok(())
}

/// Review verdicts, each from its own invented task, written directly: this
/// seeds what ranking reads and skips the runtime's feedback validation.
fn record_feedback(store: &SqliteStore, plan: &Plan, keys: &Keys) -> BenchResult<()> {
    let mut commit = Commit::new();
    for memory in &plan.memories {
        let votes = memory.feedback;
        if votes.helped + votes.hurt == 0 {
            continue;
        }
        let id = *keys
            .by_key
            .get(&memory.key)
            .ok_or_else(|| format!("feedback on {}, which admission rejected", memory.key))?;
        let verdicts = std::iter::repeat_n(MemoryVerdict::Helped, votes.helped as usize).chain(
            std::iter::repeat_n(MemoryVerdict::Hurt, votes.hurt as usize),
        );
        for verdict in verdicts {
            commit.push_mutation(Mutation::RecordMemoryFeedback(review(id, verdict)));
        }
    }
    if !commit.is_empty() {
        store.commit(commit)?;
    }
    Ok(())
}

fn review(memory: MemoryId, verdict: MemoryVerdict) -> MemoryFeedback {
    MemoryFeedback {
        id: MemoryFeedbackId::new(),
        spec: MemoryFeedbackSpec {
            memory,
            verdict,
            evidence: MemoryEvidence::Review,
            task: TaskId::new(),
            invocation: None,
            reason: "memory benchmark".into(),
        },
        actor: Actor::user(),
        created_at: now(),
        retracted_at: None,
        retraction_reason: None,
    }
}

/// Every current memory in the store, whatever its scope.
fn current_memories(store: &SqliteStore) -> BenchResult<Vec<MemoryRecord>> {
    let all = store.memories(&MemoryQuery {
        limit: 1_000_000,
        ..Default::default()
    })?;
    debug_assert!(all.iter().all(|m| m.is_current()));
    Ok(all)
}

pub fn count_current(store: &SqliteStore) -> BenchResult<usize> {
    Ok(current_memories(store)?.len())
}

/// The dataset keys of the store's current memories, as the store reports them.
fn current_keys(store: &SqliteStore, keys: &Keys) -> BenchResult<Vec<String>> {
    let mut current = current_memories(store)?
        .iter()
        .map(|record| keys.key(record.id).map(str::to_owned))
        .collect::<BenchResult<Vec<String>>>()?;
    current.sort();
    let before = current.len();
    current.dedup();
    debug_assert_eq!(before, current.len(), "the store listed a memory twice");
    Ok(current)
}

/// The size of a store with nothing in it, closed.
pub fn empty_store(dir: &Path) -> BenchResult<Footprint> {
    let store = SqliteStore::open(crate::store_path(dir))?;
    checkpoint(&store)?;
    drop(store);
    footprint(dir)
}

/// The seeded store's size, measured closed so the WAL is folded in.
pub fn storage_row(empty: Footprint, dir: &Path, seeded: &Seeded) -> BenchResult<StorageRow> {
    let footprint = footprint(dir)?;
    debug_assert!(footprint.database >= empty.database);
    Ok(StorageRow {
        empty,
        seeded: footprint,
        records: seeded.records,
        current: seeded.current,
    })
}
