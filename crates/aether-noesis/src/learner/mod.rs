//! The learner model (NOESIS.md §5, migration N2): what the evidence says one
//! person knows. It is a store of evidence about the learner, kept apart from
//! the proposition graph, which is a store of claims about the world. The two
//! share only proposition ids, stored as plain values, and neither writes the
//! other.
//!
//! - `evidence` is append-only: [`append`] and [`correct`] add rows, and
//!   nothing here updates or deletes one.
//! - `concept_state` is a cache that only [`rebuild`] writes, from [`derive`]:
//!   a pure function of a concept's evidence and the time it is asked about.

use aether_core::noesis::{
    ConceptId, ConceptStateKind, EvidenceKind, EvidenceOutcome, MissClass, ProblemMode,
};
use aether_core::time::Timestamp;

mod derive;
mod drafts;
mod rows;
mod store;

pub use derive::{
    Derived, INITIAL_HALF_LIFE_DAYS, MASTERY_STRENGTH, MAX_HALF_LIFE_DAYS, MIN_HALF_LIFE_DAYS,
    brier, derive,
};
pub use drafts::{append_drafts, append_pending_judgments};
pub use store::{
    DEFAULT_EVIDENCE_LIMIT, DEFAULT_STATES_LIMIT, EvidencePage, Selection, append, concept,
    correct, ensure_concept, evidence_page, learner_calibration, rebuild, rebuild_all,
    relink_concept, states,
};

#[derive(Debug, thiserror::Error)]
pub enum LearnerError {
    #[error("noesis.db: {0}")]
    Sqlite(#[from] rusqlite::Error),
    #[error("unknown concept `{0}`")]
    UnknownConcept(ConceptId),
    #[error("invalid learner evidence: {0}")]
    Invalid(String),
    #[error("the learner tables are corrupt: {0}")]
    Corrupt(String),
}

/// One stored evidence row: the wire [`aether_core::noesis::Evidence`] plus
/// the state an operator correction sets. "Don't know" is its outcome.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EvidenceRecord {
    /// Assigned in append order, so it orders a concept's history.
    pub id: u64,
    pub concept: ConceptId,
    pub session: Option<String>,
    pub at: Timestamp,
    pub kind: EvidenceKind,
    pub outcome: EvidenceOutcome,
    pub confidence: Option<u8>,
    pub mode: Option<ProblemMode>,
    pub hint_rung: Option<u8>,
    pub miss_class: Option<MissClass>,
    pub note: Option<String>,
    /// Set exactly on `operator_correction` rows.
    pub corrected_state: Option<ConceptStateKind>,
}

/// Evidence to append, before the store gives it an id and a time.
/// Corrections go through [`correct`], never through [`append`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NewEvidence {
    pub concept: ConceptId,
    pub session: Option<String>,
    pub kind: EvidenceKind,
    pub outcome: EvidenceOutcome,
    pub confidence: Option<u8>,
    pub mode: Option<ProblemMode>,
    pub hint_rung: Option<u8>,
    pub miss_class: Option<MissClass>,
    pub note: Option<String>,
}

#[cfg(test)]
mod tests;
