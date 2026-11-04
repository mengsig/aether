//! The derivation rules of NOESIS.md §5, on evidence built in memory.

use super::*;
use crate::learner::{INITIAL_HALF_LIFE_DAYS, MAX_HALF_LIFE_DAYS, brier, derive};
use ConceptStateKind::*;
use EvidenceOutcome::{Correct, DontKnow, Incorrect};

#[test]
fn no_evidence_is_unseen() {
    let derived = derive(&[], day(1, 0));
    assert_eq!(derived.state, Unseen);
    assert_eq!(derived.strength, 0.0);
    assert_eq!(derived.as_of_evidence, 0);
    assert!(derived.review_due_at.is_none() && derived.calibration.is_none());
}

#[test]
fn exposure_alone_never_reaches_demonstrated() {
    let exposures: Vec<_> = (1..=20).map(|d| exposure(day(d, 0))).collect();
    let derived = derive(&numbered(exposures), day(21, 0));
    assert_eq!(derived.state, Exposed);
    assert_eq!(derived.strength, 0.0, "exposure is not a retrieval");
    assert!(derived.last_success_at.is_none());
    // Nor do probe and predict answers, however many and however confident.
    let mut diagnostic = Vec::new();
    for d in 1..=10 {
        diagnostic.push(exposure(day(d, 0)));
        for kind in [EvidenceKind::ProbeAnswer, EvidenceKind::PredictAnswer] {
            diagnostic.push(EvidenceRecord {
                confidence: Some(100),
                ..record(kind, Correct, day(d, 1))
            });
        }
    }
    let derived = derive(&numbered(diagnostic), day(11, 0));
    assert_eq!(derived.state, Exposed);
    assert_eq!(derived.calibration, Some(0.0), "they still calibrate");
}

#[test]
fn two_confident_retrievals_on_different_days_demonstrate() {
    let same_day = numbered(vec![
        exposure(day(1, 0)),
        check(Correct, Some(70), day(1, 1)),
        check(Correct, Some(90), day(1, 2)),
    ]);
    assert_eq!(derive(&same_day, day(1, 3)).state, Exposed);
    let spaced = numbered(vec![
        exposure(day(1, 0)),
        check(Correct, Some(70), day(1, 1)),
        check(Correct, Some(50), day(2, 1)),
    ]);
    let derived = derive(&spaced, day(2, 2));
    assert_eq!(derived.state, Demonstrated);
    assert_eq!(derived.last_success_at, Some(day(2, 1)));
    let unsure = numbered(vec![
        check(Correct, Some(70), day(1, 1)),
        check(Correct, Some(30), day(2, 1)),
    ]);
    assert_eq!(
        derive(&unsure, day(2, 2)).state,
        Fragile,
        "correct at confidence 30"
    );
}

#[test]
fn strength_is_exactly_one_half_after_one_half_life() {
    let once = numbered(vec![check(Correct, Some(90), day(1, 0))]);
    let half_life = Duration::days(INITIAL_HALF_LIFE_DAYS as i64);
    let derived = derive(&once, day(1, 0) + half_life);
    assert_eq!(derived.strength, 0.5);
    assert_eq!(derived.review_due_at, Some(day(1, 0) + half_life));
    assert_eq!(derive(&once, day(1, 0)).strength, 1.0);
    assert_eq!(derive(&once, day(1, 0) + half_life * 2).strength, 0.25);
    // A spaced success doubles it: 4 days from the second success.
    let spaced = numbered(vec![
        check(Correct, Some(90), day(1, 0)),
        check(Correct, Some(90), day(2, 0)),
    ]);
    let derived = derive(&spaced, day(6, 0));
    assert_eq!(derived.strength, 0.5);
    assert_eq!(derived.review_due_at, Some(day(6, 0)));
}

#[test]
fn the_half_life_halves_on_failure_and_stays_between_1_and_180_days() {
    let mut lapses = vec![check(Correct, Some(90), day(1, 0))];
    lapses.extend((0..5).map(|h| check(Incorrect, Some(50), day(1, h + 1))));
    lapses.push(check(Correct, Some(90), day(1, 7)));
    let derived = derive(&numbered(lapses), day(1, 8));
    assert_eq!(
        derived.review_due_at,
        Some(day(1, 7) + Duration::days(1)),
        "floor of 1 day"
    );
    let daily: Vec<_> = (1..=30)
        .map(|d| check(Correct, Some(90), day(1, 0) + Duration::days(d)))
        .collect();
    let derived = derive(&numbered(daily), day(1, 0));
    let last = day(1, 0) + Duration::days(30);
    assert_eq!(
        derived.review_due_at,
        Some(last + Duration::days(MAX_HALF_LIFE_DAYS as i64)),
        "ceiling of 180 days"
    );
    assert_eq!(
        derived.strength, 1.0,
        "a `now` before the last success is no elapsed time"
    );
}

#[test]
fn a_demonstrated_concept_decays_to_fragile_and_a_wrong_answer_makes_it_fragile() {
    let demonstrated = vec![
        check(Correct, Some(90), day(1, 0)),
        check(Correct, Some(90), day(2, 0)),
    ];
    // Half-life 4 days from day 2: due on day 6.
    assert_eq!(
        derive(&numbered(demonstrated.clone()), day(6, 0)).state,
        Demonstrated
    );
    let decayed = derive(&numbered(demonstrated.clone()), day(6, 1));
    assert_eq!(decayed.state, Fragile);
    assert!(decayed.strength < 0.5);
    let mut wrong = demonstrated;
    wrong.push(check(Incorrect, Some(90), day(3, 0)));
    assert_eq!(derive(&numbered(wrong), day(3, 1)).state, Fragile);
    let first_wrong = numbered(vec![exposure(day(1, 0)), check(Incorrect, None, day(1, 1))]);
    assert_eq!(derive(&first_wrong, day(1, 2)).state, Fragile);
}

#[test]
fn practice_counts_up_to_r1_and_is_fragile_from_r3() {
    use ProblemMode::Recall;
    let unaided = numbered(vec![
        practice(true, Recall, 0, day(1, 0)),
        practice(true, Recall, 1, day(2, 0)),
    ]);
    assert_eq!(derive(&unaided, day(2, 1)).state, Demonstrated);
    let hinted = numbered(vec![
        practice(true, Recall, 1, day(1, 0)),
        practice(true, Recall, 2, day(2, 0)),
    ]);
    assert_eq!(
        derive(&hinted, day(2, 1)).state,
        Exposed,
        "R2 neither counts nor breaks"
    );
    let heavy = numbered(vec![
        practice(true, Recall, 0, day(1, 0)),
        practice(true, Recall, 1, day(2, 0)),
        practice(true, Recall, 3, day(2, 1)),
    ]);
    assert_eq!(derive(&heavy, day(2, 2)).state, Fragile);
}

#[test]
fn dont_know_is_absence_never_mastery_nor_a_confident_wrong_answer() {
    let absent = numbered(vec![
        exposure(day(1, 0)),
        check(DontKnow, None, day(1, 1)),
        check(DontKnow, None, day(2, 1)),
    ]);
    let derived = derive(&absent, day(2, 2));
    assert_eq!(derived.state, Exposed, "not fragile: nothing was known");
    assert!(derived.misconceptions.is_empty());
    assert_eq!(derived.calibration, None, "no stated confidence to score");
    let lapse = numbered(vec![
        check(Correct, Some(90), day(1, 0)),
        check(Correct, Some(90), day(2, 0)),
        check(DontKnow, None, day(3, 0)),
    ]);
    let derived = derive(&lapse, day(3, 1));
    assert_eq!(derived.state, Fragile, "a lapse of what was retrieved");
    // The lapse halved the half-life from 4 to 2 days.
    assert_eq!(derived.review_due_at, Some(day(4, 0)));
}

#[test]
fn a_misconception_holds_until_two_later_correct_discrimination_answers() {
    use ProblemMode::{Discrimination, Recall};
    let before = practice(true, Discrimination, 0, day(1, 0));
    let mut history = vec![
        before.clone(),
        before,
        observed("mixes up speed and velocity", day(2, 0)),
        practice(true, Recall, 0, day(2, 1)),
        practice(true, Discrimination, 0, day(3, 0)),
    ];
    let derived = derive(&numbered(history.clone()), day(3, 1));
    assert_eq!(derived.state, Misconception);
    assert_eq!(derived.misconceptions, ["mixes up speed and velocity"]);
    history.push(practice(true, Discrimination, 2, day(4, 0)));
    let derived = derive(&numbered(history), day(4, 1));
    assert_eq!(derived.state, Demonstrated);
    assert!(derived.misconceptions.is_empty());
    let confidently_wrong = EvidenceRecord {
        confidence: Some(90),
        ..practice(false, Discrimination, 0, day(1, 0))
    };
    let derived = derive(&numbered(vec![confidently_wrong]), day(1, 1));
    assert_eq!(derived.state, Misconception);
    assert_eq!(
        derived.misconceptions,
        ["a confidently wrong discrimination answer"]
    );
}

#[test]
fn a_correction_wins_until_newer_evidence_arrives() {
    let mut history = vec![
        check(Correct, Some(90), day(1, 0)),
        check(Correct, Some(90), day(2, 0)),
        correction(Fragile, day(2, 1)),
    ];
    let corrected = derive(&numbered(history.clone()), day(2, 2));
    assert_eq!(corrected.state, Fragile);
    assert_eq!(corrected.as_of_evidence, 3);
    assert_eq!(
        corrected.last_success_at,
        Some(day(2, 0)),
        "the answers still stand"
    );
    history.push(exposure(day(2, 3)));
    let after = derive(&numbered(history), day(2, 4));
    assert_eq!(
        after.state, Demonstrated,
        "newer evidence: the rules decide again"
    );
    assert_eq!(after.as_of_evidence, 4);
}

#[test]
fn calibration_is_the_brier_score_of_stated_confidence() {
    assert_eq!(brier([(100, true), (0, false)]), Some(0.0));
    assert_eq!(brier([(100, false)]), Some(1.0));
    let mixed = brier([(70, true), (30, true)]).unwrap();
    assert!((mixed - (0.09 + 0.49) / 2.0).abs() < 1e-12, "{mixed}");
    assert_eq!(brier([]), None);
    let answers = numbered(vec![
        check(Correct, Some(100), day(1, 0)),
        check(Incorrect, Some(100), day(1, 1)),
        check(DontKnow, None, day(1, 2)),
    ]);
    assert_eq!(derive(&answers, day(1, 3)).calibration, Some(0.5));
}

#[test]
fn derivation_is_a_pure_function_of_evidence_and_time() {
    let history = numbered(vec![
        exposure(day(1, 0)),
        check(Correct, Some(70), day(1, 1)),
        observed("off by one", day(1, 2)),
        check(Correct, Some(90), day(2, 0)),
    ]);
    let now = day(3, 0);
    assert_eq!(derive(&history, now), derive(&history.clone(), now));
    let later = derive(&history, day(30, 0));
    assert_eq!(
        later.state,
        derive(&history, now).state,
        "a misconception does not decay"
    );
    assert!(later.strength < derive(&history, now).strength);
}

#[test]
#[should_panic(expected = "oldest first")]
fn evidence_out_of_order_is_a_programming_error() {
    let mut history = numbered(vec![exposure(day(1, 0)), exposure(day(1, 1))]);
    history.reverse();
    derive(&history, day(2, 0));
}
