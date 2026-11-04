//! The optional abstraction step: one memory, written by an [`Abstractor`],
//! that states what a merged cluster says.
//!
//! It is off unless the caller passes [`AbstractionOptions`], refuses to start
//! when the plan needs more than its budget, and keeps every member: the
//! abstraction supersedes the survivor, which still points at its members, so
//! [`MemoryService::rollback_abstraction`] puts the survivor back as it was.

use aether_core::event::{Actor, EventDraft, EventKind};
use aether_core::id::MemoryId;
use aether_core::memory::{MemoryCandidate, MemoryProvenance, MemoryRecord};
use aether_core::memory_admission::{MemoryAdmission, Verification};
use aether_core::time::now;
use aether_store::{Commit, Mutation};

use super::cluster::Standing;
use super::{AbstractionReport, ClusterOutcome, ClusterReport, Fault, Step};
use crate::{MemoryError, MemoryService, Result};

/// The key under a record's `structured` that marks it an abstraction.
const MARKER: &str = "abstraction";

/// Writes the abstracted text of a cluster: a model, in practice.
pub trait Abstractor {
    /// `members` is the cluster, survivor first. Returns the text to store.
    fn abstract_cluster(&self, members: &[MemoryRecord]) -> Result<String>;
}

/// What one run may spend. Tokens are estimated as four characters each.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AbstractionBudget {
    /// Clusters, and so abstractor calls, at most.
    pub max_clusters: usize,
    /// Input and output tokens across the whole run, at most.
    pub max_tokens: u64,
    /// Tokens one abstraction may be; a longer one is refused.
    pub output_tokens: u32,
}

pub struct AbstractionOptions<'a> {
    pub abstractor: &'a dyn Abstractor,
    pub budget: AbstractionBudget,
}

impl std::fmt::Debug for AbstractionOptions<'_> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("AbstractionOptions")
            .field("budget", &self.budget)
            .finish_non_exhaustive()
    }
}

#[derive(Debug, Clone, PartialEq)]
pub enum AbstractionOutcome {
    /// A dry run: what the cluster would cost.
    Planned {
        estimated_tokens: u64,
    },
    Written {
        memory: MemoryId,
    },
    Skipped {
        reason: String,
    },
}

impl AbstractionBudget {
    /// Refuse, before anything is written, a plan this budget cannot pay for.
    pub(crate) fn admits(&self, plan: &[Vec<Standing>]) -> Result<()> {
        let eligible: Vec<&Vec<Standing>> = plan.iter().filter(|c| abstractable(c)).collect();
        let tokens: u64 = eligible.iter().map(|c| self.estimate(c)).sum();
        if eligible.len() > self.max_clusters || tokens > self.max_tokens {
            return Err(MemoryError::Invalid(format!(
                "abstraction over budget: {} clusters and about {tokens} tokens, against at most \
                 {} clusters and {} tokens; nothing was consolidated",
                eligible.len(),
                self.max_clusters,
                self.max_tokens
            )));
        }
        Ok(())
    }

    fn estimate(&self, cluster: &[Standing]) -> u64 {
        let input: u64 = cluster.iter().map(|s| tokens(&s.record.text)).sum();
        input + u64::from(self.output_tokens)
    }
}

/// A survivor consolidation may not retire may not be abstracted away either.
fn abstractable(cluster: &[Standing]) -> bool {
    !cluster[0].fixed()
}

fn tokens(text: &str) -> u64 {
    text.chars().count().div_ceil(4) as u64
}

/// What a dry run reports for each cluster the step would abstract.
pub(super) fn planned(
    plan: &[Vec<Standing>],
    options: Option<&AbstractionOptions<'_>>,
) -> Vec<AbstractionReport> {
    let Some(options) = options else {
        return Vec::new();
    };
    plan.iter()
        .filter(|c| abstractable(c))
        .map(|c| AbstractionReport {
            survivor: c[0].record.id,
            outcome: AbstractionOutcome::Planned {
                estimated_tokens: options.budget.estimate(c),
            },
        })
        .collect()
}

impl MemoryService {
    /// Abstract each cluster that merged, in plan order.
    pub(super) fn abstract_merged(
        &self,
        plan: &[Vec<Standing>],
        merged: &[ClusterReport],
        options: &AbstractionOptions<'_>,
        by: &Actor,
        fault: Fault<'_>,
    ) -> Result<Vec<AbstractionReport>> {
        debug_assert_eq!(plan.len(), merged.len());
        let mut reports = Vec::new();
        for (n, (cluster, report)) in plan.iter().zip(merged).enumerate() {
            debug_assert_eq!(cluster[0].record.id, report.survivor);
            if !abstractable(cluster) || !matches!(report.outcome, ClusterOutcome::Merged { .. }) {
                continue;
            }
            let outcome = self.abstract_one(n, cluster, options, by, fault)?;
            reports.push(AbstractionReport {
                survivor: report.survivor,
                outcome,
            });
        }
        Ok(reports)
    }

    fn abstract_one(
        &self,
        n: usize,
        cluster: &[Standing],
        options: &AbstractionOptions<'_>,
        by: &Actor,
        fault: Fault<'_>,
    ) -> Result<AbstractionOutcome> {
        let id = cluster[0].record.id;
        let Some(survivor) = self.store.memory(id)?.filter(MemoryRecord::is_current) else {
            return Ok(skipped(format!("survivor {id} is no longer current")));
        };
        let mut members = vec![survivor];
        members.extend(cluster[1..].iter().map(|s| s.record.clone()));
        let text = options.abstractor.abstract_cluster(&members)?;
        let text = text.trim();
        if tokens(text) > u64::from(options.budget.output_tokens) {
            return Ok(skipped(format!(
                "the abstraction is over {} tokens",
                options.budget.output_tokens
            )));
        }
        let hold = self.gate.hold();
        let unchanged = |r: &MemoryRecord| r.is_current() && r.text == members[0].text;
        let Some(survivor) = self.store.memory(id)?.filter(unchanged) else {
            return Ok(skipped(format!(
                "survivor {id} changed while it was abstracted"
            )));
        };
        let abstraction = abstraction_record(&survivor, text, &members);
        if let Some(reason) = self.reject_reason(&candidate_of(&abstraction)) {
            return Ok(skipped(format!("abstraction refused: {reason}")));
        }
        let admission = abstraction_admission(&abstraction, &self.admission(&survivor)?);
        self.commit(
            &hold,
            abstraction_commit(survivor, &abstraction, admission, by),
        )?;
        drop(hold);
        fault(Step::Abstracted(n))?;
        if let Some(index) = &self.index {
            index.forget(id)?;
        }
        self.index_committed(&abstraction)?;
        Ok(AbstractionOutcome::Written {
            memory: abstraction.id,
        })
    }

    /// Undo an abstraction: retire it and put the survivor it replaced back.
    /// Returns the survivor, current again.
    pub fn rollback_abstraction(&self, id: MemoryId, by: Actor) -> Result<MemoryRecord> {
        let hold = self.gate.hold();
        let mut abstraction = self.store.memory(id)?.ok_or(MemoryError::NotFound(id))?;
        let of = abstracted_from(&abstraction)
            .ok_or_else(|| MemoryError::Invalid(format!("memory {id} is not an abstraction")))?;
        if !abstraction.is_current() {
            return Err(MemoryError::Invalid(format!(
                "abstraction {id} is no longer current; nothing to roll back"
            )));
        }
        let mut survivor = self.store.memory(of)?.ok_or(MemoryError::NotFound(of))?;
        if survivor.superseded_by != Some(id) {
            return Err(MemoryError::Invalid(format!(
                "memory {of} is no longer superseded by abstraction {id}"
            )));
        }
        abstraction.forgotten_at = Some(now());
        survivor.superseded_by = None;
        let commit = Commit::new()
            .mutate(Mutation::PutMemory(abstraction))
            .mutate(Mutation::PutMemory(survivor.clone()))
            .event(EventDraft::new(
                by.clone(),
                EventKind::MemoryForgotten {
                    memory: id,
                    reason: Some("abstraction rolled back".into()),
                },
            ))
            .event(EventDraft::new(
                by,
                EventKind::Custom {
                    name: "memory.abstraction_rolled_back".into(),
                    payload: serde_json::json!({ "memory": id, "restored": of }),
                },
            ));
        self.commit(&hold, commit)?;
        drop(hold);
        debug_assert!(survivor.is_current());
        if let Some(index) = &self.index {
            index.forget(id)?;
        }
        self.index_committed(&survivor)?;
        Ok(survivor)
    }
}

fn skipped(reason: String) -> AbstractionOutcome {
    AbstractionOutcome::Skipped { reason }
}

/// The survivor an abstraction replaced, if `record` is one.
fn abstracted_from(record: &MemoryRecord) -> Option<MemoryId> {
    let of = record.structured.as_ref()?.get(MARKER)?.get("of")?;
    let of: MemoryId = serde_json::from_value(of.clone()).ok()?;
    (record.supersedes == Some(of)).then_some(of)
}

/// A new memory in the survivor's place. It names no author: the text is
/// the abstractor's, so it must not stand as a person's or the system's word.
fn abstraction_record(
    survivor: &MemoryRecord,
    text: &str,
    members: &[MemoryRecord],
) -> MemoryRecord {
    let ids: Vec<MemoryId> = members.iter().map(|m| m.id).collect();
    let record = MemoryRecord::from_candidate(
        MemoryCandidate {
            scope: survivor.scope.clone(),
            kind: survivor.kind,
            text: text.to_owned(),
            structured: Some(serde_json::json!({ MARKER: { "of": survivor.id, "members": ids } })),
            importance: survivor.importance,
            confidence: survivor.confidence,
            tags: survivor.tags.clone(),
            provenance: MemoryProvenance::default(),
            supersedes: Some(survivor.id),
        },
        now(),
    );
    debug_assert_eq!(abstracted_from(&record), Some(survivor.id));
    debug_assert!(record.is_current());
    record
}

fn candidate_of(record: &MemoryRecord) -> MemoryCandidate {
    MemoryCandidate {
        scope: record.scope.clone(),
        kind: record.kind,
        text: record.text.clone(),
        structured: None,
        importance: record.importance,
        confidence: record.confidence,
        tags: record.tags.clone(),
        provenance: record.provenance.clone(),
        supersedes: record.supersedes,
    }
}

/// The survivor's standing, carried over, but never more than observed:
/// nobody verified the abstractor's wording.
fn abstraction_admission(record: &MemoryRecord, survivor: &MemoryAdmission) -> MemoryAdmission {
    let mut admission = survivor.clone();
    admission.memory = record.id;
    admission.verification = admission.verification.min(Verification::Observed);
    debug_assert_ne!(admission.memory, survivor.memory);
    admission
}

fn abstraction_commit(
    mut survivor: MemoryRecord,
    abstraction: &MemoryRecord,
    admission: MemoryAdmission,
    by: &Actor,
) -> Commit {
    let old = survivor.id;
    survivor.superseded_by = Some(abstraction.id);
    Commit::new()
        .mutate(Mutation::PutMemory(abstraction.clone()))
        .mutate(Mutation::PutMemoryAdmission(admission))
        .mutate(Mutation::PutMemory(survivor))
        .event(EventDraft::new(
            by.clone(),
            EventKind::MemoryAccepted {
                memory: abstraction.id,
                reason: Some("the abstraction of a consolidated cluster".into()),
            },
        ))
        .event(EventDraft::new(
            by.clone(),
            EventKind::MemorySuperseded {
                old,
                new: abstraction.id,
                reason: Some("abstracted; roll the abstraction back to restore it".into()),
            },
        ))
}
