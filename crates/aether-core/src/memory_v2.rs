//! Memory 2.0 on the wire (docs/memory/MEMORY-V2.md): a record's standing,
//! and the trace of what retrieval did for one turn.

use serde::{Deserialize, Serialize};

use crate::id::MemoryId;
use crate::memory::MemoryRecord;
use crate::memory_admission::{MemoryAdmission, MemoryRef, MemoryState, MemoryTier, Verification};
use crate::time::Timestamp;

/// What admission and retrieval know about a record, sent beside it to a
/// client that lists `memory_v2`. Never part of the stored body.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
pub struct MemoryStanding {
    pub tier: MemoryTier,
    pub state: MemoryState,
    pub support: MemorySupport,
    pub verification: Verification,
    #[serde(default)]
    pub refs: Vec<MemoryRef>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub stale: Option<StaleNote>,
}

/// Distinct supporting tasks, and their distinct runs: the independent ones.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
pub struct MemorySupport {
    pub tasks: u32,
    pub independent: u32,
}

/// Why retrieval demotes a memory as possibly stale, and since when.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
pub struct StaleNote {
    pub reason: String,
    pub since: Timestamp,
    /// The commit whose tree stopped stating it, when a sweep set the mark.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub revision: Option<String>,
}

impl MemoryStanding {
    /// `record`'s standing from its stored admission and stale mark.
    pub fn of(
        record: &MemoryRecord,
        admission: &MemoryAdmission,
        stale: Option<StaleNote>,
    ) -> Self {
        assert_eq!(admission.memory, record.id, "an admission names its record");
        debug_assert!(admission.independent() <= admission.tasks());
        Self {
            tier: admission.tier,
            state: MemoryState::of(record, admission.state),
            support: MemorySupport {
                tasks: admission.tasks(),
                independent: admission.independent(),
            },
            verification: admission.verification,
            refs: admission.refs.clone(),
            stale,
        }
    }
}

/// Most entries past `max_items` a trace keeps of each of the two tails.
pub const TRACE_TAIL: usize = 16;

/// What retrieval did for one turn, recorded with the snapshot it built.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, schemars::JsonSchema)]
pub struct RetrievalTrace {
    pub policy: TracePolicy,
    /// Candidates considered, before the cutoff.
    pub pool: u32,
    /// Ranked first, then the cut ones by relevance.
    pub candidates: Vec<TraceCandidate>,
    #[serde(default)]
    pub omitted: TraceOmitted,
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize, schemars::JsonSchema)]
pub struct TracePolicy {
    pub max_items: u32,
    /// The cutoff applied; absent when the request had no query.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub min_score: Option<f32>,
    pub holdout_rate: f32,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, schemars::JsonSchema)]
pub struct TraceCandidate {
    pub memory: MemoryId,
    pub outcome: TraceOutcome,
    /// 1-based rank among the memories that passed the cutoff.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub rank: Option<u32>,
    /// The final score; a cut candidate is never scored.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub score: Option<f32>,
    pub relevance: f32,
    pub stale: bool,
    pub pinned: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum TraceOutcome {
    Injected,
    HeldOut,
    /// Within `max_items`, but the model's memory budget had no room.
    Trimmed,
    /// Passed the cutoff but ranked past `max_items`.
    OverLimit,
    /// Below the minimum relevance.
    Cut,
}

/// Candidates past the trace's caps, by the outcome they had.
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize, schemars::JsonSchema,
)]
pub struct TraceOmitted {
    pub over_limit: u32,
    pub cut: u32,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::memory::{MemoryCandidate, MemoryKind, MemoryScope};
    use crate::memory_admission::Supporter;
    use crate::time::now;

    #[test]
    fn standing_reads_the_record_for_superseded_and_counts_runs_once() {
        let mut record = MemoryRecord::from_candidate(
            MemoryCandidate {
                scope: MemoryScope::Global,
                kind: MemoryKind::Decision,
                text: "the store is the single writer".into(),
                structured: None,
                importance: 0.5,
                confidence: 0.5,
                tags: Vec::new(),
                provenance: Default::default(),
                supersedes: None,
            },
            now(),
        );
        let mut admission = MemoryAdmission::derived(&record, MemoryState::Candidate);
        let run = crate::id::RunId::new();
        for _ in 0..2 {
            admission.support(Supporter {
                task: crate::id::TaskId::new(),
                run,
            });
        }
        let standing = MemoryStanding::of(&record, &admission, None);
        assert_eq!(standing.tier, MemoryTier::Architecture);
        assert_eq!(standing.state, MemoryState::Candidate);
        assert_eq!(
            standing.support,
            MemorySupport {
                tasks: 2,
                independent: 1
            }
        );
        record.superseded_by = Some(MemoryId::new());
        assert_eq!(
            MemoryStanding::of(&record, &admission, None).state,
            MemoryState::Superseded
        );
    }
}
