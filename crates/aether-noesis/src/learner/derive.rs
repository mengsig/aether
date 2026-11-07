//! The derivation rules of NOESIS.md §5: a pure function of one concept's
//! evidence and the time it is asked about. Nothing here reads a clock.

use std::collections::BTreeSet;

use aether_core::noesis::{
    ConceptRef, ConceptState, ConceptStateKind, EvidenceKind, EvidenceOutcome, ProblemMode,
};
use aether_core::time::Timestamp;
use chrono::{Duration, NaiveDate};

use super::EvidenceRecord;

pub const INITIAL_HALF_LIFE_DAYS: f64 = 2.0;
pub const MIN_HALF_LIFE_DAYS: f64 = 1.0;
pub const MAX_HALF_LIFE_DAYS: f64 = 180.0;
/// Strength at one half-life; below it a concept is due and decayed.
pub const MASTERY_STRENGTH: f64 = 0.5;

const MS_PER_DAY: f64 = 86_400_000.0;
/// A retrieval counts toward mastery only at this stated confidence or more.
const CONFIDENT: u8 = 50;
/// A correct answer at this stated confidence or less is fragile.
const UNSURE: u8 = 30;
/// A wrong discrimination answer at this confidence or more is a misconception.
const CONFIDENTLY_WRONG: u8 = 70;
/// Practice counts toward mastery up to this hint rung (R1) ...
const MAX_UNAIDED_RUNG: u8 = 1;
/// ... and is fragile from this one (R3).
const HEAVY_HINT_RUNG: u8 = 3;
/// Correct discrimination answers after a misconception that clear it.
const CLEARING_ANSWERS: usize = 2;
/// Retrievals on this many different days demonstrate a concept.
const DEMONSTRATING_DAYS: usize = 2;

/// Everything in a [`ConceptState`] but the concept it describes.
#[derive(Debug, Clone, PartialEq)]
pub struct Derived {
    pub state: ConceptStateKind,
    pub strength: f64,
    pub last_success_at: Option<Timestamp>,
    pub review_due_at: Option<Timestamp>,
    pub calibration: Option<f64>,
    pub misconceptions: Vec<String>,
    pub as_of_evidence: u64,
}

impl Derived {
    pub fn into_state(self, concept: ConceptRef) -> ConceptState {
        ConceptState {
            concept,
            state: self.state,
            strength: self.strength,
            last_success_at: self.last_success_at,
            review_due_at: self.review_due_at,
            calibration: self.calibration,
            misconceptions: self.misconceptions,
            as_of_evidence: self.as_of_evidence,
        }
    }
}

/// A concept's state at `now` from its evidence, oldest first. The newest
/// row decides a correction: it wins until newer evidence arrives.
pub fn derive(evidence: &[EvidenceRecord], now: Timestamp) -> Derived {
    assert!(
        evidence.windows(2).all(|pair| pair[0].id < pair[1].id),
        "evidence is derived oldest first, each row once"
    );
    assert!(
        evidence
            .windows(2)
            .all(|pair| pair[0].concept == pair[1].concept),
        "evidence of one concept is derived at a time"
    );
    let Some(newest) = evidence.last() else {
        return Derived {
            state: ConceptStateKind::Unseen,
            strength: 0.0,
            last_success_at: None,
            review_due_at: None,
            calibration: None,
            misconceptions: Vec::new(),
            as_of_evidence: 0,
        };
    };
    let memory = Memory::replay(evidence);
    let strength = memory.strength(now);
    let misconceptions = unresolved_misconceptions(evidence);
    let state = match newest.corrected_state {
        Some(corrected) => corrected,
        None => classify(evidence, &memory, strength, &misconceptions),
    };
    Derived {
        state,
        strength,
        last_success_at: memory.last_success_at,
        review_due_at: memory.review_due_at(),
        calibration: brier(evidence.iter().filter_map(graded)),
        misconceptions,
        as_of_evidence: newest.id,
    }
}

/// How a retrieval (`check_answer`, `recall_judged`, `practice_conclude`)
/// bears on memory. Probe and predict answers come before teaching: they
/// count toward calibration only.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Attempt {
    /// Correct, confident and unaided: counts toward mastery.
    Success,
    /// Correct, but too unsure or too hinted to count.
    Weak {
        unsure: bool,
        heavily_hinted: bool,
    },
    Failure,
    /// "Don't know": evidence of absence, never a wrong answer.
    Absent,
}

fn attempt(record: &EvidenceRecord) -> Option<Attempt> {
    let retrieval = matches!(
        record.kind,
        EvidenceKind::CheckAnswer | EvidenceKind::RecallJudged | EvidenceKind::PracticeConclude
    );
    if !retrieval {
        return None;
    }
    match record.outcome {
        EvidenceOutcome::Incorrect => Some(Attempt::Failure),
        EvidenceOutcome::DontKnow => Some(Attempt::Absent),
        EvidenceOutcome::Correct => {
            // No stated confidence (practice) neither counts for nor against.
            let unsure = record.confidence.is_some_and(|c| c <= UNSURE);
            let confident = record.confidence.is_none_or(|c| c >= CONFIDENT);
            let rung = record.hint_rung.unwrap_or(0);
            if confident && rung <= MAX_UNAIDED_RUNG {
                return Some(Attempt::Success);
            }
            Some(Attempt::Weak {
                unsure,
                heavily_hinted: rung >= HEAVY_HINT_RUNG,
            })
        }
        other => {
            debug_assert!(false, "a retrieval cannot have outcome {other:?}");
            None
        }
    }
}

/// The spacing model replayed over the retrievals.
struct Memory {
    half_life_days: f64,
    last_success_at: Option<Timestamp>,
    success_days: BTreeSet<NaiveDate>,
}

impl Memory {
    /// The half-life starts at 2 days, doubles on each success on a later
    /// (UTC) day than the previous one, and halves on each failure or lapse.
    fn replay(evidence: &[EvidenceRecord]) -> Self {
        let mut memory = Memory {
            half_life_days: INITIAL_HALF_LIFE_DAYS,
            last_success_at: None,
            success_days: BTreeSet::new(),
        };
        for record in evidence {
            match attempt(record) {
                Some(Attempt::Success) => memory.succeed(record.at),
                Some(Attempt::Failure | Attempt::Absent) => memory.scale_half_life(0.5),
                Some(Attempt::Weak { .. }) | None => {}
            }
        }
        debug_assert!(
            (MIN_HALF_LIFE_DAYS..=MAX_HALF_LIFE_DAYS).contains(&memory.half_life_days),
            "the half-life stays clamped"
        );
        memory
    }

    fn succeed(&mut self, at: Timestamp) {
        if let Some(previous) = self.last_success_at
            && at.date_naive() > previous.date_naive()
        {
            self.scale_half_life(2.0);
        }
        let latest = self.last_success_at.map_or(at, |previous| previous.max(at));
        self.last_success_at = Some(latest);
        self.success_days.insert(at.date_naive());
        debug_assert!(!self.success_days.is_empty());
    }

    fn scale_half_life(&mut self, factor: f64) {
        assert!(
            factor > 0.0 && factor.is_finite(),
            "a half-life scales by a positive factor"
        );
        self.half_life_days =
            (self.half_life_days * factor).clamp(MIN_HALF_LIFE_DAYS, MAX_HALF_LIFE_DAYS);
    }

    fn half_life(&self) -> Duration {
        Duration::milliseconds((self.half_life_days * MS_PER_DAY).round() as i64)
    }

    /// `2^(-Δt / half_life)` since the last success; 0 with none. A `now`
    /// before that success reads as no time elapsed.
    fn strength(&self, now: Timestamp) -> f64 {
        let Some(anchor) = self.last_success_at else {
            return 0.0;
        };
        let elapsed = (now - anchor).num_milliseconds().max(0) as f64;
        let half_life = self.half_life().num_milliseconds() as f64;
        assert!(half_life > 0.0, "a half-life is at least a day");
        let strength = (-elapsed / half_life).exp2();
        debug_assert!((0.0..=1.0).contains(&strength), "strength {strength}");
        strength
    }

    /// When strength reaches [`MASTERY_STRENGTH`]: one half-life after the last success.
    fn review_due_at(&self) -> Option<Timestamp> {
        self.last_success_at.map(|anchor| anchor + self.half_life())
    }

    fn demonstrated_ever(&self) -> bool {
        self.success_days.len() >= DEMONSTRATING_DAYS
    }
}

/// The rules of the §5 table below a winning correction, most specific first.
fn classify(
    evidence: &[EvidenceRecord],
    memory: &Memory,
    strength: f64,
    misconceptions: &[String],
) -> ConceptStateKind {
    debug_assert!(
        !evidence.is_empty(),
        "no evidence is unseen, decided earlier"
    );
    if !misconceptions.is_empty() {
        return ConceptStateKind::Misconception;
    }
    let shaky_now = match evidence.iter().rev().find_map(attempt) {
        Some(Attempt::Failure) => true,
        // Not knowing what was once retrieved is a lapse; before that, it is absence.
        Some(Attempt::Absent) => memory.last_success_at.is_some(),
        Some(Attempt::Weak {
            unsure,
            heavily_hinted,
        }) => unsure || heavily_hinted,
        Some(Attempt::Success) | None => false,
    };
    let decayed = strength < MASTERY_STRENGTH;
    if shaky_now || (memory.demonstrated_ever() && decayed) {
        return ConceptStateKind::Fragile;
    }
    if memory.demonstrated_ever() {
        debug_assert!(!decayed && misconceptions.is_empty());
        return ConceptStateKind::Demonstrated;
    }
    // Met, but short of the mastery rule: exposure never counts as mastery.
    ConceptStateKind::Exposed
}

/// The misconceptions not yet cleared by 2 later correct discrimination
/// answers, oldest first, each named once.
fn unresolved_misconceptions(evidence: &[EvidenceRecord]) -> Vec<String> {
    let mut clearing_after = 0;
    let mut open: Vec<String> = Vec::new();
    for record in evidence.iter().rev() {
        let discrimination = record.mode == Some(ProblemMode::Discrimination);
        if discrimination && record.outcome == EvidenceOutcome::Correct {
            clearing_after += 1;
            continue;
        }
        if clearing_after >= CLEARING_ANSWERS {
            continue;
        }
        if let Some(name) = misconception_seen(record) {
            open.retain(|seen| *seen != name);
            open.push(name);
        }
    }
    open.reverse();
    debug_assert!(
        open.iter().collect::<BTreeSet<_>>().len() == open.len(),
        "each misconception is named once"
    );
    open
}

/// A `misconception_observed` row, or a confidently wrong discrimination answer.
fn misconception_seen(record: &EvidenceRecord) -> Option<String> {
    let observed = record.kind == EvidenceKind::MisconceptionObserved;
    let confidently_wrong = record.outcome == EvidenceOutcome::Incorrect
        && record.mode == Some(ProblemMode::Discrimination)
        && record.confidence.is_some_and(|c| c >= CONFIDENTLY_WRONG);
    if !observed && !confidently_wrong {
        return None;
    }
    let fallback = if observed {
        "an unnamed misconception"
    } else {
        "a confidently wrong discrimination answer"
    };
    let name = record
        .note
        .as_deref()
        .map(str::trim)
        .filter(|n| !n.is_empty());
    Some(name.unwrap_or(fallback).to_string())
}

/// A stated confidence against a right-or-wrong answer; "don't know" has neither.
fn graded(record: &EvidenceRecord) -> Option<(u8, bool)> {
    let confidence = record.confidence?;
    match record.outcome {
        EvidenceOutcome::Correct => Some((confidence, true)),
        EvidenceOutcome::Incorrect => Some((confidence, false)),
        _ => None,
    }
}

/// The Brier score of stated confidence (0..=100) against correctness: 0 is
/// perfectly calibrated, 1 the worst. Shown to the learner, never used to grade.
pub fn brier(answers: impl IntoIterator<Item = (u8, bool)>) -> Option<f64> {
    let (mut total, mut count) = (0.0, 0_u32);
    for (confidence, correct) in answers {
        assert!(confidence <= 100, "confidence {confidence} is a percentage");
        let forecast = f64::from(confidence) / 100.0;
        let truth = if correct { 1.0 } else { 0.0 };
        total += (forecast - truth) * (forecast - truth);
        count += 1;
    }
    if count == 0 {
        return None;
    }
    let score = total / f64::from(count);
    debug_assert!((0.0..=1.0).contains(&score), "a Brier score is in 0..=1");
    Some(score)
}
