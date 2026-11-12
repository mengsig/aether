//! Memory 2.0's host commands (docs/memory/MEMORY-V2.md): a record's
//! standing, the retrieval explain, consolidation on the wire, export and
//! import, and the migration status.

mod explain;
mod status;
mod transfer;

use aether_core::event::Actor;
use aether_core::id::MemoryId;
use aether_core::memory::{MemoryRecord, MemoryScope};
use aether_core::memory_admission::{MemoryAdmission, MemoryState};
use aether_core::memory_v2::{MemoryStanding, StaleNote};
use aether_core::memory_v2_reports::{
    ClusterOutcomeKind, ConsolidatedCluster, MemoryConsolidation,
};
use aether_memory::ClusterOutcome;
use aether_store::MemoryStaleMark;

use crate::{Result, Runtime, RuntimeError};

impl Runtime {
    /// Fill each record's `standing` from its admission row and stale mark,
    /// in two reads. A record with no row stands as G1 derives it: active.
    pub fn with_standing(&self, records: &mut [MemoryRecord]) -> Result<()> {
        if records.is_empty() {
            return Ok(());
        }
        let ids: Vec<MemoryId> = records.iter().map(|record| record.id).collect();
        let mut admissions = self.store.memory_admissions(&ids)?;
        let mut stale = self.store.memory_stale_marks(&ids)?;
        debug_assert!(admissions.len() <= ids.len() && stale.len() <= ids.len());
        for record in records.iter_mut() {
            let admission = admissions
                .remove(&record.id)
                .unwrap_or_else(|| MemoryAdmission::derived(record, MemoryState::Active));
            let note = stale.remove(&record.id).map(stale_note);
            record.standing = Some(MemoryStanding::of(record, &admission, note));
        }
        debug_assert!(records.iter().all(|record| record.standing.is_some()));
        Ok(())
    }

    /// `consolidate_memories` as the wire reports it.
    pub fn consolidate_memories_report(
        &self,
        scope: Option<MemoryScope>,
        dry_run: bool,
        by: Actor,
    ) -> Result<MemoryConsolidation> {
        require_operator(&by, "consolidate memories")?;
        let report = self.consolidate_memories(scope, dry_run, by)?;
        let clusters: Vec<ConsolidatedCluster> = report
            .clusters
            .into_iter()
            .map(|cluster| {
                debug_assert!(!cluster.members.contains(&cluster.survivor));
                let (outcome, promoted, reason) = match cluster.outcome {
                    ClusterOutcome::Planned => (ClusterOutcomeKind::Planned, false, None),
                    ClusterOutcome::Merged { promoted } => {
                        (ClusterOutcomeKind::Merged, promoted, None)
                    }
                    ClusterOutcome::Skipped { reason } => {
                        (ClusterOutcomeKind::Skipped, false, Some(reason))
                    }
                };
                ConsolidatedCluster {
                    scope: cluster.scope,
                    survivor: cluster.survivor,
                    members: cluster.members,
                    outcome,
                    promoted,
                    reason,
                }
            })
            .collect();
        debug_assert!(
            !dry_run
                || clusters
                    .iter()
                    .all(|c| c.outcome == ClusterOutcomeKind::Planned)
        );
        Ok(MemoryConsolidation {
            dry_run: report.dry_run,
            scanned: count(report.scanned),
            clusters,
        })
    }
}

pub(crate) fn stale_note(mark: MemoryStaleMark) -> StaleNote {
    StaleNote {
        reason: mark.reason,
        since: mark.since,
        revision: mark.revision,
    }
}

/// The host commands act on every scope; an agent's own actor never reaches
/// them past access control, and this keeps the runtime honest without it.
fn require_operator(by: &Actor, what: &str) -> Result<()> {
    if !matches!(by, Actor::User { .. }) {
        return Err(RuntimeError::Invalid(format!(
            "only an operator can {what}"
        )));
    }
    Ok(())
}

/// A count on the wire, which carries `u32`.
fn count(n: usize) -> u32 {
    u32::try_from(n).unwrap_or(u32::MAX)
}
