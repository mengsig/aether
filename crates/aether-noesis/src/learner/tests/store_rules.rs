//! The learner tables in a real `noesis.db`: append, rebuild, correct, read.

use aether_core::noesis::{ConceptKind, ConceptRef, ConceptState};
use rusqlite::Connection;

use super::*;
use crate::learner::{
    self, LearnerError, Selection, append, correct, derive, ensure_concept, evidence_page,
    rebuild_all, states,
};
use crate::store::NoesisStore;
use EvidenceOutcome::{Correct, Incorrect};

pub(super) fn open_temp() -> (tempfile::TempDir, NoesisStore) {
    let dir = tempfile::tempdir().unwrap();
    let store = NoesisStore::open(&dir.path().join("noesis.db")).unwrap();
    (dir, store)
}

pub(super) fn topic(conn: &Connection, name: &str) -> ConceptRef {
    ensure_concept(conn, ConceptKind::Topic, name, name, day(1, 0)).unwrap()
}

/// Append each record at its own time, as the teaching loop would.
pub(super) fn append_each(conn: &Connection, concept: &str, records: &[EvidenceRecord]) {
    for record in records {
        append(conn, &[new_evidence(record, concept)], record.at).unwrap();
    }
}

/// Every `concept_state` row as text, column by column.
fn cache(conn: &Connection) -> Vec<Vec<String>> {
    let mut statement = conn
        .prepare("SELECT * FROM concept_state ORDER BY concept")
        .unwrap();
    let width = statement.column_count();
    let rows = statement
        .query_map([], |row| {
            (0..width)
                .map(|i| {
                    row.get::<_, rusqlite::types::Value>(i)
                        .map(|v| format!("{v:?}"))
                })
                .collect::<Result<Vec<_>, _>>()
        })
        .unwrap();
    rows.collect::<Result<_, _>>().unwrap()
}

fn history(conn: &Connection, concept: &str) -> Vec<EvidenceRecord> {
    let sql = format!(
        "SELECT {} FROM evidence WHERE concept = ?1 ORDER BY id",
        learner::rows::EVIDENCE_COLUMNS
    );
    let mut statement = conn.prepare(&sql).unwrap();
    let raws = statement
        .query_map([concept], learner::rows::RawEvidence::read)
        .unwrap();
    raws.map(|raw| raw.unwrap().decode().unwrap()).collect()
}

fn varied_history() -> Vec<EvidenceRecord> {
    vec![
        exposure(day(1, 0)),
        check(Correct, Some(70), day(1, 1)),
        observed("confuses mass and weight", day(1, 2)),
        practice(true, ProblemMode::Discrimination, 1, day(2, 0)),
        check(EvidenceOutcome::DontKnow, None, day(2, 1)),
        practice(true, ProblemMode::Discrimination, 0, day(3, 0)),
        check(Incorrect, Some(90), day(4, 0)),
        check(Correct, Some(50), day(5, 0)),
    ]
}

#[test]
fn rebuilding_from_the_evidence_equals_the_cache() {
    let (_dir, store) = open_temp();
    let conn = store.connection();
    let mut concepts = Vec::new();
    for (n, take) in [0, 1, 3, 5, 8].into_iter().enumerate() {
        let concept = topic(conn, &format!("topic {n}"));
        append_each(conn, &concept.id, &varied_history()[..take]);
        concepts.push(concept);
    }
    // The cache as each append left it is the derivation at that append.
    for concept in &concepts {
        let evidence = history(conn, &concept.id);
        let Some(at) = evidence.last().map(|r| r.at) else {
            continue;
        };
        let derived = derive(&evidence, at).into_state(concept.clone());
        let cached = cached_state(conn, concept);
        assert_eq!(cached, derived, "{}", concept.title);
    }
    // Throwing the cache away and rebuilding gives it back, row for row.
    let now = day(9, 0);
    rebuild_all(conn, now).unwrap();
    let rebuilt = cache(conn);
    conn.execute("DELETE FROM concept_state", []).unwrap();
    assert_eq!(rebuild_all(conn, now).unwrap(), concepts.len());
    assert_eq!(cache(conn), rebuilt);
    let read = states(conn, &Selection::default(), now).unwrap();
    for (state, concept) in read.iter().zip(&concepts) {
        assert_eq!(
            *state,
            derive(&history(conn, &concept.id), now).into_state(concept.clone())
        );
    }
}

/// The cached row of `concept`, read back through the wire shape.
fn cached_state(conn: &Connection, concept: &ConceptRef) -> ConceptState {
    let row = conn
        .query_row(
            "SELECT state, strength, last_success_at, review_due_at, calibration,
                    misconceptions, as_of_evidence FROM concept_state WHERE concept = ?1",
            [&concept.id],
            |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, f64>(1)?,
                    row.get::<_, Option<String>>(2)?,
                    row.get::<_, Option<String>>(3)?,
                    row.get::<_, Option<f64>>(4)?,
                    row.get::<_, String>(5)?,
                    row.get::<_, i64>(6)?,
                ))
            },
        )
        .unwrap();
    let time = |text: Option<String>| text.map(|t| learner::rows::parse_time(&t, "t").unwrap());
    ConceptState {
        concept: concept.clone(),
        state: learner::rows::parse_name(&row.0, "state").unwrap(),
        strength: row.1,
        last_success_at: time(row.2),
        review_due_at: time(row.3),
        calibration: row.4,
        misconceptions: serde_json::from_str(&row.5).unwrap(),
        as_of_evidence: row.6 as u64,
    }
}

#[test]
fn a_correction_is_evidence_not_an_overwrite() {
    let (_dir, store) = open_temp();
    let conn = store.connection();
    let concept = topic(conn, "limits");
    append_each(conn, &concept.id, &varied_history()[..4]);
    let before = history(conn, &concept.id);
    let corrected = correct(
        conn,
        &concept.id,
        ConceptStateKind::Demonstrated,
        " knew it ",
        day(6, 0),
    )
    .unwrap();
    assert_eq!(corrected.state, ConceptStateKind::Demonstrated);
    let after = history(conn, &concept.id);
    assert_eq!(after[..before.len()], before[..], "history is untouched");
    let added = &after[before.len()..];
    assert_eq!(added.len(), 1);
    assert_eq!(added[0].kind, EvidenceKind::OperatorCorrection);
    assert_eq!(added[0].note.as_deref(), Some("knew it"));
    assert_eq!(corrected.as_of_evidence, added[0].id);
    let page = evidence_page(conn, &concept.id, None, Some(1)).unwrap();
    assert_eq!(
        page.evidence[0].note.as_deref(),
        Some("demonstrated: knew it")
    );
    // Newer evidence takes over again, and the store refuses any edit.
    append_each(conn, &concept.id, &[check(Incorrect, Some(90), day(6, 1))]);
    let read = states(conn, &Selection::default(), day(6, 2)).unwrap();
    assert_eq!(read[0].state, ConceptStateKind::Misconception);
    let edit = conn.execute("UPDATE evidence SET note = 'rewritten'", []);
    assert!(edit.unwrap_err().to_string().contains("append-only"));
}

#[test]
fn appends_are_validated_at_the_boundary() {
    let (_dir, store) = open_temp();
    let conn = store.connection();
    let concept = topic(conn, "sets");
    let base = new_evidence(&check(Correct, Some(70), day(1, 0)), &concept.id);
    let refused = [
        NewEvidence {
            confidence: Some(40),
            ..base.clone()
        },
        NewEvidence {
            outcome: EvidenceOutcome::Exposed,
            ..base.clone()
        },
        NewEvidence {
            outcome: EvidenceOutcome::DontKnow,
            ..base.clone()
        },
        NewEvidence {
            kind: EvidenceKind::OperatorCorrection,
            outcome: EvidenceOutcome::Corrected,
            ..base.clone()
        },
        NewEvidence {
            hint_rung: Some(6),
            ..base.clone()
        },
    ];
    for item in refused {
        let error = append(conn, &[base.clone(), item.clone()], day(1, 0)).unwrap_err();
        assert!(
            matches!(error, LearnerError::Invalid(_)),
            "{item:?}: {error}"
        );
    }
    let unknown = NewEvidence {
        concept: "k_missing".into(),
        ..base.clone()
    };
    let error = append(conn, &[base.clone(), unknown], day(1, 0)).unwrap_err();
    assert!(matches!(error, LearnerError::UnknownConcept(_)), "{error}");
    assert!(
        history(conn, &concept.id).is_empty(),
        "a refused batch appends nothing"
    );
    let error = correct(
        conn,
        &concept.id,
        ConceptStateKind::Fragile,
        "  ",
        day(1, 0),
    )
    .unwrap_err();
    assert!(matches!(error, LearnerError::Invalid(_)), "{error}");
}

#[test]
fn evidence_pages_run_newest_first() {
    let (_dir, store) = open_temp();
    let conn = store.connection();
    let concept = topic(conn, "graphs");
    let checks: Vec<_> = (0..5)
        .map(|h| check(Correct, Some(70), day(1, h)))
        .collect();
    append_each(conn, &concept.id, &checks);
    let mut seen = Vec::new();
    let mut before = None;
    loop {
        let page = evidence_page(conn, &concept.id, before, Some(2)).unwrap();
        assert!(page.evidence.len() <= 2);
        seen.extend(page.evidence.iter().map(|e| e.id));
        match page.next_before {
            Some(next) => before = Some(next),
            None => break,
        }
    }
    assert_eq!(seen, [5, 4, 3, 2, 1]);
    let error = evidence_page(conn, "k_missing", None, None).unwrap_err();
    assert!(matches!(error, LearnerError::UnknownConcept(_)));
    assert!(evidence_page(conn, &concept.id, None, Some(501)).is_err());
}

#[test]
fn reads_select_by_id_and_query_and_respect_the_limit() {
    let (_dir, store) = open_temp();
    let conn = store.connection();
    let names = ["Derivatives", "Integrals", "Derivative rules"];
    let ids: Vec<_> = names.iter().map(|n| topic(conn, n).id).collect();
    assert_eq!(
        topic(conn, "Integrals").id,
        ids[1],
        "a concept is created once"
    );
    let read = |selection: Selection| -> Vec<String> {
        let found = states(conn, &selection, day(2, 0)).unwrap();
        found.into_iter().map(|s| s.concept.title).collect()
    };
    let query = Some("deriv".to_string());
    assert_eq!(
        read(Selection {
            query: query.clone(),
            ..Default::default()
        }),
        ["Derivatives", "Derivative rules"]
    );
    let named = Some(vec![ids[1].clone(), "k_missing".into()]);
    assert_eq!(
        read(Selection {
            concepts: named,
            ..Default::default()
        }),
        ["Integrals"]
    );
    assert_eq!(
        read(Selection {
            limit: Some(1),
            ..Default::default()
        }),
        ["Derivatives"]
    );
    let all = states(conn, &Selection::default(), day(2, 0)).unwrap();
    assert!(
        all.iter()
            .all(|s| s.state == ConceptStateKind::Unseen && s.as_of_evidence == 0)
    );
}

#[test]
fn a_relinked_concept_keeps_its_evidence() {
    let (_dir, store) = open_temp();
    let conn = store.connection();
    let concept = topic(conn, "chain rule");
    append_each(conn, &concept.id, &[exposure(day(1, 0))]);
    let relinked = learner::relink_concept(conn, &concept.id, "p_chain").unwrap();
    assert_eq!(
        (relinked.kind, relinked.reference.as_str()),
        (ConceptKind::Proposition, "p_chain")
    );
    assert_eq!(history(conn, &concept.id).len(), 1);
    let other = topic(conn, "other");
    assert!(
        learner::relink_concept(conn, &other.id, "p_chain").is_err(),
        "one concept per proposition"
    );
    assert!(learner::relink_concept(conn, &other.id, "chain").is_err());
}

#[test]
fn learner_calibration_counts_an_answer_once_across_its_concepts() {
    let (_dir, store) = open_temp();
    let conn = store.connection();
    let (a, b) = (topic(conn, "a"), topic(conn, "b"));
    let wrong = check(Incorrect, Some(100), day(1, 0));
    let right = check(Correct, Some(100), day(1, 1));
    let both = |r: &EvidenceRecord| [new_evidence(r, &a.id), new_evidence(r, &b.id)];
    append(conn, &both(&wrong), wrong.at).unwrap();
    append(conn, &[new_evidence(&right, &a.id)], right.at).unwrap();
    assert_eq!(learner::learner_calibration(conn).unwrap(), Some(0.5));
}

#[test]
fn teaching_drafts_land_once_per_concept_they_bear_on() {
    use crate::teach::records::{self as teach, EvidenceDraft};
    let (_dir, store) = open_temp();
    let conn = store.connection();
    conn.execute(
        "INSERT INTO sessions VALUES ('ns_1', 'Learn cells', 'teach', 'probe', 'agt_1', 't0', 't0')",
        [],
    )
    .unwrap();
    let draft = |concepts: &[&str], outcome, confidence| EvidenceDraft {
        session: "ns_1".into(),
        concepts: concepts.iter().map(|c| c.to_string()).collect(),
        kind: teach::EvidenceKind::ProbeAnswer,
        outcome,
        confidence,
        dont_know: outcome == teach::EvidenceOutcome::DontKnow,
        mode: None,
        hint_rung: None,
        miss_class: None,
        note: None,
    };
    let drafts = [
        draft(&[], teach::EvidenceOutcome::Correct, Some(70)),
        draft(
            &["p_mito", "organelles", " organelles"],
            teach::EvidenceOutcome::DontKnow,
            None,
        ),
    ];
    let ids = learner::append_drafts(conn, &drafts, day(1, 0)).unwrap();
    assert_eq!(
        ids.len(),
        3,
        "the goal, then a proposition and a topic, each once"
    );
    let read = states(conn, &Selection::default(), day(1, 1)).unwrap();
    let concepts: Vec<_> = read
        .iter()
        .map(|s| (s.concept.kind, s.concept.reference.as_str()))
        .collect();
    assert_eq!(
        concepts,
        [
            (ConceptKind::Topic, "Learn cells"),
            (ConceptKind::Proposition, "p_mito"),
            (ConceptKind::Topic, "organelles")
        ]
    );
    let unknown = EvidenceDraft {
        session: "ns_missing".into(),
        ..draft(&[], teach::EvidenceOutcome::Correct, None)
    };
    assert!(matches!(
        learner::append_drafts(conn, &[unknown], day(1, 2)),
        Err(LearnerError::Invalid(_))
    ));
    assert_eq!(
        history(conn, &read[0].concept.id).len(),
        1,
        "a refused batch appends nothing"
    );
}
