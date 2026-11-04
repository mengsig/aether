//! The teaching loop's evidence ([`EvidenceDraft`], from
//! `Teaching::take_evidence`) appended to the learner model.

use aether_core::noesis::{ConceptId, ConceptKind};
use aether_core::time::Timestamp;
use rusqlite::{Connection, OptionalExtension};

use super::rows::parse_name;
use super::store::{append_within, concept, ensure_concept};
use super::{LearnerError, NewEvidence};
use crate::teach::records::EvidenceDraft;

/// Append each draft once per concept it bears on, in one transaction. A
/// draft names concepts by id (`k_`), proposition id (`p_`) or a free topic;
/// one naming none bears on its session's goal.
pub fn append_drafts(
    conn: &Connection,
    drafts: &[EvidenceDraft],
    now: Timestamp,
) -> Result<Vec<u64>, LearnerError> {
    let tx = conn.unchecked_transaction()?;
    let mut items = Vec::new();
    for draft in drafts {
        assert_eq!(
            draft.dont_know,
            draft.outcome.as_str() == "dont_know",
            "a draft's don't-know flag and outcome agree"
        );
        for concept in concepts_of(&tx, draft, now)? {
            items.push(new_evidence(draft, concept)?);
        }
    }
    let ids = append_within(&tx, &items, now)?;
    tx.commit()?;
    debug_assert_eq!(ids.len(), items.len());
    Ok(ids)
}

fn concepts_of(
    conn: &Connection,
    draft: &EvidenceDraft,
    now: Timestamp,
) -> Result<Vec<ConceptId>, LearnerError> {
    if draft.concepts.is_empty() {
        let goal = session_goal(conn, &draft.session)?;
        return Ok(vec![
            ensure_concept(conn, ConceptKind::Topic, &goal, &goal, now)?.id,
        ]);
    }
    let mut ids: Vec<ConceptId> = Vec::with_capacity(draft.concepts.len());
    for label in &draft.concepts {
        let id = resolve(conn, label.trim(), now)?;
        if !ids.contains(&id) {
            ids.push(id);
        }
    }
    Ok(ids)
}

fn resolve(conn: &Connection, label: &str, now: Timestamp) -> Result<ConceptId, LearnerError> {
    if label.starts_with("k_")
        && let Some(known) = concept(conn, label)?
    {
        return Ok(known.id);
    }
    let kind = if label.starts_with("p_") {
        ConceptKind::Proposition
    } else {
        ConceptKind::Topic
    };
    Ok(ensure_concept(conn, kind, label, label, now)?.id)
}

fn session_goal(conn: &Connection, session: &str) -> Result<String, LearnerError> {
    let goal: Option<String> = conn
        .query_row(
            "SELECT goal FROM sessions WHERE id = ?1",
            [session],
            |row| row.get(0),
        )
        .optional()?;
    goal.ok_or_else(|| LearnerError::Invalid(format!("evidence from unknown session `{session}`")))
}

fn new_evidence(draft: &EvidenceDraft, concept: ConceptId) -> Result<NewEvidence, LearnerError> {
    Ok(NewEvidence {
        concept,
        session: Some(draft.session.clone()),
        kind: parse_name(draft.kind.as_str(), "draft kind")?,
        outcome: parse_name(draft.outcome.as_str(), "draft outcome")?,
        confidence: draft.confidence,
        mode: draft
            .mode
            .map(|m| parse_name(m.as_str(), "draft mode"))
            .transpose()?,
        hint_rung: draft.hint_rung,
        miss_class: draft
            .miss_class
            .map(|c| parse_name(c.as_str(), "draft miss class"))
            .transpose()?,
        note: draft.note.clone(),
    })
}
