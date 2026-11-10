//! The learner tables in `noesis.db`. Evidence is only ever inserted, and
//! `concept_state` is only ever written by [`rebuild_one`] from [`derive`].
//! No statement here names a proposition-graph table.

use std::collections::BTreeSet;

use aether_core::noesis::{
    CONFIDENCE_LEVELS, ConceptId, ConceptKind, ConceptRef, ConceptState, ConceptStateKind,
    Evidence, EvidenceKind, EvidenceOutcome, MAX_HINT_RUNG,
};
use aether_core::time::Timestamp;
use rusqlite::{Connection, OptionalExtension, params};

use super::derive::{brier, derive};
use super::rows::{
    CONCEPT_COLUMNS, EVIDENCE_COLUMNS, RawConcept, RawEvidence, name_of, time_text, to_wire,
};
use super::{EvidenceRecord, LearnerError, NewEvidence};

pub const DEFAULT_STATES_LIMIT: u32 = 1000;
pub const DEFAULT_EVIDENCE_LIMIT: u32 = 100;
const MAX_EVIDENCE_LIMIT: u32 = 500;
const MAX_REASON_CHARS: usize = 500;

/// Which concepts a [`states`] read covers: the named ones (all when
/// `None`), narrowed to titles or refs containing `query`, in creation order.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Selection {
    pub concepts: Option<Vec<ConceptId>>,
    pub query: Option<String>,
    pub limit: Option<u32>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EvidencePage {
    /// Newest first.
    pub evidence: Vec<Evidence>,
    /// The `before` cursor of the next page; `None` on the last one.
    pub next_before: Option<u64>,
}

pub fn concept(conn: &Connection, id: &str) -> Result<Option<ConceptRef>, LearnerError> {
    let sql = format!("SELECT {CONCEPT_COLUMNS} FROM concepts WHERE id = ?1");
    let raw = conn.query_row(&sql, [id], RawConcept::read).optional()?;
    raw.map(RawConcept::decode).transpose()
}

fn known_concept(conn: &Connection, id: &str) -> Result<ConceptRef, LearnerError> {
    concept(conn, id)?.ok_or_else(|| LearnerError::UnknownConcept(id.to_string()))
}

/// The concept standing for `reference` of `kind`, created on first use.
pub fn ensure_concept(
    conn: &Connection,
    kind: ConceptKind,
    reference: &str,
    title: &str,
    now: Timestamp,
) -> Result<ConceptRef, LearnerError> {
    let (reference, title) = (reference.trim(), title.trim());
    if reference.is_empty() || title.is_empty() {
        return Err(LearnerError::Invalid(
            "a concept has a ref and a title".into(),
        ));
    }
    let kind_name = name_of(&kind)?;
    conn.execute(
        "INSERT INTO concepts (id, kind, ref, title, created_at) VALUES (?1, ?2, ?3, ?4, ?5)
         ON CONFLICT (kind, ref) DO NOTHING",
        params![
            fresh_concept_id(),
            kind_name,
            reference,
            title,
            time_text(now)
        ],
    )?;
    let sql = format!("SELECT {CONCEPT_COLUMNS} FROM concepts WHERE kind = ?1 AND ref = ?2");
    let found = conn
        .query_row(&sql, params![kind_name, reference], RawConcept::read)?
        .decode()?;
    assert!(
        found.kind == kind && found.reference == reference,
        "the concept read back is the one asked for"
    );
    debug_assert!(found.id.starts_with("k_"));
    Ok(found)
}

fn fresh_concept_id() -> ConceptId {
    format!("k_{}", ulid::Ulid::generate())
}

/// Point a plan-node or topic concept at the proposition extraction found
/// for it. Its evidence stays with it: only the concept row changes.
pub fn relink_concept(
    conn: &Connection,
    id: &str,
    proposition: &str,
) -> Result<ConceptRef, LearnerError> {
    if !proposition.starts_with("p_") {
        return Err(LearnerError::Invalid(format!(
            "`{proposition}` is not a proposition id"
        )));
    }
    let current = known_concept(conn, id)?;
    let taken: Option<String> = conn
        .query_row(
            "SELECT id FROM concepts WHERE kind = 'proposition' AND ref = ?1",
            [proposition],
            |row| row.get(0),
        )
        .optional()?;
    if let Some(other) = taken.filter(|other| *other != current.id) {
        return Err(LearnerError::Invalid(format!(
            "proposition `{proposition}` already has concept `{other}`"
        )));
    }
    let changed = conn.execute(
        "UPDATE concepts SET kind = 'proposition', ref = ?2 WHERE id = ?1",
        params![id, proposition],
    )?;
    assert_eq!(changed, 1, "the concept was read just before");
    known_concept(conn, id)
}

/// Append evidence at `now` and rebuild the concepts it bears on, in one
/// transaction. Returns the new ids, in order.
pub fn append(
    conn: &Connection,
    items: &[NewEvidence],
    now: Timestamp,
) -> Result<Vec<u64>, LearnerError> {
    let tx = rusqlite::Transaction::new_unchecked(conn, rusqlite::TransactionBehavior::Immediate)?;
    let ids = append_within(&tx, items, now)?;
    tx.commit()?;
    Ok(ids)
}

/// [`append`] inside the caller's transaction.
pub(super) fn append_within(
    conn: &Connection,
    items: &[NewEvidence],
    now: Timestamp,
) -> Result<Vec<u64>, LearnerError> {
    debug_assert!(
        !conn.is_autocommit(),
        "evidence and its rebuild land together"
    );
    for item in items {
        validate(item)?;
    }
    let mut ids = Vec::with_capacity(items.len());
    let mut touched = BTreeSet::new();
    for item in items {
        known_concept(conn, &item.concept)?;
        ids.push(insert(conn, item, None, now)?);
        touched.insert(item.concept.clone());
    }
    for concept in &touched {
        rebuild_one(conn, &known_concept(conn, concept)?, now)?;
    }
    assert!(
        ids.windows(2).all(|pair| pair[0] < pair[1]),
        "ids grow in append order"
    );
    Ok(ids)
}

/// Kind, outcome, confidence and rung as the evidence vocabulary allows.
fn validate(item: &NewEvidence) -> Result<(), LearnerError> {
    let invalid = |why: String| Err(LearnerError::Invalid(why));
    let outcomes: &[EvidenceOutcome] = match item.kind {
        EvidenceKind::OperatorCorrection => {
            return invalid("an operator correction is appended by `correct`".into());
        }
        EvidenceKind::Exposure => &[EvidenceOutcome::Exposed],
        EvidenceKind::MisconceptionObserved => &[EvidenceOutcome::Observed],
        _ => &[
            EvidenceOutcome::Correct,
            EvidenceOutcome::Incorrect,
            EvidenceOutcome::DontKnow,
        ],
    };
    if !outcomes.contains(&item.outcome) {
        return invalid(format!(
            "{:?} evidence cannot be {:?}",
            item.kind, item.outcome
        ));
    }
    if let Some(confidence) = item.confidence {
        if !CONFIDENCE_LEVELS.contains(&confidence) {
            return invalid(format!(
                "confidence {confidence} is not one of {CONFIDENCE_LEVELS:?}"
            ));
        }
        if item.outcome == EvidenceOutcome::DontKnow {
            return invalid("\"don't know\" states no confidence".into());
        }
    }
    if item.hint_rung.is_some_and(|rung| rung > MAX_HINT_RUNG) {
        return invalid(format!("hint rung above R{MAX_HINT_RUNG}"));
    }
    if item.session.as_deref().is_some_and(|s| s.trim().is_empty()) {
        return invalid("an empty session id".into());
    }
    Ok(())
}

/// The one INSERT into `evidence`.
fn insert(
    conn: &Connection,
    item: &NewEvidence,
    corrected: Option<ConceptStateKind>,
    now: Timestamp,
) -> Result<u64, LearnerError> {
    conn.execute(
        "INSERT INTO evidence (concept, session, at, kind, outcome, confidence, dont_know, mode,
                               hint_rung, miss_class, note, corrected_state)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12)",
        params![
            item.concept,
            item.session,
            time_text(now),
            name_of(&item.kind)?,
            name_of(&item.outcome)?,
            item.confidence,
            item.outcome == EvidenceOutcome::DontKnow,
            item.mode.map(|mode| name_of(&mode)).transpose()?,
            item.hint_rung,
            item.miss_class.map(|class| name_of(&class)).transpose()?,
            item.note,
            corrected.map(|state| name_of(&state)).transpose()?,
        ],
    )?;
    let id = u64::try_from(conn.last_insert_rowid())
        .map_err(|_| LearnerError::Corrupt("a negative evidence id".into()))?;
    assert!(id > 0, "evidence ids start at 1");
    Ok(id)
}

/// The operator's correction: a new `operator_correction` row that wins until
/// newer evidence arrives. History is never edited.
pub fn correct(
    conn: &Connection,
    concept: &str,
    state: ConceptStateKind,
    reason: &str,
    now: Timestamp,
) -> Result<ConceptState, LearnerError> {
    let reason = reason.trim();
    let chars = reason.chars().count();
    if !(1..=MAX_REASON_CHARS).contains(&chars) {
        return Err(LearnerError::Invalid(format!(
            "a correction's reason is 1 to {MAX_REASON_CHARS} characters"
        )));
    }
    let tx = rusqlite::Transaction::new_unchecked(conn, rusqlite::TransactionBehavior::Immediate)?;
    let target = known_concept(&tx, concept)?;
    let correction = NewEvidence {
        concept: target.id.clone(),
        session: None,
        kind: EvidenceKind::OperatorCorrection,
        outcome: EvidenceOutcome::Corrected,
        confidence: None,
        mode: None,
        hint_rung: None,
        miss_class: None,
        note: Some(reason.to_string()),
    };
    let id = insert(&tx, &correction, Some(state), now)?;
    let corrected = rebuild_one(&tx, &target, now)?;
    tx.commit()?;
    assert_eq!(corrected.state, state, "the newest correction wins");
    assert_eq!(corrected.as_of_evidence, id, "derived up to the correction");
    Ok(corrected)
}

/// One concept's evidence, oldest first.
fn history(conn: &Connection, concept: &str) -> Result<Vec<EvidenceRecord>, LearnerError> {
    let sql = format!("SELECT {EVIDENCE_COLUMNS} FROM evidence WHERE concept = ?1 ORDER BY id");
    let mut statement = conn.prepare_cached(&sql)?;
    let raws = statement.query_map([concept], RawEvidence::read)?;
    let mut records = Vec::new();
    for raw in raws {
        records.push(raw?.decode()?);
    }
    debug_assert!(records.iter().all(|r| r.concept == concept));
    Ok(records)
}

/// Derive `concept` at `now` from its evidence and store the result: the
/// only writer of `concept_state`.
fn rebuild_one(
    conn: &Connection,
    concept: &ConceptRef,
    now: Timestamp,
) -> Result<ConceptState, LearnerError> {
    let derived = derive(&history(conn, &concept.id)?, now);
    let misconceptions = serde_json::to_string(&derived.misconceptions)
        .map_err(|e| LearnerError::Corrupt(format!("encode misconceptions: {e}")))?;
    let as_of = i64::try_from(derived.as_of_evidence)
        .map_err(|_| LearnerError::Corrupt("an evidence id beyond i64".into()))?;
    let written = conn.execute(
        "INSERT INTO concept_state (concept, state, strength, last_success_at, review_due_at,
                                    calibration, misconceptions, as_of_evidence, derived_at)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)
         ON CONFLICT (concept) DO UPDATE SET
             state = excluded.state, strength = excluded.strength,
             last_success_at = excluded.last_success_at, review_due_at = excluded.review_due_at,
             calibration = excluded.calibration, misconceptions = excluded.misconceptions,
             as_of_evidence = excluded.as_of_evidence, derived_at = excluded.derived_at",
        params![
            concept.id,
            name_of(&derived.state)?,
            derived.strength,
            derived.last_success_at.map(time_text),
            derived.review_due_at.map(time_text),
            derived.calibration,
            misconceptions,
            as_of,
            time_text(now),
        ],
    )?;
    assert_eq!(written, 1, "one concept_state row per concept");
    Ok(derived.into_state(concept.clone()))
}

/// Rebuild the named concepts at `now`, in one transaction.
pub fn rebuild(
    conn: &Connection,
    concepts: &[ConceptId],
    now: Timestamp,
) -> Result<Vec<ConceptState>, LearnerError> {
    let tx = rusqlite::Transaction::new_unchecked(conn, rusqlite::TransactionBehavior::Immediate)?;
    let mut rebuilt = Vec::with_capacity(concepts.len());
    for id in concepts {
        rebuilt.push(rebuild_one(&tx, &known_concept(&tx, id)?, now)?);
    }
    tx.commit()?;
    debug_assert_eq!(rebuilt.len(), concepts.len());
    Ok(rebuilt)
}

/// Rebuild every concept at `now`; returns how many.
pub fn rebuild_all(conn: &Connection, now: Timestamp) -> Result<usize, LearnerError> {
    let ids: Vec<ConceptId> = all_concepts(conn)?.into_iter().map(|c| c.id).collect();
    Ok(rebuild(conn, &ids, now)?.len())
}

fn all_concepts(conn: &Connection) -> Result<Vec<ConceptRef>, LearnerError> {
    let sql = format!("SELECT {CONCEPT_COLUMNS} FROM concepts ORDER BY rowid");
    let mut statement = conn.prepare_cached(&sql)?;
    let raws = statement.query_map([], RawConcept::read)?;
    let mut concepts = Vec::new();
    for raw in raws {
        concepts.push(raw?.decode()?);
    }
    Ok(concepts)
}

/// The selected concepts' states as of `now`, each rebuilt from its evidence
/// first so decay is current. Unknown ids are left out.
pub fn states(
    conn: &Connection,
    selection: &Selection,
    now: Timestamp,
) -> Result<Vec<ConceptState>, LearnerError> {
    let limit = selection.limit.unwrap_or(DEFAULT_STATES_LIMIT);
    if limit == 0 {
        return Err(LearnerError::Invalid("limit is at least 1".into()));
    }
    let wanted: Option<BTreeSet<&str>> = selection
        .concepts
        .as_ref()
        .map(|ids| ids.iter().map(String::as_str).collect());
    let query = selection.query.as_deref().map(str::to_lowercase);
    let chosen: Vec<ConceptId> = all_concepts(conn)?
        .into_iter()
        .filter(|c| {
            wanted
                .as_ref()
                .is_none_or(|ids| ids.contains(c.id.as_str()))
        })
        .filter(|c| query.as_deref().is_none_or(|q| matches_query(c, q)))
        .take(limit as usize)
        .map(|c| c.id)
        .collect();
    let read = rebuild(conn, &chosen, now)?;
    assert!(read.len() <= limit as usize);
    Ok(read)
}

fn matches_query(concept: &ConceptRef, lowercase_query: &str) -> bool {
    concept.title.to_lowercase().contains(lowercase_query)
        || concept.reference.to_lowercase().contains(lowercase_query)
}

/// A concept's evidence, newest first, from before the id `before`.
pub fn evidence_page(
    conn: &Connection,
    concept: &str,
    before: Option<u64>,
    limit: Option<u32>,
) -> Result<EvidencePage, LearnerError> {
    let limit = limit.unwrap_or(DEFAULT_EVIDENCE_LIMIT);
    if !(1..=MAX_EVIDENCE_LIMIT).contains(&limit) {
        return Err(LearnerError::Invalid(format!(
            "limit is 1 to {MAX_EVIDENCE_LIMIT}"
        )));
    }
    known_concept(conn, concept)?;
    let before = i64::try_from(before.unwrap_or(i64::MAX as u64)).unwrap_or(i64::MAX);
    let sql = format!(
        "SELECT {EVIDENCE_COLUMNS} FROM evidence WHERE concept = ?1 AND id < ?2
         ORDER BY id DESC LIMIT ?3"
    );
    let mut statement = conn.prepare_cached(&sql)?;
    let raws = statement.query_map(params![concept, before, limit + 1], RawEvidence::read)?;
    let mut records = Vec::new();
    for raw in raws {
        records.push(raw?.decode()?);
    }
    let more = records.len() > limit as usize;
    records.truncate(limit as usize);
    let next_before = more.then(|| records.last().map(|r| r.id)).flatten();
    debug_assert!(
        records.windows(2).all(|pair| pair[0].id > pair[1].id),
        "newest first"
    );
    Ok(EvidencePage {
        evidence: records.into_iter().map(to_wire).collect::<Result<_, _>>()?,
        next_before,
    })
}

/// The learner's Brier score over every answer with a stated confidence. An
/// answer appended for several concepts counts once. Shown, never graded.
pub fn learner_calibration(conn: &Connection) -> Result<Option<f64>, LearnerError> {
    let mut statement = conn.prepare_cached(
        "SELECT DISTINCT session, at, kind, outcome, confidence FROM evidence
         WHERE confidence IS NOT NULL AND outcome IN ('correct', 'incorrect')",
    )?;
    let rows = statement.query_map([], |row| {
        let confidence: i64 = row.get(4)?;
        let outcome: String = row.get(3)?;
        Ok((confidence, outcome == "correct"))
    })?;
    let mut answers = Vec::new();
    for row in rows {
        let (confidence, correct) = row?;
        let confidence = u8::try_from(confidence)
            .map_err(|_| LearnerError::Corrupt(format!("confidence {confidence}")))?;
        answers.push((confidence, correct));
    }
    Ok(brier(answers))
}
