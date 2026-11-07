use aether_core::noesis::{
    ConceptStateKind, EvidenceKind, EvidenceOutcome, MissClass, ProblemMode,
};
use aether_core::time::Timestamp;
use chrono::{Duration, TimeZone, Utc};

use super::{EvidenceRecord, NewEvidence};

mod derive_rules;
mod schema;
mod store_rules;

/// Noon UTC on day `day` of November 2026, plus `hours`.
fn day(day: u32, hours: i64) -> Timestamp {
    Utc.with_ymd_and_hms(2026, 11, day, 12, 0, 0).unwrap() + Duration::hours(hours)
}

/// A retrieval or other evidence of `kind` with `outcome`, for derivation.
fn record(kind: EvidenceKind, outcome: EvidenceOutcome, at: Timestamp) -> EvidenceRecord {
    EvidenceRecord {
        id: 0,
        concept: "k_1".into(),
        session: Some("ns_1".into()),
        at,
        kind,
        outcome,
        confidence: None,
        mode: None,
        hint_rung: None,
        miss_class: None,
        note: None,
        corrected_state: None,
    }
}

fn check(outcome: EvidenceOutcome, confidence: Option<u8>, at: Timestamp) -> EvidenceRecord {
    EvidenceRecord {
        confidence,
        ..record(EvidenceKind::CheckAnswer, outcome, at)
    }
}

fn exposure(at: Timestamp) -> EvidenceRecord {
    record(EvidenceKind::Exposure, EvidenceOutcome::Exposed, at)
}

fn practice(solved: bool, mode: ProblemMode, rung: u8, at: Timestamp) -> EvidenceRecord {
    let outcome = if solved {
        EvidenceOutcome::Correct
    } else {
        EvidenceOutcome::Incorrect
    };
    EvidenceRecord {
        mode: Some(mode),
        hint_rung: Some(rung),
        miss_class: (!solved).then_some(MissClass::Discrimination),
        ..record(EvidenceKind::PracticeConclude, outcome, at)
    }
}

fn observed(misconception: &str, at: Timestamp) -> EvidenceRecord {
    EvidenceRecord {
        note: Some(misconception.into()),
        ..record(
            EvidenceKind::MisconceptionObserved,
            EvidenceOutcome::Observed,
            at,
        )
    }
}

fn correction(state: ConceptStateKind, at: Timestamp) -> EvidenceRecord {
    EvidenceRecord {
        session: None,
        note: Some("the operator knows better".into()),
        corrected_state: Some(state),
        ..record(
            EvidenceKind::OperatorCorrection,
            EvidenceOutcome::Corrected,
            at,
        )
    }
}

/// Number `records` 1, 2, ... in the order given.
fn numbered(records: Vec<EvidenceRecord>) -> Vec<EvidenceRecord> {
    records
        .into_iter()
        .zip(1..)
        .map(|(record, id)| EvidenceRecord { id, ..record })
        .collect()
}

/// The same evidence as an append, for the store.
fn new_evidence(record: &EvidenceRecord, concept: &str) -> NewEvidence {
    NewEvidence {
        concept: concept.into(),
        session: record.session.clone(),
        kind: record.kind,
        outcome: record.outcome,
        confidence: record.confidence,
        mode: record.mode,
        hint_rung: record.hint_rung,
        miss_class: record.miss_class,
        note: record.note.clone(),
    }
}
