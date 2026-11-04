//! Consolidation: near-duplicate memories merged into the one each cluster keeps.
//!
//! Clustering is deterministic (see `cluster`). Each cluster is merged in one
//! commit: the survivor takes the cluster's support, refs, tags and standing,
//! and every other member is marked superseded by it and kept, so no memory
//! is lost and a crash between commits leaves each cluster whole or untouched.
//! Running again finishes the rest, since a merged member is no longer current.
//!
//! An optional abstraction step (see `abstraction`) may then write one
//! abstracted memory per cluster. It is off unless the caller passes one.

mod abstraction;
mod cluster;
mod similarity;

#[cfg(test)]
mod tests;

pub use abstraction::{AbstractionBudget, AbstractionOptions, AbstractionOutcome, Abstractor};

use std::collections::BTreeMap;

use aether_core::event::{Actor, EventDraft, EventKind};
use aether_core::id::MemoryId;
use aether_core::memory::{MemoryRecord, MemoryScope};
use aether_core::memory_admission::{MemoryAdmission, MemoryState, Verification};
use aether_store::{Commit, MemoryQuery, Mutation};

use crate::admission::promotion;
use crate::{MemoryError, MemoryService, Result, tiers};
use cluster::Standing;

/// The most current memories one consolidation reads. Clustering compares
/// pairs that share a word, so a larger store is consolidated a scope at a time.
pub const MAX_CONSOLIDATED: usize = 10_000;

/// Tags a merged survivor keeps, as admission caps an operator's.
const MAX_TAGS: usize = 32;

/// What to do. The default merges for real and abstracts nothing.
#[derive(Debug, Default)]
pub struct ConsolidationOptions<'a> {
    /// Plan and report, writing nothing.
    pub dry_run: bool,
    /// The abstraction step; `None`, the default, skips it.
    pub abstraction: Option<AbstractionOptions<'a>>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct ConsolidationReport {
    pub dry_run: bool,
    /// Current memories read, across every scope consolidated.
    pub scanned: usize,
    pub clusters: Vec<ClusterReport>,
    pub abstractions: Vec<AbstractionReport>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct ClusterReport {
    pub scope: MemoryScope,
    pub survivor: MemoryId,
    /// The members it replaces, strongest first; never the survivor.
    pub members: Vec<MemoryId>,
    pub outcome: ClusterOutcome,
}

#[derive(Debug, Clone, PartialEq)]
pub enum ClusterOutcome {
    /// A dry run: nothing was written.
    Planned,
    /// `promoted` when the merged support made a candidate survivor active.
    Merged { promoted: bool },
    /// Something changed under the plan; the cluster was left as it was.
    Skipped { reason: String },
}

#[derive(Debug, Clone, PartialEq)]
pub struct AbstractionReport {
    pub survivor: MemoryId,
    pub outcome: AbstractionOutcome,
}

/// A point between the steps of a run, where a test can stop it as a crash would.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Step {
    Planned,
    /// Cluster `n`'s commit is built and not yet written.
    Prepared(usize),
    /// Cluster `n` is committed; the index is not yet told.
    Committed(usize),
    /// Cluster `n`'s abstraction is committed; the index is not yet told.
    Abstracted(usize),
}

pub(crate) type Fault<'f> = &'f mut dyn FnMut(Step) -> Result<()>;

impl MemoryService {
    /// Merge near-duplicates in `scope`, or in every scope when `None`.
    pub fn consolidate(
        &self,
        scope: Option<MemoryScope>,
        by: Actor,
        options: ConsolidationOptions<'_>,
    ) -> Result<ConsolidationReport> {
        self.consolidate_with(scope, by, options, &mut |_| Ok(()))
    }

    pub(crate) fn consolidate_with(
        &self,
        scope: Option<MemoryScope>,
        by: Actor,
        options: ConsolidationOptions<'_>,
        fault: Fault<'_>,
    ) -> Result<ConsolidationReport> {
        let (scanned, plan) = self.plan(scope)?;
        if let Some(abstraction) = &options.abstraction {
            abstraction.budget.admits(&plan)?;
        }
        fault(Step::Planned)?;
        let mut report = ConsolidationReport {
            dry_run: options.dry_run,
            scanned,
            clusters: Vec::with_capacity(plan.len()),
            abstractions: Vec::new(),
        };
        if options.dry_run {
            report.clusters = plan
                .iter()
                .map(|c| cluster_report(c, ClusterOutcome::Planned))
                .collect();
            report.abstractions = abstraction::planned(&plan, options.abstraction.as_ref());
            return Ok(report);
        }
        for (n, cluster) in plan.iter().enumerate() {
            let outcome = self.merge(n, cluster, &by, fault)?;
            report.clusters.push(cluster_report(cluster, outcome));
        }
        if let Some(abstraction) = &options.abstraction {
            report.abstractions =
                self.abstract_merged(&plan, &report.clusters, abstraction, &by, fault)?;
        }
        debug_assert_eq!(report.clusters.len(), plan.len());
        Ok(report)
    }

    /// Every cluster to merge, scope by scope, survivor first in each.
    fn plan(&self, scope: Option<MemoryScope>) -> Result<(usize, Vec<Vec<Standing>>)> {
        let scope = scope.map(|s| self.canonical_scope(s)).transpose()?;
        let records = self.store.memories(&MemoryQuery {
            scope: scope.clone(),
            exact_scope: true,
            include_candidates: true,
            limit: MAX_CONSOLIDATED + 1,
            ..Default::default()
        })?;
        if records.len() > MAX_CONSOLIDATED {
            return Err(MemoryError::Invalid(format!(
                "more than {MAX_CONSOLIDATED} current memories; consolidate one scope at a time"
            )));
        }
        debug_assert!(records.iter().all(MemoryRecord::is_current));
        debug_assert!(
            scope
                .as_ref()
                .is_none_or(|s| records.iter().all(|r| &r.scope == s))
        );
        let scanned = records.len();
        let mut scopes: BTreeMap<String, Vec<Standing>> = BTreeMap::new();
        for record in records {
            // A note read from a committed file is the file's to change.
            if record.provenance.source.is_some() || abstraction::is_abstraction(&record) {
                continue;
            }
            let key = serde_json::to_string(&record.scope).map_err(|e| {
                MemoryError::Invalid(format!("memory {} has an unwritable scope: {e}", record.id))
            })?;
            let admission = self.admission(&record)?;
            scopes
                .entry(key)
                .or_default()
                .push(Standing { record, admission });
        }
        let plan = scopes.into_values().flat_map(cluster::clusters).collect();
        Ok((scanned, plan))
    }

    /// Merge one planned cluster in one commit, under the write gate.
    fn merge(
        &self,
        n: usize,
        planned: &[Standing],
        by: &Actor,
        fault: Fault<'_>,
    ) -> Result<ClusterOutcome> {
        let hold = self.gate.hold();
        let fresh = match self.reread(planned)? {
            Ok(fresh) => fresh,
            Err(reason) => return Ok(ClusterOutcome::Skipped { reason }),
        };
        let (survivor, commit, promoted) = self.merged_commit(&fresh, by)?;
        fault(Step::Prepared(n))?;
        self.commit(&hold, commit)?;
        drop(hold);
        fault(Step::Committed(n))?;
        if let Some(index) = &self.index {
            fresh[1..]
                .iter()
                .try_for_each(|m| index.forget(m.record.id))?;
        }
        self.index_committed(&survivor)?;
        Ok(ClusterOutcome::Merged { promoted })
    }

    /// The cluster as it stands now, or why it no longer stands as planned.
    fn reread(&self, planned: &[Standing]) -> Result<std::result::Result<Vec<Standing>, String>> {
        let mut fresh = Vec::with_capacity(planned.len());
        for member in planned {
            let id = member.record.id;
            let Some(record) = self.store.memory(id)? else {
                return Ok(Err(format!("memory {id} is gone")));
            };
            let same = record.is_current()
                && record.scope == member.record.scope
                && record.text == member.record.text
                && record.pinned == member.record.pinned;
            if !same {
                return Ok(Err(format!("memory {id} changed since the plan")));
            }
            let admission = self.admission(&record)?;
            fresh.push(Standing { record, admission });
        }
        debug_assert_eq!(fresh.len(), planned.len());
        Ok(Ok(fresh))
    }

    /// The survivor with the cluster merged in, and the one commit that says so.
    fn merged_commit(
        &self,
        cluster: &[Standing],
        by: &Actor,
    ) -> Result<(MemoryRecord, Commit, bool)> {
        assert!(cluster.len() >= 2, "a cluster has a survivor and a member");
        let (survivor, mut admission) = merged(cluster);
        let members: Vec<MemoryId> = cluster[1..].iter().map(|s| s.record.id).collect();
        let mut commit = Commit::new();
        for member in &cluster[1..] {
            debug_assert!(member.record.superseded_by.is_none());
            let mut retired = member.record.clone();
            retired.superseded_by = Some(survivor.id);
            commit.push_mutation(Mutation::PutMemory(retired));
            commit.push_event(EventDraft::new(
                by.clone(),
                EventKind::MemorySuperseded {
                    old: member.record.id,
                    new: survivor.id,
                    reason: Some("consolidated: a near-duplicate of the survivor".into()),
                },
            ));
        }
        let promoted = tiers::promotable(&admission);
        if promoted {
            commit.push_event(promotion(&mut admission, Verification::Observed, by));
            let replacement = self.promoted_replacement(&survivor, by)?;
            let in_cluster =
                |m: &Mutation| matches!(m, Mutation::PutMemory(r) if members.contains(&r.id));
            if !replacement.mutations.iter().any(in_cluster) {
                commit.events.extend(replacement.events);
                commit.mutations.extend(replacement.mutations);
            }
        }
        commit.push_event(consolidated_event(&survivor, &admission, &members, by));
        debug_assert!(survivor.is_current());
        commit.push_mutation(Mutation::PutMemory(survivor.clone()));
        commit.push_mutation(Mutation::PutMemoryAdmission(admission));
        Ok((survivor, commit, promoted))
    }
}

/// The survivor's record and admission with every member's merged in.
fn merged(cluster: &[Standing]) -> (MemoryRecord, MemoryAdmission) {
    let mut record = cluster[0].record.clone();
    let mut admission = cluster[0].admission.clone();
    debug_assert_eq!(admission.memory, record.id);
    for member in &cluster[1..] {
        let (r, a) = (&member.record, &member.admission);
        record.importance = record.importance.max(r.importance);
        record.confidence = record.confidence.max(r.confidence);
        record.retrieval_count = record.retrieval_count.saturating_add(r.retrieval_count);
        record.last_retrieved_at = record.last_retrieved_at.max(r.last_retrieved_at);
        for tag in &r.tags {
            if record.tags.len() < MAX_TAGS && !record.tags.contains(tag) {
                record.tags.push(tag.clone());
            }
        }
        a.support.iter().for_each(|s| {
            admission.support(*s);
        });
        admission.verification = admission.verification.max(a.verification);
        admission.refs.extend(a.refs.iter().cloned());
        if a.state == MemoryState::Active && admission.state == MemoryState::Candidate {
            admission.state = MemoryState::Active;
            admission.promoted_at = a.promoted_at.or(Some(aether_core::time::now()));
        }
    }
    admission.refs.sort();
    admission.refs.dedup();
    debug_assert!(admission.refs.windows(2).all(|w| w[0] < w[1]));
    (record, admission)
}

fn consolidated_event(
    survivor: &MemoryRecord,
    admission: &MemoryAdmission,
    members: &[MemoryId],
    by: &Actor,
) -> EventDraft {
    EventDraft::new(
        by.clone(),
        EventKind::Custom {
            name: "memory.consolidated".into(),
            payload: serde_json::json!({
                "memory": survivor.id, "members": members, "scope": survivor.scope,
                "tasks": admission.tasks(), "independent": admission.independent(),
            }),
        },
    )
}

fn cluster_report(cluster: &[Standing], outcome: ClusterOutcome) -> ClusterReport {
    ClusterReport {
        scope: cluster[0].record.scope.clone(),
        survivor: cluster[0].record.id,
        members: cluster[1..].iter().map(|s| s.record.id).collect(),
        outcome,
    }
}
