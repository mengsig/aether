//! Learner rows to and from their `noesis.db` text, and evidence to its wire shape.

use aether_core::noesis::{ConceptKind, ConceptRef, Evidence, EvidenceKind};
use aether_core::time::Timestamp;
use rusqlite::Row;
use serde::Serialize;
use serde::de::DeserializeOwned;

use super::{EvidenceRecord, LearnerError};

pub(super) const CONCEPT_COLUMNS: &str = "id, kind, ref, title";
pub(super) const EVIDENCE_COLUMNS: &str = "id, concept, session, at, kind, outcome, confidence, \
     mode, hint_rung, miss_class, note, corrected_state";

/// The snake_case name a wire enum serializes to.
pub(super) fn name_of<T: Serialize + std::fmt::Debug>(value: &T) -> Result<String, LearnerError> {
    match serde_json::to_value(value) {
        Ok(serde_json::Value::String(name)) => Ok(name),
        other => Err(LearnerError::Corrupt(format!(
            "{value:?} is not named by a string: {other:?}"
        ))),
    }
}

pub(super) fn parse_name<T: DeserializeOwned>(text: &str, column: &str) -> Result<T, LearnerError> {
    serde_json::from_value(serde_json::Value::String(text.to_string()))
        .map_err(|_| LearnerError::Corrupt(format!("{column} holds `{text}`")))
}

fn parse_optional_name<T: DeserializeOwned>(
    text: Option<String>,
    column: &str,
) -> Result<Option<T>, LearnerError> {
    text.map(|text| parse_name(&text, column)).transpose()
}

/// Stored to the millisecond, as the rest of `noesis.db` is.
pub(super) fn time_text(at: Timestamp) -> String {
    at.to_rfc3339_opts(chrono::SecondsFormat::Millis, true)
}

pub(super) fn parse_time(text: &str, column: &str) -> Result<Timestamp, LearnerError> {
    chrono::DateTime::parse_from_rfc3339(text)
        .map(|at| at.with_timezone(&chrono::Utc))
        .map_err(|_| LearnerError::Corrupt(format!("{column} holds `{text}`, not a time")))
}

/// A `concepts` row read with [`CONCEPT_COLUMNS`].
pub(super) struct RawConcept {
    id: String,
    kind: String,
    reference: String,
    title: String,
}

impl RawConcept {
    pub(super) fn read(row: &Row<'_>) -> rusqlite::Result<Self> {
        Ok(Self {
            id: row.get(0)?,
            kind: row.get(1)?,
            reference: row.get(2)?,
            title: row.get(3)?,
        })
    }

    pub(super) fn decode(self) -> Result<ConceptRef, LearnerError> {
        let kind: ConceptKind = parse_name(&self.kind, "concepts.kind")?;
        Ok(ConceptRef {
            id: self.id,
            kind,
            reference: self.reference,
            title: self.title,
        })
    }
}

/// An `evidence` row read with [`EVIDENCE_COLUMNS`].
pub(super) struct RawEvidence {
    id: i64,
    concept: String,
    session: Option<String>,
    at: String,
    kind: String,
    outcome: String,
    confidence: Option<i64>,
    mode: Option<String>,
    hint_rung: Option<i64>,
    miss_class: Option<String>,
    note: Option<String>,
    corrected_state: Option<String>,
}

impl RawEvidence {
    pub(super) fn read(row: &Row<'_>) -> rusqlite::Result<Self> {
        Ok(Self {
            id: row.get(0)?,
            concept: row.get(1)?,
            session: row.get(2)?,
            at: row.get(3)?,
            kind: row.get(4)?,
            outcome: row.get(5)?,
            confidence: row.get(6)?,
            mode: row.get(7)?,
            hint_rung: row.get(8)?,
            miss_class: row.get(9)?,
            note: row.get(10)?,
            corrected_state: row.get(11)?,
        })
    }

    pub(super) fn decode(self) -> Result<EvidenceRecord, LearnerError> {
        let small = |value: Option<i64>, column: &str| -> Result<Option<u8>, LearnerError> {
            value
                .map(|v| {
                    u8::try_from(v).map_err(|_| LearnerError::Corrupt(format!("{column} {v}")))
                })
                .transpose()
        };
        let id = u64::try_from(self.id)
            .ok()
            .filter(|id| *id > 0)
            .ok_or_else(|| LearnerError::Corrupt(format!("evidence id {}", self.id)))?;
        Ok(EvidenceRecord {
            id,
            concept: self.concept,
            session: self.session,
            at: parse_time(&self.at, "evidence.at")?,
            kind: parse_name(&self.kind, "evidence.kind")?,
            outcome: parse_name(&self.outcome, "evidence.outcome")?,
            confidence: small(self.confidence, "evidence.confidence")?,
            mode: parse_optional_name(self.mode, "evidence.mode")?,
            hint_rung: small(self.hint_rung, "evidence.hint_rung")?,
            miss_class: parse_optional_name(self.miss_class, "evidence.miss_class")?,
            note: self.note,
            corrected_state: parse_optional_name(self.corrected_state, "evidence.corrected_state")?,
        })
    }
}

/// The wire shape has no field for a correction's state, so its note leads
/// with it: `fragile: <the operator's reason>`.
pub(super) fn to_wire(record: EvidenceRecord) -> Result<Evidence, LearnerError> {
    debug_assert_eq!(
        record.kind == EvidenceKind::OperatorCorrection,
        record.corrected_state.is_some(),
        "exactly corrections carry a corrected state"
    );
    let note = match record.corrected_state {
        Some(state) => Some(format!(
            "{}: {}",
            name_of(&state)?,
            record.note.as_deref().unwrap_or_default()
        )),
        None => record.note,
    };
    Ok(Evidence {
        id: record.id,
        concept: record.concept,
        session: record.session,
        at: record.at,
        kind: record.kind,
        outcome: record.outcome,
        confidence: record.confidence,
        mode: record.mode,
        hint_rung: record.hint_rung,
        miss_class: record.miss_class,
        note,
    })
}
