//! What admission knows about a memory beyond its text: the tier it belongs
//! to, whether it is still a candidate, which tasks support it, how it was
//! verified and what it refers to. Kept by the store beside the record; on
//! the wire as `MemoryRecord.standing` (`crate::memory_v2`).

use std::collections::BTreeSet;

use serde::{Deserialize, Serialize};

use crate::event::Actor;
use crate::id::{MemoryId, RunId, TaskId};
use crate::memory::{MemoryKind, MemoryProvenance, MemoryRecord, MemoryScope};
use crate::time::Timestamp;

/// How many supporting tasks a record keeps. Promotion needs two, so the cap
/// only bounds the body of a memory many tasks keep restating.
pub const MAX_SUPPORTERS: usize = 32;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum MemoryTier {
    Working,
    Operational,
    Architecture,
    History,
    Evidence,
}

impl MemoryTier {
    /// The tier a memory of `kind` is admitted to. Decisions and constraints
    /// shape the system; the rest is how to work in it.
    pub fn for_kind(kind: MemoryKind) -> Self {
        match kind {
            MemoryKind::Decision | MemoryKind::Constraint => Self::Architecture,
            MemoryKind::Fact | MemoryKind::Lesson | MemoryKind::Preference => Self::Operational,
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Self::Working => "working",
            Self::Operational => "operational",
            Self::Architecture => "architecture",
            Self::History => "history",
            Self::Evidence => "evidence",
        }
    }
}

/// Where a memory stands. The store keeps only `Candidate` or `Active`:
/// superseded and retracted are read from the record, which already says so.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum MemoryState {
    Candidate,
    Active,
    Superseded,
    Retracted,
}

impl MemoryState {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Candidate => "candidate",
            Self::Active => "active",
            Self::Superseded => "superseded",
            Self::Retracted => "retracted",
        }
    }

    /// `record`'s state, given the admission state the store keeps for it.
    /// A pending operator proposal is a candidate whatever was stored.
    pub fn of(record: &MemoryRecord, admitted: MemoryState) -> Self {
        debug_assert!(matches!(admitted, Self::Candidate | Self::Active));
        if record.superseded_by.is_some() {
            return Self::Superseded;
        }
        if record.forgotten_at.is_some() {
            return Self::Retracted;
        }
        if record.is_proposed() {
            return Self::Candidate;
        }
        admitted
    }
}

/// Ordered weakest first, so the stronger of two is their maximum.
#[derive(
    Debug,
    Clone,
    Copy,
    PartialEq,
    Eq,
    PartialOrd,
    Ord,
    Hash,
    Serialize,
    Deserialize,
    schemars::JsonSchema,
)]
#[serde(rename_all = "snake_case")]
pub enum Verification {
    /// An agent's claim.
    Inferred,
    /// Read from a committed file, or restated by independent tasks.
    Observed,
    /// Stated or accepted by an operator.
    Verified,
}

impl Verification {
    pub fn of(provenance: &MemoryProvenance) -> Self {
        if matches!(provenance.actor, Some(Actor::User { .. })) {
            return Self::Verified;
        }
        if provenance.source.is_some() {
            return Self::Observed;
        }
        Self::Inferred
    }
}

#[derive(
    Debug,
    Clone,
    Copy,
    PartialEq,
    Eq,
    PartialOrd,
    Ord,
    Hash,
    Serialize,
    Deserialize,
    schemars::JsonSchema,
)]
#[serde(rename_all = "snake_case")]
pub enum RefKind {
    Repo,
    Path,
    Symbol,
    Commit,
    Issue,
    Pr,
    Test,
    Review,
    Doc,
}

#[derive(
    Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize, schemars::JsonSchema,
)]
pub struct MemoryRef {
    pub kind: RefKind,
    pub value: String,
}

impl MemoryRef {
    /// The references a record's scope and provenance already state, sorted
    /// and without repeats.
    pub fn of(scope: &MemoryScope, provenance: &MemoryProvenance) -> Vec<Self> {
        let mut refs = BTreeSet::new();
        if let MemoryScope::Repository { id } | MemoryScope::RepositoryRole { id, .. } = scope {
            refs.insert(Self::new(RefKind::Repo, id));
        }
        if let Some(source) = &provenance.source {
            refs.insert(Self::new(RefKind::Path, &source.file));
            if let Some(commit) = &source.commit {
                refs.insert(Self::new(RefKind::Commit, commit));
            }
        }
        refs.into_iter().collect()
    }

    fn new(kind: RefKind, value: &str) -> Self {
        Self {
            kind,
            value: value.to_owned(),
        }
    }
}

/// A task that stated a memory, and the run it belonged to. Tasks of one run
/// share their context, so they count as one independent source.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct Supporter {
    pub task: TaskId,
    pub run: RunId,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MemoryAdmission {
    pub memory: MemoryId,
    pub tier: MemoryTier,
    /// `Candidate` or `Active`; see [`MemoryState::of`] for the rest.
    pub state: MemoryState,
    /// Distinct tasks, oldest first, at most [`MAX_SUPPORTERS`].
    #[serde(default)]
    pub support: Vec<Supporter>,
    pub verification: Verification,
    #[serde(default)]
    pub refs: Vec<MemoryRef>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub promoted_at: Option<Timestamp>,
}

impl MemoryAdmission {
    /// What a record stands at with `state` and no support yet: tier,
    /// verification and refs follow from the record itself.
    pub fn derived(record: &MemoryRecord, state: MemoryState) -> Self {
        assert!(matches!(
            state,
            MemoryState::Candidate | MemoryState::Active
        ));
        let admission = Self {
            memory: record.id,
            tier: MemoryTier::for_kind(record.kind),
            state,
            support: Vec::new(),
            verification: Verification::of(&record.provenance),
            refs: MemoryRef::of(&record.scope, &record.provenance),
            promoted_at: None,
        };
        debug_assert!(admission.refs.windows(2).all(|w| w[0] < w[1]));
        admission
    }

    /// Distinct tasks that stated this memory.
    pub fn tasks(&self) -> u32 {
        self.support.len() as u32
    }

    /// Distinct runs among them: the independent sources.
    pub fn independent(&self) -> u32 {
        let runs: BTreeSet<_> = self.support.iter().map(|s| s.run).collect();
        runs.len() as u32
    }

    /// Count `supporter` once per task. False when its task already counted,
    /// or the list is full.
    pub fn support(&mut self, supporter: Supporter) -> bool {
        if self.support.len() >= MAX_SUPPORTERS
            || self.support.iter().any(|s| s.task == supporter.task)
        {
            return false;
        }
        self.support.push(supporter);
        debug_assert!(self.support.len() <= MAX_SUPPORTERS);
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::memory::{MemoryCandidate, MemorySource};
    use crate::time::now;

    fn record(kind: MemoryKind, provenance: MemoryProvenance) -> MemoryRecord {
        MemoryRecord::from_candidate(
            MemoryCandidate {
                scope: MemoryScope::Repository {
                    id: "aether".into(),
                },
                kind,
                text: "the store keeps admission beside the record".into(),
                structured: None,
                importance: 0.5,
                confidence: 0.5,
                tags: Vec::new(),
                provenance,
                supersedes: None,
            },
            now(),
        )
    }

    #[test]
    fn support_counts_a_task_once_and_a_run_once() {
        let mut admission = MemoryAdmission::derived(
            &record(MemoryKind::Fact, Default::default()),
            MemoryState::Candidate,
        );
        let run = RunId::new();
        let first = Supporter {
            task: TaskId::new(),
            run,
        };
        assert!(admission.support(first));
        assert!(!admission.support(first), "the same task twice");
        assert!(admission.support(Supporter {
            task: TaskId::new(),
            run
        }));
        assert_eq!((admission.tasks(), admission.independent()), (2, 1));
        assert!(admission.support(Supporter {
            task: TaskId::new(),
            run: RunId::new()
        }));
        assert_eq!((admission.tasks(), admission.independent()), (3, 2));
        while admission.support(Supporter {
            task: TaskId::new(),
            run: RunId::new(),
        }) {}
        assert_eq!(admission.support.len(), MAX_SUPPORTERS);
    }

    #[test]
    fn derivation_reads_tier_verification_and_refs_from_the_record() {
        let agent = MemoryAdmission::derived(
            &record(MemoryKind::Lesson, Default::default()),
            MemoryState::Active,
        );
        assert_eq!(agent.tier, MemoryTier::Operational);
        assert_eq!(agent.verification, Verification::Inferred);
        assert_eq!(agent.refs, vec![MemoryRef::new(RefKind::Repo, "aether")]);

        let noted = record(
            MemoryKind::Constraint,
            MemoryProvenance {
                source: Some(MemorySource {
                    file: "AGENTS.md".into(),
                    commit: Some("abc123".into()),
                }),
                ..Default::default()
            },
        );
        let noted = MemoryAdmission::derived(&noted, MemoryState::Active);
        assert_eq!(noted.tier, MemoryTier::Architecture);
        assert_eq!(noted.verification, Verification::Observed);
        assert_eq!(
            noted.refs.iter().map(|r| r.kind).collect::<Vec<_>>(),
            [RefKind::Repo, RefKind::Path, RefKind::Commit]
        );

        let stated = record(
            MemoryKind::Fact,
            MemoryProvenance {
                actor: Some(Actor::user()),
                ..Default::default()
            },
        );
        assert_eq!(
            MemoryAdmission::derived(&stated, MemoryState::Active).verification,
            Verification::Verified
        );
    }

    #[test]
    fn the_record_decides_superseded_retracted_and_pending_review() {
        let mut memory = record(MemoryKind::Fact, Default::default());
        assert_eq!(
            MemoryState::of(&memory, MemoryState::Candidate),
            MemoryState::Candidate
        );
        assert_eq!(
            MemoryState::of(&memory, MemoryState::Active),
            MemoryState::Active
        );
        memory.proposed_at = Some(now());
        assert_eq!(
            MemoryState::of(&memory, MemoryState::Active),
            MemoryState::Candidate
        );
        memory.forgotten_at = Some(now());
        assert_eq!(
            MemoryState::of(&memory, MemoryState::Active),
            MemoryState::Retracted
        );
        memory.superseded_by = Some(MemoryId::new());
        assert_eq!(
            MemoryState::of(&memory, MemoryState::Active),
            MemoryState::Superseded
        );
    }
}
